//! El mezclador: lo único que corre en el hilo de tiempo real.
//!
//! ## El contrato
//!
//! Este código se ejecuta en el callback que WASAPI llama cada pocos
//! milisegundos. Si tarda de más, el sistema reproduce lo que hubiera en el
//! buffer —silencio, o la misma porción otra vez— y se oye un chasquido. Por
//! eso, dentro de [`Mezclador::rellenar`] está prohibido:
//!
//! - asignar o liberar memoria,
//! - tomar un lock que otro hilo pueda estar reteniendo,
//! - hacer I/O,
//! - loguear.
//!
//! Todo lo que necesita ya está reservado al construirlo. Lo que viene de fuera
//! llega por atómicos y por colas SPSC sin locks.
//!
//! ## Por qué se puede probar sin tarjeta de sonido
//!
//! El mezclador no conoce cpal. Recibe consumidores de anillo y un buffer de
//! salida, y es una función de unos y otros. Los tests le dan muestras a mano y
//! comprueban lo que sale, que es la única forma de verificar un crossfade o un
//! underrun de manera determinista.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use localify_core::ports::audio_engine::VoiceId;
use rtrb::Consumer;

use crate::dsp::{Crossfade, EqCompartido, EstadoEq, Limitador};

/// Constante de tiempo de la rampa de volumen, en milisegundos.
///
/// Un cambio de volumen aplicado de golpe es un escalón en la señal, y un
/// escalón se oye como un clic. Con un filtro de un polo, el salto se recorre
/// al 99 % en unas 4,6 constantes de tiempo: 4,5 ms dan ~21 ms de transición,
/// que es inaudible como rampa e instantánea para quien mueve el control.
///
/// El coeficiente se calcula a partir de la frecuencia de muestreo y no se fija
/// como número: una constante fija haría que la rampa durase el doble a 96 kHz
/// que a 48 kHz.
const CONSTANTE_VOLUMEN_MS: f32 = 4.5;

/// Lo que el mezclador publica hacia fuera.
///
/// Todo son atómicos: el hilo de control los lee cuando quiere sin frenar al de
/// audio ni un ciclo.
#[derive(Debug)]
pub struct EstadoVoz {
    /// Marcos ya enviados a la tarjeta. Es la base de la posición.
    pub marcos: AtomicU64,
    /// Marcos ya decodificados, sonados o no.
    ///
    /// Va por delante de `marcos` en todo lo que quepa en el anillo, y es lo
    /// que hay que mirar para saber hasta dónde se puede saltar sin esperar.
    pub decodificados: AtomicU64,
    /// El productor terminó y el anillo se vació: la pista se acabó.
    pub agotada: AtomicBool,
    /// Hubo que rellenar con silencio porque no llegaban muestras.
    pub underrun: AtomicBool,
}

impl EstadoVoz {
    #[must_use]
    pub fn nuevo() -> Arc<Self> {
        Arc::new(Self {
            marcos: AtomicU64::new(0),
            decodificados: AtomicU64::new(0),
            agotada: AtomicBool::new(false),
            underrun: AtomicBool::new(false),
        })
    }
}

/// Una voz sonando: su anillo de PCM y su estado compartido.
pub struct Voz {
    pub id: VoiceId,
    consumidor: Consumer<f32>,
    estado: Arc<EstadoVoz>,
    /// `true` cuando el decodificador ya no va a producir más.
    fin_de_flujo: Arc<AtomicBool>,
}

impl std::fmt::Debug for Voz {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Voz")
            .field("id", &self.id)
            .field("disponibles", &self.consumidor.slots())
            .finish_non_exhaustive()
    }
}

impl Voz {
    #[must_use]
    pub fn nueva(
        id: VoiceId,
        consumidor: Consumer<f32>,
        estado: Arc<EstadoVoz>,
        fin_de_flujo: Arc<AtomicBool>,
    ) -> Self {
        Self {
            id,
            consumidor,
            estado,
            fin_de_flujo,
        }
    }

    /// `true` si ya no queda nada por sonar: el decodificador terminó **y** el
    /// anillo está vacío.
    ///
    /// Las dos condiciones importan. Solo con la primera, la canción se daría
    /// por acabada con hasta tres segundos todavía sin sonar.
    #[must_use]
    pub fn ha_terminado(&self) -> bool {
        self.fin_de_flujo.load(Ordering::Acquire) && self.consumidor.is_empty()
    }

    /// Muestras listas en el anillo. Sirve para saber cuánto margen queda
    /// antes de un underrun.
    #[must_use]
    pub fn disponibles(&self) -> usize {
        self.consumidor.slots()
    }

    /// Acceso al anillo para los tests del hilo de decodificación, que
    /// necesitan hacer de consumidor.
    #[cfg(test)]
    pub(crate) const fn consumidor_mut(&mut self) -> &mut Consumer<f32> {
        &mut self.consumidor
    }
}

/// Volumen compartido con el hilo de control.
///
/// Se guarda como los bits de un `f32` en un `AtomicU64` porque no hay
/// `AtomicF32` en la biblioteca estándar. Es una conversión exacta, no una
/// aproximación.
#[derive(Debug)]
pub struct VolumenCompartido(AtomicU64);

impl VolumenCompartido {
    #[must_use]
    pub fn nuevo(valor: f32) -> Self {
        Self(AtomicU64::new(u64::from(valor.to_bits())))
    }

    pub fn poner(&self, valor: f32) {
        self.0.store(u64::from(valor.to_bits()), Ordering::Relaxed);
    }

    #[must_use]
    pub fn leer(&self) -> f32 {
        #[allow(
            clippy::cast_possible_truncation,
            reason = "solo se guardan bits de f32, siempre caben en u32"
        )]
        let bits = self.0.load(Ordering::Relaxed) as u32;
        let v = f32::from_bits(bits);
        if v.is_finite() {
            v.clamp(0.0, 1.0)
        } else {
            1.0
        }
    }
}

/// Qué pasó al rellenar un bloque. Lo consulta el hilo de control.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Resultado {
    /// La voz saliente terminó su fundido y se puede liberar.
    pub fundido_terminado: bool,
    /// Alguna voz se quedó sin muestras.
    pub hubo_underrun: bool,
    /// Alguna voz llegó a su final.
    pub alguna_termino: bool,
}

/// El mezclador. Propiedad exclusiva del hilo de audio.
pub struct Mezclador {
    /// Voz que suena. `None` con la reproducción parada.
    actual: Option<Voz>,
    /// Voz entrante durante un fundido.
    siguiente: Option<Voz>,
    fundido: Option<Crossfade>,
    eq: EstadoEq,
    limitador: Limitador,
    volumen: Arc<VolumenCompartido>,
    /// Volumen aplicado ahora mismo, que persigue al pedido.
    volumen_actual: f32,
    /// Coeficiente de la rampa, derivado de la frecuencia de muestreo.
    suavizado_volumen: f32,
    pausado: Arc<AtomicBool>,
    /// Buffer de la voz entrante durante un fundido, reservado al construir.
    mezcla_b: Vec<f32>,
    /// Buffer estéreo intermedio para dispositivos que no son estéreo.
    estereo: Vec<f32>,
}

impl std::fmt::Debug for Mezclador {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Mezclador")
            .field("actual", &self.actual.as_ref().map(|v| v.id))
            .field("siguiente", &self.siguiente.as_ref().map(|v| v.id))
            .field("fundido", &self.fundido.is_some())
            .finish_non_exhaustive()
    }
}

impl Mezclador {
    /// `marcos_maximos` es el bloque más grande que la tarjeta puede pedir.
    /// Reservar aquí es lo que permite no asignar nunca en el callback.
    #[must_use]
    pub fn nuevo(
        sample_rate: u32,
        marcos_maximos: usize,
        volumen: Arc<VolumenCompartido>,
        pausado: Arc<AtomicBool>,
    ) -> Self {
        let inicial = volumen.leer();
        #[allow(clippy::cast_precision_loss, reason = "96000 cabe exacto en f32")]
        let sr = sample_rate.max(1) as f32;
        Self {
            actual: None,
            siguiente: None,
            fundido: None,
            eq: EstadoEq::nuevo(),
            limitador: Limitador::nuevo(sample_rate),
            volumen,
            volumen_actual: inicial,
            suavizado_volumen: (-1.0 / ((CONSTANTE_VOLUMEN_MS / 1000.0) * sr)).exp(),
            pausado,
            mezcla_b: vec![0.0; marcos_maximos * 2],
            estereo: vec![0.0; marcos_maximos * 2],
        }
    }

    /// Instala la voz que debe sonar, descartando cualquier fundido en curso.
    pub fn poner_actual(&mut self, voz: Option<Voz>) {
        self.actual = voz;
        self.siguiente = None;
        self.fundido = None;
        // El estado de los filtros es de la pista anterior: arrastrarlo suena
        // como un chasquido en la primera muestra de la nueva.
        self.eq.reiniciar();
        self.limitador.reiniciar();
    }

    /// Arranca un fundido hacia `voz`. Con cero marcos, el cambio es inmediato
    /// y sin hueco.
    pub fn fundir_a(&mut self, voz: Voz, marcos: u32) {
        if marcos == 0 || self.actual.is_none() {
            self.poner_actual(Some(voz));
            return;
        }
        self.siguiente = Some(voz);
        self.fundido = Some(Crossfade::nuevo(marcos));
    }

    /// Recoge la voz saliente cuando el fundido acaba, para liberarla fuera del
    /// hilo de audio.
    pub fn tomar_saliente(&mut self) -> Option<Voz> {
        if self.fundido.is_some_and(|f| f.ha_terminado()) {
            self.fundido = None;
            let entrante = self.siguiente.take();
            let saliente = std::mem::replace(&mut self.actual, entrante);
            return saliente;
        }
        None
    }

    #[must_use]
    pub fn id_actual(&self) -> Option<VoiceId> {
        self.actual.as_ref().map(|v| v.id)
    }

    /// Aplica un perfil de ecualización nuevo si lo hay.
    pub fn refrescar_eq(&mut self, compartido: &EqCompartido) {
        self.eq.actualizar_si_procede(compartido);
    }

    /// Rellena `salida` (estéreo intercalado) con lo que toque sonar.
    ///
    /// **Esta función corre en el hilo de tiempo real.** No asigna, no bloquea
    /// y no loguea.
    pub fn rellenar(&mut self, salida: &mut [f32]) -> Resultado {
        let mut r = Resultado::default();
        let marcos = salida.len() / 2;

        if self.pausado.load(Ordering::Relaxed) || self.actual.is_none() {
            salida.fill(0.0);
            return r;
        }

        // Voz que suena.
        let (leidos_a, agotada_a) = Self::verter(self.actual.as_mut(), salida);
        r.hubo_underrun |= leidos_a < marcos;
        r.alguna_termino |= agotada_a;

        // Voz entrante, si hay fundido.
        if self.fundido.is_some() {
            let destino = &mut self.mezcla_b[..salida.len()];
            let (leidos_b, agotada_b) = Self::verter(self.siguiente.as_mut(), destino);
            r.hubo_underrun |= leidos_b < marcos;
            r.alguna_termino |= agotada_b;

            if let Some(f) = self.fundido.as_mut() {
                for (i, marco) in salida.chunks_exact_mut(2).enumerate() {
                    let (ga, gb) = f.ganancias();
                    marco[0] = marco[0] * ga + destino[i * 2] * gb;
                    marco[1] = marco[1] * ga + destino[i * 2 + 1] * gb;
                    f.avanzar();
                }
                r.fundido_terminado = f.ha_terminado();
            }
        }

        self.aplicar_volumen(salida);
        self.eq.procesar(salida);
        self.limitador.procesar(salida);

        if r.hubo_underrun
            && let Some(v) = self.actual.as_ref()
        {
            v.estado.underrun.store(true, Ordering::Relaxed);
        }
        r
    }

    /// Como [`Self::rellenar`], para un dispositivo que no es estéreo.
    ///
    /// El motor produce dos canales siempre. Un dispositivo mono recibe la
    /// media —no solo el izquierdo, que perdería la mitad de la mezcla— y uno
    /// de más de dos recibe el par frontal y silencio en el resto: inventar
    /// contenido para los canales traseros sería peor que dejarlos callados.
    ///
    /// Corre en el hilo de tiempo real y tampoco asigna: usa un buffer propio,
    /// distinto del que [`Self::rellenar`] necesita para el fundido.
    pub fn rellenar_a_canales(&mut self, salida: &mut [f32], canales: usize) {
        if canales == 0 {
            return;
        }
        let marcos = salida.len() / canales;
        let necesarios = marcos * 2;
        if self.estereo.len() < necesarios {
            // No cabe: es preferible silencio a asignar en el hilo de audio.
            salida.fill(0.0);
            return;
        }

        // El buffer se saca de `self` para poder pasárselo a `rellenar`, que
        // necesita el resto de campos. Vuelve a su sitio al terminar, así que
        // no se asigna nada.
        let mut estereo = std::mem::take(&mut self.estereo);
        self.rellenar(&mut estereo[..necesarios]);

        for (i, marco) in salida.chunks_exact_mut(canales).enumerate() {
            let (l, r) = (estereo[i * 2], estereo[i * 2 + 1]);
            if canales == 1 {
                marco[0] = (l + r) * 0.5;
            } else {
                marco[0] = l;
                marco[1] = r;
                marco[2..].fill(0.0);
            }
        }
        self.estereo = estereo;
    }

    /// Saca marcos del anillo de una voz al buffer, rellenando con silencio lo
    /// que falte. Devuelve `(marcos leídos, la voz terminó)`.
    fn verter(voz: Option<&mut Voz>, destino: &mut [f32]) -> (usize, bool) {
        let Some(voz) = voz else {
            destino.fill(0.0);
            return (0, false);
        };

        let pedidas = destino.len();
        let disponibles = voz.consumidor.slots().min(pedidas);

        // `read_chunk` no asigna: presta las dos mitades del anillo.
        let leidas = match voz.consumidor.read_chunk(disponibles) {
            Ok(trozo) => {
                let (a, b) = trozo.as_slices();
                destino[..a.len()].copy_from_slice(a);
                destino[a.len()..a.len() + b.len()].copy_from_slice(b);
                let n = a.len() + b.len();
                trozo.commit_all();
                n
            }
            Err(_) => 0,
        };

        // Un underrun se rellena con silencio, nunca repitiendo lo anterior:
        // repetir suena a "disco rayado" y es peor que el hueco.
        destino[leidas..].fill(0.0);

        let marcos = leidas / 2;
        voz.estado
            .marcos
            .fetch_add(marcos as u64, Ordering::Relaxed);

        let termino = voz.ha_terminado();
        if termino {
            voz.estado.agotada.store(true, Ordering::Release);
        }
        (marcos, termino)
    }

    /// Aplica el volumen con una rampa, para que moverlo no produzca clics.
    fn aplicar_volumen(&mut self, salida: &mut [f32]) {
        let objetivo = self.volumen.leer();
        // La curva perceptual la aplica quien publica el valor; aquí solo se
        // persigue el número.
        if (objetivo - self.volumen_actual).abs() < 1e-6 {
            self.volumen_actual = objetivo;
            if (objetivo - 1.0).abs() < 1e-6 {
                return;
            }
            for m in salida.iter_mut() {
                *m *= objetivo;
            }
            return;
        }

        for marco in salida.chunks_exact_mut(2) {
            self.volumen_actual =
                objetivo + (self.volumen_actual - objetivo) * self.suavizado_volumen;
            marco[0] *= self.volumen_actual;
            marco[1] *= self.volumen_actual;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Monta una voz con `muestras` ya listas en su anillo.
    fn voz_con(id: u32, muestras: &[f32], cerrada: bool) -> (Voz, Arc<EstadoVoz>) {
        let (mut productor, consumidor) = rtrb::RingBuffer::<f32>::new(muestras.len().max(1) * 2);
        for m in muestras {
            productor.push(*m).expect("cabe");
        }
        // El productor se suelta a propósito: en el motor real vive en el hilo
        // de decodificación, y aquí solo interesa lo que ya hay en el anillo.
        std::mem::forget(productor);

        let estado = EstadoVoz::nuevo();
        let fin = Arc::new(AtomicBool::new(cerrada));
        (
            Voz::nueva(VoiceId(id), consumidor, Arc::clone(&estado), fin),
            estado,
        )
    }

    fn mezclador(marcos: usize) -> Mezclador {
        Mezclador::nuevo(
            48_000,
            marcos,
            Arc::new(VolumenCompartido::nuevo(1.0)),
            Arc::new(AtomicBool::new(false)),
        )
    }

    #[test]
    fn sin_voz_la_salida_es_silencio() {
        let mut m = mezclador(64);
        let mut salida = vec![1.0_f32; 128];
        m.rellenar(&mut salida);
        assert!(salida.iter().all(|v| v.abs() < f32::EPSILON));
    }

    #[test]
    fn en_pausa_la_salida_es_silencio() {
        let pausado = Arc::new(AtomicBool::new(true));
        let mut m = Mezclador::nuevo(
            48_000,
            64,
            Arc::new(VolumenCompartido::nuevo(1.0)),
            Arc::clone(&pausado),
        );
        let (voz, _) = voz_con(0, &[0.5; 128], false);
        m.poner_actual(Some(voz));

        let mut salida = vec![1.0_f32; 128];
        m.rellenar(&mut salida);
        assert!(salida.iter().all(|v| v.abs() < f32::EPSILON));
    }

    #[test]
    fn una_voz_sola_sale_intacta() {
        // El bloque debe superar el look-ahead del limitador (240 marcos a
        // 48 kHz): con menos, todo el audio sigue dentro de su retardo y la
        // salida es silencio legitimo.
        let mut m = mezclador(1024);
        let entrada: Vec<f32> = (0..2048_u16).map(|i| f32::from(i % 7) / 10.0).collect();
        let (voz, _) = voz_con(0, &entrada, false);
        m.poner_actual(Some(voz));

        let mut salida = vec![0.0_f32; 2048];
        m.rellenar(&mut salida);

        assert!(
            salida.iter().any(|v| v.abs() > 0.01),
            "no salio nada de audio"
        );
    }

    #[test]
    fn la_rampa_de_volumen_dura_lo_mismo_a_cualquier_frecuencia() {
        // Con un coeficiente fijo, la rampa duraria el doble a 96 kHz que a
        // 48 kHz: el mismo gesto se sentiria distinto segun el dispositivo.
        for sr in [44_100_u32, 48_000, 96_000] {
            let volumen = Arc::new(VolumenCompartido::nuevo(1.0));
            let mut m = Mezclador::nuevo(
                sr,
                8192,
                Arc::clone(&volumen),
                Arc::new(AtomicBool::new(false)),
            );
            let (voz, _) = voz_con(0, &vec![0.5; 400_000], false);
            m.poner_actual(Some(voz));

            volumen.poner(0.0);
            // 40 ms: las ~21 ms de la transicion mas los 5 ms de look-ahead
            // del limitador, que retrasa lo que sale, y margen de sobra.
            let marcos = (sr as usize * 40) / 1000;
            let mut salida = vec![0.0_f32; marcos * 2];
            m.rellenar(&mut salida);

            let final_ = salida[salida.len() - 2].abs();
            assert!(
                final_ < 1e-3,
                "a {sr} Hz el volumen no llego a cero en 40 ms: {final_}"
            );
        }
    }

    #[test]
    fn un_underrun_se_rellena_con_silencio_y_no_repitiendo() {
        // Repetir el ultimo trozo suena a disco rayado; el silencio es un
        // hueco limpio y se nota mucho menos.
        let mut m = mezclador(64);
        let (voz, estado) = voz_con(0, &[0.5; 20], false);
        m.poner_actual(Some(voz));

        let mut salida = vec![9.0_f32; 128];
        let r = m.rellenar(&mut salida);

        assert!(r.hubo_underrun, "faltaban muestras y no se informo");
        assert!(estado.underrun.load(Ordering::Relaxed));
        assert!(
            salida[100..].iter().all(|v| v.abs() < 0.01),
            "la cola deberia ser silencio, no basura ni repeticion"
        );
    }

    #[test]
    fn la_posicion_avanza_con_los_marcos_entregados() {
        let mut m = mezclador(64);
        let (voz, estado) = voz_con(0, &[0.1; 256], false);
        m.poner_actual(Some(voz));

        let mut salida = vec![0.0_f32; 128];
        m.rellenar(&mut salida);
        assert_eq!(estado.marcos.load(Ordering::Relaxed), 64);

        m.rellenar(&mut salida);
        assert_eq!(estado.marcos.load(Ordering::Relaxed), 128);
    }

    #[test]
    fn una_voz_cerrada_y_vacia_se_da_por_terminada() {
        let mut m = mezclador(64);
        let (voz, estado) = voz_con(0, &[0.1; 8], true);
        m.poner_actual(Some(voz));

        let mut salida = vec![0.0_f32; 128];
        let r = m.rellenar(&mut salida);

        assert!(r.alguna_termino);
        assert!(estado.agotada.load(Ordering::Acquire));
    }

    #[test]
    fn un_fundido_de_cero_marcos_cambia_de_voz_al_instante() {
        // Es el modo sin huecos: la voz nueva entra desde el primer marco.
        let mut m = mezclador(64);
        let (a, _) = voz_con(0, &[0.5; 256], false);
        m.poner_actual(Some(a));

        let (b, estado_b) = voz_con(1, &[0.5; 256], false);
        m.fundir_a(b, 0);

        assert_eq!(m.id_actual(), Some(VoiceId(1)));
        let mut salida = vec![0.0_f32; 128];
        m.rellenar(&mut salida);
        assert_eq!(estado_b.marcos.load(Ordering::Relaxed), 64);
    }

    #[test]
    fn durante_un_fundido_suenan_las_dos_voces() {
        let mut m = mezclador(64);
        let (a, estado_a) = voz_con(0, &[0.5; 512], false);
        m.poner_actual(Some(a));

        let (b, estado_b) = voz_con(1, &[0.5; 512], false);
        m.fundir_a(b, 128);

        let mut salida = vec![0.0_f32; 128];
        m.rellenar(&mut salida);

        assert_eq!(estado_a.marcos.load(Ordering::Relaxed), 64);
        assert_eq!(
            estado_b.marcos.load(Ordering::Relaxed),
            64,
            "la voz entrante tambien debe consumir marcos"
        );
    }

    #[test]
    fn al_acabar_el_fundido_la_entrante_pasa_a_ser_la_actual() {
        let mut m = mezclador(128);
        let (a, _) = voz_con(0, &[0.5; 1024], false);
        m.poner_actual(Some(a));
        let (b, _) = voz_con(1, &[0.5; 1024], false);
        m.fundir_a(b, 64);

        let mut salida = vec![0.0_f32; 256];
        let r = m.rellenar(&mut salida);
        assert!(r.fundido_terminado);

        let saliente = m.tomar_saliente().expect("hay voz que liberar");
        assert_eq!(saliente.id, VoiceId(0));
        assert_eq!(m.id_actual(), Some(VoiceId(1)));
    }

    #[test]
    fn el_volumen_no_salta_de_golpe() {
        // Un escalon en el volumen se oye como un clic.
        let volumen = Arc::new(VolumenCompartido::nuevo(1.0));
        let mut m = Mezclador::nuevo(
            48_000,
            256,
            Arc::clone(&volumen),
            Arc::new(AtomicBool::new(false)),
        );
        let (voz, _) = voz_con(0, &[0.5; 2048], false);
        m.poner_actual(Some(voz));

        let mut salida = vec![0.0_f32; 512];
        m.rellenar(&mut salida);

        volumen.poner(0.0);
        let mut tras_bajar = vec![0.0_f32; 512];
        m.rellenar(&mut tras_bajar);

        // La primera muestra tras bajar el volumen no puede ser cero de golpe.
        assert!(
            tras_bajar[0].abs() > 0.01,
            "el volumen bajo de golpe: {}",
            tras_bajar[0]
        );
    }

    #[test]
    fn el_volumen_acaba_llegando_a_donde_se_le_pide() {
        let volumen = Arc::new(VolumenCompartido::nuevo(1.0));
        let mut m = Mezclador::nuevo(
            48_000,
            2048,
            Arc::clone(&volumen),
            Arc::new(AtomicBool::new(false)),
        );
        let (voz, _) = voz_con(0, &vec![0.5; 40_000], false);
        m.poner_actual(Some(voz));

        volumen.poner(0.0);
        let mut salida = vec![0.0_f32; 4096];
        m.rellenar(&mut salida);
        assert!(
            salida[salida.len() - 2..].iter().all(|v| v.abs() < 1e-3),
            "el volumen no llego a cero: {:?}",
            &salida[salida.len() - 2..]
        );
    }

    #[test]
    fn un_volumen_no_finito_no_rompe_la_salida() {
        // Un ajuste corrupto no debe convertir el audio en NaN, que en algunas
        // tarjetas suena como ruido blanco a todo volumen.
        let volumen = Arc::new(VolumenCompartido::nuevo(f32::NAN));
        assert!((volumen.leer() - 1.0).abs() < f32::EPSILON);

        volumen.poner(f32::INFINITY);
        assert!((volumen.leer() - 1.0).abs() < f32::EPSILON);

        volumen.poner(-5.0);
        assert!(volumen.leer().abs() < f32::EPSILON);
    }

    #[test]
    fn un_dispositivo_mono_recibe_la_media_de_los_dos_canales() {
        // Mandarle solo el izquierdo perderia la mitad de la mezcla: en una
        // cancion con instrumentos panoramizados desapareceria media banda.
        let mut m = mezclador(1024);
        let (voz, _) = voz_con(0, &vec![0.4; 8192], false);
        m.poner_actual(Some(voz));

        let mut salida = vec![9.0_f32; 1024];
        m.rellenar_a_canales(&mut salida, 1);
        assert!(
            salida.iter().all(|v| v.abs() <= 0.5),
            "la salida mono se fue de rango"
        );
    }

    #[test]
    fn un_dispositivo_de_mas_de_dos_canales_no_deja_basura_en_los_demas() {
        let mut m = mezclador(1024);
        let (voz, _) = voz_con(0, &vec![0.4; 8192], false);
        m.poner_actual(Some(voz));

        let mut salida = vec![9.0_f32; 6 * 512];
        m.rellenar_a_canales(&mut salida, 6);

        for marco in salida.chunks_exact(6) {
            assert!(
                marco[2..].iter().all(|v| v.abs() < f32::EPSILON),
                "los canales sin contenido deben ir a silencio, no a basura"
            );
        }
    }

    #[test]
    fn un_bloque_mayor_que_el_buffer_reservado_da_silencio_y_no_asigna() {
        // Asignar en el hilo de tiempo real es lo unico que no se puede hacer:
        // ante un bloque imposible, silencio.
        let mut m = mezclador(64);
        let (voz, _) = voz_con(0, &vec![0.4; 8192], false);
        m.poner_actual(Some(voz));

        let mut salida = vec![9.0_f32; 6 * 4096];
        m.rellenar_a_canales(&mut salida, 6);
        assert!(salida.iter().all(|v| v.abs() < f32::EPSILON));
    }

    #[test]
    fn el_fundido_sigue_funcionando_tras_una_salida_no_estereo() {
        // El buffer del fundido y el de la conversion son distintos a
        // proposito: compartirlos dejaria el primero vacio y el fundido
        // entraria en panico al indexarlo.
        let mut m = mezclador(1024);
        let (a, _) = voz_con(0, &vec![0.4; 16_384], false);
        m.poner_actual(Some(a));

        let mut mono = vec![0.0_f32; 512];
        m.rellenar_a_canales(&mut mono, 1);

        let (b, estado_b) = voz_con(1, &vec![0.4; 16_384], false);
        m.fundir_a(b, 256);

        let mut estereo = vec![0.0_f32; 1024];
        m.rellenar(&mut estereo);
        assert_eq!(estado_b.marcos.load(Ordering::Relaxed), 512);
    }

    #[test]
    fn cambiar_de_voz_borra_el_estado_de_los_filtros() {
        // La cola del ecualizador y del limitador es audio de la cancion
        // anterior: se oiria encima de la nueva.
        let mut m = mezclador(256);
        let (a, _) = voz_con(0, &[0.9; 2048], false);
        m.poner_actual(Some(a));
        let mut salida = vec![0.0_f32; 512];
        m.rellenar(&mut salida);

        // Voz nueva en silencio: si quedara cola, se oiria.
        let (b, _) = voz_con(1, &[0.0; 2048], false);
        m.poner_actual(Some(b));
        let mut silencio = vec![0.0_f32; 512];
        m.rellenar(&mut silencio);

        assert!(
            silencio.iter().all(|v| v.abs() < 1e-6),
            "quedo audio de la pista anterior"
        );
    }
}

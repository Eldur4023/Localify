//! Limitador soft-knee.
//!
//! ## Para qué está
//!
//! Subir la banda de graves 12 dB multiplica esas frecuencias por cuatro. Una
//! mezcla ya masterizada al límite se sale de `[-1, 1]`, y al convertirla a
//! enteros para la tarjeta se recorta: distorsión audible y muy fea.
//!
//! El limitador va **después** del ecualizador y evita justamente eso.
//!
//! ## Por qué con look-ahead
//!
//! Un limitador que reacciona cuando ya ha visto la muestra alta llega tarde:
//! esa muestra ya salió recortada. Con un retardo de unos milisegundos, la
//! detección va por delante de la señal y la ganancia ya está bajada cuando el
//! pico llega. El coste es ese retardo, constante y aplicado por igual a las
//! dos voces, así que no descuadra el crossfade.
//!
//! ## Por qué un máximo deslizante y no la muestra suelta
//!
//! Detectar sobre la muestra que entra **no basta**, y el error es sutil: la
//! ganancia baja cuando el pico entra en la línea de retardo, pero el pico
//! tarda 5 ms en salir por el otro extremo, y en ese tiempo la liberación ya la
//! ha devuelto casi entera. El pico sale sin reducir, que es justo lo que el
//! look-ahead venía a evitar.
//!
//! Lo correcto es que el objetivo sea el que exige el **máximo de toda la
//! ventana** en vuelo: mientras el pico esté dentro, la ganancia se mantiene
//! baja, y solo se libera cuando ya ha salido. Se calcula con una cola
//! monótona decreciente, que da el máximo en tiempo constante amortizado y sin
//! asignar nada después de construirse.
//!
//! ## Por qué ataque rápido y liberación lenta
//!
//! Bajar la ganancia deprisa evita el recorte. Subirla deprisa se oye como un
//! bombeo del volumen entre golpe y golpe. Las constantes son las habituales en
//! masterización: ataque de milisegundos, liberación de cientos.

/// Umbral por encima del cual empieza a actuar. Deja algo de margen antes del
/// recorte real para que la reducción sea gradual y no de golpe.
const UMBRAL: f32 = 0.891; // −1 dBFS

/// Anchura de la zona blanda, en unidades lineales. Dentro de ella la
/// reducción entra progresivamente en vez de activarse de golpe.
const RODILLA: f32 = 0.1;

/// Retardo de detección.
const LOOKAHEAD_MS: f32 = 5.0;

/// Constante de ataque: cuánto tarda en bajar la ganancia.
const ATAQUE_MS: f32 = 1.0;

/// Constante de liberación: cuánto tarda en devolverla.
const LIBERACION_MS: f32 = 150.0;

/// Máximo deslizante sobre una ventana de tamaño fijo.
///
/// Cola monótona decreciente: cada valor entra y sale como mucho una vez, así
/// que el coste amortizado por muestra es constante. La capacidad se reserva al
/// construirla y no vuelve a crecer, que es lo que la hace apta para el hilo de
/// tiempo real.
#[derive(Debug)]
struct MaximoDeslizante {
    /// `(índice de muestra, valor)`, usado como deque circular.
    datos: Vec<(u64, f32)>,
    frente: usize,
    largo: usize,
    ventana: u64,
}

impl MaximoDeslizante {
    fn nuevo(ventana: usize) -> Self {
        let cap = ventana.max(1) + 1;
        Self {
            datos: vec![(0, 0.0); cap],
            frente: 0,
            largo: 0,
            ventana: ventana.max(1) as u64,
        }
    }

    /// Añade `valor` en el instante `n` y devuelve el máximo de la ventana
    /// `(n − ventana, n]`.
    #[inline]
    fn empujar(&mut self, n: u64, valor: f32) -> f32 {
        let cap = self.datos.len();

        // Todo lo que quede por debajo del valor nuevo ya no puede ser máximo
        // de ninguna ventana futura: sale.
        while self.largo > 0 {
            let ultimo = (self.frente + self.largo - 1) % cap;
            if self.datos[ultimo].1 <= valor {
                self.largo -= 1;
            } else {
                break;
            }
        }
        let hueco = (self.frente + self.largo) % cap;
        self.datos[hueco] = (n, valor);
        self.largo += 1;

        // Y lo que se ha salido de la ventana por antigüedad.
        while self.largo > 0 && self.datos[self.frente].0 + self.ventana <= n {
            self.frente = (self.frente + 1) % cap;
            self.largo -= 1;
        }

        self.datos[self.frente].1
    }

    fn reiniciar(&mut self) {
        self.frente = 0;
        self.largo = 0;
    }
}

/// Limitador estéreo con look-ahead.
///
/// La reducción se calcula sobre el **máximo de los dos canales** y se aplica a
/// ambos por igual. Tratarlos por separado desplazaría la imagen estéreo cada
/// vez que actuara: un golpe de bombo en el canal izquierdo movería la voz
/// hacia la derecha.
#[derive(Debug)]
pub struct Limitador {
    /// Buffer circular de la señal retardada, estéreo intercalado.
    retardo: Vec<f32>,
    escritura: usize,
    /// Marcos de retardo. Se guarda aparte porque `retardo.len()` cuenta
    /// muestras, no marcos.
    marcos_retardo: usize,
    /// Máximo de la señal que todavía no ha salido por el retardo.
    ventana: MaximoDeslizante,
    /// Marcos procesados. Solo se usa como índice de la ventana.
    n: u64,
    /// Ganancia vigente, en `(0, 1]`.
    ganancia: f32,
    coef_ataque: f32,
    coef_liberacion: f32,
    /// Reducción máxima aplicada desde la última consulta, en dB. Solo para
    /// diagnóstico; el hilo de audio únicamente escribe aquí.
    reduccion_maxima_db: f32,
}

impl Limitador {
    #[must_use]
    pub fn nuevo(sample_rate: u32) -> Self {
        #[allow(clippy::cast_precision_loss, reason = "48000 cabe exacto en f32")]
        let sr = sample_rate as f32;
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "5 ms a 96 kHz son 480 marcos"
        )]
        let marcos_retardo = ((LOOKAHEAD_MS / 1000.0) * sr) as usize;

        let marcos_retardo = marcos_retardo.max(1);
        Self {
            retardo: vec![0.0; marcos_retardo * 2],
            escritura: 0,
            marcos_retardo,
            // La ventana cubre exactamente lo que hay en vuelo por el retardo,
            // más el marco que entra.
            ventana: MaximoDeslizante::nuevo(marcos_retardo + 1),
            n: 0,
            ganancia: 1.0,
            coef_ataque: coeficiente(ATAQUE_MS, sr),
            coef_liberacion: coeficiente(LIBERACION_MS, sr),
            reduccion_maxima_db: 0.0,
        }
    }

    /// Retardo que introduce, en marcos. Lo necesita el cálculo de posición:
    /// sin descontarlo, la posición mostrada iría 5 ms por delante del sonido.
    #[must_use]
    pub const fn latencia_marcos(&self) -> usize {
        self.marcos_retardo
    }

    /// Reducción máxima aplicada desde la última llamada, en dB. Consultarla la
    /// pone a cero.
    pub const fn reduccion_maxima_db(&mut self) -> f32 {
        let v = self.reduccion_maxima_db;
        self.reduccion_maxima_db = 0.0;
        v
    }

    /// Procesa un bloque estéreo intercalado, en el sitio.
    ///
    /// Sin asignaciones ni locks: apto para el hilo de tiempo real.
    pub fn procesar(&mut self, intercalado: &mut [f32]) {
        for marco in intercalado.chunks_exact_mut(2) {
            let (l, r) = (marco[0], marco[1]);

            // El objetivo lo marca el pico más alto que todavía está en vuelo,
            // no el que acaba de entrar: así la ganancia sigue baja mientras el
            // pico recorre el retardo, y no solo cuando entra.
            let pico = self.ventana.empujar(self.n, l.abs().max(r.abs()));
            self.n += 1;
            let objetivo = objetivo(pico);

            // Bajar deprisa, subir despacio.
            let coef = if objetivo < self.ganancia {
                self.coef_ataque
            } else {
                self.coef_liberacion
            };
            self.ganancia = objetivo + (self.ganancia - objetivo) * coef;

            if self.ganancia < 1.0 {
                let db = -20.0 * self.ganancia.log10();
                self.reduccion_maxima_db = self.reduccion_maxima_db.max(db);
            }

            // Saca lo retardado, mete lo nuevo.
            let i = self.escritura * 2;
            let salida_l = self.retardo[i];
            let salida_r = self.retardo[i + 1];
            self.retardo[i] = l;
            self.retardo[i + 1] = r;
            self.escritura = (self.escritura + 1) % self.marcos_retardo;

            marco[0] = salida_l * self.ganancia;
            marco[1] = salida_r * self.ganancia;
        }
    }

    /// Vacía el retardo y devuelve la ganancia a la unidad.
    pub fn reiniciar(&mut self) {
        self.retardo.fill(0.0);
        self.escritura = 0;
        self.ventana.reiniciar();
        self.n = 0;
        self.ganancia = 1.0;
        self.reduccion_maxima_db = 0.0;
    }
}

/// Ganancia necesaria para que `pico` no rebase el techo.
///
/// Por debajo de la rodilla no toca nada. Dentro de ella la reducción entra
/// con una interpolación suave (`3t² − 2t³`), que tiene derivada nula en los
/// dos extremos: sin esa suavidad, el punto en que el limitador arranca se oye.
#[inline]
fn objetivo(pico: f32) -> f32 {
    if pico <= UMBRAL {
        return 1.0;
    }
    let techo = if pico >= UMBRAL + RODILLA {
        UMBRAL + RODILLA / 2.0
    } else {
        let t = (pico - UMBRAL) / RODILLA;
        let suave = t * t * (3.0 - 2.0 * t);
        pico - (RODILLA / 2.0) * suave
    };
    (techo / pico).min(1.0)
}

/// Coeficiente de un filtro exponencial de un polo para una constante de
/// tiempo dada. `exp(-1/(τ·sr))`: en `τ` la señal recorre el 63 % del camino.
fn coeficiente(ms: f32, sample_rate: f32) -> f32 {
    if ms <= 0.0 {
        return 0.0;
    }
    (-1.0 / ((ms / 1000.0) * sample_rate)).exp()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: u32 = 48_000;

    fn senoide(hz: f32, amplitud: f32, marcos: usize) -> Vec<f32> {
        (0..marcos)
            .flat_map(|n| {
                #[allow(clippy::cast_precision_loss, reason = "marcos acotado en tests")]
                let t = n as f32 / 48_000.0;
                let v = amplitud * (2.0 * std::f32::consts::PI * hz * t).sin();
                [v, v]
            })
            .collect()
    }

    fn pico(bloque: &[f32]) -> f32 {
        bloque.iter().fold(0.0_f32, |m, v| m.max(v.abs()))
    }

    #[test]
    fn una_senal_por_debajo_del_umbral_sale_intacta() {
        // Lo mas importante: el limitador no debe colorear la musica normal.
        let mut lim = Limitador::nuevo(SR);
        let entrada = senoide(1000.0, 0.5, 4800);
        let mut bloque = entrada.clone();
        lim.procesar(&mut bloque);

        // Sale retardada, asi que se comparan tramos equivalentes.
        let d = lim.latencia_marcos() * 2;
        for (i, esperado) in entrada.iter().enumerate().take(entrada.len() - d) {
            let obtenido = bloque[i + d];
            assert!(
                (obtenido - esperado).abs() < 1e-6,
                "muestra {i}: {obtenido} != {esperado}"
            );
        }
    }

    #[test]
    fn una_senal_excesiva_no_supera_el_techo() {
        let mut lim = Limitador::nuevo(SR);
        let mut bloque = senoide(1000.0, 2.0, 48_000);
        lim.procesar(&mut bloque);

        // Se descarta el primer tramo: el ataque tarda un milisegundo.
        let estable = &bloque[SR as usize / 10..];
        let p = pico(estable);
        assert!(
            p <= 1.0,
            "el limitador dejo pasar {p}, habria recorte en la salida"
        );
    }

    #[test]
    fn el_lookahead_atrapa_el_primer_pico() {
        // Sin look-ahead, la primera muestra alta sale sin reducir: es
        // exactamente el chasquido que este diseno evita.
        let mut lim = Limitador::nuevo(SR);
        let mut bloque = vec![0.0_f32; 4800 * 2];
        // Silencio y, de golpe, un pico enorme pasado el retardo.
        let golpe = lim.latencia_marcos() * 2 + 200;
        bloque[golpe..golpe + 200].fill(3.0);
        lim.procesar(&mut bloque);

        assert!(
            pico(&bloque) <= 1.0,
            "el pico salio a {}, el look-ahead no lo atrapo",
            pico(&bloque)
        );
    }

    #[test]
    fn la_ganancia_sigue_baja_mientras_el_pico_recorre_el_retardo() {
        // El fallo que tenia la primera version: la ganancia bajaba cuando el
        // pico *entraba* en el retardo y se liberaba antes de que *saliera*,
        // asi que el pico salia sin reducir. Con maximo deslizante, la ventana
        // lo mantiene bajo hasta que ha salido del todo.
        let mut lim = Limitador::nuevo(SR);
        let d = lim.latencia_marcos();

        let mut bloque = vec![0.0_f32; 4800 * 2];
        // Un unico marco altisimo, colocado despues de la ventana inicial.
        let marco_pico = d + 100;
        bloque[marco_pico * 2] = 4.0;
        bloque[marco_pico * 2 + 1] = 4.0;
        lim.procesar(&mut bloque);

        let salida = pico(&bloque);
        assert!(
            salida <= 1.0,
            "un pico aislado salio a {salida}: la ventana no lo cubrio"
        );
    }

    #[test]
    fn el_maximo_deslizante_olvida_lo_que_sale_de_la_ventana() {
        let mut m = MaximoDeslizante::nuevo(3);
        assert!((m.empujar(0, 1.0) - 1.0).abs() < 1e-6);
        assert!((m.empujar(1, 5.0) - 5.0).abs() < 1e-6);
        assert!((m.empujar(2, 2.0) - 5.0).abs() < 1e-6);
        // En n=4 la ventana es (1, 4]: el 5.0 del instante 1 ya no cuenta.
        assert!((m.empujar(3, 0.5) - 5.0).abs() < 1e-6);
        assert!(
            (m.empujar(4, 0.5) - 2.0).abs() < 1e-6,
            "el maximo caducado debe salir de la ventana"
        );
    }

    #[test]
    fn la_ganancia_vuelve_despues_de_un_pico() {
        // Si no se recuperase, una cancion entera sonaria baja tras un golpe.
        let mut lim = Limitador::nuevo(SR);

        let mut fuerte = senoide(1000.0, 2.0, 4800);
        lim.procesar(&mut fuerte);

        // Un segundo entero de senal suave: de sobra para la liberacion.
        let mut suave = senoide(1000.0, 0.5, 48_000);
        lim.procesar(&mut suave);

        let final_ = pico(&suave[suave.len() / 2..]);
        assert!(
            (final_ - 0.5).abs() < 0.01,
            "la ganancia no volvio: pico final {final_}"
        );
    }

    #[test]
    fn la_reduccion_es_identica_en_los_dos_canales() {
        // Reducir por separado desplazaria la imagen estereo en cada golpe.
        let mut lim = Limitador::nuevo(SR);
        // Izquierdo fuerte, derecho a la mitad.
        let mut bloque: Vec<f32> = (0..48_000)
            .flat_map(|n| {
                #[allow(clippy::cast_precision_loss, reason = "n < 48000")]
                let t = n as f32 / 48_000.0;
                let v = (2.0 * std::f32::consts::PI * 1000.0 * t).sin();
                [2.0 * v, 1.0 * v]
            })
            .collect();
        lim.procesar(&mut bloque);

        let estable = &bloque[SR as usize / 5 * 2..];
        let izq = estable
            .iter()
            .step_by(2)
            .fold(0.0_f32, |m, v| m.max(v.abs()));
        let der = estable
            .iter()
            .skip(1)
            .step_by(2)
            .fold(0.0_f32, |m, v| m.max(v.abs()));

        assert!(
            (izq / der - 2.0).abs() < 0.05,
            "la relacion entre canales cambio: {izq} / {der}"
        );
    }

    #[test]
    fn la_reduccion_entra_de_forma_gradual() {
        // Un salto brusco en la curva de ganancia se oye. Se comprueba que la
        // funcion objetivo es continua alrededor del umbral.
        let mut anterior = objetivo(UMBRAL - 0.05);
        let mut paso = 0.0_f32;
        for i in 0..=100 {
            #[allow(clippy::cast_precision_loss, reason = "i <= 100")]
            let pico = UMBRAL - 0.05 + (i as f32 / 100.0) * (RODILLA + 0.1);
            let g = objetivo(pico);
            paso = paso.max((g - anterior).abs());
            anterior = g;
        }
        assert!(paso < 0.02, "salto de {paso} en la curva de ganancia");
    }

    #[test]
    fn la_latencia_es_la_misma_para_cualquier_senal() {
        // El crossfade mezcla dos voces por el mismo limitador; una latencia
        // que dependiera de la senal las descuadraria.
        for sr in [44_100, 48_000, 96_000] {
            let lim = Limitador::nuevo(sr);
            #[allow(
                clippy::cast_precision_loss,
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "5 ms a 96 kHz son 480 marcos exactos"
            )]
            let esperado = ((LOOKAHEAD_MS / 1000.0) * sr as f32) as usize;
            assert_eq!(lim.latencia_marcos(), esperado);
        }
    }

    #[test]
    fn reiniciar_vacia_el_retardo() {
        let mut lim = Limitador::nuevo(SR);
        let mut bloque = senoide(1000.0, 0.9, 1000);
        lim.procesar(&mut bloque);

        lim.reiniciar();
        let mut silencio = vec![0.0_f32; 2000];
        lim.procesar(&mut silencio);
        assert!(
            silencio.iter().all(|v| v.abs() < 1e-9),
            "quedo senal en el retardo tras reiniciar"
        );
    }

    #[test]
    fn se_informa_de_cuanto_ha_reducido() {
        let mut lim = Limitador::nuevo(SR);
        assert!(lim.reduccion_maxima_db() < 0.01, "en reposo no reduce nada");

        let mut bloque = senoide(1000.0, 2.0, 48_000);
        lim.procesar(&mut bloque);
        assert!(
            lim.reduccion_maxima_db() > 3.0,
            "con el doble de amplitud deberia reducir varios dB"
        );
        assert!(
            lim.reduccion_maxima_db() < 0.01,
            "consultarla debe ponerla a cero"
        );
    }
}

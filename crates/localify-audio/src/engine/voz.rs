//! El hilo de decodificación de una voz.
//!
//! Aquí sí se puede bloquear: leer de disco, esperar a que la descarga avance,
//! asignar memoria. Su único compromiso es mantener el anillo lleno para que el
//! hilo de audio nunca se quede sin muestras.
//!
//! ## Por qué un anillo de tres segundos
//!
//! El hilo de audio pide unos 10 ms cada vez. Si el de decodificación se
//! retrasa —el planificador lo aparta, el disco tarda, la descarga se atasca—,
//! lo único que separa ese retraso de un corte audible es lo que haya
//! acumulado. Tres segundos absorben cualquier pausa realista del sistema y
//! cuestan 1,1 MB por voz: barato para lo que compran.
//!
//! ## El salto de posición reconstruye la voz
//!
//! Un `seek` tiene que descartar lo que ya está en el anillo, y el productor no
//! puede vaciar un anillo que el consumidor está leyendo. En vez de inventar un
//! mecanismo para ello, saltar **crea una voz nueva** desde la posición pedida
//! y el mezclador la sustituye. Cuesta reabrir el fichero —decenas de
//! milisegundos— y a cambio no hay ningún estado compartido que sincronizar.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use localify_core::domain::audio::DurationMs;
use localify_core::ports::audio_engine::{AudioError, VoiceId};
use rtrb::{Producer, RingBuffer};
use tracing::{debug, warn};

use crate::decode::{Avance, Decodificador};
use crate::engine::mezclador::{EstadoVoz, Voz};
use crate::source::{EstadoDescarga, GrowingFileSource};

/// Segundos de PCM que se mantienen por delante del hilo de audio.
const SEGUNDOS_BUFFER: usize = 3;

/// Espera cuando el anillo está lleno. Corta comparada con los tres segundos
/// que caben, así que el hilo despierta de sobra a tiempo.
const ESPERA_ANILLO: Duration = Duration::from_millis(5);

/// De dónde sale el audio de una voz.
///
/// Se guarda la ruta y el estado de descarga —no un fichero abierto— porque un
/// salto de posición tiene que poder reabrirlo desde cero.
#[derive(Debug, Clone)]
pub struct OrigenAudio {
    pub ruta: PathBuf,
    /// Estado de la descarga. Para un fichero de la biblioteca, uno ya completo.
    pub estado: Arc<EstadoDescarga>,
}

impl OrigenAudio {
    /// Un fichero que ya está entero en disco.
    ///
    /// # Errors
    /// Si no se puede consultar su tamaño.
    pub fn completo(ruta: PathBuf) -> Result<Self, AudioError> {
        let bytes = std::fs::metadata(&ruta)
            .map_err(|e| AudioError::Source(format!("{}: {e}", ruta.display())))?
            .len();
        Ok(Self {
            ruta,
            estado: EstadoDescarga::completo(bytes),
        })
    }

    /// Un `.part` que todavía está creciendo.
    #[must_use]
    pub const fn en_descarga(ruta: PathBuf, estado: Arc<EstadoDescarga>) -> Self {
        Self { ruta, estado }
    }

    fn abrir(&self) -> Result<GrowingFileSource, AudioError> {
        GrowingFileSource::abrir(&self.ruta, Arc::clone(&self.estado))
            .map_err(|e| AudioError::Source(format!("{}: {e}", self.ruta.display())))
    }

    fn extension(&self) -> Option<String> {
        self.ruta
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_owned)
    }
}

/// Lo que el motor conserva de una voz mientras suena.
///
/// Al soltarlo, el hilo de decodificación se entera y termina solo.
pub struct ManejadorVoz {
    pub id: VoiceId,
    pub estado: Arc<EstadoVoz>,
    /// Duración declarada por el contenedor, si la trae.
    pub duracion: Option<DurationMs>,
    /// Posición desde la que arrancó, para calcular la absoluta.
    pub desplazamiento: DurationMs,
    /// Frecuencia a la que se resampleó esta voz.
    ///
    /// Va aquí y no se pregunta a la salida porque **es un dato de la voz**: el
    /// contador de marcos cuenta marcos de *esta* frecuencia, y si el usuario
    /// cambia de dispositivo a mitad de canción la salida pasa a decir otra. Con
    /// la de la salida, la posición de la canción que ya sonaba daba un salto
    /// —de 3:20 a 3:38 al pasar de 44,1 a 48 kHz— sin que nadie hubiera tocado
    /// nada.
    pub sample_rate: u32,
    parar: Arc<AtomicBool>,
    fallo: Arc<Mutex<Option<String>>>,
    hilo: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for ManejadorVoz {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ManejadorVoz")
            .field("id", &self.id)
            .field("duracion", &self.duracion)
            .finish_non_exhaustive()
    }
}

impl ManejadorVoz {
    /// Posición absoluta de la reproducción de esta voz.
    #[must_use]
    pub fn posicion(&self) -> DurationMs {
        self.desde_marcos(self.estado.marcos.load(Ordering::Relaxed))
    }

    /// Hasta dónde llega el audio ya decodificado.
    ///
    /// Con un fichero completo va siempre por delante de la posición. Con uno
    /// en descarga, es exactamente lo que se puede escuchar sin esperar, y por
    /// tanto lo que la interfaz debe pintar como "cargado".
    #[must_use]
    pub fn decodificado(&self) -> DurationMs {
        self.desde_marcos(self.estado.decodificados.load(Ordering::Relaxed))
    }

    fn desde_marcos(&self, marcos: u64) -> DurationMs {
        let ms = if self.sample_rate == 0 {
            0
        } else {
            u32::try_from(marcos * 1000 / u64::from(self.sample_rate)).unwrap_or(u32::MAX)
        };
        DurationMs::new(self.desplazamiento.as_ms().saturating_add(ms))
    }

    /// Motivo del fallo, si la decodificación se cortó por un error.
    #[must_use]
    pub fn fallo(&self) -> Option<String> {
        self.fallo.lock().ok().and_then(|g| g.clone())
    }

    /// Pide al hilo que termine. No espera a que lo haga.
    pub fn detener(&self) {
        self.parar.store(true, Ordering::Release);
    }
}

impl Drop for ManejadorVoz {
    fn drop(&mut self) {
        self.detener();
        // Se espera al hilo: si no, seguiría escribiendo en un anillo cuyo otro
        // extremo puede estar liberándose, y el fichero quedaría abierto
        // impidiendo que Windows lo borre.
        if let Some(h) = self.hilo.take() {
            let _ = h.join();
        }
    }
}

/// Arranca una voz: abre el origen, salta a `desde` y lanza su hilo.
///
/// Devuelve el manejador —para el hilo de control— y la voz —para el
/// mezclador—. La separación es deliberada: el mezclador solo recibe lo que
/// puede tocar sin bloquear.
///
/// # Errors
/// Si el fichero no se puede abrir o decodificar.
pub fn arrancar(
    id: VoiceId,
    origen: &OrigenAudio,
    desde: DurationMs,
    sample_rate: u32,
) -> Result<(ManejadorVoz, Voz), AudioError> {
    let fuente = origen.abrir()?;
    let extension = origen.extension();
    let mut decodificador =
        Decodificador::abrir(Box::new(fuente), extension.as_deref(), sample_rate)?;

    // El salto se hace **antes** de producir nada: así el anillo solo contiene
    // audio de la posición pedida y no hay que descartar nada después.
    let desplazamiento = if desde.is_zero() {
        DurationMs::ZERO
    } else {
        decodificador.buscar(desde).unwrap_or(DurationMs::ZERO)
    };
    let duracion = decodificador.duracion();

    let capacidad = SEGUNDOS_BUFFER * sample_rate.max(1) as usize * 2;
    let (productor, consumidor) = RingBuffer::<f32>::new(capacidad);

    let estado = EstadoVoz::nuevo();
    let fin_de_flujo = Arc::new(AtomicBool::new(false));
    let parar = Arc::new(AtomicBool::new(false));
    let fallo = Arc::new(Mutex::new(None));

    let hilo = std::thread::Builder::new()
        .name(format!("localify-decode-{}", id.0))
        .spawn({
            let fin_de_flujo = Arc::clone(&fin_de_flujo);
            let parar = Arc::clone(&parar);
            let fallo = Arc::clone(&fallo);
            let estado = Arc::clone(&estado);
            move || {
                bucle(
                    decodificador,
                    productor,
                    &fin_de_flujo,
                    &parar,
                    &fallo,
                    &estado,
                );
            }
        })
        .map_err(|e| {
            AudioError::Source(format!("no se pudo crear el hilo de decodificacion: {e}"))
        })?;

    Ok((
        ManejadorVoz {
            id,
            estado: Arc::clone(&estado),
            duracion,
            desplazamiento,
            sample_rate,
            parar,
            fallo,
            hilo: Some(hilo),
        },
        Voz::nueva(id, consumidor, estado, fin_de_flujo),
    ))
}

/// El bucle del hilo: decodificar y verter al anillo hasta el final.
fn bucle(
    mut decodificador: Decodificador,
    mut productor: Producer<f32>,
    fin_de_flujo: &AtomicBool,
    parar: &AtomicBool,
    fallo: &Mutex<Option<String>>,
    estado: &EstadoVoz,
) {
    let mut pcm: Vec<f32> = Vec::with_capacity(16_384);
    let mut escrito = 0_usize;

    loop {
        if parar.load(Ordering::Acquire) {
            break;
        }

        // Si el consumidor desapareció, no hay nadie a quien servir.
        if productor.is_abandoned() {
            break;
        }

        if escrito >= pcm.len() {
            pcm.clear();
            escrito = 0;
            match decodificador.siguiente(&mut pcm) {
                Ok(Avance::Muestras) => {
                    estado
                        .decodificados
                        .fetch_add((pcm.len() / 2) as u64, Ordering::Relaxed);
                }
                Ok(Avance::Fin) => {
                    estado
                        .decodificados
                        .fetch_add((pcm.len() / 2) as u64, Ordering::Relaxed);
                    // Lo último que quedaba se vierte antes de cerrar.
                    verter(&mut productor, &pcm, &mut escrito, parar);
                    fin_de_flujo.store(true, Ordering::Release);
                    debug!("decodificacion terminada");
                    break;
                }
                Err(e) => {
                    warn!(error = %e, "la decodificacion se corto");
                    if let Ok(mut g) = fallo.lock() {
                        *g = Some(e.to_string());
                    }
                    fin_de_flujo.store(true, Ordering::Release);
                    break;
                }
            }
        }

        verter(&mut productor, &pcm, &mut escrito, parar);
    }
}

/// Vierte `pcm[*escrito..]` al anillo, esperando si está lleno.
fn verter(productor: &mut Producer<f32>, pcm: &[f32], escrito: &mut usize, parar: &AtomicBool) {
    while *escrito < pcm.len() {
        if parar.load(Ordering::Acquire) || productor.is_abandoned() {
            return;
        }

        let libres = productor.slots();
        if libres == 0 {
            // El anillo está lleno: es lo normal y lo deseable. Se espera sin
            // consumir CPU hasta que el hilo de audio haga sitio.
            std::thread::sleep(ESPERA_ANILLO);
            continue;
        }

        let n = libres.min(pcm.len() - *escrito);
        let Ok(mut trozo) = productor.write_chunk(n) else {
            return;
        };
        let (a, b) = trozo.as_mut_slices();
        a.copy_from_slice(&pcm[*escrito..*escrito + a.len()]);
        let corte = *escrito + a.len();
        b.copy_from_slice(&pcm[corte..corte + b.len()]);
        trozo.commit_all();
        *escrito += n;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    const SR: u32 = 48_000;

    fn fixture(nombre: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(nombre)
    }

    /// Espera a que la voz produzca al menos `n` muestras o se agote.
    fn esperar_muestras(voz: &Voz, n: usize) -> bool {
        for _ in 0..200 {
            if voz.disponibles() >= n {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        false
    }

    #[test]
    fn una_voz_produce_audio_en_su_anillo() {
        let origen = OrigenAudio::completo(fixture("tono.wav")).expect("origen");
        let (manejador, voz) =
            arrancar(VoiceId(0), &origen, DurationMs::ZERO, SR).expect("arranca");

        assert!(
            esperar_muestras(&voz, 4096),
            "el hilo de decodificacion no lleno el anillo"
        );
        assert_eq!(manejador.duracion.map(DurationMs::as_ms), Some(1000));
    }

    #[test]
    fn arrancar_desde_una_posicion_deja_el_desplazamiento_apuntado() {
        // Sin el desplazamiento, la posicion mostrada volveria a cero tras cada
        // salto y la barra de progreso saltaria hacia atras.
        let origen = OrigenAudio::completo(fixture("tono.wav")).expect("origen");
        let (manejador, voz) =
            arrancar(VoiceId(1), &origen, DurationMs::new(500), SR).expect("arranca");

        assert!(
            manejador.desplazamiento.as_ms().abs_diff(500) < 50,
            "desplazamiento {} ms",
            manejador.desplazamiento.as_ms()
        );
        assert!(
            manejador.posicion().as_ms() >= 450,
            "la posicion debe partir del salto, no de cero"
        );
        assert!(esperar_muestras(&voz, 1024));
    }

    #[test]
    fn al_soltar_el_manejador_el_hilo_termina() {
        // Si el hilo sobreviviera, seguiria con el fichero abierto y Windows no
        // dejaria renombrarlo ni borrarlo.
        let origen = OrigenAudio::completo(fixture("tono.wav")).expect("origen");
        let (manejador, voz) =
            arrancar(VoiceId(2), &origen, DurationMs::ZERO, SR).expect("arranca");
        assert!(esperar_muestras(&voz, 1024));

        // `drop` espera al hilo: si no terminase, este test se colgaria.
        drop(manejador);
    }

    #[test]
    fn un_fichero_que_no_existe_falla_al_arrancar() {
        let origen = OrigenAudio::completo(fixture("no-existe.opus"));
        assert!(origen.is_err(), "deberia fallar al consultar el tamano");
    }

    #[test]
    fn una_voz_completa_marca_su_fin_de_flujo() {
        let origen = OrigenAudio::completo(fixture("tono.ogg")).expect("origen");
        let (_manejador, mut voz) =
            arrancar(VoiceId(3), &origen, DurationMs::ZERO, SR).expect("arranca");

        // El fichero dura un segundo y el anillo aguanta tres: cabe entero.
        let mut consumidas = 0;
        for _ in 0..300 {
            let n = voz.consumidor_mut().slots();
            if n > 0 {
                let trozo = voz.consumidor_mut().read_chunk(n).expect("hay datos");
                trozo.commit_all();
                consumidas += n;
            }
            if voz.ha_terminado() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }

        assert!(voz.ha_terminado(), "la voz nunca se dio por terminada");
        assert!(
            consumidas > SR as usize,
            "se consumieron {consumidas} muestras, se esperaban ~96000"
        );
    }
}

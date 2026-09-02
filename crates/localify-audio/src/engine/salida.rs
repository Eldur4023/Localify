//! La salida por el dispositivo del sistema.
//!
//! ## Por qué un hilo propio
//!
//! `cpal::Stream` no es `Send` en Windows: WASAPI ata el stream al hilo que lo
//! creó. No es un detalle que se pueda esquivar guardándolo en un `Mutex`.
//!
//! Así que el stream vive en un hilo dedicado que no hace nada más: lo crea, lo
//! mantiene vivo y espera órdenes. Cambiar de dispositivo o recuperarse de una
//! desconexión es reconstruirlo **ahí dentro**, sin que nadie más se entere.
//!
//! ## Qué pasa cuando desaparece el dispositivo
//!
//! Desenchufar unos auriculares es lo normal, no una avería. cpal avisa por su
//! callback de error; este hilo lo recoge, reconstruye el stream sobre el
//! dispositivo que quede por defecto y sigue. El mezclador ni se entera: su
//! estado no vive aquí.
//!
//! ## Seguir al sistema, pero solo si se le estaba siguiendo
//!
//! Elegir «predeterminado» en Ajustes significa *el que use el sistema*, así que
//! cuando el usuario cambia la salida de Windows, Localify la cambia con él. Se
//! comprueba desde aquí y no por notificación de WASAPI porque cpal no expone
//! `IMMNotificationClient`, y sondear un identificador dos veces por segundo
//! cuesta menos que mantener un objeto COM vivo con su propio hilo.
//!
//! Elegir un dispositivo **concreto** significa lo contrario: ese y no otro. Ahí
//! no se sigue a nadie, y el único motivo para reconstruir es que desaparezca.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use localify_core::domain::audio::AudioDevice;
use localify_core::ports::audio_engine::AudioError;
use tracing::{debug, info, warn};

use crate::engine::mezclador::Mezclador;

/// Cada cuánto despierta el hilo a comprobar si hay que reconstruir.
const LATIDO: Duration = Duration::from_millis(200);

/// Cada cuánto se le pregunta al sistema cuál es su salida predeterminada.
///
/// Medio segundo: el usuario acaba de cambiarla desde la bandeja del sistema y
/// no percibe ese retraso, y son dos consultas COM por segundo en vez de cinco.
const VIGILANCIA_PREDETERMINADO: Duration = Duration::from_millis(500);

/// Lo que el hilo de salida acepta.
enum Orden {
    /// Reconstruir sobre otro dispositivo. `None` es "el que sea por defecto".
    Dispositivo(Option<String>),
    Cerrar,
}

/// Lo que el hilo de salida publica hacia arriba.
#[derive(Debug)]
pub struct EstadoSalida {
    /// Frecuencia de muestreo negociada con el dispositivo.
    pub sample_rate: AtomicU32,
    /// El dispositivo desapareció y todavía no se ha reconstruido.
    pub perdido: AtomicBool,
    /// Nombre del dispositivo en uso.
    pub dispositivo: Mutex<Option<AudioDevice>>,
}

impl EstadoSalida {
    fn nuevo() -> Arc<Self> {
        Arc::new(Self {
            sample_rate: AtomicU32::new(48_000),
            perdido: AtomicBool::new(false),
            dispositivo: Mutex::new(None),
        })
    }

    /// Identificador del dispositivo sobre el que está montado el stream.
    fn id_en_uso(&self) -> Option<String> {
        self.dispositivo.lock().ok()?.as_ref().map(|d| d.id.clone())
    }
}

/// Handle del hilo de salida.
pub struct Salida {
    ordenes: mpsc::Sender<Orden>,
    estado: Arc<EstadoSalida>,
    hilo: Option<std::thread::JoinHandle<()>>,
}

impl std::fmt::Debug for Salida {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Salida")
            .field("sample_rate", &self.sample_rate())
            .finish_non_exhaustive()
    }
}

impl Salida {
    /// Arranca la salida y devuelve su handle junto al mezclador ya instalado.
    ///
    /// El `Mezclador` se construye **dentro** del hilo, cuando ya se conoce la
    /// frecuencia real del dispositivo: construirlo antes obligaría a adivinarla
    /// y a recalcular todos los filtros después.
    ///
    /// # Errors
    /// Si no hay ningún dispositivo de salida utilizable.
    pub fn arrancar<F>(construir: F) -> Result<Self, AudioError>
    where
        F: FnOnce(u32, usize) -> Arc<Mutex<Mezclador>> + Send + 'static,
    {
        let (tx, rx) = mpsc::channel();
        let estado = EstadoSalida::nuevo();
        let (aviso_tx, aviso_rx) = mpsc::channel::<Result<(), String>>();

        let hilo = std::thread::Builder::new()
            .name("localify-audio-out".to_owned())
            .spawn({
                let estado = Arc::clone(&estado);
                move || bucle(&rx, &estado, construir, &aviso_tx)
            })
            .map_err(|e| AudioError::NoDevice.tap(e))?;

        // Se espera a la primera construcción: si no hay dispositivo, es mejor
        // saberlo aquí que descubrirlo cuando el usuario pulse play. El tope
        // deja margen a los reintentos de `abrir` (`REINTENTOS_APERTURA_INICIAL`
        // más abajo): sin él, un dispositivo que tarda en aparecer -PipeWire
        // arrancando a la vez que Localify, por `exec-once`- se reportaría
        // como ausente aunque el hilo lo consiguiera un instante después.
        match aviso_rx.recv_timeout(Duration::from_secs(8)) {
            Ok(Ok(())) => Ok(Self {
                ordenes: tx,
                estado,
                hilo: Some(hilo),
            }),
            Ok(Err(e)) => {
                warn!(error = %e, "no se pudo abrir el dispositivo de audio");
                Err(AudioError::NoDevice)
            }
            Err(_) => Err(AudioError::NoDevice),
        }
    }

    #[must_use]
    pub fn sample_rate(&self) -> u32 {
        self.estado.sample_rate.load(Ordering::Acquire)
    }

    /// El estado compartido, para quien necesite enterarse de los cambios.
    ///
    /// Lo usa el vigilante del motor: la frecuencia no es un dato fijo del
    /// arranque —cambia si el usuario cambia de salida— y leerla una vez y
    /// guardarla es exactamente el fallo que dejaba la posición desalineada.
    #[must_use]
    pub fn estado(&self) -> Arc<EstadoSalida> {
        Arc::clone(&self.estado)
    }

    #[must_use]
    pub fn dispositivo_actual(&self) -> Option<AudioDevice> {
        self.estado.dispositivo.lock().ok().and_then(|g| g.clone())
    }

    /// Cambia de dispositivo. `None` vuelve al predeterminado.
    ///
    /// # Errors
    /// Si el hilo de salida ya no está.
    pub fn cambiar_dispositivo(&self, id: Option<&str>) -> Result<(), AudioError> {
        self.ordenes
            .send(Orden::Dispositivo(id.map(str::to_owned)))
            .map_err(|_| AudioError::ShuttingDown)
    }
}

impl Drop for Salida {
    fn drop(&mut self) {
        let _ = self.ordenes.send(Orden::Cerrar);
        if let Some(h) = self.hilo.take() {
            let _ = h.join();
        }
    }
}

/// Añade contexto a un error de dispositivo sin perder el tipo.
trait Tap {
    fn tap(self, e: impl std::fmt::Display) -> Self;
}

impl Tap for AudioError {
    fn tap(self, e: impl std::fmt::Display) -> Self {
        warn!(error = %e, "salida de audio");
        self
    }
}

/// El bucle del hilo de salida.
fn bucle<F>(
    ordenes: &mpsc::Receiver<Orden>,
    estado: &Arc<EstadoSalida>,
    construir: F,
    aviso: &mpsc::Sender<Result<(), String>>,
) where
    F: FnOnce(u32, usize) -> Arc<Mutex<Mezclador>> + Send + 'static,
{
    let mut deseado: Option<String> = None;

    // Primera construcción: hay que conocer la frecuencia antes de fabricar el
    // mezclador, así que se abre el dispositivo, se lee su configuración y solo
    // entonces se llama a `construir`.
    let (mut stream, mezclador) = match abrir(deseado.as_deref(), estado, construir) {
        Ok(v) => {
            let _ = aviso.send(Ok(()));
            v
        }
        Err(e) => {
            let _ = aviso.send(Err(e.to_string()));
            return;
        }
    };

    let mut ultima_comprobacion = Instant::now();

    loop {
        match ordenes.recv_timeout(LATIDO) {
            Ok(Orden::Cerrar) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Ok(Orden::Dispositivo(id)) => {
                deseado = id;
                // Solo si de verdad cambia de dispositivo. `aplicar_a_audio`
                // manda esta orden tras **cualquier** cambio de la sección de
                // audio, así que mover el deslizador del crossfade reconstruía
                // el stream y cortaba el sonido un instante por cada pixel.
                if toca_reconstruir(estado, deseado.as_deref()) {
                    estado.perdido.store(true, Ordering::Release);
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }

        // Seguir al sistema **solo** si se le estaba siguiendo: con un
        // dispositivo elegido a mano, que Windows cambie el suyo no es asunto
        // nuestro.
        if deseado.is_none() && ultima_comprobacion.elapsed() >= VIGILANCIA_PREDETERMINADO {
            ultima_comprobacion = Instant::now();
            if toca_reconstruir(estado, None) {
                info!("el sistema ha cambiado de salida predeterminada");
                estado.perdido.store(true, Ordering::Release);
            }
        }

        if estado.perdido.load(Ordering::Acquire) {
            // Soltar el stream viejo antes de abrir el nuevo: WASAPI no da dos
            // sesiones exclusivas sobre el mismo punto final.
            drop(stream);
            match reabrir(deseado.as_deref(), estado, &mezclador) {
                Ok(s) => {
                    stream = s;
                    estado.perdido.store(false, Ordering::Release);
                    info!("dispositivo de audio reconstruido");
                }
                Err(e) => {
                    warn!(error = %e, "no se pudo reconstruir la salida; se reintenta");
                    // Sin dispositivo no hay nada que hacer salvo esperar a que
                    // aparezca uno. Se sigue en el bucle.
                    std::thread::sleep(LATIDO);
                    match reabrir(deseado.as_deref(), estado, &mezclador) {
                        Ok(s) => {
                            stream = s;
                            estado.perdido.store(false, Ordering::Release);
                        }
                        Err(_) => return,
                    }
                }
            }
        }
    }

    debug!("hilo de salida terminado");
}

/// Bloque más grande que la tarjeta puede llegar a pedir.
///
/// Es lo que el mezclador tiene que reservar: si se queda corto, el callback
/// pide más de lo que hay y el hilo de tiempo real no puede asignar para
/// arreglarlo.
#[allow(
    clippy::cast_possible_truncation,
    reason = "el tope de 16384 lo acota muy por debajo de usize"
)]
fn marcos_maximos_de(config: &cpal::SupportedStreamConfig) -> usize {
    match config.buffer_size() {
        cpal::SupportedBufferSize::Range { max, .. } => *max as usize,
        cpal::SupportedBufferSize::Unknown => 4096,
    }
    .clamp(64, 16_384)
}

/// Cuántas veces se reintenta la apertura inicial antes de rendirse, y cada
/// cuánto. 25 × 300 ms ≈ 7,5 s: se queda por debajo de los 8 s que espera
/// `Salida::arrancar` (`aviso_rx.recv_timeout`), para no tener éxito después
/// de que quien llama ya haya desistido.
const REINTENTOS_APERTURA_INICIAL: u32 = 25;
const ESPERA_ENTRE_REINTENTOS: Duration = Duration::from_millis(300);

/// Elige dispositivo, negocia configuración y arranca el stream.
///
/// Si el primer intento falla, reintenta con `reabrir` —el mismo camino que
/// ya existe para cuando el dispositivo desaparece en marcha— antes de
/// rendirse. `construir` es `FnOnce`: solo puede llamarse una vez, así que el
/// mezclador se fabrica aquí, con la configuración del primer intento, y los
/// reintentos posteriores lo reconfiguran en vez de reconstruirlo.
///
/// ## Por qué hace falta
///
/// `elegir` puede encontrar el dispositivo en la enumeración —el sistema ya
/// sabe que existe— antes de que el servidor de sonido (PipeWire, PulseAudio)
/// termine de dejarlo listo para abrir un stream de verdad. Es exactamente lo
/// que pasa al lanzar Localify muy pronto en el inicio de sesión, por
/// ejemplo con `exec-once = localify --headless` en un compositor Wayland:
/// sin reintento, esa carrera se traduce en "sin dispositivo de audio" para
/// el resto de la sesión, aunque el dispositivo esté listo un segundo
/// después.
fn abrir<F>(
    id: Option<&str>,
    estado: &Arc<EstadoSalida>,
    construir: F,
) -> Result<(cpal::Stream, Arc<Mutex<Mezclador>>), AudioError>
where
    F: FnOnce(u32, usize) -> Arc<Mutex<Mezclador>>,
{
    let (dispositivo, config) = elegir(id)?;
    let sample_rate = config.sample_rate();
    let mezclador = construir(sample_rate, marcos_maximos_de(&config));

    match montar(&dispositivo, &config, &mezclador, estado) {
        Ok(stream) => {
            anotar(estado, &dispositivo, sample_rate);
            Ok((stream, mezclador))
        }
        Err(e) => {
            warn!(error = %e, "el primer intento de abrir el dispositivo falló; reintentando");
            let stream = reintentar_apertura(id, estado, &mezclador)?;
            Ok((stream, mezclador))
        }
    }
}

/// Reintenta `reabrir` a intervalos cortos. Ver [`abrir`] para el motivo.
fn reintentar_apertura(
    id: Option<&str>,
    estado: &Arc<EstadoSalida>,
    mezclador: &Arc<Mutex<Mezclador>>,
) -> Result<cpal::Stream, AudioError> {
    for intento in 1..=REINTENTOS_APERTURA_INICIAL {
        std::thread::sleep(ESPERA_ENTRE_REINTENTOS);
        match reabrir(id, estado, mezclador) {
            Ok(stream) => {
                info!(
                    intento,
                    "dispositivo de audio listo tras esperar al servidor de sonido"
                );
                return Ok(stream);
            }
            Err(e) if intento == REINTENTOS_APERTURA_INICIAL => return Err(e),
            Err(_) => {}
        }
    }
    Err(AudioError::NoDevice)
}

/// Reconstruye el stream conservando el mezclador y todo su estado.
///
/// El mezclador sobrevive a propósito —perderlo cortaría la canción— pero sus
/// buffers, su limitador y su rampa de volumen estaban calculados para el
/// dispositivo anterior. Se reajustan **antes** de montar el stream nuevo: si se
/// hiciera después, el primer bloque que pidiera la tarjeta encontraría todavía
/// la configuración vieja.
fn reabrir(
    id: Option<&str>,
    estado: &Arc<EstadoSalida>,
    mezclador: &Arc<Mutex<Mezclador>>,
) -> Result<cpal::Stream, AudioError> {
    let (dispositivo, config) = elegir(id)?;
    let sample_rate = config.sample_rate();

    if let Ok(mut m) = mezclador.lock() {
        m.reconfigurar(sample_rate, marcos_maximos_de(&config));
    }

    let stream = montar(&dispositivo, &config, mezclador, estado)?;
    anotar(estado, &dispositivo, sample_rate);
    Ok(stream)
}

/// `true` si el dispositivo que tocaría usar no es el que está montado.
///
/// Con `None` —«el predeterminado»— se le pregunta al sistema cuál es ahora, que
/// es lo que permite seguirle cuando el usuario lo cambia desde Windows. Si no
/// contesta, se deja lo que hay: quedarse con la salida actual es mejor que
/// reconstruir a ciegas.
fn toca_reconstruir(estado: &Arc<EstadoSalida>, deseado: Option<&str>) -> bool {
    let Some(en_uso) = estado.id_en_uso() else {
        return true;
    };
    match deseado {
        Some(id) => id != en_uso,
        None => cpal::default_host()
            .default_output_device()
            .and_then(|d| d.id().ok())
            .is_some_and(|id| id.to_string() != en_uso),
    }
}

fn anotar(estado: &Arc<EstadoSalida>, dispositivo: &cpal::Device, sample_rate: u32) {
    estado.sample_rate.store(sample_rate, Ordering::Release);
    if let Ok(mut g) = estado.dispositivo.lock() {
        *g = describir(dispositivo);
    }
}

/// Pasa un dispositivo de cpal al tipo del dominio.
///
/// El identificador es el `DeviceId` de cpal y no el nombre visible: el nombre
/// cambia de idioma con el sistema y puede repetirse entre dos tarjetas
/// iguales, mientras que el id está pensado para persistirse en los ajustes.
fn describir(dispositivo: &cpal::Device) -> Option<AudioDevice> {
    let id = dispositivo.id().ok()?.to_string();
    let name = dispositivo
        .description()
        .ok()
        .map_or_else(|| id.clone(), |d| d.name().to_owned());
    let predeterminado = cpal::default_host()
        .default_output_device()
        .and_then(|d| d.id().ok())
        .is_some_and(|d| d.to_string() == id);

    Some(AudioDevice {
        id,
        name,
        is_default: predeterminado,
    })
}

/// Localiza el dispositivo y negocia su configuración predeterminada.
fn elegir(id: Option<&str>) -> Result<(cpal::Device, cpal::SupportedStreamConfig), AudioError> {
    let host = cpal::default_host();

    let dispositivo = match id {
        Some(buscado) => host
            .output_devices()
            .map_err(|_| AudioError::NoDevice)?
            .find(|d| d.id().is_ok_and(|x| x.to_string() == buscado))
            // Un dispositivo guardado en los ajustes puede haberse
            // desenchufado. Volver al predeterminado es mucho mejor que no
            // sonar y dejar al usuario preguntándose por qué.
            .or_else(|| host.default_output_device()),
        None => host.default_output_device(),
    }
    .ok_or(AudioError::NoDevice)?;

    let config = dispositivo
        .default_output_config()
        .map_err(|_| AudioError::NoDevice)?;

    Ok((dispositivo, config))
}

/// Enumera los dispositivos de salida disponibles.
///
/// # Errors
/// Si el host de audio no responde.
pub fn dispositivos() -> Result<Vec<AudioDevice>, AudioError> {
    let host = cpal::default_host();
    Ok(host
        .output_devices()
        .map_err(|_| AudioError::NoDevice)?
        .filter_map(|d| describir(&d))
        .collect())
}

/// Construye el stream con el mezclador dentro del callback.
fn montar(
    dispositivo: &cpal::Device,
    config: &cpal::SupportedStreamConfig,
    mezclador: &Arc<Mutex<Mezclador>>,
    estado: &Arc<EstadoSalida>,
) -> Result<cpal::Stream, AudioError> {
    let canales = config.channels() as usize;
    let mezclador = Arc::clone(mezclador);
    let perdido = Arc::clone(estado);

    let al_fallar = move |e: cpal::Error| {
        warn!(error = %e, "el dispositivo de salida fallo");
        perdido.perdido.store(true, Ordering::Release);
    };

    let stream = match config.sample_format() {
        cpal::SampleFormat::F32 => dispositivo.build_output_stream(
            config.config(),
            move |datos: &mut [f32], _| rellenar(&mezclador, datos, canales),
            al_fallar,
            None,
        ),
        // El resto de formatos los negocia WASAPI en modo compartido; si el
        // predeterminado no fuera f32, es preferible fallar claro aquí que
        // reproducir ruido.
        otro => {
            return Err(AudioError::UnsupportedFormat(format!(
                "el dispositivo pide muestras en {otro}, y el motor produce f32"
            )));
        }
    }
    .map_err(|e| AudioError::NoDevice.tap(e))?;

    stream.play().map_err(|e| AudioError::NoDevice.tap(e))?;
    Ok(stream)
}

/// El callback de audio.
///
/// Toma el lock del mezclador con `try_lock`: el hilo de control solo lo retiene
/// para instalar una voz, que son unos pocos movimientos de puntero, así que la
/// colisión es rarísima. Y si ocurre, se sale un bloque de silencio —10 ms— en
/// vez de bloquear el hilo de tiempo real, que es lo que de verdad se oiría.
fn rellenar(mezclador: &Arc<Mutex<Mezclador>>, datos: &mut [f32], canales: usize) {
    let Ok(mut m) = mezclador.try_lock() else {
        datos.fill(0.0);
        return;
    };

    if canales == 2 {
        m.rellenar(datos);
        return;
    }

    // Un dispositivo mono o multicanal: se mezcla a estéreo en un buffer del
    // propio mezclador y se reparte. No se asigna nada aquí.
    m.rellenar_a_canales(datos, canales);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Un estado que dice estar montado sobre `id`.
    fn montado_en(id: &str) -> Arc<EstadoSalida> {
        let estado = EstadoSalida::nuevo();
        if let Ok(mut g) = estado.dispositivo.lock() {
            *g = Some(AudioDevice {
                id: id.to_owned(),
                name: id.to_owned(),
                is_default: false,
            });
        }
        estado
    }

    /// El identificador del predeterminado del sistema, si esta maquina tiene.
    fn predeterminado() -> Option<String> {
        cpal::default_host()
            .default_output_device()
            .and_then(|d| d.id().ok())
            .map(|i| i.to_string())
    }

    #[test]
    fn con_un_dispositivo_elegido_a_mano_no_se_sigue_al_sistema() {
        // La regla que pidió el usuario: si ha elegido una salida concreta, que
        // Windows cambie la suya no debe moverle la música de sitio.
        let estado = montado_en("los-cascos");
        assert!(
            !toca_reconstruir(&estado, Some("los-cascos")),
            "el dispositivo pedido ya es el montado: no hay nada que reconstruir"
        );
    }

    #[test]
    fn con_el_predeterminado_se_sigue_al_sistema() {
        let Some(pred) = predeterminado() else {
            return; // sin tarjeta de sonido en esta maquina
        };

        // Montado sobre algo que ya no es el predeterminado: hay que seguirle.
        let viejo = montado_en("una-salida-que-ya-no-es-la-del-sistema");
        assert!(toca_reconstruir(&viejo, None));

        // Y montado sobre el predeterminado, no se toca nada. Sin esto, el
        // sondeo reconstruiria el stream dos veces por segundo para siempre.
        let actual = montado_en(&pred);
        assert!(!toca_reconstruir(&actual, None));
    }

    #[test]
    fn cambiar_a_otro_dispositivo_si_reconstruye() {
        let estado = montado_en("los-cascos");
        assert!(toca_reconstruir(&estado, Some("los-altavoces")));
    }

    #[test]
    fn sin_stream_montado_siempre_toca_reconstruir() {
        // Es el arranque: no hay nada, así que hay que abrir algo.
        assert!(toca_reconstruir(&EstadoSalida::nuevo(), None));
    }

    #[test]
    fn sin_dispositivo_no_se_finge_que_lo_hay() {
        // `elegir` con un nombre inexistente debe caer al predeterminado, no
        // fallar: un dispositivo guardado en ajustes puede haberse desenchufado.
        let resultado = elegir(Some("dispositivo-que-no-existe-12345"));
        match resultado {
            // Hay tarjeta de sonido: debe haber vuelto al predeterminado.
            Ok((d, _)) => assert!(describir(&d).is_some()),
            // Sin tarjeta (CI sin audio): el error debe ser claro.
            Err(e) => assert!(matches!(e, AudioError::NoDevice), "{e}"),
        }
    }

    #[test]
    fn los_dispositivos_se_identifican_por_id_y_no_por_nombre() {
        // El nombre visible cambia con el idioma del sistema y se repite entre
        // dos tarjetas iguales; guardarlo en los ajustes seria fragil.
        let Ok(lista) = dispositivos() else {
            return; // sin audio en esta maquina
        };
        for d in &lista {
            assert!(
                !d.id.is_empty(),
                "un dispositivo sin id no se puede guardar"
            );
        }
        let ids: std::collections::HashSet<_> = lista.iter().map(|d| &d.id).collect();
        assert_eq!(ids.len(), lista.len(), "hay identificadores repetidos");
    }
}

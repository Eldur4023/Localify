//! Servicio de descargas, implementado como actor.
//!
//! Convierte "quiero esta pista" en un fichero local completo y etiquetado, sin
//! que el usuario tenga que gestionar nada: no hay botón de descargar, no hay
//! gestor, no hay cola que atender. Lo que sí se ve, pasivamente, es *si* algo
//! está pasando: el progreso de una descarga en curso y un indicador en lo ya
//! descargado, para que una canción que tarda no se confunda con una rota.
//!
//! ## Lo que este servicio NO tiene
//!
//! No hay `cancel` ni `pause`. No es una funcionalidad pendiente: **no existen
//! en el diseño** (ADR-016). Cambiar de canción no cancela nada; las dos
//! descargas continúan. Codificarlo así, y no en un comentario, hace la regla
//! imposible de violar por descuido.
//!
//! ## Dos carriles
//!
//! - `Immediate`: el usuario pulsó play y está esperando.
//! - `Prefetch`: las siguientes de la cola.
//!
//! Cada uno con su propio límite de concurrencia, para que precargar nunca
//! robe ancho de banda a lo que suena ahora.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use localify_core::domain::availability::Availability;
use localify_core::domain::download::{DownloadJob, DownloadProgress, DownloadState, Priority};
use localify_core::domain::ids::TrackId;
use localify_core::domain::library::{AudioFileRecord, AudioSource};
use localify_core::domain::settings::FormatPreference;
use localify_core::error::{CoreError, CoreResult};
use localify_core::events::{DomainEvent, EventPublisher};
use localify_core::ports::database::{
    AudioFileRepository, DownloadJobRepository, TrackRepository, YoutubeMatchRepository,
};
use localify_core::ports::platform::{AppPaths, FileSystem};
use localify_core::ports::services::{DownloadHandle, DownloadService};
use localify_core::ports::youtube::{
    DownloadObserver, TagWriter, YoutubeDownloader, YoutubeMatcher,
};
use tokio::sync::{Semaphore, mpsc, oneshot};
use tracing::{debug, info, warn};

/// Descargas simultáneas por carril.
const CONCURRENCIA_POR_CARRIL: usize = 2;

/// Esperas entre intentos, por defecto.
///
/// El número de intentos se deduce de aquí: una descarga se prueba una vez más
/// que esperas hay. Mantener un solo dato evita que ambos se desincronicen.
pub const BACKOFF_POR_DEFECTO: [Duration; 2] = [Duration::from_secs(2), Duration::from_secs(8)];

/// Frecuencia máxima de eventos de progreso, por descarga.
///
/// Sin este límite, una descarga rápida emitiría cientos de eventos por segundo
/// y saturaría el puente IPC para mover una barra que nadie está mirando.
const INTERVALO_PROGRESO: Duration = Duration::from_millis(500);

/// Dependencias del servicio.
pub struct Dependencias {
    pub matcher: Arc<dyn YoutubeMatcher>,
    /// El catálogo, solo para preguntarle si ya sabe qué vídeo es esta pista.
    ///
    /// Es la única razón por la que el actor de descargas conoce el proveedor de
    /// metadatos, y merece la pena: con la respuesta no hay que emparejar nada,
    /// y un emparejamiento equivocado queda grabado para siempre porque lo
    /// descargado no se vuelve a descargar.
    pub provider: Arc<dyn localify_core::ports::metadata_provider::MetadataProvider>,
    pub downloader: Arc<dyn YoutubeDownloader>,
    pub tagger: Arc<dyn TagWriter>,
    pub tracks: Arc<dyn TrackRepository>,
    pub audio: Arc<dyn AudioFileRepository>,
    pub jobs: Arc<dyn DownloadJobRepository>,
    pub matches: Arc<dyn YoutubeMatchRepository>,
    pub fs: Arc<dyn FileSystem>,
    pub paths: Arc<dyn AppPaths>,
    pub bus: Arc<dyn EventPublisher>,
    pub formato: FormatPreference,
    /// Esperas entre reintentos. Ver [`BACKOFF_POR_DEFECTO`].
    pub backoff: Vec<Duration>,
}

impl Dependencias {
    /// Intentos totales: uno inicial más uno por cada espera configurada.
    fn intentos(&self) -> u8 {
        u8::try_from(self.backoff.len().saturating_add(1)).unwrap_or(u8::MAX)
    }
}

impl std::fmt::Debug for Dependencias {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Dependencias").finish_non_exhaustive()
    }
}

enum Comando {
    Ensure {
        track: TrackId,
        priority: Priority,
        responder: oneshot::Sender<CoreResult<DownloadHandle>>,
    },
    Terminado {
        track: TrackId,
    },
}

/// Handle público del servicio. Barato de clonar.
#[derive(Clone)]
pub struct DownloadActor {
    tx: mpsc::Sender<Comando>,
    deps: Arc<Dependencias>,
}

impl std::fmt::Debug for DownloadActor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DownloadActor").finish_non_exhaustive()
    }
}

impl DownloadActor {
    /// Arranca el actor y devuelve su handle.
    #[must_use]
    pub fn arrancar(deps: Dependencias) -> Self {
        let deps = Arc::new(deps);
        let (tx, rx) = mpsc::channel(256);

        let handle = Self {
            tx: tx.clone(),
            deps: Arc::clone(&deps),
        };
        tokio::spawn(bucle(rx, tx, deps));
        handle
    }

    /// Descarta lo que quedó a medias en la sesión anterior.
    ///
    /// Se borran los `.part` y sus filas: yt-dlp puede reanudar por su cuenta,
    /// pero arriesgarse a un fichero mal concatenado violaría "nunca dejar
    /// archivos corruptos". Nada se reencola aquí; la próxima vez que alguien
    /// pida la pista, se descargará entera y limpia.
    ///
    /// Después se barre `.tmp/` completo. Un corte de luz entre crear el
    /// fichero y persistir la fila deja un `.part` sin dueño que ninguna
    /// consulta encontraría, y que se quedaría ocupando disco para siempre.
    ///
    /// # Errors
    /// Si la consulta a la base de datos falla.
    pub async fn limpiar_interrumpidos(&self) -> CoreResult<u32> {
        let interrumpidos = self.deps.jobs.interrupted().await?;
        let mut descartados = 0_u32;

        for job in interrumpidos {
            if let Some(tmp) = &job.tmp_path {
                let _ = self.deps.fs.remove_file(tmp).await;
            }
            self.deps.jobs.delete(&job.track_id).await?;
            descartados += 1;
        }

        let huerfanos = self
            .deps
            .fs
            .clear_dir(&self.deps.paths.temp_dir())
            .await
            .unwrap_or(0);

        if descartados > 0 || huerfanos > 0 {
            info!(descartados, huerfanos, "temporales de descarga purgados");
        }
        Ok(descartados)
    }
}

/// Estado que posee el actor en exclusiva.
struct Estado {
    /// Descargas en curso, para no duplicar trabajo.
    en_curso: HashMap<TrackId, PathBuf>,
    /// Un permiso por carril limita la concurrencia sin frenar al actor.
    inmediato: Arc<Semaphore>,
    prefetch: Arc<Semaphore>,
}

async fn bucle(
    mut rx: mpsc::Receiver<Comando>,
    tx: mpsc::Sender<Comando>,
    deps: Arc<Dependencias>,
) {
    let mut estado = Estado {
        en_curso: HashMap::new(),
        inmediato: Arc::new(Semaphore::new(CONCURRENCIA_POR_CARRIL)),
        prefetch: Arc::new(Semaphore::new(CONCURRENCIA_POR_CARRIL)),
    };

    while let Some(comando) = rx.recv().await {
        match comando {
            Comando::Ensure {
                track,
                priority,
                responder,
            } => {
                let resultado = atender(&mut estado, &deps, &tx, &track, priority).await;

                // Un fichero en crecimiento todavía no se puede abrir: hay que
                // dejar que yt-dlp escriba la cabecera. Esa espera va en una
                // tarea aparte y **nunca en el bucle**, que es la invariante de
                // este actor: esperar aquí congelaría todas las peticiones
                // siguientes durante segundos.
                match resultado {
                    Ok(handle) if !handle.complete => {
                        let deps = Arc::clone(&deps);
                        tokio::spawn(async move {
                            let listo =
                                esperar_a_que_sea_reproducible(&deps, &track, handle.playable_path)
                                    .await;
                            let _ = responder.send(listo);
                        });
                    }
                    otro => {
                        let _ = responder.send(otro);
                    }
                }
            }
            Comando::Terminado { track } => {
                estado.en_curso.remove(&track);
            }
        }
    }

    debug!("el actor de descargas termina");
}

/// Resuelve una petición sin bloquear el bucle.
///
/// El actor **nunca espera a una descarga dentro de su bucle**: si lo hiciera,
/// una descarga lenta congelaría todas las peticiones siguientes. El trabajo
/// pesado va a una tarea hija que avisa al terminar.
async fn atender(
    estado: &mut Estado,
    deps: &Arc<Dependencias>,
    tx: &mpsc::Sender<Comando>,
    track: &TrackId,
    priority: Priority,
) -> CoreResult<DownloadHandle> {
    // Si ya está en disco, no se toca nada. Es la invariante del proyecto: una
    // pista descargada nunca se vuelve a descargar.
    if let Some(registro) = deps.audio.get(track).await? {
        return Ok(DownloadHandle {
            playable_path: deps.paths.resolve(&registro.rel_path),
            complete: true,
        });
    }

    // Ya hay un trabajo en marcha: se engancha a él en lugar de duplicarlo.
    if let Some(ruta) = estado.en_curso.get(track) {
        debug!(pista = %track, "descarga ya en curso; se comparte");
        return Ok(DownloadHandle {
            playable_path: ruta.clone(),
            complete: false,
        });
    }

    let extension = localify_ytdlp_extension(deps.formato);
    let temporal = deps
        .paths
        .temp_dir()
        .join(format!("{track}.{extension}.part"));

    estado.en_curso.insert(track.clone(), temporal.clone());

    let permisos = match priority {
        Priority::Immediate => Arc::clone(&estado.inmediato),
        Priority::Prefetch => Arc::clone(&estado.prefetch),
    };

    let deps = Arc::clone(deps);
    let tx = tx.clone();
    let id = track.clone();
    let destino = temporal.clone();

    tokio::spawn(async move {
        // El permiso se toma dentro de la tarea: así el actor sigue atendiendo
        // peticiones mientras las descargas esperan su turno.
        let _permiso = permisos.acquire().await;
        let resultado = ejecutar(&deps, &id, &destino, priority).await;

        if let Err(e) = &resultado {
            warn!(pista = %id, error = %e, "descarga fallida");
        }
        let _ = tx.send(Comando::Terminado { track: id }).await;
    });

    Ok(DownloadHandle {
        playable_path: temporal,
        complete: false,
    })
}

/// Bytes mínimos antes de dar una ruta por reproducible.
///
/// Abrir un WebM de doscientos bytes no falla por poco: el decodificador no
/// encuentra ni la cabecera y descarta el fichero como formato desconocido.
/// Con esto hay cabecera y algún bloque, que es lo que necesita para arrancar.
const BYTES_MINIMOS: u64 = 64 * 1024;

/// Cada cuánto se mira si el temporal ya sirve.
const INTERVALO_ESPERA: Duration = Duration::from_millis(50);

/// Cuánto se espera como mucho.
///
/// ## Este plazo cubre el emparejamiento, no solo la descarga
///
/// Decía "cinco segundos cubren de sobra el arranque de yt-dlp", y era cierto
/// para lo que medía: el tiempo entre lanzar la descarga y los primeros bytes.
/// Pero el reloj arranca cuando el usuario pulsa play, y antes de descargar hay
/// que **decidir qué vídeo**: varias consultas a yt-dlp de unos dos segundos y
/// medio cada una.
///
/// El resultado era esto, sacado de un log real:
///
/// ```text
/// 17:51:21.97  el reproductor pide la pista
/// 17:51:25.64  emparejamiento resuelto
/// 17:51:25.72  "la pista no llegó a ser reproducible"
/// 17:51:29.00  descarga completada
/// ```
///
/// Se rendía ochenta milisegundos después de saber qué bajar, y la canción
/// terminaba de descargarse tres segundos más tarde. El usuario veía un error
/// sobre una canción que ya estaba en su disco.
///
/// Treinta segundos no dejan a nadie esperando de más: el camino que de verdad
/// falla —una pista sin resultados en YouTube— sale por la comprobación de
/// `Failed`, que corta en menos de un segundo. Este plazo solo lo agota una red
/// muy lenta, y ahí esperar es exactamente lo que hay que hacer.
const ESPERA_MAXIMA: Duration = Duration::from_secs(30);

/// Espera a que la pista se pueda abrir de verdad, y dice por dónde.
///
/// ## Por qué está aquí y no en quien reproduce
///
/// `ensure` promete devolver una "ruta ya reproducible", y durante un tiempo no
/// lo cumplió: devolvía el `.part` en el mismo instante en que se lanzaba
/// yt-dlp, así que el motor intentaba abrir un fichero que todavía no existía y
/// la reproducción moría con "el sistema no puede encontrar el archivo". La
/// descarga terminaba bien unos segundos después y nadie reintentaba.
///
/// Arreglarlo en el reproductor —que esperase él— habría repartido por dos
/// sitios el conocimiento de cuándo un `.part` sirve. La promesa la hace esta
/// función, así que la cumple esta función.
///
/// ## Se vigilan las dos salidas, no solo una
///
/// Mientras se espera, la descarga puede **terminar**: el `.part` se renombra
/// al fichero definitivo y la ruta que se iba a devolver deja de existir.
/// Devolverla entonces reproduce el mismo fallo que esto venía a arreglar, solo
/// que en canciones cortas o conexiones rápidas. Por eso se comprueban las dos
/// cosas en cada vuelta y gana la que ocurra antes.
///
/// La tercera salida es rendirse: si el trabajo ya consta como fallido, el
/// tiempo restante es espera pura y el usuario merece saberlo ya.
async fn esperar_a_que_sea_reproducible(
    deps: &Arc<Dependencias>,
    track: &TrackId,
    temporal: PathBuf,
) -> CoreResult<DownloadHandle> {
    let limite = std::time::Instant::now() + ESPERA_MAXIMA;

    while std::time::Instant::now() < limite {
        // ¿Terminó? Entonces la buena es la definitiva.
        if let Ok(Some(registro)) = deps.audio.get(track).await {
            return Ok(DownloadHandle {
                playable_path: deps.paths.resolve(&registro.rel_path),
                complete: true,
            });
        }
        // ¿Hay ya cabecera y algún bloque? Se puede empezar a sonar.
        if deps.fs.file_size(&temporal).await.unwrap_or(0) >= BYTES_MINIMOS {
            return Ok(DownloadHandle {
                playable_path: temporal,
                complete: false,
            });
        }
        // ¿Ya se rindió? Entonces nadie está escribiendo ese fichero y esperar
        // los cinco segundos completos solo retrasa el aviso. Una pista sin
        // resultado en YouTube fracasa en menos de un segundo.
        if deps
            .jobs
            .get(track)
            .await
            .ok()
            .flatten()
            .is_some_and(|j| j.state == DownloadState::Failed)
        {
            break;
        }
        tokio::time::sleep(INTERVALO_ESPERA).await;
    }

    // Antes se devolvía el temporal igualmente. Era lo peor de las dos
    // opciones: el motor abría un fichero vacío, symphonia respondía "formato
    // no soportado" —que no tiene nada que ver con lo que pasa— y el
    // reproductor se quedaba con la canción cargada, en pausa y sin avanzar,
    // sin decir por qué.
    //
    // Fallar aquí hace que el error viaje por el camino de errores, que ya
    // sabe enseñar un aviso al usuario y reintentar.
    warn!(
        pista = %track,
        fichero = %temporal.display(),
        "la pista no llegó a ser reproducible"
    );
    Err(CoreError::not_found("audio_reproducible", track.as_str()))
}

/// Extensión del temporal según la preferencia.
const fn localify_ytdlp_extension(preferencia: FormatPreference) -> &'static str {
    match preferencia {
        FormatPreference::Opus => "webm",
        FormatPreference::M4a => "m4a",
        FormatPreference::Best => "bin",
    }
}

/// Publica el progreso de una descarga, limitando la frecuencia.
struct Observador {
    track: TrackId,
    bus: Arc<dyn EventPublisher>,
    /// Para persistir el progreso real, no solo emitirlo. Sin esto, una
    /// consulta de snapshot (`library_availability`) veía siempre 0% a mitad
    /// de descarga: el evento en vivo llevaba el número correcto, pero la fila
    /// en la base de datos se quedaba con lo que puso `guardar_estado` al
    /// entrar en `Downloading`.
    jobs: Arc<dyn DownloadJobRepository>,
    priority: Priority,
    video: String,
    attempts: u8,
    ultimo: std::sync::Mutex<std::time::Instant>,
    avisado: std::sync::atomic::AtomicBool,
}

impl DownloadObserver for Observador {
    fn on_progress(&self, progress: &DownloadProgress) {
        let Some(fraccion) = progress.fraccion() else {
            return;
        };

        // El límite se aplica aquí, en el emisor, y no en el consumidor: nadie
        // debería tener que defenderse de una avalancha que podemos no causar.
        let ahora = std::time::Instant::now();
        let toca = self.ultimo.lock().is_ok_and(|mut u| {
            if ahora.duration_since(*u) >= INTERVALO_PROGRESO {
                *u = ahora;
                true
            } else {
                false
            }
        });

        if !toca {
            return;
        }

        self.bus.publish(DomainEvent::DownloadProgress {
            track_id: self.track.clone(),
            percent: fraccion,
        });

        // `on_progress` es síncrono y llega desde el hilo que lee la salida de
        // yt-dlp; persistir es async, así que se lanza una tarea aparte y no
        // se espera. Puede haber una carrera cosmética e inofensiva —un
        // progreso tardío que se escribe justo después de que `finalizar` ya
        // haya movido el trabajo a `Finalizing`—, pero el siguiente evento (o
        // el borrado del trabajo al terminar) lo corrige enseguida.
        let jobs = Arc::clone(&self.jobs);
        let job = DownloadJob {
            track_id: self.track.clone(),
            state: DownloadState::Downloading,
            priority: self.priority,
            video_id: Some(self.video.clone()),
            tmp_path: None,
            bytes_done: progress.bytes_done,
            bytes_total: progress.bytes_total,
            attempts: self.attempts,
            last_error_key: None,
        };
        tokio::spawn(async move {
            if let Err(e) = jobs.upsert(&job).await {
                warn!(error = %e, "no se pudo persistir el progreso de la descarga");
            }
        });
    }

    fn on_playable(&self, _ruta: &std::path::Path) {
        // Una sola vez: el aviso dispara la reproducción, y repetirlo haría que
        // el reproductor volviera a cargar el fichero a media canción.
        if self
            .avisado
            .swap(true, std::sync::atomic::Ordering::Relaxed)
        {
            return;
        }
        self.bus.publish(DomainEvent::DownloadPlayable {
            track_id: self.track.clone(),
        });
    }
}

/// Ejecuta el pipeline completo, con reintentos.
async fn ejecutar(
    deps: &Arc<Dependencias>,
    track: &TrackId,
    temporal: &std::path::Path,
    priority: Priority,
) -> CoreResult<()> {
    deps.bus.publish(DomainEvent::DownloadStarted {
        track_id: track.clone(),
    });

    let mut ultimo: Option<CoreError> = None;

    for intento in 0..deps.intentos() {
        if intento > 0 {
            let espera = deps
                .backoff
                .get(usize::from(intento) - 1)
                .copied()
                .unwrap_or(Duration::from_secs(30));
            tokio::time::sleep(espera).await;
        }

        match intentar(deps, track, temporal, priority, intento).await {
            Ok(()) => return Ok(()),
            Err(e) => {
                // Un fallo que no mejora reintentando corta la serie: sin
                // coincidencia fiable, insistir gastaría red y acabaría
                // metiendo basura en la biblioteca (ADR-017).
                let definitivo = matches!(e.code(), "NOT_FOUND" | "NOT_CONFIGURED" | "INVALID");
                ultimo = Some(e);
                if definitivo {
                    break;
                }
            }
        }
    }

    let error = ultimo.unwrap_or_else(|| CoreError::internal("descarga fallida sin causa"));
    marcar_fallida(deps, track, &error).await;
    Err(error)
}

/// Un intento completo del pipeline: emparejar, descargar y finalizar.
async fn intentar(
    deps: &Arc<Dependencias>,
    track: &TrackId,
    temporal: &std::path::Path,
    priority: Priority,
    intento: u8,
) -> CoreResult<()> {
    let pista = deps
        .tracks
        .get(track)
        .await?
        .ok_or_else(|| CoreError::not_found("track", track.as_str()))?;

    let video = emparejar(deps, track, &pista, priority, intento).await?;
    let descargado = descargar(deps, track, &pista, &video, temporal, priority, intento).await?;
    finalizar(deps, track, &pista, &video, descargado, priority, intento).await
}

/// Elige el vídeo del que traer el audio.
///
/// # Errors
/// Si no hay ningún candidato de confianza suficiente.
async fn emparejar(
    deps: &Arc<Dependencias>,
    track: &TrackId,
    pista: &localify_core::domain::track::Track,
    priority: Priority,
    intento: u8,
) -> CoreResult<String> {
    // ── Atajo: la pista ya sabe cuál es su vídeo ────────────────────────────
    //
    // Cuando el catálogo es YouTube Music, el identificador de la pista **es**
    // el del vídeo (ver `domain::ids`). No hay dos catálogos que emparejar, así
    // que buscar candidatos y puntuarlos sería salir a la red para reencontrar
    // algo que ya se tiene.
    //
    // Todo el sistema de puntuación sigue existiendo y sigue haciendo falta:
    // con Spotify como catálogo, el identificador no dice nada de YouTube y
    // hay que emparejar. Este atajo no lo sustituye, lo evita cuando sobra.
    if let Some(video) = video_id_directo(track) {
        debug!(pista = %track, "el identificador ya es el del vídeo: sin emparejamiento");
        return Ok(video);
    }

    guardar_estado(
        deps,
        track,
        DownloadState::Matching,
        priority,
        None,
        intento,
    )
    .await;

    let excluidos = deps.matches.rejected_ids(track).await?;

    // Antes de adivinar, preguntar al catálogo. Sabe responder de tres formas:
    // porque el identificador de la pista **es** el del vídeo (YouTube Music),
    // porque tiene la relación guardada (MusicBrainz), o buscándola en su propio
    // catálogo, que es de música y no de vídeos de YouTube en general.
    //
    // Un fallo aquí no interrumpe nada: se sigue por el camino de siempre. Es
    // una pista, no un requisito, y la descarga tiene que funcionar igual con un
    // catálogo que no sepa nada de YouTube.
    let resuelta = match deps.provider.resolve_recording(pista).await {
        Ok(v) => v,
        Err(e) => {
            debug!(pista = %track, error = %e, "no se pudo preguntar por el vídeo oficial");
            None
        }
    };

    let conocido = resuelta.map(|r| r.video_id);

    let emparejamiento = deps
        .matcher
        .find_best(pista, &excluidos, conocido.as_deref())
        .await?;
    deps.matches.save(&emparejamiento).await?;

    // Confianza baja: **no se descarga**. Una biblioteca más pequeña vale más
    // que una con karaokes, porque lo descargado no se vuelve a descargar.
    if !emparejamiento.confidence.permite_descarga_automatica() {
        info!(
            pista = %track,
            score = emparejamiento.best.score,
            "sin coincidencia fiable; no se descarga"
        );
        return Err(CoreError::not_found("youtube_match", track.as_str()));
    }

    Ok(emparejamiento.best.video_id)
}

/// Longitud de un identificador de vídeo de YouTube.
const LONGITUD_VIDEO: usize = 11;

/// El identificador de vídeo, si la pista **es** un vídeo de YouTube.
///
/// Distinguirlo por la forma y no por un campo aparte es lo que permite que
/// pistas de los dos catálogos convivan en la misma biblioteca sin una columna
/// que diga de dónde vino cada una: once caracteres en base64url solo los tiene
/// un vídeo, y los identificadores de Spotify son veintidós.
fn video_id_directo(track: &TrackId) -> Option<String> {
    let s = track.as_str();
    let forma_de_video = s.len() == LONGITUD_VIDEO
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');

    forma_de_video.then(|| s.to_owned())
}

/// Trae el audio al fichero temporal, ya verificado.
async fn descargar(
    deps: &Arc<Dependencias>,
    track: &TrackId,
    pista: &localify_core::domain::track::Track,
    video: &str,
    temporal: &std::path::Path,
    priority: Priority,
    intento: u8,
) -> CoreResult<localify_core::ports::youtube::DownloadedFile> {
    guardar_estado(
        deps,
        track,
        DownloadState::Downloading,
        priority,
        Some(video),
        intento,
    )
    .await;

    let observador = Observador {
        track: track.clone(),
        bus: Arc::clone(&deps.bus),
        jobs: Arc::clone(&deps.jobs),
        priority,
        video: video.to_owned(),
        attempts: intento + 1,
        ultimo: std::sync::Mutex::new(std::time::Instant::now()),
        avisado: std::sync::atomic::AtomicBool::new(false),
    };

    match deps
        .downloader
        .download(video, deps.formato, temporal, pista.duration, &observador)
        .await
    {
        Ok(d) => Ok(d),
        Err(e) => {
            // El vídeo no sirve: se excluye para que el siguiente intento
            // busque otro en lugar de repetir el mismo error.
            if e.code() == "PROVIDER_UNAVAILABLE" {
                let _ = deps.matches.reject(track, video).await;
            }
            let _ = deps.fs.remove_file(temporal).await;
            Err(e)
        }
    }
}

/// Etiqueta, mueve a la biblioteca y registra.
///
/// El orden importa: hasta el rename atómico, la biblioteca no sabe nada del
/// fichero. Es lo que garantiza que en `audio/` no aparezca jamás algo
/// incompleto.
async fn finalizar(
    deps: &Arc<Dependencias>,
    track: &TrackId,
    pista: &localify_core::domain::track::Track,
    video: &str,
    descargado: localify_core::ports::youtube::DownloadedFile,
    priority: Priority,
    intento: u8,
) -> CoreResult<()> {
    guardar_estado(
        deps,
        track,
        DownloadState::Finalizing,
        priority,
        Some(video),
        intento,
    )
    .await;

    // Etiquetar antes de mover: si falla, el fichero sigue en `.tmp/` y nunca
    // llega a verse en la biblioteca. Y que falle no es motivo para descartar
    // la descarga: sin etiquetas suena igual.
    if let Err(e) = deps.tagger.write(&descargado.path, pista, None).await {
        warn!(pista = %track, error = %e, "no se pudieron escribir las etiquetas");
    }

    let rel_path = ruta_relativa(track, &descargado.extension);
    let definitivo = deps.paths.resolve(&rel_path);

    deps.fs.atomic_rename(&descargado.path, &definitivo).await?;

    let registro = AudioFileRecord {
        track_id: track.clone(),
        rel_path,
        format: localify_core::domain::audio::AudioFormat::from_extension(&descargado.extension)
            .unwrap_or(localify_core::domain::audio::AudioFormat::M4a),
        codec: descargado.info.codec.clone(),
        bitrate_kbps: descargado.info.bitrate_kbps,
        sample_rate: descargado.info.sample_rate,
        channels: descargado.info.channels,
        size_bytes: deps.fs.file_size(&definitivo).await.unwrap_or(0),
        duration: descargado.info.duration,
        source: AudioSource::Youtube,
        youtube_id: Some(video.to_owned()),
        verified_at: chrono::Utc::now(),
    };

    // `insert` borra el trabajo de descarga en la misma transacción: dejarlo
    // vivo haría que un reinicio reencolara una pista que ya está.
    deps.audio.insert(&registro).await?;

    info!(pista = %track, codec = %registro.codec, "descarga completada");

    deps.bus.publish(DomainEvent::DownloadCompleted {
        track_id: track.clone(),
    });
    deps.bus.publish(DomainEvent::AvailabilityChanged {
        track_id: track.clone(),
        availability: Availability::Local {
            rel_path: registro.rel_path.clone(),
            format: registro.format,
            bytes: registro.size_bytes,
        },
    });

    Ok(())
}

/// Ruta relativa del fichero definitivo, con sharding por prefijo del ID.
fn ruta_relativa(track: &TrackId, extension: &str) -> PathBuf {
    let shard: String = track
        .as_str()
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(2)
        .collect();
    let shard = if shard.len() < 2 {
        "_".to_owned()
    } else {
        shard
    };

    PathBuf::from("audio")
        .join(shard)
        .join(format!("{track}.{extension}"))
}

async fn guardar_estado(
    deps: &Arc<Dependencias>,
    track: &TrackId,
    estado: DownloadState,
    priority: Priority,
    video: Option<&str>,
    intento: u8,
) {
    let job = DownloadJob {
        track_id: track.clone(),
        state: estado,
        priority,
        video_id: video.map(str::to_owned),
        tmp_path: None,
        bytes_done: 0,
        bytes_total: None,
        attempts: intento + 1,
        last_error_key: None,
    };
    if let Err(e) = deps.jobs.upsert(&job).await {
        warn!(error = %e, "no se pudo persistir el estado de la descarga");
    }
}

async fn marcar_fallida(deps: &Arc<Dependencias>, track: &TrackId, error: &CoreError) {
    let clave = error.message_key().to_owned();
    let intentos = deps.intentos();

    let job = DownloadJob {
        track_id: track.clone(),
        state: DownloadState::Failed,
        priority: Priority::Prefetch,
        video_id: None,
        tmp_path: None,
        bytes_done: 0,
        bytes_total: None,
        attempts: intentos,
        last_error_key: Some(clave.clone()),
    };
    let _ = deps.jobs.upsert(&job).await;

    deps.bus.publish(DomainEvent::DownloadFailed {
        track_id: track.clone(),
        reason_key: clave.clone(),
    });
    deps.bus.publish(DomainEvent::AvailabilityChanged {
        track_id: track.clone(),
        availability: Availability::Failed {
            reason_key: clave,
            attempts: intentos,
        },
    });
}

#[async_trait]
impl DownloadService for DownloadActor {
    async fn ensure(&self, track: &TrackId, priority: Priority) -> CoreResult<DownloadHandle> {
        let (responder, rx) = oneshot::channel();
        self.tx
            .send(Comando::Ensure {
                track: track.clone(),
                priority,
                responder,
            })
            .await
            .map_err(|_| CoreError::ShuttingDown)?;

        rx.await.map_err(|_| CoreError::ShuttingDown)?
    }

    async fn status(&self, track: &TrackId) -> CoreResult<Availability> {
        Ok(self
            .deps
            .audio
            .availability(std::slice::from_ref(track))
            .await?
            .into_iter()
            .next()
            .map_or(Availability::Absent, |(_, a)| a))
    }

    async fn statuses(&self, tracks: &[TrackId]) -> CoreResult<Vec<(TrackId, Availability)>> {
        self.deps.audio.availability(tracks).await
    }

    async fn retry_failed(&self) -> CoreResult<u32> {
        let fallidos = self.deps.jobs.failed().await?;
        let mut reintentados = 0_u32;

        for job in fallidos {
            // Se borra el registro de fallo antes de reintentar: si no, el
            // estado seguiría diciendo "fallida" mientras se descarga.
            self.deps.jobs.delete(&job.track_id).await?;
            if self.ensure(&job.track_id, Priority::Prefetch).await.is_ok() {
                reintentados += 1;
            }
        }

        Ok(reintentados)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn la_ruta_se_reparte_por_prefijo_del_identificador() {
        let id = TrackId::from_trusted("3z8h0TU7ReDPLIbEnYhWZb");
        assert_eq!(
            ruta_relativa(&id, "opus"),
            PathBuf::from("audio")
                .join("3z")
                .join("3z8h0TU7ReDPLIbEnYhWZb.opus")
        );
    }

    #[test]
    fn un_identificador_local_no_genera_rutas_invalidas() {
        // 'local:0193...' lleva dos puntos, que Windows no admite en una ruta.
        let id = TrackId::from_trusted("local:0193abcdef");
        let ruta = ruta_relativa(&id, "opus");
        let shard = ruta
            .parent()
            .and_then(std::path::Path::file_name)
            .expect("hay shard");
        assert_eq!(shard, "lo");
    }

    #[test]
    fn el_temporal_usa_el_contenedor_que_sirve_youtube() {
        assert_eq!(localify_ytdlp_extension(FormatPreference::Opus), "webm");
        assert_eq!(localify_ytdlp_extension(FormatPreference::M4a), "m4a");
    }

    #[test]
    fn el_backoff_crece_entre_intentos() {
        assert!(BACKOFF_POR_DEFECTO[1] > BACKOFF_POR_DEFECTO[0]);
    }

    #[test]
    fn el_servicio_no_expone_forma_de_cancelar() {
        // ADR-016: la regla vive en el tipo, no en un comentario. Si alguien
        // añadiera `cancel` al trait, este fichero dejaria de compilar contra
        // el puerto y habria que revisar la decision a conciencia.
        fn acepta_el_contrato<T: DownloadService>() {}
        acepta_el_contrato::<DownloadActor>();
    }

    /// Repositorio de trabajos que solo recuerda el último `upsert`.
    #[derive(Default)]
    struct JobsDePrueba {
        guardado: std::sync::Mutex<Option<DownloadJob>>,
    }

    #[async_trait]
    impl DownloadJobRepository for JobsDePrueba {
        async fn upsert(&self, job: &DownloadJob) -> CoreResult<()> {
            *self.guardado.lock().expect("lock") = Some(job.clone());
            Ok(())
        }
        async fn get(&self, _track: &TrackId) -> CoreResult<Option<DownloadJob>> {
            Ok(self.guardado.lock().expect("lock").clone())
        }
        async fn delete(&self, _track: &TrackId) -> CoreResult<()> {
            Ok(())
        }
        async fn interrupted(&self) -> CoreResult<Vec<DownloadJob>> {
            Ok(Vec::new())
        }
        async fn failed(&self) -> CoreResult<Vec<DownloadJob>> {
            Ok(Vec::new())
        }
    }

    struct BusMudo;
    impl EventPublisher for BusMudo {
        fn publish(&self, _event: DomainEvent) {}
    }

    #[tokio::test]
    async fn el_progreso_se_persiste_de_verdad_y_no_solo_se_emite() {
        // Antes, `guardar_estado` siempre subía `bytes_done: 0, bytes_total:
        // None` al entrar en `Downloading`, y solo el evento en vivo llevaba
        // el número real. Una consulta de snapshot a mitad de descarga
        // (`library_availability`) veía siempre 0%. Este test comprueba la
        // fila, no el evento.
        let jobs = Arc::new(JobsDePrueba::default());
        let track = TrackId::from_trusted("kM0Fpbz0W8U");

        let observador = Observador {
            track: track.clone(),
            bus: Arc::new(BusMudo),
            jobs: Arc::clone(&jobs) as Arc<dyn DownloadJobRepository>,
            priority: Priority::Immediate,
            video: "kM0Fpbz0W8U".to_owned(),
            attempts: 1,
            // Ya pasado el intervalo: sin esto, la primera llamada real se
            // descartaría por el límite de frecuencia y el test no probaría
            // nada.
            ultimo: std::sync::Mutex::new(
                std::time::Instant::now()
                    .checked_sub(INTERVALO_PROGRESO)
                    .unwrap_or_else(std::time::Instant::now),
            ),
            avisado: std::sync::atomic::AtomicBool::new(false),
        };

        observador.on_progress(&DownloadProgress {
            bytes_done: 12_345,
            bytes_total: Some(50_000),
            playable: false,
            state: DownloadState::Downloading,
        });

        // La persistencia se lanza en una tarea aparte (`on_progress` es
        // síncrono); hay que darle ocasión de correr.
        let mut visto = false;
        for _ in 0..100 {
            if jobs
                .get(&track)
                .await
                .ok()
                .flatten()
                .is_some_and(|j| j.bytes_done == 12_345 && j.bytes_total == Some(50_000))
            {
                visto = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(
            visto,
            "el progreso real debe persistirse en el repositorio, no solo emitirse por evento"
        );
    }
}

#[cfg(test)]
mod tests_atajo {
    use super::*;

    #[test]
    fn un_identificador_de_youtube_se_reconoce() {
        // Es el caso que hace que la descarga se salte el emparejamiento
        // entero: no hay nada que buscar, el vídeo ya está identificado.
        assert_eq!(
            video_id_directo(&TrackId::from_trusted("kM0Fpbz0W8U")).as_deref(),
            Some("kM0Fpbz0W8U")
        );
        assert_eq!(
            video_id_directo(&TrackId::from_trusted("6Wg1_YOfiM0")).as_deref(),
            Some("6Wg1_YOfiM0"),
            "el guion bajo es legal en base64url"
        );
    }

    #[test]
    fn un_identificador_de_spotify_sigue_necesitando_emparejamiento() {
        // Veintidós caracteres: no dice nada de YouTube, así que hay que
        // buscar el vídeo y puntuarlo. Este es el caso que justifica que todo
        // el sistema de matching siga existiendo.
        assert!(video_id_directo(&TrackId::from_trusted("3z8h0TU7ReDPLIbEnYhWZb")).is_none());
    }

    #[test]
    fn una_pista_local_no_se_confunde_con_un_video() {
        assert!(video_id_directo(&TrackId::nuevo_local()).is_none());
    }
}

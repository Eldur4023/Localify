//! El servicio de reproducción: el que decide, implementado como actor.
//!
//! ## Qué decide y qué no
//!
//! El motor de audio es mecanismo: carga, funde, para. **Aquí** vive toda la
//! política: qué suena después, cuándo empieza el fundido, si "anterior"
//! reinicia o retrocede, qué se precarga (ADR-015).
//!
//! Separarlos es lo que permite probar esta lógica sin tarjeta de sonido: los
//! tests inyectan un motor falso y comprueban las decisiones.
//!
//! ## Por qué un actor de verdad
//!
//! A diferencia de la cola, este servicio **sí** llama a otros mientras
//! coordina: pide a la cola qué viene, al descargador que garantice el fichero,
//! al motor que lo cargue. Con locks, el orden de adquisición entre esos
//! caminos sería una fuente real de bloqueos mutuos, y el estado podría
//! observarse a medio actualizar (ADR-008).
//!
//! Un actor lo resuelve por construcción: el estado tiene un propietario único
//! y las transiciones se serializan solas.
//!
//! **Regla estricta:** el bucle nunca espera algo lento. Descargar y decodificar
//! se delegan a tareas hijas que devuelven el resultado por el propio canal.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use localify_core::domain::audio::{DurationMs, Volume};
use localify_core::domain::download::Priority;
use localify_core::domain::ids::{QueueEntryId, TrackId};
use localify_core::domain::queue::{
    AdvanceReason, ChangeSource, PlayStatus, PlaybackContext, PlayerState, RepeatMode,
    UMBRAL_ANTERIOR,
};
use localify_core::domain::track::TrackRow;
use localify_core::error::{CoreError, CoreResult};
use localify_core::events::{DomainEvent, EventPublisher, ToastLevel};
use localify_core::ports::audio_engine::{AudioEngine, AudioSource, VoiceId};
use localify_core::ports::database::{
    AlbumRepository, FavoriteRepository, HistoryRepository, PersistedPlayerState,
    PlayerStateRepository, PlaylistRepository, TrackRepository,
};
use localify_core::ports::services::{DownloadService, PlaybackService};
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, warn};

use crate::actors::cola::QueueActor;

/// Cuántas pistas se precargan por delante.
///
/// Dos: la siguiente para el fundido y la de después para que un salto rápido
/// tampoco espere. Más sería descargar cosas que el usuario quizá nunca oiga.
const PREFETCH: usize = 2;

/// Cada cuánto se vuelca la posición a disco.
///
/// Cinco segundos acotan lo que se pierde en un corte de luz a cinco segundos
/// de posición, que es imperceptible. Escribir en cada cambio castigaría el
/// disco sin ganar nada.
const PERIODO_PERSISTENCIA: Duration = Duration::from_secs(5);

/// Órdenes que el actor acepta.
enum Orden {
    Reproducir {
        track: TrackId,
        contexto: PlaybackContext,
        responder: oneshot::Sender<CoreResult<PlayerState>>,
    },
    Alternar(oneshot::Sender<CoreResult<PlayerState>>),
    Pausar(oneshot::Sender<CoreResult<PlayerState>>),
    Reanudar(oneshot::Sender<CoreResult<PlayerState>>),
    Siguiente {
        motivo: AdvanceReason,
        responder: oneshot::Sender<CoreResult<PlayerState>>,
    },
    Anterior(oneshot::Sender<CoreResult<PlayerState>>),
    Saltar {
        posicion: DurationMs,
        responder: oneshot::Sender<CoreResult<PlayerState>>,
    },
    Volumen {
        valor: Volume,
        responder: oneshot::Sender<CoreResult<PlayerState>>,
    },
    Estado(oneshot::Sender<PlayerState>),
    Persistir(oneshot::Sender<CoreResult<()>>),
    /// Una descarga terminó de prepararse y la pista puede sonar.
    Preparada {
        track: TrackId,
        ruta: std::path::PathBuf,
        completo: bool,
        desde: DurationMs,
    },
    /// La descarga terminó: hay que pasar del temporal al fichero definitivo.
    ///
    /// **El `.part` no es el fichero que acaba en la biblioteca.** La tubería lo
    /// verifica, lo remuxea a otro fichero y mueve ese. Cuando eso pasa, el
    /// temporal se desenlaza mientras el motor lo tiene abierto: en Windows el
    /// handle sobrevive —se abre con `FILE_SHARE_DELETE`— pero leer devuelve
    /// cero bytes, y la canción se queda muda a media reproducción.
    ///
    /// Sin este relevo, la reproducción progresiva compite con su propia
    /// descarga y pierde siempre que la canción termine de bajarse mientras
    /// suena, que es casi siempre.
    CambiarAFinal {
        track: TrackId,
        ruta: std::path::PathBuf,
    },
    /// La preparación falló: la pista no va a poder sonar.
    ///
    /// Existe porque sin este aviso el actor se quedaba en `Buffering` para
    /// siempre, con la canción puesta en la barra y **sin voz**. El botón de
    /// reproducir entonces no hacía nada —`reanudar` sale por el `if let
    /// Some(voz)`— y la única salida era reiniciar.
    FalloAlPreparar {
        track: TrackId,
    },
    /// El motor avisó de que la pista se acaba: toca preparar el fundido.
    CercaDelFinal,
    /// La pista terminó de sonar.
    Terminada,
}

/// Dependencias del servicio.
pub struct Dependencias {
    pub motor: Arc<dyn AudioEngine>,
    pub cola: QueueActor,
    pub descargas: Arc<dyn DownloadService>,
    pub tracks: Arc<dyn TrackRepository>,
    pub albums: Arc<dyn AlbumRepository>,
    pub playlists: Arc<dyn PlaylistRepository>,
    pub favoritos: Arc<dyn FavoriteRepository>,
    /// Historial de escuchas.
    ///
    /// Lo escribe el reproductor y no la biblioteca porque es el único que sabe
    /// cuánto ha sonado de verdad una canción y desde dónde se puso. Estuvo un
    /// tiempo colgando de `LibraryService`, con el método implementado y sin
    /// ningún llamante: el historial quedaba siempre vacío y con él todas las
    /// recomendaciones de Inicio.
    pub historial: Arc<dyn HistoryRepository>,
    pub estado_repo: Arc<dyn PlayerStateRepository>,
    pub bus: Arc<dyn EventPublisher>,
    /// Duración del fundido, en milisegundos. Cero es reproducción sin huecos.
    ///
    /// Es un atómico compartido y no un valor fijo porque el usuario lo cambia
    /// desde Ajustes mientras suena música: reconstruir el reproductor para
    /// aplicarlo cortaría la canción.
    pub crossfade: Arc<AtomicU32>,
}

impl std::fmt::Debug for Dependencias {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Dependencias")
            .field("crossfade_ms", &self.crossfade.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

/// Estado que el actor posee en exclusiva.
struct Estado {
    pista: Option<TrackRow>,
    voz: Option<VoiceId>,
    /// Voz de la pista siguiente, ya cargada y esperando.
    voz_siguiente: Option<VozPreparada>,
    situacion: PlayStatus,
    volumen: Volume,
    /// Posición donde arrancó la voz actual, para "anterior" y persistencia.
    duracion: DurationMs,
    /// `true` si ya se pidió el fundido de esta pista.
    fundido_pedido: bool,
    /// Pista a la que ya se le dio un segundo intento de carga.
    ///
    /// Acota el reintento del temporal desaparecido a uno por canción: si al
    /// segundo intento tampoco abre, el problema es el contenido y volver a
    /// pedirlo es un bucle.
    reintentado: Option<TrackId>,
    sucia: bool,
    /// Escucha en curso: qué suena y desde cuándo.
    ///
    /// Sin esto no habría forma de saber cuánto duró una escucha cuando el
    /// usuario cambia de canción a mitad: la posición del motor se pierde en
    /// cuanto se para la voz.
    escucha: Option<EscuchaEnCurso>,
}

/// La pista siguiente, cargada por adelantado.
///
/// Se prepara quince segundos antes del final. Con crossfade **ya está
/// sonando** —el fundido es precisamente eso—; sin él solo está cargada y
/// decodificando, y no debe oírse hasta que la actual termine.
///
/// La distinción tiene que estar guardada porque quien la usa es otro momento
/// del código: al llegar el final hay que instalarla si nadie lo hizo, y
/// tratar los dos casos igual es lo que hacía que cada canción saltara quince
/// segundos antes.
#[derive(Debug, Clone)]
struct VozPreparada {
    voz: VoiceId,
    track: TrackId,
    /// `true` si el motor ya la puso a sonar.
    sonando: bool,
}

/// Lo que hay que recordar de una canción mientras suena, para anotarla.
#[derive(Debug, Clone)]
struct EscuchaEnCurso {
    track: TrackId,
    duracion: DurationMs,
    /// Contexto desde el que se puso, ya en texto.
    contexto: Option<String>,
    /// Milisegundos oídos antes de la pausa actual, si la hay.
    acumulado: u32,
    /// Cuándo empezó el tramo actual. `None` mientras está en pausa.
    desde: Option<std::time::Instant>,
}

impl Estado {
    const fn nuevo() -> Self {
        Self {
            pista: None,
            voz: None,
            voz_siguiente: None,
            situacion: PlayStatus::Stopped,
            volumen: Volume::MAX,
            duracion: DurationMs::ZERO,
            fundido_pedido: false,
            reintentado: None,
            sucia: false,
            escucha: None,
        }
    }
}

/// Handle público del reproductor. Barato de clonar.
#[derive(Clone)]
pub struct PlaybackActor {
    tx: mpsc::Sender<Orden>,
    deps: Arc<Dependencias>,
    /// Posición y buffer, publicados para que `position()` no toque el canal.
    ///
    /// La interfaz los sondea varias veces por segundo para mover la barra de
    /// progreso; hacerlo pasar por el actor lo saturaría de mensajes que no
    /// cambian nada.
    posicion: Arc<(AtomicU32, AtomicU32)>,
}

impl std::fmt::Debug for PlaybackActor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PlaybackActor").finish_non_exhaustive()
    }
}

impl PlaybackActor {
    /// Arranca el actor y sus tareas de fondo.
    #[must_use]
    pub fn arrancar(deps: Dependencias) -> Self {
        let deps = Arc::new(deps);
        let (tx, rx) = mpsc::channel(128);
        let posicion = Arc::new((AtomicU32::new(0), AtomicU32::new(0)));

        let handle = Self {
            tx: tx.clone(),
            deps: Arc::clone(&deps),
            posicion: Arc::clone(&posicion),
        };

        tokio::spawn(bucle(rx, tx.clone(), Arc::clone(&deps)));
        tokio::spawn(muestrear_posicion(
            Arc::clone(&deps),
            Arc::clone(&posicion),
            tx,
        ));

        handle
    }

    /// Restaura la sesión anterior sin ponerla a sonar.
    ///
    /// Deja la pista cargada y pausada en su segundo exacto. Arrancar sonando
    /// solo porque la aplicación se abrió sería una sorpresa desagradable.
    ///
    /// # Errors
    /// Si la lectura del estado guardado falla.
    pub async fn restaurar(&self) -> CoreResult<bool> {
        let Some(guardado) = self.deps.estado_repo.load().await? else {
            return Ok(false);
        };

        // El volumen y los modos van **antes** de mirar si había una canción, y
        // no dentro del camino que la restaura.
        //
        // No dependen de que la sesión anterior dejara algo puesto: son ajustes
        // del reproductor, no parte de una sesión. Colgarlos de que hubiera
        // pista es lo que hacía que cerrar la aplicación sin nada sonando
        // devolviera el volumen al máximo y apagara el aleatorio.
        //
        // El volumen, además, no se aplicaba nunca: se leía de disco y se
        // ignoraba, así que volvía al máximo en cada arranque.
        self.pedir(|responder| Orden::Volumen {
            valor: guardado.volume,
            responder,
        })
        .await??;
        // Y se anuncia: la interfaz pinta el deslizador con lo que le dio
        // `player_get_state` al arrancar, que puede haber sido antes de esto.
        self.deps.bus.publish(DomainEvent::VolumeChanged {
            volume: guardado.volume.as_f32(),
        });

        // La cola se restaura **antes** de la pista: así el índice y la
        // permutación ya están puestos cuando el reproductor pregunta qué viene
        // después, y la primera precarga acierta.
        self.deps.cola.restaurar(
            guardado.context.clone(),
            guardado.context_queue.clone(),
            guardado.track_id.clone(),
            guardado.shuffle,
            guardado.shuffle_seed.unwrap_or_default(),
            guardado.repeat,
        );

        let Some(pista) = guardado.track_id.clone() else {
            // Sin canción no hay sesión que continuar, pero el volumen y los
            // modos ya están puestos.
            return Ok(false);
        };
        if !guardado.user_queue.is_empty() {
            use localify_core::ports::services::QueueService;
            self.deps.cola.add_last(&guardado.user_queue).await?;
        }

        let contexto = guardado.context.unwrap_or(PlaybackContext::Single);
        self.pedir(|responder| Orden::Reproducir {
            track: pista.clone(),
            contexto,
            responder,
        })
        .await??;

        // Se coloca en su segundo y se deja en pausa: arrancar sonando solo
        // porque la aplicación se abrió sería una sorpresa desagradable.
        self.pedir(|responder| Orden::Saltar {
            posicion: guardado.position,
            responder,
        })
        .await??;
        self.pedir(Orden::Pausar).await??;

        self.deps.bus.publish(DomainEvent::TrackChanged {
            track_id: pista,
            source: ChangeSource::Restore,
        });
        Ok(true)
    }

    async fn pedir<T>(&self, construir: impl FnOnce(oneshot::Sender<T>) -> Orden) -> CoreResult<T> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(construir(tx))
            .await
            .map_err(|_| CoreError::internal("el reproductor se cerro"))?;
        rx.await
            .map_err(|_| CoreError::internal("el reproductor no respondio"))
    }
}

/// Tarea de fondo: publica la posición y vuelca el estado a disco.
async fn muestrear_posicion(
    deps: Arc<Dependencias>,
    posicion: Arc<(AtomicU32, AtomicU32)>,
    tx: mpsc::Sender<Orden>,
) {
    // El primer volcado se retrasa un periodo entero. `interval` dispara su
    // primer tick de inmediato, y eso guardaba el estado recién nacido —vacío—
    // **antes** de que nadie hubiera podido restaurar la sesión anterior: al
    // abrir la aplicación, la sesión guardada se destruía sola.
    let mut persistir = tokio::time::interval_at(
        tokio::time::Instant::now() + PERIODO_PERSISTENCIA,
        PERIODO_PERSISTENCIA,
    );
    // Cada 200 ms basta para una barra de progreso: el ojo no distingue más.
    let mut muestreo = tokio::time::interval(Duration::from_millis(200));

    loop {
        tokio::select! {
            _ = muestreo.tick() => {
                posicion.0.store(deps.motor.position().as_ms(), Ordering::Relaxed);
                posicion.1.store(deps.motor.buffered().as_ms(), Ordering::Relaxed);
            }
            _ = persistir.tick() => {
                let (tx_r, _rx) = oneshot::channel();
                if tx.send(Orden::Persistir(tx_r)).await.is_err() {
                    break;
                }
            }
        }
    }
}

/// El bucle del actor.
async fn bucle(mut rx: mpsc::Receiver<Orden>, tx: mpsc::Sender<Orden>, deps: Arc<Dependencias>) {
    let mut e = Estado::nuevo();

    while let Some(orden) = rx.recv().await {
        match orden {
            Orden::Reproducir {
                track,
                contexto,
                responder,
            } => {
                let r = reproducir(&mut e, &deps, &tx, &track, contexto).await;
                let _ = responder.send(r.map(|()| instantanea(&e, &deps)));
            }
            Orden::Alternar(responder) => {
                if e.situacion == PlayStatus::Playing {
                    pausar(&mut e, &deps);
                } else {
                    reanudar(&mut e, &deps, &tx);
                }
                let _ = responder.send(Ok(instantanea(&e, &deps)));
            }
            Orden::Pausar(responder) => {
                pausar(&mut e, &deps);
                let _ = responder.send(Ok(instantanea(&e, &deps)));
            }
            Orden::Reanudar(responder) => {
                reanudar(&mut e, &deps, &tx);
                let _ = responder.send(Ok(instantanea(&e, &deps)));
            }
            Orden::Siguiente { motivo, responder } => {
                let r = avanzar(&mut e, &deps, &tx, motivo).await;
                let _ = responder.send(r.map(|()| instantanea(&e, &deps)));
            }
            Orden::Anterior(responder) => {
                let r = anterior(&mut e, &deps, &tx).await;
                let _ = responder.send(r.map(|()| instantanea(&e, &deps)));
            }
            Orden::Saltar {
                posicion,
                responder,
            } => {
                saltar(&mut e, &deps, posicion);
                let _ = responder.send(Ok(instantanea(&e, &deps)));
            }
            Orden::Volumen { valor, responder } => {
                e.volumen = valor;
                deps.motor.set_volume(valor);
                e.sucia = true;
                let _ = responder.send(Ok(instantanea(&e, &deps)));
            }
            Orden::Estado(responder) => {
                refrescar_favorito(&mut e, &deps).await;
                let _ = responder.send(instantanea(&e, &deps));
            }
            Orden::Persistir(responder) => {
                let _ = responder.send(persistir(&e, &deps).await);
            }
            Orden::Preparada {
                track,
                ruta,
                completo,
                desde,
            } => {
                instalar(&mut e, &deps, &tx, &track, &ruta, completo, desde);
            }
            Orden::CambiarAFinal { track, ruta } => {
                // Solo si sigue siendo la que suena: el usuario puede haber
                // cambiado de canción mientras esta terminaba de bajarse.
                if e.pista.as_ref().map(|p| &p.id) == Some(&track) {
                    let posicion = deps.motor.position();
                    debug!(
                        track = %track.as_str(),
                        posicion_ms = posicion.as_ms(),
                        "relevo del temporal al fichero definitivo"
                    );
                    instalar(&mut e, &deps, &tx, &track, &ruta, true, posicion);
                }
            }
            Orden::FalloAlPreparar { track } => {
                // Solo si sigue siendo la pista vigente: el usuario puede
                // haber pulsado otra mientras esta fallaba.
                if e.pista.as_ref().map(|p| &p.id) == Some(&track) {
                    e.situacion = PlayStatus::Stopped;
                    e.sucia = true;
                    anunciar_estado(&e, &deps);
                }
            }
            Orden::CercaDelFinal => {
                preparar_fundido(&mut e, &deps).await;
            }
            Orden::Terminada => {
                let _ = avanzar(&mut e, &deps, &tx, AdvanceReason::NaturalEnd).await;
            }
        }
    }
    debug!("actor de reproduccion terminado");
}

/// Pone una pista a sonar.
///
/// Lo que hace invisible la descarga: se pide el fichero y, si no está, la
/// espera se delega a una tarea hija. El bucle vuelve enseguida y el usuario ve
/// "cargando", no un botón de descargar.
async fn reproducir(
    e: &mut Estado,
    deps: &Arc<Dependencias>,
    tx: &mpsc::Sender<Orden>,
    track: &TrackId,
    contexto: PlaybackContext,
) -> CoreResult<()> {
    use localify_core::ports::services::QueueService;

    let fila = deps
        .tracks
        .rows_by_ids(std::slice::from_ref(track))
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| CoreError::not_found("track", track.as_str()))?;

    // El contexto se instala **antes** de sonar: es lo que decide qué viene
    // después, y sin él la canción se quedaría sola aunque venga de un álbum.
    if deps.cola.contexto().as_ref() != Some(&contexto) {
        deps.cola.set_context(contexto.clone(), 0).await?;
        let pistas = resolver_contexto(deps, &contexto).await;
        if !pistas.is_empty() {
            let desde = pistas.iter().position(|t| t == track).unwrap_or(0);
            deps.cola.poner_pistas(pistas, desde);
        }
    }
    deps.cola.ir_a(track);

    // La canción anterior se calla **ya**, no cuando la nueva esté lista.
    //
    // Preparar una pista que no está descargada tarda segundos. Dejando sonar
    // la anterior mientras tanto, pulsar una canción parece no hacer nada y
    // luego, de golpe, salta a otra: el usuario cree que ha pulsado mal. Peor
    // aún, si pausaba durante ese hueco, la nueva pista arrancaba igual al
    // instalarse y la pausa parecía no funcionar.
    if let Some(anterior) = e.voz.take() {
        deps.motor.stop(anterior);
    }

    // Poner otra canción cierra la escucha de la que sonaba: es una escucha
    // parcial, y si pasó del mínimo cuenta igual.
    cerrar_escucha(e, deps);

    e.duracion = fila.duration;
    abrir_escucha(e, deps, &fila);
    e.pista = Some(fila);
    e.fundido_pedido = false;
    e.situacion = PlayStatus::Buffering;
    e.sucia = true;

    deps.bus.publish(DomainEvent::TrackChanged {
        track_id: track.clone(),
        source: ChangeSource::User,
    });
    anunciar_estado(e, deps);

    preparar(
        deps,
        tx,
        track.clone(),
        DurationMs::ZERO,
        Priority::Immediate,
    );
    prefetch(deps);
    Ok(())
}

/// Cada cuánto se comprueba si la descarga ya terminó.
///
/// Es una lectura de una fila; medio segundo es holgado y no se nota en el
/// relevo, que ocurre en un punto arbitrario de la canción.
const PERIODO_VIGILANCIA: Duration = Duration::from_millis(500);

/// Cuánto se vigila como mucho.
///
/// Una descarga que no ha terminado en diez minutos no va a terminar, y dejar
/// la tarea viva para siempre sería una fuga por cada canción reproducida.
const VIGILANCIA_MAXIMA: Duration = Duration::from_secs(600);

/// Espera a que la descarga termine y pide el relevo al fichero definitivo.
///
/// Se sondea en vez de escuchar el bus porque el reproductor no está suscrito a
/// él: recibe sus avisos por el canal del motor. Añadir una suscripción para
/// esto obligaría a cablear el bus dentro del actor y a filtrar eventos ajenos.
async fn vigilar_final(deps: &Arc<Dependencias>, tx: &mpsc::Sender<Orden>, track: TrackId) {
    let limite = std::time::Instant::now() + VIGILANCIA_MAXIMA;

    while std::time::Instant::now() < limite {
        tokio::time::sleep(PERIODO_VIGILANCIA).await;

        // Se vuelve a pedir: cuando ya está en disco, `ensure` devuelve la ruta
        // definitiva al instante y sin descargar nada.
        match deps.descargas.ensure(&track, Priority::Prefetch).await {
            Ok(handle) if handle.complete => {
                let _ = tx
                    .send(Orden::CambiarAFinal {
                        track,
                        ruta: handle.playable_path,
                    })
                    .await;
                return;
            }
            Ok(_) => {}
            Err(e) => {
                debug!(%track, error = %e, "la descarga vigilada falló; no habrá relevo");
                return;
            }
        }
    }

    warn!(%track, "la descarga no terminó a tiempo; sin relevo al fichero final");
}

/// Resuelve las pistas que forman un contexto.
///
/// Un álbum o una playlist hay que ir a buscarlos; una búsqueda ya trae su
/// lista consigo. La biblioteca entera y los favoritos se acotan a una página
/// grande: cargar 50 000 identificadores para reproducir una canción sería
/// pagar por adelantado algo que casi nadie recorre entero.
async fn resolver_contexto(deps: &Arc<Dependencias>, ctx: &PlaybackContext) -> Vec<TrackId> {
    use localify_core::domain::track::{TrackFilter, TrackSort};
    use localify_core::page::PageRequest;

    /// Tope de pistas que un contexto abierto aporta a la cola.
    const TOPE: u32 = 1000;

    match ctx {
        PlaybackContext::Album { id } => deps
            .albums
            .tracks_of(id)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|t| t.id)
            .collect(),
        PlaybackContext::Playlist { id } => deps
            .playlists
            .entries(id, &PageRequest::new(0, TOPE))
            .await
            .map(|p| p.items.into_iter().map(|e| e.track.id).collect())
            .unwrap_or_default(),
        PlaybackContext::Liked => deps
            .favoritos
            .list(&PageRequest::new(0, TOPE))
            .await
            .map(|p| p.items.into_iter().map(|t| t.id).collect())
            .unwrap_or_default(),
        PlaybackContext::Library | PlaybackContext::Artist { .. } => deps
            .tracks
            .list_rows(
                &TrackFilter::default(),
                TrackSort::TitleAsc,
                &PageRequest::new(0, TOPE),
            )
            .await
            .map(|p| p.items.into_iter().map(|t| t.id).collect())
            .unwrap_or_default(),
        // Búsqueda y recomendaciones traen su lista; `set_context` ya la
        // instaló. `Single` no tiene siguiente a propósito.
        PlaybackContext::Search { .. }
        | PlaybackContext::Recommendation { .. }
        | PlaybackContext::Single => Vec::new(),
    }
}

/// Lanza la preparación de una pista sin bloquear el bucle.
fn preparar(
    deps: &Arc<Dependencias>,
    tx: &mpsc::Sender<Orden>,
    track: TrackId,
    desde: DurationMs,
    prioridad: Priority,
) {
    let deps = Arc::clone(deps);
    let tx = tx.clone();
    tokio::spawn(async move {
        match deps.descargas.ensure(&track, prioridad).await {
            Ok(handle) => {
                let incompleta = !handle.complete;
                let _ = tx
                    .send(Orden::Preparada {
                        track: track.clone(),
                        ruta: handle.playable_path,
                        completo: handle.complete,
                        desde,
                    })
                    .await;

                // Si se ha empezado a sonar sobre el temporal, hay que vigilar
                // cuándo termina la descarga: ese es el momento en que el
                // `.part` desaparece y la canción se quedaría muda.
                if incompleta {
                    vigilar_final(&deps, &tx, track).await;
                }
            }
            Err(err) => {
                warn!(track = %track.as_str(), error = %err, "no se pudo preparar la pista");
                deps.bus.publish(DomainEvent::Toast {
                    level: ToastLevel::Error,
                    message_key: err.message_key().to_owned(),
                    params: Vec::new(),
                });
                // El actor tiene que enterarse: si no, se queda esperando una
                // pista que ya se sabe que no viene.
                let _ = tx.send(Orden::FalloAlPreparar { track }).await;
            }
        }
    });
}

/// Carga el fichero en el motor y lo pone a sonar.
fn instalar(
    e: &mut Estado,
    deps: &Arc<Dependencias>,
    tx: &mpsc::Sender<Orden>,
    track: &TrackId,
    ruta: &std::path::Path,
    completo: bool,
    desde: DurationMs,
) {
    // Si el usuario ya cambió de canción mientras se preparaba esta, se
    // descarta: instalarla ahora sonaría como un salto hacia atrás.
    if e.pista.as_ref().map(|p| &p.id) != Some(track) {
        debug!(track = %track.as_str(), "pista preparada fuera de tiempo, descartada");
        return;
    }

    // Se registran ruta, tamaño y si viene completa **antes** de intentar
    // abrirla. Sin esto, un fallo del decodificador solo dice "formato no
    // soportado" y no hay forma de saber si el problema es el códec, un
    // fichero a medio escribir o una ruta que no es la que se cree.
    let bytes = std::fs::metadata(ruta).map_or(0, |m| m.len());
    debug!(
        track = %track.as_str(),
        fichero = %ruta.display(),
        bytes,
        completo,
        "cargando en el motor"
    );

    let origen = if completo {
        AudioSource::File(ruta.to_path_buf())
    } else {
        AudioSource::Growing {
            path: ruta.to_path_buf(),
            expected_bytes: None,
        }
    };

    match deps.motor.load(origen, desde) {
        Ok(voz) => {
            if let Some(anterior) = e.voz.take() {
                deps.motor.stop(anterior);
            }
            e.voz = Some(voz);

            // Se respeta lo que el usuario haya pedido **mientras** se
            // preparaba. Arrancar siempre a sonar convertía una pausa hecha
            // durante la carga en un botón que no hace nada: la pista se
            // instalaba un segundo después y volvía a sonar sola.
            if e.situacion == PlayStatus::Paused {
                debug!(track = %track.as_str(), "preparada, pero el usuario pausó: se queda cargada");
            } else {
                deps.motor.play(voz);
                // El cronómetro arranca aquí, no al pedir la canción: entre
                // pulsar y sonar puede haber una descarga entera, y contarla
                // como escucha inflaría todas las recomendaciones a favor de
                // lo que más tarda en llegar.
                reanudar_escucha(e);
                e.situacion = PlayStatus::Playing;
            }
            anunciar_estado(e, deps);
        }
        Err(err) => {
            // Un fichero a medio bajar que no se puede abrir casi siempre es la
            // misma carrera: la descarga acabó entre pedir la ruta y usarla, el
            // `.part` se verificó, se remuxeó a otro fichero y se desenlazó. En
            // Windows el handle sigue abriéndose —`FILE_SHARE_DELETE`— pero lee
            // cero bytes, así que el error que llega es "formato no soportado"
            // y no "el fichero no está".
            //
            // Volver a pedirlo devuelve ya el fichero definitivo. Se hace una
            // sola vez por pista: si al segundo intento tampoco carga, el
            // problema es el contenido y reintentar es un bucle.
            if !completo && e.reintentado.as_ref() != Some(track) {
                debug!(
                    track = %track.as_str(),
                    bytes,
                    "el temporal se esfumó al terminar la descarga; se pide el definitivo"
                );
                e.reintentado = Some(track.clone());
                preparar(deps, tx, track.clone(), desde, Priority::Immediate);
                return;
            }

            warn!(
                error = %err,
                fichero = %ruta.display(),
                bytes,
                completo,
                "el motor no pudo cargar la pista"
            );
            e.situacion = PlayStatus::Stopped;
            deps.bus.publish(DomainEvent::Toast {
                level: ToastLevel::Error,
                message_key: "playback.load_failed".to_owned(),
                params: Vec::new(),
            });
            anunciar_estado(e, deps);
        }
    }
}

/// Mueve la aguja dentro de la canción y avisa de dónde quedó.
///
/// El aviso importa porque saltar no cambia ni de pista ni de estado: sin él, no
/// se emitía absolutamente nada, y quien publica la posición hacia fuera —el
/// perfil de Discord— seguía anunciando la hora a la que empezó la canción.
fn saltar(e: &mut Estado, deps: &Arc<Dependencias>, posicion: DurationMs) {
    let Some(v) = e.voz else { return };
    deps.motor.seek(v, posicion);
    e.fundido_pedido = false;

    if let Some(pista) = &e.pista {
        deps.bus.publish(DomainEvent::Seeked {
            track_id: pista.id.clone(),
            position_ms: posicion.as_ms(),
        });
    }
}

fn pausar(e: &mut Estado, deps: &Arc<Dependencias>) {
    deps.motor.pause();
    // El cronómetro se detiene con la música. Sin esto, dejar la aplicación
    // pausada toda la noche anotaría ocho horas de escucha de una canción.
    pausar_escucha(e);
    e.situacion = PlayStatus::Paused;
    e.sucia = true;
    anunciar_estado(e, deps);
}

/// Reanuda; si la pista nunca llegó a cargarse, lo vuelve a intentar.
///
/// El reintento no es un extra: sin él, una pista cuya preparación falló deja
/// el botón de reproducir muerto. Hay canción en la barra, hay botón, y pulsarlo
/// no hace absolutamente nada porque no hay voz que reanudar. La única salida
/// era reiniciar la aplicación.
///
/// Volver a intentarlo es además lo correcto: casi todos los motivos por los
/// que una preparación falla —la red, YouTube, un temporal que no llegó a
/// tiempo— se arreglan solos al segundo intento.
fn reanudar(e: &mut Estado, deps: &Arc<Dependencias>, tx: &mpsc::Sender<Orden>) {
    if let Some(v) = e.voz {
        deps.motor.play(v);
        reanudar_escucha(e);
        e.situacion = PlayStatus::Playing;
        anunciar_estado(e, deps);
        return;
    }

    let Some(pista) = e.pista.as_ref().map(|p| p.id.clone()) else {
        return;
    };

    debug!(track = %pista.as_str(), "sin voz cargada: se reintenta preparar");
    e.situacion = PlayStatus::Buffering;
    e.sucia = true;
    anunciar_estado(e, deps);
    preparar(deps, tx, pista, DurationMs::ZERO, Priority::Immediate);
}

/// Avanza a la siguiente pista.
async fn avanzar(
    e: &mut Estado,
    deps: &Arc<Dependencias>,
    tx: &mpsc::Sender<Orden>,
    motivo: AdvanceReason,
) -> CoreResult<()> {
    use localify_core::ports::services::QueueService;

    // La siguiente pista puede estar ya cargada de antes.
    if let Some(preparada) = e.voz_siguiente.take() {
        if motivo == AdvanceReason::NaturalEnd {
            // Si no hubo fundido, la voz está lista pero callada: este es su
            // momento. Con duración cero el relevo es inmediato, que aquí es lo
            // correcto —la anterior acaba de terminar— y no un corte.
            if !preparada.sonando {
                deps.motor.crossfade_to(preparada.voz, DurationMs::ZERO);
            }

            let _ = deps.cola.advance(motivo).await?;
            e.voz = Some(preparada.voz);
            e.fundido_pedido = false;
            actualizar_pista(e, deps, &preparada.track).await;
            prefetch(deps);
            return Ok(());
        }

        // El usuario se adelantó: la que se preparó ya no es la que toca. Se
        // suelta en vez de dejarla olvidada dentro del motor, decodificando una
        // canción que nadie va a oír.
        deps.motor.stop(preparada.voz);
        e.fundido_pedido = false;
    }

    let Some(siguiente) = deps.cola.advance(motivo).await? else {
        // Fin de la cola: se para, no se apaga la sesión.
        if let Some(v) = e.voz.take() {
            deps.motor.stop(v);
        }
        // La última canción de la cola también se escuchó: sin esto, la que
        // cierra cada sesión no contaba nunca.
        cerrar_escucha(e, deps);
        e.situacion = PlayStatus::Stopped;
        e.sucia = true;
        anunciar_estado(e, deps);
        return Ok(());
    };

    actualizar_pista(e, deps, &siguiente).await;
    preparar(deps, tx, siguiente, DurationMs::ZERO, Priority::Immediate);
    prefetch(deps);
    Ok(())
}

/// "Anterior" con la regla de los tres segundos.
///
/// Por debajo del umbral va a la pista previa; por encima reinicia la actual.
/// Es lo que hace Spotify, y lo que evita que un doble toque se lleve por
/// delante la canción que acababa de empezar.
async fn anterior(
    e: &mut Estado,
    deps: &Arc<Dependencias>,
    tx: &mpsc::Sender<Orden>,
) -> CoreResult<()> {
    use localify_core::ports::services::QueueService;

    if deps.motor.position() >= UMBRAL_ANTERIOR
        && let Some(v) = e.voz
    {
        deps.motor.seek(v, DurationMs::ZERO);
        e.fundido_pedido = false;
        return Ok(());
    }

    let Some(previa) = deps.cola.go_back().await? else {
        // No hay anterior: reiniciar es mejor que no hacer nada.
        if let Some(v) = e.voz {
            deps.motor.seek(v, DurationMs::ZERO);
        }
        return Ok(());
    };

    actualizar_pista(e, deps, &previa).await;
    preparar(deps, tx, previa, DurationMs::ZERO, Priority::Immediate);
    Ok(())
}

/// Prepara el fundido hacia la siguiente pista.
async fn preparar_fundido(e: &mut Estado, deps: &Arc<Dependencias>) {
    use localify_core::ports::services::QueueService;

    if e.fundido_pedido || e.voz_siguiente.is_some() {
        return;
    }
    let Ok(Some(siguiente)) = deps.cola.peek_next().await else {
        return;
    };
    // Con repetición de pista, la siguiente es ella misma: fundir consigo
    // misma no tiene sentido.
    if e.pista.as_ref().map(|p| &p.id) == Some(&siguiente) {
        return;
    }

    e.fundido_pedido = true;

    let Ok(handle) = deps.descargas.ensure(&siguiente, Priority::Prefetch).await else {
        return;
    };
    let origen = if handle.complete {
        AudioSource::File(handle.playable_path)
    } else {
        AudioSource::Growing {
            path: handle.playable_path,
            expected_bytes: None,
        }
    };
    let Ok(voz) = deps.motor.load(origen, DurationMs::ZERO) else {
        return;
    };

    // **Solo se funde si hay fundido.** `crossfade_to` con duración cero no
    // encadena: sustituye la voz en el acto. Como esto corre quince segundos
    // antes del final —el margen que necesita el crossfade más largo—, pedirlo
    // con el ajuste a cero cortaba la canción justo ahí. Sin fundido la voz se
    // queda cargada y decodificando, que es lo que hace que el cambio al
    // terminar sea inmediato, y no suena hasta que le toca.
    let cruce = deps.crossfade.load(Ordering::Relaxed);
    if cruce > 0 {
        deps.motor.crossfade_to(voz, DurationMs::new(cruce));
    }

    e.voz_siguiente = Some(VozPreparada {
        voz,
        track: siguiente,
        sonando: cruce > 0,
    });
}

/// Pide al descargador que garantice las siguientes pistas.
///
/// Es el único acoplamiento con las descargas, y va por trait: la cola no sabe
/// que existe un descargador.
///
/// Va en el carril `Prefetch`, que tiene su propia concurrencia: precargar no
/// puede robarle ancho de banda a lo que suena ahora.
fn prefetch(deps: &Arc<Dependencias>) {
    let proximas = deps.cola.proximas(PREFETCH);
    if proximas.is_empty() {
        return;
    }
    let descargas = Arc::clone(&deps.descargas);
    tokio::spawn(async move {
        for t in proximas {
            let _ = descargas.ensure(&t, Priority::Prefetch).await;
        }
    });
}

/// Actualiza la pista mostrada y avisa.
async fn actualizar_pista(e: &mut Estado, deps: &Arc<Dependencias>, track: &TrackId) {
    let fila = deps
        .tracks
        .rows_by_ids(std::slice::from_ref(track))
        .await
        .ok()
        .and_then(|v| v.into_iter().next());

    // Cada vez que se pone una canción vuelve a tener derecho a su reintento.
    // Sin esto, una pista que falló hace media hora ya no lo tendría, y el caso
    // que se quiere cubrir —que la descarga acabe justo al abrirla— puede
    // repetirse perfectamente la segunda vez que se pone.
    e.reintentado = None;

    // La escucha anterior se cierra aquí y no en cada sitio que cambia de
    // canción: por este punto pasan las cuatro formas de hacerlo —terminar,
    // siguiente, anterior y encadenado— y repartir la anotación entre ellas
    // garantizaba olvidarse de alguna.
    cerrar_escucha(e, deps);

    if let Some(f) = fila {
        e.duracion = f.duration;
        e.pista = Some(f.clone());
        abrir_escucha(e, deps, &f);
        deps.bus.publish(DomainEvent::TrackChanged {
            track_id: track.clone(),
            source: ChangeSource::Queue,
        });
    }
    e.fundido_pedido = false;
    e.sucia = true;
    anunciar_estado(e, deps);
}

/// Fracción de una canción a partir de la cual cuenta como escuchada entera.
///
/// El 90 % es lo que usa Last.fm, y distingue haber oído una canción de haberla
/// dejado sonando mientras se busca otra cosa.
const FRACCION_COMPLETA: f32 = 0.9;

/// Por debajo de esto no se anota nada.
///
/// Pasar por encima de una canción para llegar a la siguiente no es escucharla,
/// y contarlo envenenaría las recomendaciones: saltar algo lo recomendaría más.
const MINIMO_ESCUCHA_MS: u32 = 5_000;

/// Empieza a contar una escucha.
fn abrir_escucha(e: &mut Estado, deps: &Arc<Dependencias>, fila: &TrackRow) {
    e.escucha = Some(EscuchaEnCurso {
        track: fila.id.clone(),
        duracion: fila.duration,
        // El contexto se guarda **al empezar**, no al terminar: para cuando la
        // canción acabe, la cola puede estar en otro sitio y la escucha se
        // atribuiría a la playlist equivocada.
        contexto: deps.cola.contexto().as_ref().map(texto_de_contexto),
        acumulado: 0,
        // Todavía no cuenta: el cronómetro lo arranca `instalar` cuando el
        // motor empieza a sonar de verdad.
        desde: None,
    });
}

/// Detiene el cronómetro sin cerrar la escucha.
fn pausar_escucha(e: &mut Estado) {
    if let Some(escucha) = e.escucha.as_mut()
        && let Some(desde) = escucha.desde.take()
    {
        let ms = u32::try_from(desde.elapsed().as_millis()).unwrap_or(u32::MAX);
        escucha.acumulado = escucha.acumulado.saturating_add(ms);
    }
}

/// Reanuda el cronómetro.
fn reanudar_escucha(e: &mut Estado) {
    if let Some(escucha) = e.escucha.as_mut()
        && escucha.desde.is_none()
    {
        escucha.desde = Some(std::time::Instant::now());
    }
}

/// Cierra la escucha en curso y la anota.
///
/// ## Por qué se mide con reloj y no con la posición del motor
///
/// La posición dice por dónde va la canción, no cuánto se ha oído: saltar al
/// minuto tres y parar daría tres minutos de escucha que no ocurrieron. El
/// reloj cuenta tiempo real de reproducción, que es lo que significa "he
/// escuchado esto".
///
/// ## Se anota en segundo plano
///
/// El actor no espera a la base de datos: un `INSERT` entre canción y canción
/// metería una pausa audible en el encadenado.
fn cerrar_escucha(e: &mut Estado, deps: &Arc<Dependencias>) {
    pausar_escucha(e);
    let Some(escucha) = e.escucha.take() else {
        return;
    };
    if escucha.acumulado < MINIMO_ESCUCHA_MS {
        return;
    }

    let completa = if escucha.duracion.is_zero() {
        false
    } else {
        #[allow(clippy::cast_precision_loss, reason = "duraciones de minutos")]
        let fraccion = escucha.acumulado as f32 / escucha.duracion.as_ms() as f32;
        fraccion >= FRACCION_COMPLETA
    };

    let historial = Arc::clone(&deps.historial);
    let bus = Arc::clone(&deps.bus);
    let entrada = localify_core::domain::library::PlayHistoryEntry {
        track_id: escucha.track.clone(),
        played_at: chrono::Utc::now(),
        ms_played: escucha.acumulado,
        completed: completa,
        context: escucha.contexto,
    };

    tokio::spawn(async move {
        if let Err(err) = historial.record(&entrada).await {
            warn!(error = %err, "no se pudo anotar la escucha");
            return;
        }
        bus.publish(DomainEvent::TrackFinished {
            track_id: entrada.track_id,
            completed: entrada.completed,
            ms_played: entrada.ms_played,
        });
    });
}

/// Texto con el que se guarda un contexto en el historial.
///
/// Guardarlo es lo que permite decir "tus playlists más escuchadas" en lugar de
/// "las playlists que contienen canciones que has oído": son cosas distintas, y
/// solo la primera es una recomendación.
fn texto_de_contexto(contexto: &PlaybackContext) -> String {
    match contexto {
        PlaybackContext::Playlist { id } => format!("playlist:{}", id.as_uuid()),
        PlaybackContext::Album { id } => format!("album:{}", id.as_str()),
        PlaybackContext::Artist { id } => format!("artist:{}", id.as_str()),
        PlaybackContext::Library => "library".to_owned(),
        PlaybackContext::Liked => "liked".to_owned(),
        PlaybackContext::Search { .. } => "search".to_owned(),
        PlaybackContext::Recommendation { .. } => "recommendation".to_owned(),
        PlaybackContext::Single => "single".to_owned(),
    }
}

/// Pone al día el "me gusta" de la pista que suena.
///
/// ## Por qué no basta con la fila cacheada
///
/// El actor guarda la `TrackRow` de la pista actual desde que empezó a sonar, y
/// esa fila lleva dentro `is_favorite`. Marcar la canción como favorita escribe
/// en la tabla de favoritos, pero **nadie tocaba esa copia**: el corazón de la
/// barra se pintaba desde un valor congelado y no se ponía verde hasta cambiar
/// de canción. Desde fuera parecía que el clic no hacía nada.
///
/// ## Por qué se consulta aquí y no se avisa desde fuera
///
/// La alternativa era que el servicio de biblioteca avisara al actor al marcar
/// un favorito. Eso ata dos servicios que hoy no se conocen y añade un camino
/// más por el que la copia puede quedarse atrás —el siguiente campo de la fila
/// que alguien edite desde otro sitio volvería a tener el mismo problema—.
///
/// Consultarlo al componer el estado cuesta una búsqueda por clave primaria en
/// una tabla de una columna, que es lo más barato que hace SQLite. El bucle ya
/// espera lecturas de este tamaño al cambiar de pista.
async fn refrescar_favorito(e: &mut Estado, deps: &Arc<Dependencias>) {
    let Some(fila) = e.pista.as_mut() else {
        return;
    };
    if let Ok(marcada) = deps.favoritos.is_favorite(&fila.id).await {
        fila.is_favorite = marcada;
    }
}

/// Compone el estado completo.
fn instantanea(e: &Estado, deps: &Arc<Dependencias>) -> PlayerState {
    let (aleatorio, repeticion) = deps.cola.modos();
    PlayerState {
        track: e.pista.clone(),
        status: e.situacion,
        position: deps.motor.position(),
        duration: e.duracion,
        buffered: deps.motor.buffered(),
        volume: e.volumen,
        repeat: repeticion,
        shuffle: aleatorio,
        context: deps.cola.contexto(),
    }
}

fn anunciar_estado(e: &Estado, deps: &Arc<Dependencias>) {
    deps.bus.publish(DomainEvent::PlayStatusChanged {
        status: e.situacion,
    });
}

/// Vuelca el estado a disco.
///
/// Se guardan identificadores, no filas: los metadatos pueden cambiar entre
/// sesiones y rehidratarlos al arrancar es más correcto que restaurar una copia
/// obsoleta.
async fn persistir(e: &Estado, deps: &Arc<Dependencias>) -> CoreResult<()> {
    // Sin pista no hay sesión que guardar, y escribir una vacía **borraría** la
    // que hubiera. Es la segunda mitad de la misma protección que el retraso
    // del primer volcado.
    if e.pista.is_none() {
        return Ok(());
    }

    let (contexto_pistas, semilla) = deps.cola.para_persistir();
    let (aleatorio, repeticion) = deps.cola.modos();

    let estado = PersistedPlayerState {
        track_id: e.pista.as_ref().map(|p| p.id.clone()),
        position: deps.motor.position(),
        volume: e.volumen,
        repeat: repeticion,
        shuffle: aleatorio,
        shuffle_seed: Some(semilla),
        context: deps.cola.contexto(),
        context_queue: contexto_pistas,
        user_queue: deps.cola.pendientes_de_usuario(),
        queue_index: 0,
    };
    deps.estado_repo.save(&estado).await
}

#[async_trait]
impl PlaybackService for PlaybackActor {
    async fn play_track(&self, id: &TrackId, ctx: PlaybackContext) -> CoreResult<PlayerState> {
        self.pedir(|responder| Orden::Reproducir {
            track: id.clone(),
            contexto: ctx,
            responder,
        })
        .await?
    }

    async fn toggle(&self) -> CoreResult<PlayerState> {
        self.pedir(Orden::Alternar).await?
    }

    async fn pause(&self) -> CoreResult<PlayerState> {
        self.pedir(Orden::Pausar).await?
    }

    async fn resume(&self) -> CoreResult<PlayerState> {
        self.pedir(Orden::Reanudar).await?
    }

    async fn next(&self) -> CoreResult<PlayerState> {
        self.pedir(|responder| Orden::Siguiente {
            motivo: AdvanceReason::UserSkip,
            responder,
        })
        .await?
    }

    async fn previous(&self) -> CoreResult<PlayerState> {
        self.pedir(Orden::Anterior).await?
    }

    async fn seek(&self, position: DurationMs) -> CoreResult<PlayerState> {
        self.pedir(|responder| Orden::Saltar {
            posicion: position,
            responder,
        })
        .await?
    }

    async fn set_volume(&self, volume: Volume) -> CoreResult<PlayerState> {
        self.pedir(|responder| Orden::Volumen {
            valor: volume,
            responder,
        })
        .await?
    }

    async fn set_repeat(&self, mode: RepeatMode) -> CoreResult<PlayerState> {
        use localify_core::ports::services::QueueService;
        self.deps.cola.set_repeat(mode).await?;
        Ok(self.state().await)
    }

    async fn set_shuffle(&self, enabled: bool) -> CoreResult<PlayerState> {
        use localify_core::ports::services::QueueService;
        self.deps.cola.set_shuffle(enabled).await?;
        Ok(self.state().await)
    }

    async fn jump_to(&self, entry: QueueEntryId) -> CoreResult<PlayerState> {
        use localify_core::ports::services::QueueService;

        // Se busca la entrada en la cola de usuario y se consume todo lo que
        // haya antes: saltar a la tercera implica descartar las dos primeras,
        // que es lo que el usuario espera al pulsar sobre ella.
        let snapshot = self.deps.cola.snapshot().await;
        let Some(pos) = snapshot.user_queue.iter().position(|e| e.entry_id == entry) else {
            return Err(CoreError::not_found("queue_entry", "entrada"));
        };
        for _ in 0..=pos {
            self.deps.cola.advance(AdvanceReason::UserSkip).await?;
        }
        let Some(track) = self.deps.cola.actual() else {
            return Ok(self.state().await);
        };
        self.play_track(&track, PlaybackContext::Single).await
    }

    async fn state(&self) -> PlayerState {
        self.pedir(Orden::Estado)
            .await
            .unwrap_or_else(|_| PlayerState::detenido())
    }

    fn position(&self) -> (DurationMs, DurationMs) {
        (
            DurationMs::new(self.posicion.0.load(Ordering::Relaxed)),
            DurationMs::new(self.posicion.1.load(Ordering::Relaxed)),
        )
    }

    async fn persist_now(&self) -> CoreResult<()> {
        self.pedir(Orden::Persistir).await?
    }
}

/// Conecta los eventos del motor con el actor.
///
/// Va aparte del constructor porque el receptor de eventos solo lo tiene quien
/// arrancó el motor, y ese es el contexto de la aplicación.
pub fn conectar_eventos(
    actor: &PlaybackActor,
    mut fuente: Box<dyn localify_core::ports::audio_engine::AudioEventSource>,
) {
    let tx = actor.tx.clone();

    std::thread::Builder::new()
        .name("localify-engine-events".to_owned())
        .spawn(move || {
            use localify_core::ports::audio_engine::EngineEvent;
            while let Some(evento) = fuente.recv() {
                let orden = match evento {
                    EngineEvent::ApproachingEnd { .. } => Some(Orden::CercaDelFinal),
                    EngineEvent::Ended { .. } => Some(Orden::Terminada),
                    EngineEvent::Underrun { .. } => {
                        // Un underrun no corta la reproducción: el mezclador
                        // rellena con silencio y el buffer se recupera solo.
                        debug!("underrun de audio");
                        None
                    }
                    EngineEvent::DeviceChanged { device } => {
                        debug!(dispositivo = %device.name, "cambio el dispositivo de salida");
                        None
                    }
                    _ => None,
                };
                if let Some(o) = orden
                    && tx.blocking_send(o).is_err()
                {
                    break;
                }
            }
        })
        .ok();
}

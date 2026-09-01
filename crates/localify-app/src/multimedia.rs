//! Conexión entre el reproductor y el panel multimedia del sistema.
//!
//! Es una traducción en dos direcciones:
//!
//! - **Hacia el sistema**: cada cambio de pista o de estado actualiza el panel
//!   que Windows muestra al subir el volumen.
//! - **Desde el sistema**: los botones de ese panel y las teclas de medios del
//!   teclado llegan aquí y se convierten en llamadas al reproductor.
//!
//! ## Por qué escucha el bus y no llama el reproductor
//!
//! `PlaybackService` no sabe que existe un panel del sistema, y no debería:
//! es una integración opcional de una plataforma concreta. Publicar eventos ya
//! era necesario para el frontend, así que este módulo se engancha ahí. Añadir
//! MPRIS en Linux sería otro suscriptor, sin tocar el reproductor.
//!
//! ## El manejador no puede bloquear
//!
//! Los botones del panel llegan en un hilo de COM. Bloquearlo esperando a que
//! el actor responda congelaría el panel del sistema —no solo Localify—, así
//! que lo único que se hace ahí es encolar en un canal.
//!
//! ## Por qué `tauri::async_runtime::spawn`
//!
//! Esto se arranca desde el `setup` de Tauri, que corre en el hilo principal
//! **fuera** del runtime asíncrono. Un `tokio::spawn` ahí entra en pánico con
//! "there is no reactor running", y la ventana no llega a aparecer. Tauri
//! expone su propio runtime justamente para este caso.

use std::sync::Arc;

use localify_core::domain::audio::DurationMs;
use localify_core::domain::queue::PlayStatus;
use localify_core::events::DomainEvent;
use localify_core::ports::platform::{MediaCommand, NowPlaying, SystemMediaIntegration};
use localify_core::ports::services::{MetadataService, PlaybackService};
use tokio::sync::mpsc;
use tracing::{debug, warn};

use crate::bridge::EventBus;

/// Arranca la integración multimedia para la ventana `hwnd`.
///
/// Si el sistema no concede el panel, todo esto se convierte en una
/// implementación que no hace nada y la reproducción sigue igual.
///
/// Es `async` porque MPRIS necesita abrir una conexión de D-Bus antes de
/// poder publicar nada; `bus` se recibe por valor (clonarlo es barato, es un
/// `broadcast::Sender`) porque esta función se lanza con
/// `tauri::async_runtime::spawn`, cuyo futuro debe ser `'static`.
pub async fn arrancar(
    hwnd: isize,
    playback: Arc<dyn PlaybackService>,
    metadata: Arc<dyn MetadataService>,
    bus: EventBus,
) {
    let sistema = localify_platform::integracion_multimedia(hwnd).await;

    // ── Del sistema hacia el reproductor ────────────────────────────────────
    let (tx, rx) = mpsc::unbounded_channel();
    sistema.set_command_handler(Box::new(move |orden| {
        // Se encola y se vuelve: este código corre en un hilo de COM en
        // Windows, y en la tarea de D-Bus de `mpris-server` en Linux.
        let _ = tx.send(orden);
    }));
    tauri::async_runtime::spawn(atender_ordenes(rx, Arc::clone(&playback)));

    // ── Del reproductor hacia el sistema ────────────────────────────────────
    tauri::async_runtime::spawn(reflejar_estado(
        bus.subscribe(),
        sistema,
        playback,
        metadata,
    ));
}

/// Traduce las órdenes del sistema a llamadas al reproductor.
async fn atender_ordenes(
    mut rx: mpsc::UnboundedReceiver<MediaCommand>,
    playback: Arc<dyn PlaybackService>,
) {
    while let Some(orden) = rx.recv().await {
        debug!(?orden, "orden multimedia del sistema");
        let resultado = match orden {
            MediaCommand::Play => playback.resume().await.map(|_| ()),
            MediaCommand::Pause => playback.pause().await.map(|_| ()),
            MediaCommand::Toggle => playback.toggle().await.map(|_| ()),
            MediaCommand::Next => playback.next().await.map(|_| ()),
            MediaCommand::Previous => playback.previous().await.map(|_| ()),
            // El sistema pide parar; Localify no tiene "stop", así que lo más
            // parecido —y lo menos sorprendente— es pausar.
            MediaCommand::Stop => playback.pause().await.map(|_| ()),
            MediaCommand::Seek { position_ms } => playback
                .seek(localify_core::domain::audio::DurationMs::new(position_ms))
                .await
                .map(|_| ()),
        };
        if let Err(e) = resultado {
            warn!(?orden, error = %e, "la orden multimedia no se pudo atender");
        }
    }
}

/// Mantiene el panel del sistema al día.
async fn reflejar_estado(
    mut eventos: tokio::sync::broadcast::Receiver<DomainEvent>,
    sistema: Arc<dyn SystemMediaIntegration>,
    playback: Arc<dyn PlaybackService>,
    metadata: Arc<dyn MetadataService>,
) {
    loop {
        let evento = match eventos.recv().await {
            Ok(e) => e,
            // Si este suscriptor se retrasa, se pierde algún evento; el
            // siguiente cambio lo pone al día. No hace falta resincronizar.
            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                debug!(perdidos = n, "el panel multimedia se retraso");
                continue;
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
        };

        match evento {
            DomainEvent::TrackChanged { .. } => {
                // La fila completa se pide al reproductor en vez de al
                // repositorio: es él quien sabe qué suena, y así este módulo no
                // necesita conocer la persistencia.
                let Some(fila) = playback.state().await.track else {
                    continue;
                };
                // La portada sale de la misma caché que alimenta la interfaz, así
                // que casi siempre ya está en disco y esto es leer una ruta. Si
                // no lo está, se baja aquí: el panel del sistema es lo que se ve
                // al subir el volumen, y enseñar ahí el icono genérico de la
                // aplicación mientras la pantalla muestra la carátula correcta
                // se lee como un fallo.
                //
                // Un error no interrumpe nada: `None` es exactamente lo que este
                // campo valía antes, y el panel sigue funcionando.
                let cover_path = match &fila.album_id {
                    Some(album) => metadata.ensure_cover(album).await.ok().flatten(),
                    None => None,
                };
                let info = NowPlaying {
                    title: fila.title,
                    artist: fila.artist_display,
                    album: fila.album_title,
                    duration: fila.duration,
                    cover_path,
                };
                if let Err(e) = sistema.set_now_playing(&info).await {
                    warn!(error = %e, "no se pudo actualizar el panel multimedia");
                }
                let (pos, _) = playback.position();
                let _ = sistema.set_position(pos, info.duration).await;
            }
            DomainEvent::PlayStatusChanged { status } => {
                let _ = sistema.set_status(status).await;
                if status == PlayStatus::Stopped {
                    let _ = sistema.clear().await;
                }
            }
            DomainEvent::Seeked { position_ms, .. } => {
                // La duración no viaja en el evento; se pide la fila actual
                // solo por eso. Sin este brazo, saltar dentro de una canción
                // no movía la posición del panel del sistema hasta el
                // siguiente cambio de pista.
                let Some(fila) = playback.state().await.track else {
                    continue;
                };
                let _ = sistema
                    .set_position(DurationMs::new(position_ms), fila.duration)
                    .await;
            }
            _ => {}
        }
    }
    debug!("integracion multimedia terminada");
}

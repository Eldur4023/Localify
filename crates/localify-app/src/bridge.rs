//! Bus de eventos y puente hacia el WebView.
//!
//! Es el único punto del programa que conoce a la vez el bus del dominio y a
//! Tauri. Los servicios publican `DomainEvent` sin saber que existe un WebView;
//! aquí se traducen a DTOs y se emiten.

use std::sync::Arc;

use localify_core::events::{DomainEvent, EventPublisher};
use tauri::{AppHandle, Emitter};
use tokio::sync::broadcast;
use tracing::{debug, warn};

use crate::dto::events::LocalifyEvent;

/// Canal por el que viajan los eventos hacia el frontend.
pub const CANAL_EVENTOS: &str = "localify://event";

/// Canal de resincronización.
///
/// Se emite cuando el bus pierde mensajes. El frontend responde recargando su
/// estado con los comandos de consulta, en lugar de quedarse desincronizado sin
/// enterarse.
pub const CANAL_RESYNC: &str = "localify://resync";

/// Capacidad del bus.
///
/// Suficiente para absorber una ráfaga (importar una playlist, terminar veinte
/// descargas) sin que un consumidor momentáneamente ocupado pierda eventos.
/// Más allá, el coste de memoria no compensa: para eso está la resincronización.
const CAPACIDAD: usize = 512;

/// Bus de eventos del dominio.
#[derive(Clone)]
pub struct EventBus {
    tx: broadcast::Sender<DomainEvent>,
}

impl std::fmt::Debug for EventBus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EventBus")
            .field("suscriptores", &self.tx.receiver_count())
            .finish()
    }
}

impl EventBus {
    #[must_use]
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(CAPACIDAD);
        Self { tx }
    }

    /// Nuevo receptor. Cada consumidor (puente, Discord, SMTC) tiene el suyo
    /// y avanza a su ritmo.
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<DomainEvent> {
        self.tx.subscribe()
    }

    #[must_use]
    pub fn suscriptores(&self) -> usize {
        self.tx.receiver_count()
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

impl EventPublisher for EventBus {
    fn publish(&self, event: DomainEvent) {
        // `send` falla solo si no hay ningún suscriptor. No es un error: al
        // arrancar, o con todas las integraciones desactivadas, es lo normal.
        // Y la corrección del sistema no depende de que el evento llegue: para
        // cada uno existe un comando que reconstruye el estado.
        let _ = self.tx.send(event);
    }
}

/// Arranca la tarea que traduce eventos del dominio y los emite al WebView.
///
/// # Manejo de `Lagged`
///
/// `broadcast` descarta mensajes si un receptor no sigue el ritmo. Ignorarlo
/// dejaría la interfaz desincronizada de forma **silenciosa**, que es el peor
/// modo de fallo posible: todo parece funcionar y los datos están mal. En su
/// lugar se convierte en una señal explícita de resincronización.
pub fn arrancar(app: AppHandle, bus: &EventBus) {
    let mut rx = bus.subscribe();

    tauri::async_runtime::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(evento) => {
                    let dto: LocalifyEvent = evento.into();
                    if let Err(e) = app.emit(CANAL_EVENTOS, &dto) {
                        warn!(error = %e, "no se pudo emitir el evento al WebView");
                    }
                }
                Err(broadcast::error::RecvError::Lagged(perdidos)) => {
                    warn!(perdidos, "el puente se retrasó; forzando resincronización");
                    if let Err(e) = app.emit(CANAL_RESYNC, ()) {
                        warn!(error = %e, "no se pudo emitir la resincronización");
                    }
                }
                Err(broadcast::error::RecvError::Closed) => {
                    debug!("bus cerrado; el puente termina");
                    break;
                }
            }
        }
    });
}

/// Publicador compartible.
pub type SharedPublisher = Arc<dyn EventPublisher>;

#[cfg(test)]
mod tests {
    use localify_core::domain::ids::TrackId;

    use super::*;

    #[test]
    fn publicar_sin_suscriptores_no_falla() {
        // Ocurre al arrancar y con todas las integraciones desactivadas.
        let bus = EventBus::new();
        assert_eq!(bus.suscriptores(), 0);
        bus.publish(DomainEvent::QueueChanged { revision: 1 });
    }

    #[tokio::test]
    async fn todos_los_suscriptores_reciben_cada_evento() {
        let bus = EventBus::new();
        let mut uno = bus.subscribe();
        let mut otro = bus.subscribe();
        assert_eq!(bus.suscriptores(), 2);

        bus.publish(DomainEvent::QueueChanged { revision: 7 });

        for rx in [&mut uno, &mut otro] {
            match rx.recv().await.expect("recibe") {
                DomainEvent::QueueChanged { revision } => assert_eq!(revision, 7),
                otro => panic!("evento inesperado: {otro:?}"),
            }
        }
    }

    #[tokio::test]
    async fn un_consumidor_lento_recibe_lagged_en_vez_de_datos_incorrectos() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();

        // Desborda el bus sin consumir nada.
        for i in 0..(CAPACIDAD as u64 + 50) {
            bus.publish(DomainEvent::QueueChanged { revision: i });
        }

        let resultado = rx.recv().await;
        assert!(
            matches!(resultado, Err(broadcast::error::RecvError::Lagged(_))),
            "un desbordamiento debe avisar, no entregar datos salteados en silencio"
        );

        // Tras el aviso, el receptor sigue siendo utilizable y retoma por el
        // mensaje más antiguo que quede en el buffer. Por eso basta con emitir
        // una resincronización: no hace falta reconstruir el puente.
        let siguiente = rx.recv().await;
        assert!(
            siguiente.is_ok(),
            "el receptor debe seguir vivo tras un Lagged, no quedar inservible"
        );
    }

    #[tokio::test]
    async fn el_receptor_termina_cuando_se_cierra_el_bus() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        drop(bus);

        assert!(matches!(
            rx.recv().await,
            Err(broadcast::error::RecvError::Closed)
        ));
    }

    #[tokio::test]
    async fn los_eventos_llegan_en_orden() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();

        bus.publish(DomainEvent::DownloadStarted {
            track_id: TrackId::nuevo_local(),
        });
        bus.publish(DomainEvent::QueueChanged { revision: 1 });

        assert!(matches!(
            rx.recv().await.expect("recibe"),
            DomainEvent::DownloadStarted { .. }
        ));
        assert!(matches!(
            rx.recv().await.expect("recibe"),
            DomainEvent::QueueChanged { .. }
        ));
    }
}

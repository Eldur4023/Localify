//! Bus de eventos del dominio.
//!
//! ## Contrato
//!
//! 1. Un evento es un **hecho consumado**: `TrackDownloaded`, nunca
//!    `DownloadTrack`.
//! 2. Un evento lleva **identificadores y deltas**, nunca agregados completos.
//!    Todo lo que cruza el puente IPC se serializa a JSON, y un evento gordo
//!    emitido con frecuencia es la forma más fácil de estrangular la UI.
//! 3. **Perder un evento nunca corrompe nada.** Para cada uno existe un comando
//!    que reconstruye el estado (`player_get_state`, `queue_get`). El evento es
//!    una optimización, no la fuente de verdad.
//! 4. Los eventos de alta frecuencia (posición de reproducción) **no van por
//!    aquí**: se sondean con un comando que lee un atómico.
//! 5. El throttling se aplica en el **emisor**, antes de publicar. Un consumidor
//!    no debería tener que defenderse de una avalancha.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::domain::availability::Availability;
use crate::domain::ids::{PlaylistId, TrackId};
use crate::domain::queue::{ChangeSource, PlayStatus, RepeatMode};
use crate::domain::settings::SettingsSection;

/// Ámbito de la biblioteca afectado por un cambio. Permite que una vista
/// decida si le concierne sin recargar por cualquier cosa.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LibraryScope {
    Tracks,
    Albums,
    Artists,
    Favorites,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PlaylistChangeKind {
    Created,
    Renamed,
    Deleted,
    /// Cambió el contenido: se añadieron, quitaron o reordenaron entradas.
    Items,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ToastLevel {
    Info,
    Warn,
    Error,
}

/// Estado de un proveedor externo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum ProviderStatus {
    /// Operativo.
    Ready,
    /// Faltan credenciales. Es accionable desde Ajustes.
    NotConfigured,
    /// Configurado pero sin respuesta. La app sigue funcionando en local.
    #[serde(rename_all = "camelCase")]
    Unavailable { reason_key: String },
}

impl ProviderStatus {
    #[must_use]
    pub const fn esta_operativo(&self) -> bool {
        matches!(self, Self::Ready)
    }
}

/// Todo lo que el backend comunica hacia fuera.
///
/// Enum exhaustivo a propósito: añadir un evento obliga a revisar el puente y
/// el tipo del frontend, en lugar de colarse como un string suelto.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum DomainEvent {
    // ── Reproducción ────────────────────────────────────────────────────────
    #[serde(rename_all = "camelCase")]
    TrackChanged {
        track_id: TrackId,
        source: ChangeSource,
    },
    #[serde(rename_all = "camelCase")]
    PlayStatusChanged { status: PlayStatus },
    #[serde(rename_all = "camelCase")]
    VolumeChanged { volume: f32 },
    #[serde(rename_all = "camelCase")]
    RepeatModeChanged { mode: RepeatMode },
    #[serde(rename_all = "camelCase")]
    ShuffleChanged { enabled: bool },
    /// La pista terminó. `completed` distingue una escucha real de un salto,
    /// y es lo que alimenta el historial y las recomendaciones.
    ///
    /// `ms_played` va además de `completed` porque el scrobbling **no usa la
    /// misma regla**: aquí una escucha cuenta como completa al 90 %, y Last.fm
    /// scrobblea al 50 % o a los cuatro minutos, lo que llegue antes. Sin el
    /// tiempo en bruto, quien scrobblea solo puede elegir entre usar el 90 % de
    /// otro —y perder scrobbles legítimos— o volver a medir por su cuenta algo
    /// que el reproductor ya sabe.
    #[serde(rename_all = "camelCase")]
    TrackFinished {
        track_id: TrackId,
        completed: bool,
        ms_played: u32,
    },

    // ── Cola ────────────────────────────────────────────────────────────────
    #[serde(rename_all = "camelCase")]
    QueueChanged { revision: u64 },

    // ── Descargas ───────────────────────────────────────────────────────────
    // Invisibles para el usuario: solo mueven indicadores discretos.
    #[serde(rename_all = "camelCase")]
    DownloadStarted { track_id: TrackId },
    /// Hay bytes suficientes para empezar a sonar. Es el evento que dispara la
    /// reproducción progresiva.
    #[serde(rename_all = "camelCase")]
    DownloadPlayable { track_id: TrackId },
    /// Limitado a 2 Hz por descarga, en el emisor.
    #[serde(rename_all = "camelCase")]
    DownloadProgress { track_id: TrackId, percent: f32 },
    #[serde(rename_all = "camelCase")]
    DownloadCompleted { track_id: TrackId },
    #[serde(rename_all = "camelCase")]
    DownloadFailed {
        track_id: TrackId,
        reason_key: String,
    },
    #[serde(rename_all = "camelCase")]
    AvailabilityChanged {
        track_id: TrackId,
        availability: Availability,
    },

    // ── Biblioteca y playlists ──────────────────────────────────────────────
    #[serde(rename_all = "camelCase")]
    LibraryChanged { scope: LibraryScope },
    #[serde(rename_all = "camelCase")]
    PlaylistChanged {
        playlist_id: PlaylistId,
        kind: PlaylistChangeKind,
    },
    #[serde(rename_all = "camelCase")]
    PlaylistImportProgress {
        import_id: Uuid,
        done: u32,
        total: u32,
    },
    #[serde(rename_all = "camelCase")]
    PlaylistImportFinished {
        import_id: Uuid,
        playlist_id: PlaylistId,
    },
    #[serde(rename_all = "camelCase")]
    ScanProgress {
        scan_id: Uuid,
        done: u32,
        total: u32,
    },

    // ── Búsqueda ────────────────────────────────────────────────────────────
    /// Los resultados remotos de `query_id` ya están en la base de datos local.
    /// El frontend repite la consulta para recogerlos.
    #[serde(rename_all = "camelCase")]
    SearchRemoteReady { query_id: u64 },

    // ── Sistema ─────────────────────────────────────────────────────────────
    #[serde(rename_all = "camelCase")]
    SettingsChanged { sections: Vec<SettingsSection> },
    #[serde(rename_all = "camelCase")]
    ProviderStatusChanged {
        provider: String,
        status: ProviderStatus,
    },
    /// Avance de la copia al cambiar de carpeta de biblioteca.
    ///
    /// Existe porque `change_library_path` devuelve su identificador
    /// inmediatamente: la copia puede durar minutos y sin este evento el
    /// identificador no serviría para nada. Termina con
    /// [`DomainEvent::LibraryPathChanged`], que es la señal de que la carpeta
    /// nueva ya es la buena.
    #[serde(rename_all = "camelCase")]
    LibraryMoveProgress {
        move_id: Uuid,
        done: u32,
        total: u32,
    },
    #[serde(rename_all = "camelCase")]
    LibraryPathChanged { path: String },
    /// Aviso in-app. Discreto: Localify nunca notifica descargas.
    #[serde(rename_all = "camelCase")]
    Toast {
        level: ToastLevel,
        message_key: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        params: Vec<(String, String)>,
    },
}

impl DomainEvent {
    /// `true` si el evento puede llegar en ráfaga y conviene limitarlo antes de
    /// publicarlo.
    #[must_use]
    pub const fn es_de_alta_frecuencia(&self) -> bool {
        matches!(
            self,
            Self::DownloadProgress { .. }
                | Self::ScanProgress { .. }
                | Self::LibraryMoveProgress { .. }
        )
    }

    /// Nombre estable para logs y métricas.
    #[must_use]
    pub const fn nombre(&self) -> &'static str {
        match self {
            Self::TrackChanged { .. } => "trackChanged",
            Self::PlayStatusChanged { .. } => "playStatusChanged",
            Self::VolumeChanged { .. } => "volumeChanged",
            Self::RepeatModeChanged { .. } => "repeatModeChanged",
            Self::ShuffleChanged { .. } => "shuffleChanged",
            Self::TrackFinished { .. } => "trackFinished",
            Self::QueueChanged { .. } => "queueChanged",
            Self::DownloadStarted { .. } => "downloadStarted",
            Self::DownloadPlayable { .. } => "downloadPlayable",
            Self::DownloadProgress { .. } => "downloadProgress",
            Self::DownloadCompleted { .. } => "downloadCompleted",
            Self::DownloadFailed { .. } => "downloadFailed",
            Self::AvailabilityChanged { .. } => "availabilityChanged",
            Self::LibraryChanged { .. } => "libraryChanged",
            Self::PlaylistChanged { .. } => "playlistChanged",
            Self::PlaylistImportProgress { .. } => "playlistImportProgress",
            Self::PlaylistImportFinished { .. } => "playlistImportFinished",
            Self::ScanProgress { .. } => "scanProgress",
            Self::SearchRemoteReady { .. } => "searchRemoteReady",
            Self::SettingsChanged { .. } => "settingsChanged",
            Self::ProviderStatusChanged { .. } => "providerStatusChanged",
            Self::LibraryMoveProgress { .. } => "libraryMoveProgress",
            Self::LibraryPathChanged { .. } => "libraryPathChanged",
            Self::Toast { .. } => "toast",
        }
    }
}

/// Publicador de eventos.
///
/// Es un trait y no un `broadcast::Sender` concreto para que `core` no dependa
/// de un runtime async, y para que los tests puedan capturar los eventos
/// emitidos con un doble que solo los acumula en un `Vec`.
pub trait EventPublisher: Send + Sync + 'static {
    /// Publica un evento.
    ///
    /// **No falla y no bloquea.** Si no hay suscriptores o el bus va saturado,
    /// el evento se descarta: la corrección del sistema no depende de que
    /// llegue (regla 3). Un fallo aquí nunca debe abortar una operación de
    /// negocio que ya se completó.
    fn publish(&self, event: DomainEvent);
}

/// Publicador nulo, para tests y para arranques en los que el bus aún no
/// existe.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoopPublisher;

impl EventPublisher for NoopPublisher {
    fn publish(&self, _event: DomainEvent) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn los_eventos_se_serializan_con_discriminante_type() {
        let ev = DomainEvent::TrackChanged {
            track_id: TrackId::from_trusted("3z8h0TU7ReDPLIbEnYhWZb"),
            source: ChangeSource::User,
        };
        let json = serde_json::to_string(&ev).expect("serializa");
        assert!(json.contains(r#""type":"trackChanged""#), "{json}");
        assert!(
            json.contains(r#""trackId""#),
            "las claves deben ir en camelCase: {json}"
        );
    }

    #[test]
    fn el_nombre_coincide_con_el_discriminante_serializado() {
        let eventos = [
            DomainEvent::QueueChanged { revision: 1 },
            DomainEvent::SearchRemoteReady { query_id: 9 },
            DomainEvent::LibraryChanged {
                scope: LibraryScope::Tracks,
            },
            DomainEvent::PlayStatusChanged {
                status: PlayStatus::Playing,
            },
        ];
        for ev in eventos {
            let json: serde_json::Value = serde_json::to_value(&ev).expect("serializa");
            assert_eq!(json["type"].as_str(), Some(ev.nombre()));
        }
    }

    #[test]
    fn solo_los_eventos_de_progreso_son_de_alta_frecuencia() {
        let progreso = DomainEvent::DownloadProgress {
            track_id: TrackId::nuevo_local(),
            percent: 0.5,
        };
        assert!(progreso.es_de_alta_frecuencia());
        assert!(!DomainEvent::QueueChanged { revision: 1 }.es_de_alta_frecuencia());
    }

    #[test]
    fn el_publicador_nulo_acepta_cualquier_evento() {
        let p = NoopPublisher;
        p.publish(DomainEvent::QueueChanged { revision: 1 });
    }
}

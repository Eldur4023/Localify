//! Eventos tal como los recibe el frontend.
//!
//! Se corresponden uno a uno con [`localify_core::events::DomainEvent`], pero
//! son un tipo aparte: el dominio puede cambiar su representación interna sin
//! romper a los clientes.

use localify_core::domain::queue::ChangeSource;
use localify_core::events::{DomainEvent, LibraryScope, PlaylistChangeKind, ToastLevel};
use serde::Serialize;
use ts_rs::TS;

use super::common::AvailabilityDto;
use super::player::{estado_a_str, repeticion_a_str};
use super::settings::ProviderStatusDto;

/// Todo lo que el backend comunica hacia el frontend.
///
/// Enum exhaustivo a propósito: añadir un evento obliga a tocar aquí y a
/// regenerar los tipos, en lugar de colarse como una cadena suelta.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "types.gen.ts")]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum LocalifyEvent {
    // ── Reproducción ────────────────────────────────────────────────────────
    #[serde(rename_all = "camelCase")]
    TrackChanged { track_id: String, source: String },
    #[serde(rename_all = "camelCase")]
    PlayStatusChanged { status: String },
    /// Alguien movió la aguja dentro de la misma canción. Ver `DomainEvent::Seeked`.
    #[serde(rename_all = "camelCase")]
    Seeked { track_id: String, position_ms: u32 },
    #[serde(rename_all = "camelCase")]
    VolumeChanged { volume: f32 },
    #[serde(rename_all = "camelCase")]
    RepeatModeChanged { mode: String },
    #[serde(rename_all = "camelCase")]
    ShuffleChanged { enabled: bool },
    #[serde(rename_all = "camelCase")]
    TrackFinished {
        track_id: String,
        completed: bool,
        ms_played: u32,
    },

    // ── Cola ────────────────────────────────────────────────────────────────
    #[serde(rename_all = "camelCase")]
    QueueChanged { revision: u64 },

    // ── Descargas ───────────────────────────────────────────────────────────
    // Invisibles para el usuario: solo mueven indicadores discretos.
    #[serde(rename_all = "camelCase")]
    DownloadStarted { track_id: String },
    #[serde(rename_all = "camelCase")]
    DownloadPlayable { track_id: String },
    #[serde(rename_all = "camelCase")]
    DownloadProgress { track_id: String, percent: f32 },
    #[serde(rename_all = "camelCase")]
    DownloadCompleted { track_id: String },
    #[serde(rename_all = "camelCase")]
    DownloadFailed {
        track_id: String,
        reason_key: String,
    },
    #[serde(rename_all = "camelCase")]
    AvailabilityChanged {
        track_id: String,
        availability: AvailabilityDto,
    },

    // ── Biblioteca y playlists ──────────────────────────────────────────────
    #[serde(rename_all = "camelCase")]
    LibraryChanged { scope: String },
    #[serde(rename_all = "camelCase")]
    PlaylistChanged { playlist_id: String, kind: String },
    #[serde(rename_all = "camelCase")]
    PlaylistImportProgress {
        import_id: String,
        done: u32,
        total: u32,
    },
    #[serde(rename_all = "camelCase")]
    PlaylistImportFinished {
        import_id: String,
        playlist_id: String,
    },
    #[serde(rename_all = "camelCase")]
    ScanProgress {
        scan_id: String,
        done: u32,
        total: u32,
    },

    // ── Búsqueda ────────────────────────────────────────────────────────────
    #[serde(rename_all = "camelCase")]
    SearchRemoteReady { query_id: u64 },

    // ── Sistema ─────────────────────────────────────────────────────────────
    #[serde(rename_all = "camelCase")]
    SettingsChanged { sections: Vec<String> },
    #[serde(rename_all = "camelCase")]
    ProviderStatusChanged {
        provider: String,
        status: ProviderStatusDto,
    },
    #[serde(rename_all = "camelCase")]
    LibraryMoveProgress {
        move_id: String,
        done: u32,
        total: u32,
    },
    #[serde(rename_all = "camelCase")]
    LibraryPathChanged { path: String },
    #[serde(rename_all = "camelCase")]
    UpdateAvailable { version: String },
    #[serde(rename_all = "camelCase")]
    Toast {
        level: String,
        message_key: String,
        params: Vec<(String, String)>,
    },
}

const fn origen_a_str(s: ChangeSource) -> &'static str {
    match s {
        ChangeSource::User => "user",
        ChangeSource::Queue => "queue",
        ChangeSource::Restore => "restore",
    }
}

const fn ambito_a_str(s: LibraryScope) -> &'static str {
    match s {
        LibraryScope::Tracks => "tracks",
        LibraryScope::Albums => "albums",
        LibraryScope::Artists => "artists",
        LibraryScope::Favorites => "favorites",
    }
}

const fn cambio_playlist_a_str(k: PlaylistChangeKind) -> &'static str {
    match k {
        PlaylistChangeKind::Created => "created",
        PlaylistChangeKind::Renamed => "renamed",
        PlaylistChangeKind::Deleted => "deleted",
        PlaylistChangeKind::Items => "items",
    }
}

const fn nivel_a_str(l: ToastLevel) -> &'static str {
    match l {
        ToastLevel::Info => "info",
        ToastLevel::Warn => "warn",
        ToastLevel::Error => "error",
    }
}

fn seccion_a_str(s: localify_core::domain::settings::SettingsSection) -> &'static str {
    use localify_core::domain::settings::SettingsSection as S;
    match s {
        S::Language => "language",
        S::Provider => "provider",
        S::LibraryPath => "libraryPath",
        S::Audio => "audio",
        S::Download => "download",
        S::Spotify => "spotify",
        S::Integrations => "integrations",
        S::Ui => "ui",
    }
}

impl From<DomainEvent> for LocalifyEvent {
    // Un `match` exhaustivo sobre 23 variantes es largo por naturaleza.
    // Trocearlo en cuatro funciones obligaría a mirar en cuatro sitios para
    // seguir una sola variante, y el compilador dejaría de garantizar que están
    // todas cubiertas en un único punto.
    #[allow(
        clippy::too_many_lines,
        reason = "traducción 1:1 de un enum exhaustivo"
    )]
    fn from(e: DomainEvent) -> Self {
        match e {
            DomainEvent::TrackChanged { track_id, source } => Self::TrackChanged {
                track_id: track_id.into_string(),
                source: origen_a_str(source).to_owned(),
            },
            DomainEvent::PlayStatusChanged { status } => Self::PlayStatusChanged {
                status: estado_a_str(status).to_owned(),
            },
            DomainEvent::Seeked {
                track_id,
                position_ms,
            } => Self::Seeked {
                track_id: track_id.into_string(),
                position_ms,
            },
            DomainEvent::VolumeChanged { volume } => Self::VolumeChanged { volume },
            DomainEvent::RepeatModeChanged { mode } => Self::RepeatModeChanged {
                mode: repeticion_a_str(mode).to_owned(),
            },
            DomainEvent::ShuffleChanged { enabled } => Self::ShuffleChanged { enabled },
            DomainEvent::TrackFinished {
                track_id,
                completed,
                ms_played,
            } => Self::TrackFinished {
                track_id: track_id.into_string(),
                completed,
                ms_played,
            },
            DomainEvent::QueueChanged { revision } => Self::QueueChanged { revision },
            DomainEvent::DownloadStarted { track_id } => Self::DownloadStarted {
                track_id: track_id.into_string(),
            },
            DomainEvent::DownloadPlayable { track_id } => Self::DownloadPlayable {
                track_id: track_id.into_string(),
            },
            DomainEvent::DownloadProgress { track_id, percent } => Self::DownloadProgress {
                track_id: track_id.into_string(),
                percent,
            },
            DomainEvent::DownloadCompleted { track_id } => Self::DownloadCompleted {
                track_id: track_id.into_string(),
            },
            DomainEvent::DownloadFailed {
                track_id,
                reason_key,
            } => Self::DownloadFailed {
                track_id: track_id.into_string(),
                reason_key,
            },
            DomainEvent::AvailabilityChanged {
                track_id,
                availability,
            } => Self::AvailabilityChanged {
                track_id: track_id.into_string(),
                availability: availability.into(),
            },
            DomainEvent::LibraryChanged { scope } => Self::LibraryChanged {
                scope: ambito_a_str(scope).to_owned(),
            },
            DomainEvent::PlaylistChanged { playlist_id, kind } => Self::PlaylistChanged {
                playlist_id: playlist_id.to_string(),
                kind: cambio_playlist_a_str(kind).to_owned(),
            },
            DomainEvent::PlaylistImportProgress {
                import_id,
                done,
                total,
            } => Self::PlaylistImportProgress {
                import_id: import_id.to_string(),
                done,
                total,
            },
            DomainEvent::PlaylistImportFinished {
                import_id,
                playlist_id,
            } => Self::PlaylistImportFinished {
                import_id: import_id.to_string(),
                playlist_id: playlist_id.to_string(),
            },
            DomainEvent::ScanProgress {
                scan_id,
                done,
                total,
            } => Self::ScanProgress {
                scan_id: scan_id.to_string(),
                done,
                total,
            },
            DomainEvent::SearchRemoteReady { query_id } => Self::SearchRemoteReady { query_id },
            DomainEvent::SettingsChanged { sections } => Self::SettingsChanged {
                sections: sections
                    .into_iter()
                    .map(|s| seccion_a_str(s).to_owned())
                    .collect(),
            },
            DomainEvent::ProviderStatusChanged { provider, status } => {
                Self::ProviderStatusChanged {
                    provider,
                    status: status.into(),
                }
            }
            DomainEvent::LibraryMoveProgress {
                move_id,
                done,
                total,
            } => Self::LibraryMoveProgress {
                move_id: move_id.to_string(),
                done,
                total,
            },
            DomainEvent::LibraryPathChanged { path } => Self::LibraryPathChanged { path },
            DomainEvent::UpdateAvailable { version } => Self::UpdateAvailable { version },
            DomainEvent::Toast {
                level,
                message_key,
                params,
            } => Self::Toast {
                level: nivel_a_str(level).to_owned(),
                message_key,
                params,
            },
        }
    }
}

impl LocalifyEvent {
    /// Nombre del evento, tal como aparece en `type`.
    #[must_use]
    pub fn nombre(&self) -> String {
        serde_json::to_value(self)
            .ok()
            .and_then(|v| v.get("type").and_then(|t| t.as_str().map(str::to_owned)))
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use localify_core::domain::ids::TrackId;
    use localify_core::domain::queue::PlayStatus;

    use super::*;

    #[test]
    fn el_nombre_del_dto_coincide_con_el_del_dominio() {
        // Si divergieran, el frontend escucharía un nombre y el backend
        // emitiría otro, y el fallo sería silencioso.
        let casos = vec![
            DomainEvent::QueueChanged { revision: 1 },
            DomainEvent::SearchRemoteReady { query_id: 9 },
            DomainEvent::LibraryChanged {
                scope: LibraryScope::Tracks,
            },
            DomainEvent::PlayStatusChanged {
                status: PlayStatus::Playing,
            },
            DomainEvent::DownloadStarted {
                track_id: TrackId::nuevo_local(),
            },
            DomainEvent::TrackFinished {
                track_id: TrackId::nuevo_local(),
                completed: true,
                ms_played: 200_000,
            },
            DomainEvent::LibraryPathChanged {
                path: "D:/M".into(),
            },
        ];

        for ev in casos {
            let esperado = ev.nombre().to_owned();
            let dto: LocalifyEvent = ev.into();
            assert_eq!(dto.nombre(), esperado);
        }
    }

    #[test]
    fn los_eventos_se_serializan_con_discriminante_type_y_camel_case() {
        let dto: LocalifyEvent = DomainEvent::TrackChanged {
            track_id: TrackId::from_trusted("3z8h0TU7ReDPLIbEnYhWZb"),
            source: ChangeSource::Restore,
        }
        .into();

        let json = serde_json::to_value(&dto).expect("serializa");
        assert_eq!(json["type"], "trackChanged");
        assert_eq!(json["trackId"], "3z8h0TU7ReDPLIbEnYhWZb");
        assert_eq!(json["source"], "restore");
    }

    #[test]
    fn el_progreso_de_descarga_no_expone_rutas() {
        let dto: LocalifyEvent = DomainEvent::AvailabilityChanged {
            track_id: TrackId::nuevo_local(),
            availability: localify_core::domain::availability::Availability::Local {
                rel_path: std::path::PathBuf::from("audio/aa/privado.opus"),
                format: localify_core::domain::audio::AudioFormat::Opus,
                bytes: 100,
            },
        }
        .into();

        let json = serde_json::to_string(&dto).expect("serializa");
        assert!(!json.contains("privado"), "{json}");
    }
}

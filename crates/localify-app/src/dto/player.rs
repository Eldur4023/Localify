//! DTOs del reproductor y de la cola.

use localify_core::domain::ids::{AlbumId, ArtistId, PlaylistId, TrackId};
use localify_core::domain::queue::{
    PlayStatus, PlaybackContext, PlayerState, QueueEntry, QueueSnapshot, RepeatMode,
};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::catalog::TrackRowDto;

/// De dónde salió la reproducción. Determina qué suena después y qué texto
/// muestra el panel de cola.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "types.gen.ts")]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum PlaybackContextDto {
    #[serde(rename_all = "camelCase")]
    Album {
        id: String,
    },
    #[serde(rename_all = "camelCase")]
    Playlist {
        id: String,
    },
    #[serde(rename_all = "camelCase")]
    Artist {
        id: String,
    },
    Liked,
    Library,
    /// Los resultados de búsqueda son efímeros: el conjunto viaja con el
    /// contexto porque no puede reconstruirse desde un identificador.
    #[serde(rename_all = "camelCase")]
    Search {
        query: String,
        track_ids: Vec<String>,
    },
    #[serde(rename_all = "camelCase")]
    Recommendation {
        seed_track_id: String,
        track_ids: Vec<String>,
    },
    /// Una sola pista: al acabar, no hay siguiente.
    Single,
}

impl From<PlaybackContext> for PlaybackContextDto {
    fn from(c: PlaybackContext) -> Self {
        match c {
            PlaybackContext::Album { id } => Self::Album {
                id: id.into_string(),
            },
            PlaybackContext::Playlist { id } => Self::Playlist { id: id.to_string() },
            PlaybackContext::Artist { id } => Self::Artist {
                id: id.into_string(),
            },
            PlaybackContext::Liked => Self::Liked,
            PlaybackContext::Library => Self::Library,
            PlaybackContext::Search { query, track_ids } => Self::Search {
                query,
                track_ids: track_ids.into_iter().map(TrackId::into_string).collect(),
            },
            PlaybackContext::Recommendation {
                seed_track_id,
                track_ids,
            } => Self::Recommendation {
                seed_track_id: seed_track_id.into_string(),
                track_ids: track_ids.into_iter().map(TrackId::into_string).collect(),
            },
            PlaybackContext::Single => Self::Single,
        }
    }
}

impl TryFrom<PlaybackContextDto> for PlaybackContext {
    type Error = localify_core::error::CoreError;

    fn try_from(d: PlaybackContextDto) -> Result<Self, Self::Error> {
        // Los identificadores que llegan del cliente **sí** se validan: es la
        // frontera de confianza. Los que salen del dominio no hace falta.
        Ok(match d {
            PlaybackContextDto::Album { id } => Self::Album {
                id: AlbumId::parse(id)?,
            },
            PlaybackContextDto::Playlist { id } => Self::Playlist {
                id: PlaylistId::parse(&id)?,
            },
            PlaybackContextDto::Artist { id } => Self::Artist {
                id: ArtistId::parse(id)?,
            },
            PlaybackContextDto::Liked => Self::Liked,
            PlaybackContextDto::Library => Self::Library,
            PlaybackContextDto::Search { query, track_ids } => Self::Search {
                query,
                track_ids: track_ids
                    .into_iter()
                    .map(TrackId::parse)
                    .collect::<Result<Vec<_>, _>>()?,
            },
            PlaybackContextDto::Recommendation {
                seed_track_id,
                track_ids,
            } => Self::Recommendation {
                seed_track_id: TrackId::parse(seed_track_id)?,
                track_ids: track_ids
                    .into_iter()
                    .map(TrackId::parse)
                    .collect::<Result<Vec<_>, _>>()?,
            },
            PlaybackContextDto::Single => Self::Single,
        })
    }
}

/// Estado completo del reproductor.
///
/// Es la respuesta de `player_get_state`, el comando de resincronización cuando
/// el frontend pierde eventos.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "types.gen.ts")]
#[serde(rename_all = "camelCase")]
pub struct PlayerStateDto {
    pub track: Option<TrackRowDto>,
    pub status: String,
    pub position_ms: u32,
    pub duration_ms: u32,
    /// Cuánto hay decodificable. Solo difiere de `durationMs` durante una
    /// descarga progresiva.
    pub buffered_ms: u32,
    pub volume: f32,
    pub repeat: String,
    pub shuffle: bool,
    pub context: Option<PlaybackContextDto>,
}

/// Nombre estable de un estado de reproducción.
#[must_use]
pub const fn estado_a_str(s: PlayStatus) -> &'static str {
    match s {
        PlayStatus::Playing => "playing",
        PlayStatus::Paused => "paused",
        PlayStatus::Buffering => "buffering",
        PlayStatus::Stopped => "stopped",
    }
}

/// Nombre estable de un modo de repetición.
#[must_use]
pub const fn repeticion_a_str(m: RepeatMode) -> &'static str {
    match m {
        RepeatMode::Off => "off",
        RepeatMode::Queue => "queue",
        RepeatMode::Track => "track",
    }
}

/// # Errors
/// Si el texto no corresponde a ningún modo conocido.
pub fn repeticion_desde_str(s: &str) -> Result<RepeatMode, localify_core::error::CoreError> {
    match s {
        "off" => Ok(RepeatMode::Off),
        "queue" => Ok(RepeatMode::Queue),
        "track" => Ok(RepeatMode::Track),
        otro => Err(localify_core::error::CoreError::invalid(format!(
            "modo de repetición desconocido: '{otro}'"
        ))),
    }
}

impl From<PlayerState> for PlayerStateDto {
    fn from(s: PlayerState) -> Self {
        Self {
            track: s.track.map(Into::into),
            status: estado_a_str(s.status).to_owned(),
            position_ms: s.position.as_ms(),
            duration_ms: s.duration.as_ms(),
            buffered_ms: s.buffered.as_ms(),
            volume: s.volume.as_f32(),
            repeat: repeticion_a_str(s.repeat).to_owned(),
            shuffle: s.shuffle,
            context: s.context.map(Into::into),
        }
    }
}

/// Posición y buffer. Se sondea varias veces por segundo, así que es
/// deliberadamente diminuto: emitirlo como evento saturaría el puente IPC.
#[derive(Debug, Clone, Copy, Serialize, TS)]
#[ts(export, export_to = "types.gen.ts")]
#[serde(rename_all = "camelCase")]
pub struct PositionDto {
    pub position_ms: u32,
    pub buffered_ms: u32,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "types.gen.ts")]
#[serde(rename_all = "camelCase")]
pub struct QueueEntryDto {
    pub entry_id: String,
    pub track: TrackRowDto,
}

impl From<QueueEntry> for QueueEntryDto {
    fn from(e: QueueEntry) -> Self {
        Self {
            entry_id: e.entry_id.to_string(),
            track: e.track.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "types.gen.ts")]
#[serde(rename_all = "camelCase")]
pub struct QueueSnapshotDto {
    /// Monótona: permite descartar respuestas obsoletas sin comparar contenido.
    pub revision: u64,
    pub current: Option<QueueEntryDto>,
    /// "Siguiente en la cola": prioridad absoluta, se consume al reproducirse.
    pub user_queue: Vec<QueueEntryDto>,
    /// Ventana de las siguientes del contexto, no el contexto entero.
    pub context_queue: Vec<QueueEntryDto>,
    pub context: Option<PlaybackContextDto>,
    /// Clave i18n de "Siguiente desde: …".
    pub context_label_key: Option<String>,
}

impl From<QueueSnapshot> for QueueSnapshotDto {
    fn from(q: QueueSnapshot) -> Self {
        Self {
            revision: q.revision,
            current: q.current.map(Into::into),
            user_queue: q.user_queue.into_iter().map(Into::into).collect(),
            context_queue: q.context_queue.into_iter().map(Into::into).collect(),
            context_label_key: q.context.as_ref().map(|c| c.label_key().to_owned()),
            context: q.context.map(Into::into),
        }
    }
}

#[cfg(test)]
mod tests {
    use localify_core::domain::audio::{DurationMs, Volume};

    use super::*;

    #[test]
    fn el_contexto_hace_ida_y_vuelta() {
        let original = PlaybackContext::Album {
            id: AlbumId::from_trusted("1GbtB4zTqAsyfZEsm1RZfx"),
        };
        let dto: PlaybackContextDto = original.clone().into();
        let vuelta: PlaybackContext = dto.try_into().expect("convierte");
        assert_eq!(vuelta, original);
    }

    #[test]
    fn un_contexto_con_id_invalido_se_rechaza_en_la_frontera() {
        // El cliente no es de fiar: un id mal formado debe fallar aquí y no
        // llegar al dominio.
        //
        // El caso de prueba era antes `"no-es-un-id"`, y dejó de servir al
        // admitir identificadores de YouTube: son once caracteres en base64url,
        // que es exactamente lo que mide esa cadena. No hay forma de
        // distinguirlos, y es el precio consciente de aceptar dos catálogos
        // (ver `domain::ids`). Lo que la frontera sigue cazando —y es lo que de
        // verdad llega del cliente cuando algo va mal— es texto que no tiene
        // forma de identificador de nada.
        let dto = PlaybackContextDto::Album {
            id: "esto no es un identificador".into(),
        };
        assert!(PlaybackContext::try_from(dto).is_err());
    }

    #[test]
    fn el_contexto_de_busqueda_lleva_sus_pistas() {
        let dto = PlaybackContextDto::Search {
            query: "queen".into(),
            track_ids: vec!["3z8h0TU7ReDPLIbEnYhWZb".into()],
        };
        let ctx: PlaybackContext = dto.try_into().expect("convierte");
        match ctx {
            PlaybackContext::Search { query, track_ids } => {
                assert_eq!(query, "queen");
                assert_eq!(track_ids.len(), 1);
            }
            otro => panic!("se esperaba Search, llegó {otro:?}"),
        }
    }

    #[test]
    fn los_modos_de_repeticion_hacen_ida_y_vuelta() {
        for m in [RepeatMode::Off, RepeatMode::Queue, RepeatMode::Track] {
            assert_eq!(
                repeticion_desde_str(repeticion_a_str(m)).expect("convierte"),
                m
            );
        }
        assert!(repeticion_desde_str("otro").is_err());
    }

    #[test]
    fn el_estado_se_serializa_con_nombres_estables() {
        let estado = PlayerState {
            track: None,
            status: PlayStatus::Buffering,
            position: DurationMs::new(1234),
            duration: DurationMs::new(248_000),
            buffered: DurationMs::new(30_000),
            volume: Volume::new(0.5),
            repeat: RepeatMode::Queue,
            shuffle: true,
            context: Some(PlaybackContext::Liked),
        };
        let json = serde_json::to_value(PlayerStateDto::from(estado)).expect("serializa");

        assert_eq!(json["status"], "buffering");
        assert_eq!(json["repeat"], "queue");
        assert_eq!(json["positionMs"], 1234);
        assert_eq!(json["bufferedMs"], 30_000);
        assert_eq!(json["context"]["kind"], "liked");
    }

    #[test]
    fn la_cola_expone_la_clave_de_su_contexto() {
        let snapshot = QueueSnapshot {
            revision: 3,
            current: None,
            user_queue: Vec::new(),
            context_queue: Vec::new(),
            context: Some(PlaybackContext::Library),
        };
        let dto: QueueSnapshotDto = snapshot.into();
        assert_eq!(dto.context_label_key.as_deref(), Some("context.library"));
        assert_eq!(dto.revision, 3);
    }
}

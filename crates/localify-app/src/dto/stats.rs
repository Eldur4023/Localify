//! DTOs de estadísticas de escucha.

use localify_core::domain::stats::{ArtistListeningStat, ListeningStats, TrackListeningStat};
use serde::Serialize;
use ts_rs::TS;

use super::catalog::{ArtistRowDto, TrackRowDto};

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "types.gen.ts")]
#[serde(rename_all = "camelCase")]
pub struct ListeningStatsDto {
    pub total_ms_played: u64,
    pub total_plays: u64,
    pub distinct_tracks: u64,
    pub top_tracks: Vec<TrackListeningStatDto>,
    pub top_artists: Vec<ArtistListeningStatDto>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "types.gen.ts")]
#[serde(rename_all = "camelCase")]
pub struct TrackListeningStatDto {
    pub track: TrackRowDto,
    pub ms_played: u64,
    pub plays: u32,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "types.gen.ts")]
#[serde(rename_all = "camelCase")]
pub struct ArtistListeningStatDto {
    pub artist: ArtistRowDto,
    pub ms_played: u64,
}

impl From<ListeningStats> for ListeningStatsDto {
    fn from(s: ListeningStats) -> Self {
        Self {
            total_ms_played: s.total_ms_played,
            total_plays: s.total_plays,
            distinct_tracks: s.distinct_tracks,
            top_tracks: s.top_tracks.into_iter().map(Into::into).collect(),
            top_artists: s.top_artists.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<TrackListeningStat> for TrackListeningStatDto {
    fn from(s: TrackListeningStat) -> Self {
        Self {
            track: s.track.into(),
            ms_played: s.ms_played,
            plays: s.plays,
        }
    }
}

impl From<ArtistListeningStat> for ArtistListeningStatDto {
    fn from(s: ArtistListeningStat) -> Self {
        Self {
            artist: s.artist.into(),
            ms_played: s.ms_played,
        }
    }
}

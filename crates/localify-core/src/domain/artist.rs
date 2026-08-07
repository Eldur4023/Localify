//! Artistas.

use serde::{Deserialize, Serialize};

use super::album::AlbumRow;
use super::ids::ArtistId;
use super::track::TrackRow;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Artist {
    pub id: ArtistId,
    pub name: String,
    pub image_url: Option<String>,
    /// Géneros según Spotify. Son la señal principal del motor de
    /// recomendaciones local: Spotify no asigna géneros a las pistas, solo a
    /// los artistas, así que el género de una pista se hereda de su artista.
    pub genres: Vec<String>,
    pub popularity: Option<u8>,
    pub followers: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtistRow {
    pub id: ArtistId,
    pub name: String,
    pub image_url: Option<String>,
    pub track_count: u32,
    pub local_track_count: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtistDetail {
    pub artist: Artist,
    pub top_tracks: Vec<TrackRow>,
    pub albums: Vec<AlbumRow>,
    pub local_track_count: u32,
}

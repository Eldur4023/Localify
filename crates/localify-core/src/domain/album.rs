//! Álbumes.

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

use super::audio::DurationMs;
use super::ids::AlbumId;
use super::track::{ArtistRef, TrackRow};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AlbumType {
    #[default]
    Album,
    Single,
    Compilation,
}

impl AlbumType {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Album => "album",
            Self::Single => "single",
            Self::Compilation => "compilation",
        }
    }

    #[must_use]
    pub fn from_str_lax(valor: &str) -> Self {
        match valor.to_ascii_lowercase().as_str() {
            "single" => Self::Single,
            "compilation" => Self::Compilation,
            _ => Self::Album,
        }
    }
}

/// Juego de portadas cacheadas en disco, en tres tamaños.
///
/// Se guardan las tres para no reescalar en cada pintado: la rejilla de Inicio
/// usa `small`, las cabeceras `medium`, y la vista ampliada `large`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CoverSet {
    pub small: Option<String>,
    pub medium: Option<String>,
    pub large: Option<String>,
}

impl CoverSet {
    #[must_use]
    pub fn esta_vacio(&self) -> bool {
        self.small.is_none() && self.medium.is_none() && self.large.is_none()
    }

    /// Mejor tamaño disponible, degradando hacia abajo.
    #[must_use]
    pub fn mejor(&self) -> Option<&str> {
        self.large
            .as_deref()
            .or(self.medium.as_deref())
            .or(self.small.as_deref())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Album {
    pub id: AlbumId,
    pub title: String,
    pub artists: Vec<ArtistRef>,
    pub album_type: AlbumType,
    pub release_date: Option<NaiveDate>,
    pub total_tracks: Option<u16>,
    pub cover_url: Option<String>,
    pub covers: CoverSet,
    pub label: Option<String>,
}

/// Fila de rejilla o de lista de álbumes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AlbumRow {
    pub id: AlbumId,
    pub title: String,
    pub artist_display: String,
    pub year: Option<i32>,
    pub cover: Option<String>,
    pub track_count: u16,
    /// Cuántas de sus pistas están descargadas. Permite mostrar "12 de 14" sin
    /// consultar pista a pista.
    pub local_count: u16,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AlbumDetail {
    pub album: Album,
    /// Completo y ordenado por disco y número de pista. Un álbum rara vez pasa
    /// de 50 pistas, así que no se pagina.
    pub tracks: Vec<TrackRow>,
    pub total_duration: DurationMs,
    pub local_count: u16,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AlbumFilter {
    pub artist_id: Option<super::ids::ArtistId>,
    pub local_only: bool,
    pub text: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cover_set_degrada_al_mejor_disponible() {
        let solo_pequenya = CoverSet {
            small: Some("s.jpg".into()),
            ..CoverSet::default()
        };
        assert_eq!(solo_pequenya.mejor(), Some("s.jpg"));
        assert!(!solo_pequenya.esta_vacio());
        assert!(CoverSet::default().esta_vacio());
        assert_eq!(CoverSet::default().mejor(), None);
    }

    #[test]
    fn el_tipo_de_album_tolera_valores_desconocidos() {
        assert_eq!(AlbumType::from_str_lax("SINGLE"), AlbumType::Single);
        assert_eq!(AlbumType::from_str_lax("ep"), AlbumType::Album);
    }
}

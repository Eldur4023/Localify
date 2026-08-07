//! DTOs del catálogo: pistas, álbumes y artistas.

use localify_core::domain::album::{AlbumDetail, AlbumRow};
use localify_core::domain::artist::{ArtistDetail, ArtistRow};
use localify_core::domain::track::{ArtistRef, Track, TrackRow};
use serde::Serialize;
use ts_rs::TS;

use super::common::AvailabilityDto;

/// Referencia ligera a un artista.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "types.gen.ts")]
#[serde(rename_all = "camelCase")]
pub struct ArtistRefDto {
    pub id: String,
    pub name: String,
}

impl From<ArtistRef> for ArtistRefDto {
    fn from(a: ArtistRef) -> Self {
        Self {
            id: a.id.into_string(),
            name: a.name,
        }
    }
}

/// Fila de lista.
///
/// Plana y estrecha a propósito: es lo que se serializa 50 000 veces al
/// recorrer la biblioteca, y cada campo de más se paga en cada scroll.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "types.gen.ts")]
#[serde(rename_all = "camelCase")]
pub struct TrackRowDto {
    pub id: String,
    pub title: String,
    pub artist_display: String,
    pub album_id: Option<String>,
    pub album_title: Option<String>,
    pub duration_ms: u32,
    pub availability: AvailabilityDto,
    pub is_favorite: bool,
    pub explicit: bool,
    /// Segundos desde época en que la fila entró en **esta** lista, o `null` si
    /// la lista no fecha sus filas. Ver [`TrackRow::added_at`].
    pub added_at: Option<i64>,
}

impl From<TrackRow> for TrackRowDto {
    fn from(t: TrackRow) -> Self {
        Self {
            id: t.id.into_string(),
            title: t.title,
            artist_display: t.artist_display,
            album_id: t
                .album_id
                .map(localify_core::domain::ids::AlbumId::into_string),
            album_title: t.album_title,
            duration_ms: t.duration.as_ms(),
            availability: t.availability.into(),
            is_favorite: t.is_favorite,
            explicit: t.explicit,
            added_at: t.added_at.map(|f| f.timestamp()),
        }
    }
}

/// Detalle de una pista. Solo se pide al abrir una vista concreta.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "types.gen.ts")]
#[serde(rename_all = "camelCase")]
pub struct TrackDetailDto {
    #[serde(flatten)]
    pub row: TrackRowDto,
    pub artists: Vec<ArtistRefDto>,
    pub track_number: Option<u16>,
    pub disc_number: Option<u16>,
    pub isrc: Option<String>,
    pub release_date: Option<String>,
    pub added_at: i64,
    pub play_count: u32,
    pub last_played_at: Option<i64>,
}

impl TrackDetailDto {
    /// Compone el detalle a partir del agregado y de los datos que solo conoce
    /// la capa de aplicación (recuento y última escucha).
    #[must_use]
    pub fn nuevo(
        track: Track,
        row: TrackRow,
        play_count: u32,
        last_played_at: Option<i64>,
    ) -> Self {
        Self {
            row: row.into(),
            artists: track.artists.into_iter().map(Into::into).collect(),
            track_number: track.track_number,
            disc_number: track.disc_number,
            isrc: track.isrc,
            release_date: track.release_date.map(|d| d.format("%Y-%m-%d").to_string()),
            added_at: track.added_at.timestamp(),
            play_count,
            last_played_at,
        }
    }
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "types.gen.ts")]
#[serde(rename_all = "camelCase")]
pub struct AlbumRowDto {
    pub id: String,
    pub title: String,
    pub artist_display: String,
    pub year: Option<i32>,
    /// Identificador de la portada cacheada, o `null` si aún no lo está. El
    /// frontend lo resuelve contra el protocolo `asset:`; la ruta en disco no
    /// cruza el puente.
    pub cover: Option<String>,
    pub track_count: u16,
    pub local_count: u16,
}

impl From<AlbumRow> for AlbumRowDto {
    fn from(a: AlbumRow) -> Self {
        Self {
            id: a.id.into_string(),
            title: a.title,
            artist_display: a.artist_display,
            year: a.year,
            cover: a.cover,
            track_count: a.track_count,
            local_count: a.local_count,
        }
    }
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "types.gen.ts")]
#[serde(rename_all = "camelCase")]
pub struct AlbumDetailDto {
    pub id: String,
    pub title: String,
    pub artists: Vec<ArtistRefDto>,
    pub album_type: String,
    pub release_date: Option<String>,
    pub cover: Option<String>,
    pub label: Option<String>,
    pub total_duration_ms: u32,
    pub local_count: u16,
    /// Completo: un álbum rara vez pasa de 50 pistas y no se pagina.
    pub tracks: Vec<TrackRowDto>,
}

impl From<AlbumDetail> for AlbumDetailDto {
    fn from(d: AlbumDetail) -> Self {
        Self {
            id: d.album.id.clone().into_string(),
            title: d.album.title,
            artists: d.album.artists.into_iter().map(Into::into).collect(),
            album_type: d.album.album_type.as_str().to_owned(),
            release_date: d
                .album
                .release_date
                .map(|x| x.format("%Y-%m-%d").to_string()),
            cover: d.album.covers.mejor().map(str::to_owned),
            label: d.album.label,
            total_duration_ms: d.total_duration.as_ms(),
            local_count: d.local_count,
            tracks: d.tracks.into_iter().map(Into::into).collect(),
        }
    }
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "types.gen.ts")]
#[serde(rename_all = "camelCase")]
pub struct ArtistRowDto {
    pub id: String,
    pub name: String,
    pub image_url: Option<String>,
    pub track_count: u32,
    pub local_track_count: u32,
}

impl From<ArtistRow> for ArtistRowDto {
    fn from(a: ArtistRow) -> Self {
        Self {
            id: a.id.into_string(),
            name: a.name,
            image_url: a.image_url,
            track_count: a.track_count,
            local_track_count: a.local_track_count,
        }
    }
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "types.gen.ts")]
#[serde(rename_all = "camelCase")]
pub struct ArtistDetailDto {
    pub id: String,
    pub name: String,
    pub image_url: Option<String>,
    pub genres: Vec<String>,
    pub local_track_count: u32,
    pub top_tracks: Vec<TrackRowDto>,
    pub albums: Vec<AlbumRowDto>,
}

impl From<ArtistDetail> for ArtistDetailDto {
    fn from(d: ArtistDetail) -> Self {
        Self {
            id: d.artist.id.into_string(),
            name: d.artist.name,
            image_url: d.artist.image_url,
            genres: d.artist.genres,
            local_track_count: d.local_track_count,
            top_tracks: d.top_tracks.into_iter().map(Into::into).collect(),
            albums: d.albums.into_iter().map(Into::into).collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use localify_core::domain::audio::DurationMs;
    use localify_core::domain::availability::Availability;
    use localify_core::domain::ids::{AlbumId, TrackId};

    use super::*;

    fn fila() -> TrackRow {
        TrackRow {
            id: TrackId::from_trusted("3z8h0TU7ReDPLIbEnYhWZb"),
            title: "Under Pressure".into(),
            artist_display: "Queen, David Bowie".into(),
            album_id: Some(AlbumId::from_trusted("1GbtB4zTqAsyfZEsm1RZfx")),
            album_title: Some("Hot Space".into()),
            duration: DurationMs::new(248_000),
            availability: Availability::Absent,
            is_favorite: true,
            explicit: false,
            popularity: Some(80),
            added_at: None,
        }
    }

    #[test]
    fn la_fila_se_serializa_en_camel_case() {
        let dto: TrackRowDto = fila().into();
        let json = serde_json::to_value(&dto).expect("serializa");

        assert_eq!(json["artistDisplay"], "Queen, David Bowie");
        assert_eq!(json["durationMs"], 248_000);
        assert_eq!(json["isFavorite"], true);
        assert!(
            json.get("artist_display").is_none(),
            "no debe haber snake_case"
        );
    }

    #[test]
    fn los_campos_opcionales_viajan_como_null_y_no_se_omiten() {
        let mut f = fila();
        f.album_id = None;
        f.album_title = None;
        let dto: TrackRowDto = f.into();
        let json = serde_json::to_value(&dto).expect("serializa");

        // El contrato dice que ningún campo declarado se omite: el cliente
        // puede distinguir "no hay álbum" de "el backend no lo mandó".
        assert!(json.get("albumId").is_some());
        assert_eq!(json["albumId"], serde_json::Value::Null);
        assert_eq!(json["albumTitle"], serde_json::Value::Null);
    }

    #[test]
    fn el_detalle_aplana_la_fila() {
        let track = Track {
            id: TrackId::from_trusted("3z8h0TU7ReDPLIbEnYhWZb"),
            title: "Under Pressure".into(),
            album: None,
            artists: vec![ArtistRef {
                id: localify_core::domain::ids::ArtistId::from_trusted("1dfeR4HaWDbWqFHLkxsg1d"),
                name: "Queen".into(),
            }],
            duration: DurationMs::new(248_000),
            track_number: Some(11),
            disc_number: Some(1),
            explicit: false,
            isrc: Some("GBUM71029604".into()),
            release_date: chrono::NaiveDate::from_ymd_opt(1982, 5, 21),
            popularity: Some(80),
            added_at: chrono::Utc::now(),
        };

        let dto = TrackDetailDto::nuevo(track, fila(), 7, Some(1_700_000_000));
        let json = serde_json::to_value(&dto).expect("serializa");

        // `flatten` debe dejar los campos de la fila al mismo nivel, no
        // anidados bajo "row".
        assert_eq!(json["title"], "Under Pressure");
        assert!(json.get("row").is_none());
        assert_eq!(json["playCount"], 7);
        assert_eq!(json["releaseDate"], "1982-05-21");
        assert_eq!(json["artists"][0]["name"], "Queen");
    }
}

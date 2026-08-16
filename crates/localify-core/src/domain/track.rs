//! Pista: la entidad central del dominio.

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};

use super::audio::DurationMs;
use super::availability::Availability;
use super::ids::{AlbumId, ArtistId, TrackId};

/// Referencia ligera a un artista. Se usa dentro de otras entidades para no
/// arrastrar el agregado completo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtistRef {
    pub id: ArtistId,
    pub name: String,
}

/// Referencia ligera a un álbum.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AlbumRef {
    pub id: AlbumId,
    pub title: String,
}

/// Pista completa, con sus metadatos de Spotify.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Track {
    pub id: TrackId,
    pub title: String,
    pub album: Option<AlbumRef>,
    /// Ordenados por relevancia; el índice 0 es el artista principal.
    pub artists: Vec<ArtistRef>,
    /// Duración **según Spotify**. Es la autoridad para validar coincidencias
    /// en YouTube, y puede diferir unos milisegundos de la del fichero
    /// descargado (que se guarda aparte, en `audio_files`).
    pub duration: DurationMs,
    pub track_number: Option<u16>,
    pub disc_number: Option<u16>,
    pub explicit: bool,
    /// Código ISRC. Cuando existe, es la señal más fiable para localizar la
    /// grabación exacta en YouTube.
    pub isrc: Option<String>,
    pub release_date: Option<NaiveDate>,
    /// Popularidad de Spotify, 0-100.
    pub popularity: Option<u8>,
    pub added_at: DateTime<Utc>,
}

impl Track {
    #[must_use]
    pub fn artista_principal(&self) -> Option<&ArtistRef> {
        self.artists.first()
    }

    /// Cadena de artistas para mostrar: `"Queen, David Bowie"`.
    ///
    /// Se persiste denormalizada en `tracks.artist_display` (ADR-011) para que
    /// listar pistas no requiera un `JOIN` con agregación por fila.
    #[must_use]
    pub fn artist_display(&self) -> String {
        self.artists
            .iter()
            .map(|a| a.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// Fila de lista: proyección plana y estrecha de una pista.
///
/// **No es un `Track` recortado, es un tipo distinto con otro propósito.** Se
/// resuelve con una única consulta sin `JOIN` por fila, y es lo que permite
/// listas de 50 000 elementos a 60 fps. Cargar el agregado completo para pintar
/// una fila sería el error de rendimiento más caro del proyecto.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackRow {
    pub id: TrackId,
    pub title: String,
    pub artist_display: String,
    /// El artista principal, para poder ir a su ficha.
    ///
    /// Solo el primero, no la lista entera: `artist_display` es texto —los
    /// nombres unidos por comas, ADR-011— y arrastrar aquí un vector por fila
    /// costaría una consulta más en cada una de las cincuenta mil, que es
    /// exactamente lo que este tipo existe para evitar. Con el principal basta
    /// para lo que el menú ofrece, que es «ir al artista», y es el que la
    /// posición 0 de `track_artists` marca como tal.
    ///
    /// `None` en una pista sin artistas acreditados.
    pub artist_id: Option<ArtistId>,
    pub album_id: Option<AlbumId>,
    pub album_title: Option<String>,
    pub duration: DurationMs,
    pub availability: Availability,
    pub is_favorite: bool,
    pub explicit: bool,
    /// Popularidad relativa, 0-100, o `None` si el catálogo no la da.
    ///
    /// Viaja en la fila —y no solo en [`Track`]— porque es lo único con lo que
    /// se puede decidir **cuál de seis grabaciones que se llaman igual** es la
    /// que alguien busca. Sin ella, esa elección era el orden de llegada.
    ///
    /// `None` es "no se sabe", no "cero": MusicBrainz no mide popularidad, y
    /// tratar su silencio como impopularidad hundiría su catálogo entero.
    pub popularity: Option<u8>,
    /// Cuándo entró la canción en la lista que la trae.
    ///
    /// Es **de la fila, no de la pista**: en una playlist es cuándo se añadió a
    /// esa playlist, en "Tus me gusta" cuándo se marcó como favorita y en la
    /// biblioteca cuándo llegó. La misma canción tiene tres fechas distintas y
    /// cada lista debe enseñar la suya.
    ///
    /// `None` donde la pregunta no tiene sentido —los resultados de una
    /// búsqueda, las pistas de un álbum— y entonces la columna no se pinta.
    pub added_at: Option<DateTime<Utc>>,
}

/// Criterios de filtrado de la biblioteca.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackFilter {
    pub favorites_only: bool,
    /// Restringe a pistas con fichero en disco. La vista Biblioteca lo activa;
    /// la de resultados de búsqueda no.
    pub local_only: bool,
    pub album_id: Option<AlbumId>,
    pub artist_id: Option<ArtistId>,
    pub genre_id: Option<i64>,
    /// Filtro de texto sobre la biblioteca ya cargada, distinto de la búsqueda
    /// global (que además consulta a Spotify).
    pub text: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TrackSort {
    #[default]
    AddedDesc,
    TitleAsc,
    ArtistAsc,
    AlbumAsc,
    DurationAsc,
    PlayCountDesc,
    LastPlayedDesc,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn artista(nombre: &str) -> ArtistRef {
        ArtistRef {
            id: ArtistId::nuevo_local(),
            name: nombre.into(),
        }
    }

    fn pista(artistas: Vec<ArtistRef>) -> Track {
        Track {
            id: TrackId::nuevo_local(),
            title: "Under Pressure".into(),
            album: None,
            artists: artistas,
            duration: DurationMs::new(248_000),
            track_number: Some(1),
            disc_number: Some(1),
            explicit: false,
            isrc: None,
            release_date: None,
            popularity: None,
            added_at: Utc::now(),
        }
    }

    #[test]
    fn artist_display_une_con_coma_en_orden() {
        let t = pista(vec![artista("Queen"), artista("David Bowie")]);
        assert_eq!(t.artist_display(), "Queen, David Bowie");
        assert_eq!(
            t.artista_principal().map(|a| a.name.as_str()),
            Some("Queen")
        );
    }

    #[test]
    fn una_pista_sin_artistas_no_revienta() {
        let t = pista(vec![]);
        assert_eq!(t.artist_display(), "");
        assert!(t.artista_principal().is_none());
    }
}

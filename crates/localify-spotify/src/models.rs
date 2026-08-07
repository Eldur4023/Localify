//! Modelos crudos de la Web API de Spotify.
//!
//! Copian la forma de la respuesta, no la del dominio. Se traducen en
//! [`crate::mapper`]. Mantenerlos separados es lo que permite que un cambio en
//! la API de Spotify se absorba en un solo sitio.
//!
//! Todos los campos que Spotify documenta como opcionales, o que en la práctica
//! faltan en algunas respuestas, van como `Option`. Ser estricto aquí no
//! aportaría nada: convertiría un campo ausente en un fallo de toda la búsqueda.

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct TokenRespuesta {
    pub access_token: String,
    pub token_type: String,
    pub expires_in: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ErrorRespuesta {
    pub error: ErrorDetalle,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ErrorDetalle {
    pub status: Option<u16>,
    pub message: Option<String>,
}

/// Página de resultados.
#[derive(Debug, Clone, Deserialize)]
pub struct Paginado<T> {
    pub items: Vec<T>,
    pub total: Option<u32>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub next: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BusquedaRespuesta {
    pub tracks: Option<Paginado<PistaCruda>>,
    pub albums: Option<Paginado<AlbumSimple>>,
    pub artists: Option<Paginado<ArtistaCrudo>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PistasRespuesta {
    /// Puede contener nulos: Spotify devuelve `null` por cada id inexistente,
    /// conservando la posición.
    pub tracks: Vec<Option<PistaCruda>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ArtistasRespuesta {
    pub artists: Vec<Option<ArtistaCrudo>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TopTracksRespuesta {
    pub tracks: Vec<PistaCruda>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ImagenCruda {
    pub url: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ArtistaSimple {
    pub id: Option<String>,
    pub name: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ArtistaCrudo {
    pub id: Option<String>,
    pub name: String,
    #[serde(default)]
    pub genres: Vec<String>,
    #[serde(default)]
    pub images: Vec<ImagenCruda>,
    pub popularity: Option<u8>,
    pub followers: Option<Seguidores>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Seguidores {
    pub total: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AlbumSimple {
    pub id: Option<String>,
    pub name: String,
    pub album_type: Option<String>,
    pub release_date: Option<String>,
    /// `year`, `month` o `day`. Determina cómo interpretar `release_date`.
    pub release_date_precision: Option<String>,
    pub total_tracks: Option<u16>,
    #[serde(default)]
    pub images: Vec<ImagenCruda>,
    #[serde(default)]
    pub artists: Vec<ArtistaSimple>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AlbumCompleto {
    #[serde(flatten)]
    pub simple: AlbumSimple,
    pub label: Option<String>,
    pub tracks: Option<Paginado<PistaSimple>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct IdsExternos {
    pub isrc: Option<String>,
}

/// Pista dentro de un álbum: no repite el álbum al que pertenece.
#[derive(Debug, Clone, Deserialize)]
pub struct PistaSimple {
    pub id: Option<String>,
    pub name: String,
    pub duration_ms: u32,
    pub track_number: Option<u16>,
    pub disc_number: Option<u16>,
    #[serde(default)]
    pub explicit: bool,
    #[serde(default)]
    pub artists: Vec<ArtistaSimple>,
    pub external_ids: Option<IdsExternos>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PistaCruda {
    pub id: Option<String>,
    pub name: String,
    pub duration_ms: u32,
    pub track_number: Option<u16>,
    pub disc_number: Option<u16>,
    #[serde(default)]
    pub explicit: bool,
    pub popularity: Option<u8>,
    pub album: Option<AlbumSimple>,
    #[serde(default)]
    pub artists: Vec<ArtistaSimple>,
    pub external_ids: Option<IdsExternos>,
    /// `true` cuando la pista no está disponible en ningún mercado. Localify no
    /// reproduce desde Spotify, así que no se descarta por esto: los metadatos
    /// siguen sirviendo para localizar el audio.
    #[serde(default)]
    pub is_local: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PlaylistCruda {
    pub id: Option<String>,
    pub name: String,
    pub description: Option<String>,
    #[serde(default)]
    pub images: Vec<ImagenCruda>,
    pub tracks: Option<Paginado<EntradaPlaylist>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EntradaPlaylist {
    /// `null` cuando la pista fue retirada del catálogo o es un fichero local
    /// del usuario. Hay que saltarla, no fallar.
    pub track: Option<PistaCruda>,
}

/// Selecciona la imagen más grande. Spotify las devuelve de mayor a menor, pero
/// documentarlo no es lo mismo que garantizarlo.
#[must_use]
pub fn imagen_mayor(imagenes: &[ImagenCruda]) -> Option<&str> {
    imagenes
        .iter()
        .max_by_key(|i| i.width.unwrap_or(0).max(i.height.unwrap_or(0)))
        .map(|i| i.url.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn la_imagen_mayor_no_depende_del_orden() {
        let imagenes = vec![
            ImagenCruda {
                url: "peq.jpg".into(),
                width: Some(64),
                height: Some(64),
            },
            ImagenCruda {
                url: "gra.jpg".into(),
                width: Some(640),
                height: Some(640),
            },
            ImagenCruda {
                url: "med.jpg".into(),
                width: Some(300),
                height: Some(300),
            },
        ];
        assert_eq!(imagen_mayor(&imagenes), Some("gra.jpg"));
    }

    #[test]
    fn sin_imagenes_no_hay_portada() {
        assert_eq!(imagen_mayor(&[]), None);
    }

    #[test]
    fn una_pista_con_campos_minimos_se_deserializa() {
        // Spotify omite campos con frecuencia. Exigirlos convertiría una
        // respuesta incompleta en un fallo de toda la búsqueda.
        let json = r#"{
            "id": "3z8h0TU7ReDPLIbEnYhWZb",
            "name": "Under Pressure",
            "duration_ms": 248000
        }"#;
        let pista: PistaCruda = serde_json::from_str(json).expect("deserializa");
        assert_eq!(pista.name, "Under Pressure");
        assert!(pista.artists.is_empty());
        assert!(pista.album.is_none());
        assert!(!pista.explicit);
    }

    #[test]
    fn una_respuesta_con_pistas_nulas_conserva_las_posiciones() {
        let json = r#"{"tracks":[null,{"id":"abc","name":"X","duration_ms":1000},null]}"#;
        let r: PistasRespuesta = serde_json::from_str(json).expect("deserializa");
        assert_eq!(r.tracks.len(), 3);
        assert!(r.tracks[0].is_none());
        assert!(r.tracks[1].is_some());
    }

    #[test]
    fn una_entrada_de_playlist_sin_pista_se_deserializa() {
        // Ocurre con pistas retiradas del catálogo o ficheros locales.
        let json = r#"{"track": null}"#;
        let e: EntradaPlaylist = serde_json::from_str(json).expect("deserializa");
        assert!(e.track.is_none());
    }

    #[test]
    fn el_album_completo_aplana_los_campos_del_simple() {
        let json = r#"{
            "id": "1GbtB4zTqAsyfZEsm1RZfx",
            "name": "Hot Space",
            "album_type": "album",
            "release_date": "1982-05-21",
            "release_date_precision": "day",
            "label": "EMI"
        }"#;
        let al: AlbumCompleto = serde_json::from_str(json).expect("deserializa");
        assert_eq!(al.simple.name, "Hot Space");
        assert_eq!(al.label.as_deref(), Some("EMI"));
        assert_eq!(al.simple.release_date_precision.as_deref(), Some("day"));
    }
}

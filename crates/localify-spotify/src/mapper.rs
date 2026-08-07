//! Traducción de las respuestas de Spotify a entidades del dominio.
//!
//! Es lógica pura y sin I/O, así que se prueba entera con fixtures. Toda la
//! tolerancia a respuestas incompletas vive aquí: una pista sin álbum, un
//! artista sin identificador o una fecha con precisión de año no deben tumbar
//! una búsqueda.

use chrono::NaiveDate;
use localify_core::domain::album::{Album, AlbumType, CoverSet};
use localify_core::domain::artist::Artist;
use localify_core::domain::audio::DurationMs;
use localify_core::domain::ids::{AlbumId, ArtistId, TrackId};
use localify_core::domain::track::{AlbumRef, ArtistRef, Track};

use crate::models::{
    AlbumCompleto, AlbumSimple, ArtistaCrudo, ArtistaSimple, PistaCruda, PistaSimple, imagen_mayor,
};

/// Interpreta la fecha de lanzamiento según su precisión declarada.
///
/// Spotify devuelve `"1982"`, `"1982-05"` o `"1982-05-21"` y un campo aparte
/// que dice cuál es. Fiarse solo de la longitud funcionaría casi siempre, pero
/// el campo existe precisamente para no tener que adivinar.
#[must_use]
pub fn fecha_lanzamiento(fecha: Option<&str>, precision: Option<&str>) -> Option<NaiveDate> {
    let texto = fecha?;
    let completa = match precision {
        Some("year") => format!("{texto}-01-01"),
        Some("month") => format!("{texto}-01"),
        Some("day") => texto.to_owned(),
        // Sin precisión declarada, se deduce de la forma.
        _ => match texto.len() {
            4 => format!("{texto}-01-01"),
            7 => format!("{texto}-01"),
            _ => texto.to_owned(),
        },
    };
    NaiveDate::parse_from_str(&completa, "%Y-%m-%d").ok()
}

/// Convierte una lista de artistas simples, descartando los que no traen id.
///
/// Un artista sin identificador no se puede referenciar ni volver a consultar:
/// conservarlo generaría entradas huérfanas en el catálogo.
#[must_use]
pub fn artistas(crudos: &[ArtistaSimple]) -> Vec<ArtistRef> {
    crudos
        .iter()
        .filter_map(|a| {
            a.id.as_ref().map(|id| ArtistRef {
                id: ArtistId::from_trusted(id.clone()),
                name: a.name.clone(),
            })
        })
        .collect()
}

/// Referencia al álbum de una pista.
#[must_use]
pub fn album_ref(album: Option<&AlbumSimple>) -> Option<AlbumRef> {
    let al = album?;
    al.id.as_ref().map(|id| AlbumRef {
        id: AlbumId::from_trusted(id.clone()),
        title: al.name.clone(),
    })
}

/// Convierte una pista completa.
///
/// Devuelve `None` si no trae identificador: sin él no hay clave de dominio.
#[must_use]
pub fn pista(cruda: &PistaCruda) -> Option<Track> {
    let id = cruda.id.as_ref()?;

    Some(Track {
        id: TrackId::from_trusted(id.clone()),
        title: cruda.name.clone(),
        album: album_ref(cruda.album.as_ref()),
        artists: artistas(&cruda.artists),
        duration: DurationMs::new(cruda.duration_ms),
        track_number: cruda.track_number,
        disc_number: cruda.disc_number,
        explicit: cruda.explicit,
        isrc: cruda.external_ids.as_ref().and_then(|e| e.isrc.clone()),
        release_date: cruda.album.as_ref().and_then(|a| {
            fecha_lanzamiento(
                a.release_date.as_deref(),
                a.release_date_precision.as_deref(),
            )
        }),
        popularity: cruda.popularity,
        added_at: chrono::Utc::now(),
    })
}

/// Convierte una pista de dentro de un álbum, que no repite el álbum.
#[must_use]
pub fn pista_de_album(cruda: &PistaSimple, album: &AlbumSimple) -> Option<Track> {
    let id = cruda.id.as_ref()?;

    Some(Track {
        id: TrackId::from_trusted(id.clone()),
        title: cruda.name.clone(),
        album: album_ref(Some(album)),
        artists: artistas(&cruda.artists),
        duration: DurationMs::new(cruda.duration_ms),
        track_number: cruda.track_number,
        disc_number: cruda.disc_number,
        explicit: cruda.explicit,
        isrc: cruda.external_ids.as_ref().and_then(|e| e.isrc.clone()),
        release_date: fecha_lanzamiento(
            album.release_date.as_deref(),
            album.release_date_precision.as_deref(),
        ),
        popularity: None,
        added_at: chrono::Utc::now(),
    })
}

#[must_use]
pub fn album(simple: &AlbumSimple) -> Option<Album> {
    let id = simple.id.as_ref()?;

    Some(Album {
        id: AlbumId::from_trusted(id.clone()),
        title: simple.name.clone(),
        artists: artistas(&simple.artists),
        album_type: simple
            .album_type
            .as_deref()
            .map_or(AlbumType::Album, AlbumType::from_str_lax),
        release_date: fecha_lanzamiento(
            simple.release_date.as_deref(),
            simple.release_date_precision.as_deref(),
        ),
        total_tracks: simple.total_tracks,
        cover_url: imagen_mayor(&simple.images).map(str::to_owned),
        covers: CoverSet::default(),
        label: None,
    })
}

#[must_use]
pub fn album_completo(completo: &AlbumCompleto) -> Option<Album> {
    let mut al = album(&completo.simple)?;
    al.label.clone_from(&completo.label);
    Some(al)
}

#[must_use]
pub fn artista(crudo: &ArtistaCrudo) -> Option<Artist> {
    let id = crudo.id.as_ref()?;

    Some(Artist {
        id: ArtistId::from_trusted(id.clone()),
        name: crudo.name.clone(),
        image_url: imagen_mayor(&crudo.images).map(str::to_owned),
        genres: crudo.genres.clone(),
        popularity: crudo.popularity,
        followers: crudo.followers.as_ref().and_then(|f| f.total),
    })
}

/// Convierte una lista de pistas, descartando en silencio las que no sirven.
///
/// Descartar es lo correcto: una búsqueda con veinte resultados de los que uno
/// viene incompleto debe mostrar diecinueve, no fallar.
#[must_use]
pub fn pistas(crudas: &[PistaCruda]) -> Vec<Track> {
    crudas.iter().filter_map(pista).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{IdsExternos, ImagenCruda, Seguidores};

    fn artista_simple(id: Option<&str>, nombre: &str) -> ArtistaSimple {
        ArtistaSimple {
            id: id.map(str::to_owned),
            name: nombre.to_owned(),
        }
    }

    fn album_simple() -> AlbumSimple {
        AlbumSimple {
            id: Some("1GbtB4zTqAsyfZEsm1RZfx".into()),
            name: "Hot Space".into(),
            album_type: Some("album".into()),
            release_date: Some("1982-05-21".into()),
            release_date_precision: Some("day".into()),
            total_tracks: Some(11),
            images: vec![ImagenCruda {
                url: "https://i.scdn.co/grande.jpg".into(),
                width: Some(640),
                height: Some(640),
            }],
            artists: vec![artista_simple(Some("1dfeR4HaWDbWqFHLkxsg1d"), "Queen")],
        }
    }

    fn pista_cruda() -> PistaCruda {
        PistaCruda {
            id: Some("3z8h0TU7ReDPLIbEnYhWZb".into()),
            name: "Under Pressure".into(),
            duration_ms: 248_000,
            track_number: Some(11),
            disc_number: Some(1),
            explicit: false,
            popularity: Some(80),
            album: Some(album_simple()),
            artists: vec![
                artista_simple(Some("1dfeR4HaWDbWqFHLkxsg1d"), "Queen"),
                artista_simple(Some("0oSGxfWSnnOXhD2fKuz2Gy"), "David Bowie"),
            ],
            external_ids: Some(IdsExternos {
                isrc: Some("GBUM71029604".into()),
            }),
            is_local: false,
        }
    }

    #[test]
    fn una_pista_completa_se_traduce_entera() {
        let t = pista(&pista_cruda()).expect("tiene id");

        assert_eq!(t.title, "Under Pressure");
        assert_eq!(t.duration, DurationMs::new(248_000));
        assert_eq!(t.isrc.as_deref(), Some("GBUM71029604"));
        assert_eq!(t.track_number, Some(11));
        assert_eq!(t.artist_display(), "Queen, David Bowie");
        assert_eq!(t.album.map(|a| a.title), Some("Hot Space".into()));
        assert_eq!(t.release_date, NaiveDate::from_ymd_opt(1982, 5, 21));
    }

    #[test]
    fn una_pista_sin_id_se_descarta() {
        // Sin id no hay clave de dominio: el id de Spotify ES el identificador.
        let mut cruda = pista_cruda();
        cruda.id = None;
        assert!(pista(&cruda).is_none());
    }

    #[test]
    fn una_lista_con_pistas_rotas_conserva_las_buenas() {
        let mut rota = pista_cruda();
        rota.id = None;
        let lista = vec![pista_cruda(), rota, pista_cruda()];

        assert_eq!(
            pistas(&lista).len(),
            2,
            "una respuesta parcialmente rota debe seguir siendo útil"
        );
    }

    #[test]
    fn los_artistas_sin_id_no_generan_entradas_huerfanas() {
        let mut cruda = pista_cruda();
        cruda.artists = vec![
            artista_simple(Some("1dfeR4HaWDbWqFHLkxsg1d"), "Queen"),
            artista_simple(None, "Artista Anónimo"),
        ];

        let t = pista(&cruda).expect("tiene id");
        assert_eq!(t.artists.len(), 1);
        assert_eq!(t.artist_display(), "Queen");
    }

    #[test]
    fn una_pista_sin_album_se_traduce_igual() {
        let mut cruda = pista_cruda();
        cruda.album = None;
        let t = pista(&cruda).expect("tiene id");
        assert!(t.album.is_none());
        assert!(t.release_date.is_none());
    }

    #[test]
    fn las_fechas_respetan_la_precision_declarada() {
        assert_eq!(
            fecha_lanzamiento(Some("1982"), Some("year")),
            NaiveDate::from_ymd_opt(1982, 1, 1)
        );
        assert_eq!(
            fecha_lanzamiento(Some("1982-05"), Some("month")),
            NaiveDate::from_ymd_opt(1982, 5, 1)
        );
        assert_eq!(
            fecha_lanzamiento(Some("1982-05-21"), Some("day")),
            NaiveDate::from_ymd_opt(1982, 5, 21)
        );
    }

    #[test]
    fn sin_precision_la_fecha_se_deduce_de_su_forma() {
        assert_eq!(
            fecha_lanzamiento(Some("1982"), None),
            NaiveDate::from_ymd_opt(1982, 1, 1)
        );
        assert_eq!(
            fecha_lanzamiento(Some("1982-05-21"), None),
            NaiveDate::from_ymd_opt(1982, 5, 21)
        );
    }

    #[test]
    fn una_fecha_ilegible_no_rompe_la_traduccion() {
        assert_eq!(
            fecha_lanzamiento(Some("no es una fecha"), Some("day")),
            None
        );
        assert_eq!(fecha_lanzamiento(None, Some("day")), None);
    }

    #[test]
    fn el_album_toma_la_portada_de_mayor_tamano() {
        let mut simple = album_simple();
        simple.images = vec![
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
        ];
        let al = album(&simple).expect("tiene id");
        assert_eq!(al.cover_url.as_deref(), Some("gra.jpg"));
        assert_eq!(al.album_type, AlbumType::Album);
        assert_eq!(al.total_tracks, Some(11));
    }

    #[test]
    fn el_album_completo_aporta_el_sello() {
        let completo = AlbumCompleto {
            simple: album_simple(),
            label: Some("EMI".into()),
            tracks: None,
        };
        let al = album_completo(&completo).expect("tiene id");
        assert_eq!(al.label.as_deref(), Some("EMI"));
        assert_eq!(al.title, "Hot Space");
    }

    #[test]
    fn el_artista_conserva_generos_y_seguidores() {
        let crudo = ArtistaCrudo {
            id: Some("1dfeR4HaWDbWqFHLkxsg1d".into()),
            name: "Queen".into(),
            genres: vec!["glam rock".into(), "classic rock".into()],
            images: vec![ImagenCruda {
                url: "https://i.scdn.co/q.jpg".into(),
                width: Some(640),
                height: Some(640),
            }],
            popularity: Some(88),
            followers: Some(Seguidores {
                total: Some(45_000_000),
            }),
        };

        let a = artista(&crudo).expect("tiene id");
        assert_eq!(
            a.genres.len(),
            2,
            "los géneros alimentan las recomendaciones"
        );
        assert_eq!(a.followers, Some(45_000_000));
        assert_eq!(a.image_url.as_deref(), Some("https://i.scdn.co/q.jpg"));
    }

    #[test]
    fn una_pista_de_album_hereda_la_fecha_del_album() {
        let al = album_simple();
        let simple = PistaSimple {
            id: Some("3z8h0TU7ReDPLIbEnYhWZb".into()),
            name: "Under Pressure".into(),
            duration_ms: 248_000,
            track_number: Some(11),
            disc_number: Some(1),
            explicit: false,
            artists: vec![artista_simple(Some("1dfeR4HaWDbWqFHLkxsg1d"), "Queen")],
            external_ids: None,
        };

        let t = pista_de_album(&simple, &al).expect("tiene id");
        assert_eq!(t.release_date, NaiveDate::from_ymd_opt(1982, 5, 21));
        assert_eq!(t.album.map(|a| a.title), Some("Hot Space".into()));
    }
}

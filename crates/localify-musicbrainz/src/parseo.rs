//! Traducción de las respuestas de MusicBrainz al dominio.
//!
//! ## Se declara la forma que se lee, no la que manda el servidor
//!
//! Las respuestas de MusicBrainz son grandes y traen mucho que aquí no se usa.
//! Los `struct` de este módulo describen **solo** los campos que se leen, con
//! `#[serde(default)]` en casi todo: así un campo que MusicBrainz añada o deje
//! de mandar no rompe el parseo entero.
//!
//! ## Una grabación no es una edición
//!
//! Es la diferencia que hay que tener en la cabeza al leer esto. MusicBrainz
//! separa la **grabación** (`recording`: una interpretación concreta) de la
//! **edición** (`release`: un disco que la contiene), y una grabación puede
//! estar en muchas. Nuestro dominio tiene una pista con un álbum opcional, así
//! que al traducir se elige la primera edición: la que MusicBrainz considera
//! más representativa.

use chrono::{NaiveDate, Utc};
use localify_core::domain::album::{Album, AlbumType, CoverSet};
use localify_core::domain::audio::DurationMs;
use localify_core::domain::ids::{AlbumId, ArtistId, TrackId};
use localify_core::domain::track::{AlbumRef, ArtistRef, Track};
use serde::Deserialize;

use crate::cliente::COVER_ART;

#[derive(Debug, Deserialize)]
pub struct BusquedaGrabaciones {
    #[serde(default)]
    pub count: u64,
    #[serde(default)]
    pub recordings: Vec<Grabacion>,
}

#[derive(Debug, Deserialize)]
pub struct Grabacion {
    pub id: String,
    #[serde(default)]
    pub title: String,
    /// Duración en milisegundos. Falta en grabaciones sin datos de tiempo.
    #[serde(default)]
    pub length: Option<u32>,
    #[serde(default, rename = "artist-credit")]
    pub artist_credit: Vec<Credito>,
    #[serde(default)]
    pub releases: Vec<Edicion>,
    #[serde(default)]
    pub isrcs: Vec<String>,
    /// Fecha de la primera edición. Es lo más parecido a "cuándo salió".
    #[serde(default, rename = "first-release-date")]
    pub first_release_date: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Credito {
    #[serde(default)]
    pub name: String,
    pub artist: Option<ArtistaBreve>,
}

#[derive(Debug, Deserialize)]
pub struct ArtistaBreve {
    pub id: String,
    #[serde(default)]
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct Edicion {
    pub id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub date: Option<String>,
    #[serde(default, rename = "artist-credit")]
    pub artist_credit: Vec<Credito>,
    #[serde(default, rename = "release-group")]
    pub release_group: Option<GrupoDeEdicion>,
    #[serde(default)]
    pub media: Vec<Soporte>,
    /// Sellos. Se lee el primero, que es lo que enseña la ficha.
    #[serde(default, rename = "label-info")]
    pub label_info: Vec<InfoDeSello>,
}

#[derive(Debug, Deserialize)]
pub struct GrupoDeEdicion {
    #[serde(default, rename = "primary-type")]
    pub primary_type: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct InfoDeSello {
    pub label: Option<Sello>,
}

#[derive(Debug, Deserialize)]
pub struct Sello {
    #[serde(default)]
    pub name: Option<String>,
}

/// Un disco de una edición: un CD, un vinilo, un fichero.
#[derive(Debug, Deserialize)]
pub struct Soporte {
    #[serde(default)]
    pub position: Option<u16>,
    #[serde(default)]
    pub tracks: Vec<PistaDeSoporte>,
}

#[derive(Debug, Deserialize)]
pub struct PistaDeSoporte {
    #[serde(default)]
    pub position: Option<u16>,
    #[serde(default)]
    pub number: Option<String>,
    pub recording: Option<Grabacion>,
}

#[derive(Debug, Deserialize)]
pub struct Artista {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub tags: Vec<Etiqueta>,
}

/// Etiqueta de la folksonomía de MusicBrainz.
///
/// Es lo más cercano a un género que hay aquí, y no es lo mismo: las pone la
/// gente y su `count` dice cuántos han votado. Se ordenan por ese recuento y se
/// usan como géneros, que es mejor que dejar el campo vacío.
#[derive(Debug, Deserialize)]
pub struct Etiqueta {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub count: i32,
}

#[derive(Debug, Deserialize)]
pub struct BusquedaEdiciones {
    #[serde(default)]
    pub releases: Vec<Edicion>,
}

/// Grabación con sus enlaces externos.
#[derive(Debug, Deserialize)]
pub struct GrabacionConEnlaces {
    #[serde(default)]
    pub relations: Vec<Relacion>,
}

#[derive(Debug, Deserialize)]
pub struct Relacion {
    #[serde(default, rename = "type")]
    pub tipo: String,
    pub url: Option<Enlace>,
}

#[derive(Debug, Deserialize)]
pub struct Enlace {
    #[serde(default)]
    pub resource: String,
}

/// Identificador del vídeo de YouTube entre los enlaces de una grabación.
///
/// ## Se aceptan los dos dominios
///
/// MusicBrainz guarda unas veces `music.youtube.com/watch?v=…` y otras
/// `www.youtube.com/watch?v=…`. Es el mismo vídeo y el mismo identificador;
/// mirar solo uno perdería la mitad de los enlaces.
///
/// ## Y solo `watch?v=`
///
/// Un enlace a un canal o a una lista también es "free streaming" y no sirve:
/// identifica al artista o al disco, no a **esta** grabación. Confundirlos
/// mandaría a descargar cualquier cosa.
#[must_use]
pub fn video_de_youtube(relaciones: &[Relacion]) -> Option<String> {
    relaciones.iter().find_map(|r| {
        let url = r.url.as_ref()?.resource.as_str();
        if !url.contains("youtube.com/watch") {
            return None;
        }
        // El identificador va en `v=`, y detrás puede haber más parámetros.
        let id: String = url
            .split("v=")
            .nth(1)?
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
            .collect();
        (id.len() == 11).then_some(id)
    })
}

/// Fecha parcial de MusicBrainz: `2020`, `2020-05` o `2020-05-17`.
///
/// Se completa hacia el primer día en lugar de descartarla. Saber que algo es
/// de 2020 es útil —la ficha enseña el año— y exigir el día perdería la mayoría
/// de los lanzamientos antiguos, que solo tienen año.
#[must_use]
pub fn fecha(texto: Option<&str>) -> Option<NaiveDate> {
    let t = texto?;
    let completa = match t.len() {
        4 => format!("{t}-01-01"),
        7 => format!("{t}-01"),
        _ => t.to_owned(),
    };
    NaiveDate::parse_from_str(&completa, "%Y-%m-%d").ok()
}

/// Artistas de un crédito.
///
/// El crédito de MusicBrainz alterna artistas con las palabras que los unen
/// —"feat.", " & "—, y esas entradas no traen `artist`. Se descartan: son texto
/// de presentación, no artistas.
///
/// ## Y no se repite un artista
///
/// MusicBrainz acredita al mismo artista dos veces cuando aparece con dos
/// nombres —un alias y el real— o cuando figura como intérprete y como
/// remezclador. Para nosotros es el mismo identificador dos veces, y eso rompe
/// la clave primaria de `track_artists`.
///
/// Esto tumbó la persistencia de **búsquedas enteras**: la escritura es una sola
/// transacción, así que una grabación con el crédito repetido se llevaba por
/// delante las otras treinta y nueve, y en la interfaz parecía que MusicBrainz
/// no respondía. Se queda la primera aparición, que es la que lleva el orden
/// bueno.
#[must_use]
pub fn artistas(credito: &[Credito]) -> Vec<ArtistRef> {
    let mut vistos = std::collections::HashSet::new();
    credito
        .iter()
        .filter_map(|c| {
            let a = c.artist.as_ref()?;
            if !vistos.insert(a.id.clone()) {
                return None;
            }
            Some(ArtistRef {
                id: ArtistId::from_trusted(a.id.clone()),
                // El nombre del crédito manda sobre el del artista: es como
                // aparece **en esta grabación**, que puede ser un alias.
                name: if c.name.is_empty() {
                    a.name.clone()
                } else {
                    c.name.clone()
                },
            })
        })
        .collect()
}

/// URL de la portada de una edición en Cover Art Archive.
///
/// Se construye sin preguntar. El servicio responde 404 si no la hay, y el
/// descargador de imágenes ya trata eso como "no hay portada": comprobarlo antes
/// costaría una petición por álbum para saber lo que la propia descarga dice.
#[must_use]
pub fn portada(edicion: &str) -> String {
    format!("{COVER_ART}/release/{edicion}/front-500")
}

#[must_use]
pub fn a_track(g: Grabacion) -> Track {
    // La primera edición es la que MusicBrainz pone por delante. Con `inc=releases`
    // vienen ordenadas por relevancia, así que no hay que elegir.
    let edicion = g.releases.first();

    Track {
        id: TrackId::from_trusted(g.id),
        title: g.title,
        album: edicion.map(|e| AlbumRef {
            id: AlbumId::from_trusted(e.id.clone()),
            title: e.title.clone(),
        }),
        artists: artistas(&g.artist_credit),
        duration: DurationMs::new(g.length.unwrap_or(0)),
        // MusicBrainz sí sabe el número de pista, pero solo dentro de una
        // edición concreta y no en la respuesta de búsqueda. Se rellena al
        // pedir el álbum, que es donde el dato significa algo.
        track_number: None,
        disc_number: None,
        // No existe el concepto. Dejarlo en `false` es decir "no consta", no
        // "no es explícita": inventar el dato sería peor.
        explicit: false,
        isrc: g.isrcs.into_iter().next(),
        release_date: fecha(
            g.first_release_date
                .as_deref()
                .or_else(|| edicion.and_then(|e| e.date.as_deref())),
        ),
        // MusicBrainz no mide popularidad. Ver la cabecera del proveedor.
        popularity: None,
        added_at: Utc::now(),
    }
}

#[must_use]
pub fn a_album(e: &Edicion) -> Album {
    Album {
        id: AlbumId::from_trusted(e.id.clone()),
        title: e.title.clone(),
        artists: artistas(&e.artist_credit),
        album_type: e
            .release_group
            .as_ref()
            .and_then(|g| g.primary_type.as_deref())
            .map_or(AlbumType::Album, AlbumType::from_str_lax),
        release_date: fecha(e.date.as_deref()),
        total_tracks: u16::try_from(e.media.iter().map(|m| m.tracks.len()).sum::<usize>())
            .ok()
            .filter(|n| *n > 0),
        cover_url: Some(portada(&e.id)),
        covers: CoverSet::default(),
        label: e
            .label_info
            .iter()
            .find_map(|i| i.label.as_ref()?.name.clone()),
    }
}

/// Pistas de una edición, con su número de pista y de disco.
///
/// Es el único sitio donde esos dos números se conocen: en MusicBrainz no son
/// de la grabación, son de su posición **en esta edición**. La misma grabación
/// es la pista 3 de un disco y la 11 de un recopilatorio.
#[must_use]
pub fn pistas_de(e: Edicion) -> Vec<Track> {
    let referencia = AlbumRef {
        id: AlbumId::from_trusted(e.id.clone()),
        title: e.title.clone(),
    };

    let mut salida = Vec::new();
    for (i, soporte) in e.media.into_iter().enumerate() {
        let disco = soporte
            .position
            .or_else(|| u16::try_from(i + 1).ok())
            .unwrap_or(1);

        for pista in soporte.tracks {
            let Some(grabacion) = pista.recording else {
                continue;
            };
            let numero = pista
                .position
                .or_else(|| pista.number.as_deref()?.parse().ok());

            let mut t = a_track(grabacion);
            t.album = Some(referencia.clone());
            t.track_number = numero;
            t.disc_number = Some(disco);
            salida.push(t);
        }
    }
    salida
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn se_reconoce_el_video_en_los_dos_dominios_de_youtube() {
        // MusicBrainz guarda unas veces `music.youtube.com` y otras
        // `www.youtube.com`. Es el mismo vídeo; mirar uno solo perdería la mitad.
        for url in [
            "https://music.youtube.com/watch?v=pvy9km7g6fw",
            "https://www.youtube.com/watch?v=pvy9km7g6fw",
            "https://www.youtube.com/watch?v=pvy9km7g6fw&list=PLabc",
        ] {
            let rels = vec![Relacion {
                tipo: "free streaming".into(),
                url: Some(Enlace {
                    resource: url.into(),
                }),
            }];
            assert_eq!(
                video_de_youtube(&rels),
                Some("pvy9km7g6fw".to_owned()),
                "{url}"
            );
        }
    }

    #[test]
    fn un_enlace_que_no_es_un_video_no_cuenta() {
        // Un canal o una lista también son "free streaming", y apuntan al
        // artista o al disco. Tomarlos por la grabación mandaría a descargar
        // cualquier cosa, y lo descargado no se vuelve a descargar.
        let rels = vec![
            Relacion {
                tipo: "free streaming".into(),
                url: Some(Enlace {
                    resource: "https://open.spotify.com/album/55RULuYZGg7QUBqYQePPR7".into(),
                }),
            },
            Relacion {
                tipo: "free streaming".into(),
                url: Some(Enlace {
                    resource: "https://www.youtube.com/channel/UCLlchLQvkIB_QWxH6J2tLIA".into(),
                }),
            },
        ];
        assert_eq!(video_de_youtube(&rels), None);
    }

    #[test]
    fn las_fechas_parciales_se_completan_hacia_el_primer_dia() {
        // La mayoría de los lanzamientos antiguos solo tienen año. Exigir el día
        // los perdería todos.
        assert_eq!(fecha(Some("2020")), NaiveDate::from_ymd_opt(2020, 1, 1));
        assert_eq!(fecha(Some("2020-05")), NaiveDate::from_ymd_opt(2020, 5, 1));
        assert_eq!(
            fecha(Some("2020-05-17")),
            NaiveDate::from_ymd_opt(2020, 5, 17)
        );
        assert_eq!(fecha(None), None);
        assert_eq!(fecha(Some("ayer")), None);
    }

    #[test]
    fn las_palabras_de_union_del_credito_no_son_artistas() {
        // MusicBrainz mete "feat." y " & " como entradas del crédito sin
        // `artist`. Contarlas daría artistas llamados "feat." en la biblioteca.
        let json = serde_json::json!([
            { "name": "Casey Edwards", "artist": { "id": "aaaaaaaa-1111-2222-3333-444444444444", "name": "Casey Edwards" } },
            { "name": " feat. " },
            { "name": "Victor Borba", "artist": { "id": "bbbbbbbb-1111-2222-3333-444444444444", "name": "Victor Borba" } }
        ]);
        let credito: Vec<Credito> = serde_json::from_value(json).expect("json");

        let artistas = artistas(&credito);
        assert_eq!(artistas.len(), 2);
        assert_eq!(artistas[0].name, "Casey Edwards");
        assert_eq!(artistas[1].name, "Victor Borba");
    }

    #[test]
    fn el_nombre_del_credito_manda_sobre_el_del_artista() {
        // Es como aparece en esa grabación concreta; puede ser un alias.
        let json = serde_json::json!([
            { "name": "Yorushika", "artist": { "id": "aaaaaaaa-1111-2222-3333-444444444444", "name": "ヨルシカ" } }
        ]);
        let credito: Vec<Credito> = serde_json::from_value(json).expect("json");
        assert_eq!(artistas(&credito)[0].name, "Yorushika");
    }

    #[test]
    fn una_grabacion_sin_duracion_no_rompe_el_parseo() {
        // Pasa con grabaciones recién añadidas. Cero significa "no consta", y el
        // emparejador ya sabe que sin duración no puede validar por tiempo.
        let json = serde_json::json!({
            "id": "0578c31a-4ab4-4181-b05d-1a0a62e49bec",
            "title": "Bury the Light"
        });
        let g: Grabacion = serde_json::from_value(json).expect("json");
        let t = a_track(g);
        assert_eq!(t.duration, DurationMs::new(0));
        assert!(t.album.is_none());
        assert!(t.artists.is_empty());
    }

    #[test]
    fn los_numeros_de_pista_salen_de_la_edicion_no_de_la_grabacion() {
        // La misma grabación es la 3 de un disco y la 11 de un recopilatorio:
        // el número pertenece a la edición, y por eso solo se rellena aquí.
        let json = serde_json::json!({
            "id": "cccccccc-1111-2222-3333-444444444444",
            "title": "Disco de prueba",
            "media": [{
                "position": 2,
                "tracks": [
                    { "position": 3, "recording": { "id": "dddddddd-1111-2222-3333-444444444444", "title": "Una" } }
                ]
            }]
        });
        let e: Edicion = serde_json::from_value(json).expect("json");

        let pistas = pistas_de(e);
        assert_eq!(pistas.len(), 1);
        assert_eq!(pistas[0].track_number, Some(3));
        assert_eq!(pistas[0].disc_number, Some(2));
        assert_eq!(
            pistas[0].album.as_ref().map(|a| a.title.as_str()),
            Some("Disco de prueba")
        );
    }
}

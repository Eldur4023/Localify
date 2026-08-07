//! Lectura de una playlist pública **sin credenciales**.
//!
//! ## Por qué existe habiendo una API
//!
//! La Web API de Spotify obliga a registrar una aplicación en su panel de
//! desarrolladores. Es un peaje razonable para quien quiere Spotify como
//! catálogo, y absurdo para quien solo quiere traerse una lista que un amigo le
//! ha pasado por WhatsApp. Este camino no pide nada.
//!
//! ## Qué se lee, y de dónde
//!
//! De la página de incrustación —`open.spotify.com/embed/playlist/<id>`—, que
//! Spotify sirve a cualquiera sin token ni firma. Dentro hay un `<script
//! id="__NEXT_DATA__">` con el estado de la página en JSON: nombre, portada y la
//! lista de canciones con título, artista y duración.
//!
//! No es lo mismo que forzar `/api/token`, que exige una firma derivada de un
//! secreto de su JavaScript y que existe precisamente para dejar fuera a los
//! clientes que no son su navegador. Aquí no se sortea nada: es una página
//! pública leída como lo que es.
//!
//! ## Lo que este camino no da
//!
//! - **La descripción.** No está en la carga del embed —`subtitle` es el dueño
//!   de la lista, no su descripción—, ni en la página, ni en oEmbed. Con
//!   credenciales sí viene.
//! - **Identidad de los artistas.** Llegan como una cadena para mostrar
//!   ("Ariana Grande"), sin identificador. Se crean artistas locales: basta para
//!   buscar la canción en YouTube, que es para lo que se importa.
//! - **Listas muy largas.** Se han visto respuestas de 50 y de 100 canciones;
//!   por encima de cien no está comprobado que las traiga todas.
//!
//! Las tres se resuelven poniendo credenciales, y ninguna impide traerse la
//! lista.

use chrono::Utc;
use localify_core::domain::audio::DurationMs;
use localify_core::domain::ids::{ArtistId, TrackId};
use localify_core::domain::track::{ArtistRef, Track};
use localify_core::error::{CoreError, CoreResult};
use localify_core::ports::metadata_provider::PlaylistImport;
use serde::Deserialize;
use tracing::debug;

use crate::transporte::Transporte;

/// La página de incrustación, que es la que lleva los datos.
const EMBED: &str = "https://open.spotify.com/embed/playlist";

/// Sin esto Spotify devuelve una página distinta, sin `__NEXT_DATA__`.
const AGENTE: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                      (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";

#[derive(Debug, Deserialize)]
struct Pagina {
    props: Props,
}

#[derive(Debug, Deserialize)]
struct Props {
    #[serde(rename = "pageProps")]
    page_props: PageProps,
}

#[derive(Debug, Deserialize)]
struct PageProps {
    state: Estado,
}

#[derive(Debug, Deserialize)]
struct Estado {
    data: Datos,
}

#[derive(Debug, Deserialize)]
struct Datos {
    entity: Entidad,
}

#[derive(Debug, Deserialize)]
struct Entidad {
    #[serde(default)]
    name: String,
    #[serde(default, rename = "coverArt")]
    cover_art: Option<Portada>,
    #[serde(default, rename = "trackList")]
    track_list: Vec<Fila>,
}

#[derive(Debug, Deserialize)]
struct Portada {
    #[serde(default)]
    sources: Vec<Fuente>,
}

#[derive(Debug, Deserialize)]
struct Fuente {
    url: String,
    #[serde(default)]
    width: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct Fila {
    /// `spotify:track:<id>`.
    uri: String,
    #[serde(default)]
    title: String,
    /// Artistas ya unidos para mostrar. No trae identidad.
    #[serde(default)]
    subtitle: String,
    #[serde(default)]
    duration: Option<u32>,
    #[serde(default, rename = "isExplicit")]
    is_explicit: bool,
}

/// Trae una playlist pública leyendo su página de incrustación.
///
/// # Errors
/// Si la página no responde, no trae el JSON esperado o la lista está vacía.
pub async fn leer(
    transporte: &dyn Transporte,
    id: &str,
    page_callback: &(dyn Fn(u32, u32) + Send + Sync),
) -> CoreResult<PlaylistImport> {
    let url = format!("{EMBED}/{id}");
    let respuesta = transporte
        .get_publico(&url, AGENTE)
        .await
        .map_err(|e| CoreError::provider_unavailable("spotify", Box::new(e)))?;

    if !respuesta.es_ok() {
        return Err(CoreError::not_found("spotify_playlist", id));
    }
    let html = String::from_utf8_lossy(&respuesta.cuerpo);

    let json = extraer_next_data(&html)
        .ok_or_else(|| CoreError::invalid("la página de Spotify no trae los datos de la lista"))?;

    let pagina: Pagina = serde_json::from_str(json)
        .map_err(|e| CoreError::invalid(format!("datos de Spotify ilegibles: {e}")))?;
    let entidad = pagina.props.page_props.state.data.entity;

    let total = u32::try_from(entidad.track_list.len()).unwrap_or(0);
    let tracks: Vec<Track> = entidad.track_list.into_iter().filter_map(a_pista).collect();

    if tracks.is_empty() {
        return Err(CoreError::invalid(
            "la lista de Spotify no trae ninguna canción legible",
        ));
    }

    // Una sola página: el embed lo devuelve todo de golpe. Se avisa igual para
    // que quien escucha el progreso no se quede sin el evento final.
    page_callback(u32::try_from(tracks.len()).unwrap_or(0), total);
    debug!(
        id,
        canciones = tracks.len(),
        "playlist leída sin credenciales"
    );

    Ok(PlaylistImport {
        source_id: id.to_owned(),
        name: if entidad.name.trim().is_empty() {
            "Lista importada".to_owned()
        } else {
            entidad.name
        },
        // Ver la cabecera: el camino anónimo no la trae.
        description: None,
        cover_url: entidad.cover_art.and_then(mayor),
        total,
        tracks,
    })
}

/// El contenido del `<script id="__NEXT_DATA__">`.
///
/// Se busca a mano en vez de con un analizador de HTML: es una etiqueta con un
/// identificador fijo y arrastrar una dependencia entera para encontrarla sería
/// desproporcionado.
fn extraer_next_data(html: &str) -> Option<&str> {
    const ABRE: &str = r#"<script id="__NEXT_DATA__" type="application/json">"#;
    let inicio = html.find(ABRE)? + ABRE.len();
    let resto = &html[inicio..];
    let fin = resto.find("</script>")?;
    Some(&resto[..fin])
}

/// La portada más grande que ofrezcan.
///
/// Vienen varios tamaños y se guarda una sola: la mayor, porque la ficha de una
/// playlist la enseña grande y ampliar una miniatura se ve.
fn mayor(portada: Portada) -> Option<String> {
    portada
        .sources
        .into_iter()
        .max_by_key(|f| f.width.unwrap_or(0))
        .map(|f| f.url)
}

/// Convierte una fila del embed en una pista del dominio.
///
/// Descarta lo que no tenga identidad o duración: sin duración el emparejador no
/// puede validar nada, y meter una pista a ciegas en la biblioteca es peor que
/// no meterla.
fn a_pista(fila: Fila) -> Option<Track> {
    let id = fila.uri.strip_prefix("spotify:track:")?;
    if id.is_empty() || fila.title.trim().is_empty() {
        return None;
    }
    let duracion = fila.duration?;

    Some(Track {
        id: TrackId::from_trusted(id.to_owned()),
        title: fila.title,
        album: None,
        // Sin identidad: el embed da el nombre unido para mostrar. Se parte por
        // comas para que el artista principal sea el primero, que es lo que usa
        // el emparejador.
        artists: artistas(&fila.subtitle),
        duration: DurationMs::new(duracion),
        track_number: None,
        disc_number: None,
        explicit: fila.is_explicit,
        isrc: None,
        release_date: None,
        popularity: None,
        added_at: Utc::now(),
    })
}

/// Parte la cadena de artistas y les da identificadores locales.
///
/// Son artistas sin identidad real: el embed no la da. Un identificador local
/// deja la pista bien formada y no finge que sabemos cuál es el artista de
/// Spotify. Si algún día se importa con credenciales, esa misma pista se
/// actualiza con los identificadores buenos.
fn artistas(subtitulo: &str) -> Vec<ArtistRef> {
    subtitulo
        .split(',')
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(|n| ArtistRef {
            id: ArtistId::nuevo_local(),
            name: n.to_owned(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn se_encuentra_el_bloque_de_datos() {
        let html = r#"<html><body><script id="__NEXT_DATA__" type="application/json">{"a":1}</script></body></html>"#;
        assert_eq!(extraer_next_data(html), Some(r#"{"a":1}"#));
    }

    #[test]
    fn sin_bloque_de_datos_no_se_inventa_nada() {
        // Es lo que pasa si Spotify cambia la página: mejor un error claro que
        // una lista vacía que parece "esta playlist no tiene canciones".
        assert_eq!(extraer_next_data("<html>nada</html>"), None);
    }

    #[test]
    fn una_fila_del_embed_se_convierte_en_pista() {
        // Datos reales de `open.spotify.com/embed/playlist/…`.
        let fila = Fila {
            uri: "spotify:track:70pVCVMGjmIWPbWXDwf11e".into(),
            title: "petal".into(),
            subtitle: "Ariana Grande".into(),
            duration: Some(184_248),
            is_explicit: true,
        };

        let pista = a_pista(fila).expect("es una pista válida");
        assert_eq!(pista.id.as_str(), "70pVCVMGjmIWPbWXDwf11e");
        assert_eq!(pista.title, "petal");
        assert_eq!(pista.duration.as_ms(), 184_248);
        assert!(pista.explicit);
        assert_eq!(pista.artists.len(), 1);
        assert_eq!(pista.artists[0].name, "Ariana Grande");
    }

    #[test]
    fn una_fila_sin_duracion_se_descarta() {
        // Sin duración el emparejador no puede validar la coincidencia, y lo
        // descargado no se vuelve a descargar.
        let fila = Fila {
            uri: "spotify:track:70pVCVMGjmIWPbWXDwf11e".into(),
            title: "petal".into(),
            subtitle: "Ariana Grande".into(),
            duration: None,
            is_explicit: false,
        };
        assert!(a_pista(fila).is_none());
    }

    #[test]
    fn los_artistas_se_separan_por_comas() {
        let a = artistas("Casey Edwards, Victor Borba");
        assert_eq!(a.len(), 2);
        assert_eq!(a[0].name, "Casey Edwards");
        assert_eq!(a[1].name, "Victor Borba");
        assert!(a[0].id.es_local(), "no fingimos conocer su id de Spotify");
    }

    #[test]
    fn se_elige_la_portada_mas_grande() {
        // El embed ofrece 64, 300 y 640; la ficha la enseña grande.
        let portada = Portada {
            sources: vec![
                Fuente {
                    url: "chica".into(),
                    width: Some(64),
                },
                Fuente {
                    url: "grande".into(),
                    width: Some(640),
                },
                Fuente {
                    url: "media".into(),
                    width: Some(300),
                },
            ],
        };
        assert_eq!(mayor(portada).as_deref(), Some("grande"));
    }
}

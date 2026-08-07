//! Búsqueda de candidatos en YouTube.
//!
//! Nunca se expone al frontend: no existe ningún comando para buscar en
//! YouTube. Es un detalle interno de la capa de obtención de audio, y esa
//! separación es deliberada (ver `06-api.md`).
//!
//! ## Estrategia de consultas
//!
//! Se emiten en orden de fiabilidad decreciente y se para en cuanto hay un
//! candidato de confianza alta. Cada consulta cuesta segundos, así que el orden
//! importa tanto como el contenido.

use localify_core::domain::audio::DurationMs;
use localify_core::domain::track::Track;
use localify_core::text;

use crate::rules_de_consulta;

/// Candidato tal como llega de yt-dlp, antes de puntuar.
#[derive(Debug, Clone, PartialEq)]
pub struct RawCandidate {
    pub video_id: String,
    pub title: String,
    pub channel: Option<String>,
    pub description: Option<String>,
    pub duration: DurationMs,
    pub view_count: Option<u64>,
    /// Procede de music.youtube.com.
    pub from_youtube_music: bool,
    /// La descripción contiene "Provided to YouTube by": subida por el titular
    /// de los derechos.
    pub provided_to_youtube: bool,
}

/// Una consulta a emitir, con su origen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Consulta {
    /// Texto tal como se pasa a yt-dlp.
    pub texto: String,
    /// `true` si va contra YouTube Music.
    pub music: bool,
    /// `true` si `texto` **es un vídeo concreto** y no algo que buscar.
    ///
    /// Lo usa el enlace que trae el catálogo: cuando MusicBrainz ya sabe qué
    /// vídeo es esta grabación, no hay nada que buscar. Sigue pasando por el
    /// mismo camino que el resto —se pide su ficha y se puntúa— para que un
    /// enlace equivocado no entre a ciegas en la biblioteca.
    pub directa: bool,
    /// Motivo, para trazas.
    pub origen: &'static str,
}

/// Candidatos que se piden por consulta.
///
/// Diez es suficiente: los resultados relevantes de YouTube salen en los
/// primeros puestos, y pedir más multiplica el tiempo de la consulta sin mejorar
/// el emparejamiento.
pub const RESULTADOS_POR_CONSULTA: u8 = 10;

/// Construye el plan de consultas para una pista.
///
/// El orden refleja fiabilidad: primero lo que identifica la grabación exacta,
/// después lo que identifica la canción.
///
/// `conocido` es el vídeo que el catálogo asocia a esta grabación. Va el
/// primero de todos —por delante incluso del ISRC— porque no identifica la
/// canción sino **la grabación concreta**, que es el objetivo de todo el plan.
/// Aun así es una consulta más: se pide su ficha y se puntúa con el resto, así
/// que un enlace equivocado en el catálogo lo caza el scorer.
#[must_use]
pub fn plan_de_consultas(pista: &Track, conocido: Option<&str>) -> Vec<Consulta> {
    let mut plan = Vec::with_capacity(6);

    if let Some(video) = conocido
        && !video.is_empty()
    {
        plan.push(Consulta {
            texto: format!("https://www.youtube.com/watch?v={video}"),
            music: false,
            directa: true,
            origen: "catalogo",
        });
    }

    let artista = pista
        .artista_principal()
        .map(|a| a.name.as_str())
        .unwrap_or_default();
    let titulo = text::search_title(&pista.title);

    // El ISRC identifica la grabación, no la canción: cuando existe y aparece
    // en la descripción de un vídeo, la coincidencia es prácticamente segura.
    if let Some(isrc) = &pista.isrc
        && !isrc.is_empty()
    {
        plan.push(Consulta {
            texto: format!("\"{isrc}\""),
            music: false,
            directa: false,
            origen: "isrc",
        });
    }

    if !artista.is_empty() && !titulo.is_empty() {
        // YouTube Music devuelve el catálogo oficial: es la mejor fuente.
        plan.push(Consulta {
            texto: format!("{artista} {titulo}"),
            music: true,
            directa: false,
            origen: "music",
        });

        if let Some(album) = &pista.album
            && album.title.len() >= 5
        {
            plan.push(Consulta {
                texto: format!("{artista} - {titulo} {}", album.title),
                music: false,
                directa: false,
                origen: "album",
            });
        }

        // Los canales `- Topic` son subidas automáticas de la discográfica.
        plan.push(Consulta {
            texto: format!("{artista} {titulo} topic"),
            music: false,
            directa: false,
            origen: "topic",
        });

        plan.push(Consulta {
            texto: format!("{artista} - {titulo}"),
            music: false,
            directa: false,
            origen: "general",
        });
    } else if !titulo.is_empty() {
        // Sin artista, lo único que queda es el título.
        plan.push(Consulta {
            texto: titulo,
            music: false,
            directa: false,
            origen: "solo_titulo",
        });
    }

    plan
}

/// `true` si el candidato es lo bastante bueno como para dejar de buscar.
///
/// Seguir consultando tras encontrar una coincidencia segura solo añadiría
/// segundos de espera a una descarga que ya puede empezar.
#[must_use]
pub fn basta_con(puntuacion: f32) -> bool {
    puntuacion >= localify_core::domain::download::UMBRAL_ALTA
}

pub use rules_de_consulta::detectar_provided;

#[cfg(test)]
mod tests {
    use localify_core::domain::ids::{AlbumId, ArtistId, TrackId};
    use localify_core::domain::track::{AlbumRef, ArtistRef};

    use super::*;

    fn pista(titulo: &str, artista: &str, album: Option<&str>, isrc: Option<&str>) -> Track {
        Track {
            id: TrackId::nuevo_local(),
            title: titulo.to_owned(),
            album: album.map(|a| AlbumRef {
                id: AlbumId::nuevo_local(),
                title: a.to_owned(),
            }),
            artists: if artista.is_empty() {
                Vec::new()
            } else {
                vec![ArtistRef {
                    id: ArtistId::nuevo_local(),
                    name: artista.to_owned(),
                }]
            },
            duration: DurationMs::new(248_000),
            track_number: None,
            disc_number: None,
            explicit: false,
            isrc: isrc.map(str::to_owned),
            release_date: None,
            popularity: None,
            added_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn el_video_del_catalogo_va_por_delante_incluso_del_isrc() {
        // El ISRC identifica la grabación pero hay que buscarla; el enlace del
        // catálogo **es** la grabación. MusicBrainz lo guarda para muchas, y
        // cuando lo hay no queda nada que adivinar.
        let p = pista(
            "Bury the Light",
            "Casey Edwards",
            None,
            Some("JPE102003410"),
        );
        let plan = plan_de_consultas(&p, Some("pvy9km7g6fw"));

        assert_eq!(plan[0].origen, "catalogo");
        assert!(plan[0].directa, "es un vídeo concreto, no una búsqueda");
        assert_eq!(plan[0].texto, "https://www.youtube.com/watch?v=pvy9km7g6fw");
        assert_eq!(plan[1].origen, "isrc", "el resto del plan sigue detrás");
    }

    #[test]
    fn sin_enlace_del_catalogo_el_plan_es_el_de_siempre() {
        // Es el caso normal: solo MusicBrainz lo sabe, y solo para parte de su
        // catálogo. El plan no puede cambiar de forma por eso.
        let p = pista("Under Pressure", "Queen", None, Some("GBUM71029604"));
        assert_eq!(plan_de_consultas(&p, None)[0].origen, "isrc");
        assert_eq!(plan_de_consultas(&p, Some(""))[0].origen, "isrc");
    }

    #[test]
    fn el_isrc_encabeza_el_plan_cuando_existe() {
        let p = pista(
            "Under Pressure",
            "Queen",
            Some("Hot Space"),
            Some("GBUM71029604"),
        );
        let plan = plan_de_consultas(&p, None);

        assert_eq!(plan[0].origen, "isrc");
        assert!(plan[0].texto.contains("GBUM71029604"));
        assert!(
            plan[0].texto.starts_with('"'),
            "el ISRC va entrecomillado para exigir coincidencia exacta"
        );
    }

    #[test]
    fn sin_isrc_la_primera_consulta_va_a_youtube_music() {
        let p = pista("Under Pressure", "Queen", Some("Hot Space"), None);
        let plan = plan_de_consultas(&p, None);

        assert_eq!(plan[0].origen, "music");
        assert!(plan[0].music, "YouTube Music devuelve el catálogo oficial");
    }

    #[test]
    fn el_plan_cubre_las_fuentes_previstas() {
        let p = pista(
            "Under Pressure",
            "Queen",
            Some("Hot Space"),
            Some("GBUM71029604"),
        );
        let origenes: Vec<&str> = plan_de_consultas(&p, None)
            .iter()
            .map(|c| c.origen)
            .collect();

        assert_eq!(origenes, vec!["isrc", "music", "album", "topic", "general"]);
    }

    #[test]
    fn el_titulo_se_limpia_de_sufijos_editoriales() {
        let p = pista("Bohemian Rhapsody - Remastered 2011", "Queen", None, None);
        let plan = plan_de_consultas(&p, None);

        assert!(
            plan.iter().all(|c| !c.texto.contains("remastered")),
            "buscar 'remastered' sesgaría hacia reediciones concretas"
        );
        assert!(plan.iter().any(|c| c.texto.contains("bohemian rhapsody")));
    }

    #[test]
    fn el_titulo_conserva_lo_que_distingue_la_version() {
        let p = pista("Smells Like Teen Spirit (Live)", "Nirvana", None, None);
        let plan = plan_de_consultas(&p, None);

        assert!(
            plan.iter().any(|c| c.texto.contains("live")),
            "sin 'live' se buscaría la versión de estudio"
        );
    }

    #[test]
    fn un_album_de_nombre_muy_corto_no_genera_consulta_propia() {
        // "Post" o "Mix" producirían resultados por casualidad.
        let p = pista("Hyperballad", "Björk", Some("Post"), None);
        let origenes: Vec<&str> = plan_de_consultas(&p, None)
            .iter()
            .map(|c| c.origen)
            .collect();
        assert!(!origenes.contains(&"album"));
    }

    #[test]
    fn sin_artista_queda_el_titulo_solo() {
        let p = pista("Una Cancion", "", None, None);
        let plan = plan_de_consultas(&p, None);

        assert_eq!(plan.len(), 1);
        assert_eq!(plan[0].origen, "solo_titulo");
    }

    #[test]
    fn una_pista_sin_datos_no_genera_consultas() {
        let p = pista("", "", None, None);
        assert!(
            plan_de_consultas(&p, None).is_empty(),
            "salir a buscar sin nada que buscar solo gastaría tiempo"
        );
    }

    #[test]
    fn la_busqueda_se_detiene_con_confianza_alta() {
        assert!(basta_con(90.0));
        assert!(basta_con(localify_core::domain::download::UMBRAL_ALTA));
        assert!(!basta_con(60.0));
        assert!(!basta_con(40.0));
    }
}

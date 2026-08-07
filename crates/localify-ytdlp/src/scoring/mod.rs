//! Puntuación de candidatos de YouTube.
//!
//! Es **lógica pura y determinista**: entra un `Track` de Spotify y una lista de
//! candidatos, sale una puntuación con su desglose. No hace I/O, así que se
//! prueba entera con fixtures.
//!
//! ## La fórmula
//!
//! ```text
//! puntuación = clamp(BASE + bonos − penalizaciones, 0, 100) × factor_duración
//! ```
//!
//! El factor de duración es **multiplicativo** a propósito. La duración es la
//! señal más fiable de que dos grabaciones son la misma, y ninguna cantidad de
//! bonificaciones debería rescatar a un candidato que dura medio minuto más que
//! la pista de Spotify.
//!
//! ## La excepción que lo hace utilizable
//!
//! Un término como `live` o `remix` penaliza… **salvo que el título de Spotify
//! también lo lleve**. Sin esa excepción, las canciones que legítimamente son
//! directos o remixes no encontrarían nunca su versión. Y funciona en los dos
//! sentidos: si Spotify dice "(Live)" y el candidato no, es que es la versión
//! equivocada.

pub mod rules;

use localify_core::domain::audio::DurationMs;
use localify_core::domain::download::{Confidence, MatchResult, ScoreBreakdown, YoutubeCandidate};
use localify_core::domain::track::Track;
use localify_core::text;

use crate::search::RawCandidate;

/// Puntúa un candidato contra la pista de referencia.
#[must_use]
pub fn puntuar(pista: &Track, candidato: &RawCandidate) -> YoutubeCandidate {
    let contexto = Contexto::nuevo(pista, candidato);
    let mut desglose = ScoreBreakdown {
        duration_diff_ms: contexto.diferencia_ms,
        ..ScoreBreakdown::default()
    };

    // Un candidato que se aleja demasiado en duración no es la misma
    // grabación: se descarta sin gastar más análisis.
    if contexto.diferencia_ms > rules::DESCARTE_DURACION_MS {
        desglose.duration_factor = 0.0;
        desglose
            .penalty_reasons
            .push("duration.discarded".to_owned());
        return contexto.candidato_con(desglose, 0.0);
    }

    desglose.duration_factor = rules::factor_duracion(contexto.diferencia_ms);
    desglose.source_bonus = contexto.bono_fuente();
    desglose.title_bonus = contexto.bono_titulo();
    desglose.artist_bonus = contexto.bono_artista();
    desglose.album_bonus = contexto.bono_album();

    let (penalizacion, motivos) = contexto.penalizaciones();
    desglose.penalties = -penalizacion;
    desglose.penalty_reasons = motivos;

    let bruto = rules::BASE
        + desglose.source_bonus
        + desglose.title_bonus
        + desglose.artist_bonus
        + desglose.album_bonus
        - penalizacion;

    let total = bruto.clamp(0.0, 100.0) * desglose.duration_factor;
    desglose.total = total;

    contexto.candidato_con(desglose, total)
}

/// Puntúa una lista y devuelve el mejor con su nivel de confianza.
///
/// `excluidos` son vídeos ya rechazados por el usuario o que fallaron al
/// descargar: volver a elegirlos sería ignorar información que ya tenemos.
#[must_use]
pub fn elegir_mejor(
    pista: &Track,
    candidatos: &[RawCandidate],
    excluidos: &[String],
) -> Option<MatchResult> {
    let mut puntuados: Vec<YoutubeCandidate> = candidatos
        .iter()
        .filter(|c| !excluidos.contains(&c.video_id))
        .map(|c| puntuar(pista, c))
        .collect();

    if puntuados.is_empty() {
        return None;
    }

    // Desempate por identificador: con dos candidatos idénticos, la elección
    // debe ser la misma en cada ejecución. Un emparejamiento que cambia entre
    // reintentos es imposible de depurar.
    puntuados.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.video_id.cmp(&b.video_id))
    });

    let considerados = u16::try_from(puntuados.len()).unwrap_or(u16::MAX);
    let mejor = puntuados.remove(0);

    Some(MatchResult {
        track_id: pista.id.clone(),
        confidence: Confidence::desde_puntuacion(mejor.score),
        best: mejor,
        candidates_considered: considerados,
    })
}

/// Datos ya normalizados de una comparación, para no repetir el trabajo.
struct Contexto<'a> {
    candidato: &'a RawCandidate,
    diferencia_ms: u32,
    /// Título del candidato, normalizado.
    titulo_norm: String,
    /// Título + descripción + canal, para buscar términos.
    texto_completo: String,
    canal_norm: String,
    /// Título de búsqueda de Spotify (sin sufijos editoriales).
    titulo_spotify: String,
    /// Título completo de Spotify, normalizado. Aquí sí están los términos que
    /// distinguen la versión.
    titulo_spotify_completo: String,
    artista_principal: String,
    album_norm: Option<String>,
    duracion_pista: DurationMs,
}

impl<'a> Contexto<'a> {
    fn nuevo(pista: &Track, candidato: &'a RawCandidate) -> Self {
        let titulo_norm = text::normalize(&candidato.title);
        let canal_norm = candidato
            .channel
            .as_deref()
            .map(text::normalize)
            .unwrap_or_default();
        let descripcion_norm = candidato
            .description
            .as_deref()
            .map(text::normalize)
            .unwrap_or_default();

        Self {
            diferencia_ms: pista.duration.diff(candidato.duration).as_ms(),
            texto_completo: format!("{titulo_norm} {canal_norm} {descripcion_norm}"),
            titulo_norm,
            canal_norm,
            titulo_spotify: text::search_title(&pista.title),
            titulo_spotify_completo: text::normalize(&pista.title),
            artista_principal: pista
                .artista_principal()
                .map(|a| text::normalize(&a.name))
                .unwrap_or_default(),
            album_norm: pista.album.as_ref().map(|a| text::normalize(&a.title)),
            duracion_pista: pista.duration,
            candidato,
        }
    }

    fn candidato_con(&self, desglose: ScoreBreakdown, total: f32) -> YoutubeCandidate {
        YoutubeCandidate {
            video_id: self.candidato.video_id.clone(),
            title: self.candidato.title.clone(),
            channel: self.candidato.channel.clone(),
            duration: self.candidato.duration,
            view_count: self.candidato.view_count,
            from_youtube_music: self.candidato.from_youtube_music,
            score: total,
            breakdown: desglose,
        }
    }

    /// Bonificación por la procedencia del vídeo.
    ///
    /// Solo cuenta la mejor señal, no la suma: un canal Topic servido desde
    /// YouTube Music es una sola cosa, no dos.
    fn bono_fuente(&self) -> f32 {
        let mut mejor = 0.0_f32;

        if self.candidato.from_youtube_music {
            mejor = mejor.max(rules::BONO_YOUTUBE_MUSIC);
        }
        if self.candidato.provided_to_youtube {
            mejor = mejor.max(rules::BONO_PROVIDED_TO_YOUTUBE);
        }
        if self.canal_norm.ends_with(rules::SUFIJO_TOPIC) {
            mejor = mejor.max(rules::BONO_CANAL_TOPIC);
        }
        // El canal es el del artista: "Queen Official", "queenofficial"…
        if !self.artista_principal.is_empty()
            && !self.canal_norm.is_empty()
            && (self.canal_norm.contains(&self.artista_principal)
                || text::similarity(&self.canal_norm, &self.artista_principal) > 0.9)
        {
            mejor = mejor.max(rules::BONO_CANAL_DEL_ARTISTA);
        }

        mejor
    }

    /// Bonificación proporcional a la similitud del título.
    fn bono_titulo(&self) -> f32 {
        // El título de YouTube suele ser "Artista - Título (algo)". Se compara
        // contra el título de búsqueda de Spotify, ya limpio de sufijos
        // editoriales, y se acepta también que lo contenga.
        let similitud = text::similarity(&self.titulo_norm, &self.titulo_spotify);
        let contiene =
            !self.titulo_spotify.is_empty() && self.titulo_norm.contains(&self.titulo_spotify);

        let efectiva = if contiene {
            similitud.max(0.95)
        } else {
            similitud
        };
        escalar(efectiva, rules::BONO_TITULO_MAX)
    }

    fn bono_artista(&self) -> f32 {
        if self.artista_principal.is_empty() {
            return 0.0;
        }
        // El artista puede aparecer en el título o en el canal; vale cualquiera.
        let en_titulo = self.titulo_norm.contains(&self.artista_principal);
        let en_canal = self.canal_norm.contains(&self.artista_principal);

        if en_titulo || en_canal {
            return rules::BONO_ARTISTA_MAX;
        }
        escalar(
            text::similarity(&self.canal_norm, &self.artista_principal),
            rules::BONO_ARTISTA_MAX,
        )
    }

    fn bono_album(&self) -> f32 {
        let Some(album) = &self.album_norm else {
            return 0.0;
        };
        // Un álbum de una sola palabra corriente ("post", "mix") produciría
        // coincidencias por casualidad.
        if album.len() < 5 {
            return 0.0;
        }
        if self.texto_completo.contains(album) {
            rules::BONO_ALBUM
        } else {
            0.0
        }
    }

    /// `true` si el término está en el título de Spotify.
    ///
    /// Es la excepción central: lo que Spotify declara no es ruido, es parte de
    /// la identidad de la versión.
    fn spotify_lo_pide(&self, termino: &str) -> bool {
        contiene_termino(&self.titulo_spotify_completo, termino)
    }

    /// `true` si el término está en el título del candidato.
    fn candidato_lo_dice(&self, termino: &str) -> bool {
        contiene_termino(&self.titulo_norm, termino)
    }

    /// Aplica un grupo de términos, respetando la excepción en ambos sentidos.
    ///
    /// Se penaliza **como mucho una vez por grupo**. Un título que diga
    /// "Karaoke Cover" está mal por una razón, no por dos, y sumar las dos
    /// penalizaciones distorsionaría la escala. Además evita que términos que
    /// se contienen entre sí (`session` dentro de `live session`) castiguen
    /// doble.
    ///
    /// El orden de evaluación es indiferente: primero se busca si el grupo está
    /// *satisfecho* (el mismo término en ambos lados), lo que anula cualquier
    /// otro veredicto.
    fn aplicar_grupo(
        &self,
        terminos: &[&str],
        castigo: f32,
        etiqueta: &str,
        total: &mut f32,
        motivos: &mut Vec<String>,
    ) {
        let mut sobra: Option<&str> = None;
        let mut falta: Option<&str> = None;

        for termino in terminos {
            let en_candidato = self.candidato_lo_dice(termino);
            let en_spotify = self.spotify_lo_pide(termino);

            match (en_candidato, en_spotify) {
                // Ambos lo llevan: es exactamente la versión buscada. Anula el
                // grupo entero, aunque haya otros términos sueltos.
                (true, true) => return,
                (true, false) => sobra = sobra.or(Some(termino)),
                (false, true) => falta = falta.or(Some(termino)),
                (false, false) => {}
            }
        }

        // Que sobre pesa más que que falte: un karaoke es peor error que una
        // versión acústica servida como eléctrica.
        if let Some(termino) = sobra {
            *total += castigo;
            motivos.push(format!("{etiqueta}.{termino}"));
        } else if let Some(termino) = falta {
            *total += rules::PENALIZA_FALTA_REQUERIDO;
            motivos.push(format!("missing.{termino}"));
        }
    }

    fn penalizaciones(&self) -> (f32, Vec<String>) {
        let mut total = 0.0_f32;
        let mut motivos = Vec::new();

        self.aplicar_grupo(
            rules::TERMINOS_DIRECTO,
            rules::PENALIZA_DIRECTO,
            "live",
            &mut total,
            &mut motivos,
        );
        self.aplicar_grupo(
            rules::TERMINOS_VERSION,
            rules::PENALIZA_VERSION,
            "version",
            &mut total,
            &mut motivos,
        );
        self.aplicar_grupo(
            rules::TERMINOS_MANIPULADO,
            rules::PENALIZA_MANIPULADO,
            "altered",
            &mut total,
            &mut motivos,
        );

        // Los videoclips no se penalizan si Spotify no dice nada: solo indican
        // preferencia por el audio, que suele estar más limpio.
        for termino in rules::TERMINOS_VIDEOCLIP {
            if self.candidato_lo_dice(termino) {
                total += rules::PENALIZA_VIDEOCLIP;
                motivos.push(format!("videoclip.{termino}"));
                break;
            }
        }

        // Recopilatorio: por duración desmesurada o por vocabulario.
        let sospechosamente_largo = self.candidato.duration.as_ms()
            > rules::DURACION_RECOPILATORIO_MS
            && self.duracion_pista.as_ms() < rules::DURACION_PISTA_NORMAL_MS;
        let dice_ser_recopilatorio = rules::TERMINOS_RECOPILATORIO
            .iter()
            .any(|t| self.candidato_lo_dice(t) && !self.spotify_lo_pide(t));

        if sospechosamente_largo || dice_ser_recopilatorio {
            total += rules::PENALIZA_RECOPILATORIO;
            motivos.push("compilation".to_owned());
        }

        // Canal desconocido y sin recorrido: probablemente una resubida.
        let sin_recorrido = self
            .candidato
            .view_count
            .is_some_and(|v| v < rules::VISTAS_MINIMAS);
        let canal_desconocido = !self.canal_norm.ends_with(rules::SUFIJO_TOPIC)
            && !self.candidato.provided_to_youtube
            && !self.candidato.from_youtube_music;

        if sin_recorrido && canal_desconocido {
            total += rules::PENALIZA_SIN_RECORRIDO;
            motivos.push("low_reach".to_owned());
        }

        (total, motivos)
    }
}

/// `true` si `texto` contiene `termino` **como palabra completa**.
///
/// Buscar por subcadena sería un desastre silencioso: `live` aparece dentro de
/// "Stayin' Alive", `mix` dentro de "Remix" y `8d` dentro de un identificador
/// cualquiera. Cada uno de esos falsos positivos descartaría la versión correcta
/// de una canción sin que nadie se enterara.
///
/// Ambos argumentos vienen normalizados, así que son palabras separadas por un
/// espacio. Un término de varias palabras se busca como secuencia contigua.
#[must_use]
pub fn contiene_termino(texto: &str, termino: &str) -> bool {
    if termino.is_empty() {
        return false;
    }
    let palabras: Vec<&str> = texto.split(' ').filter(|p| !p.is_empty()).collect();
    let buscadas: Vec<&str> = termino.split(' ').filter(|p| !p.is_empty()).collect();

    if buscadas.is_empty() || palabras.len() < buscadas.len() {
        return false;
    }
    palabras
        .windows(buscadas.len())
        .any(|v| v == buscadas.as_slice())
}

/// Escala una similitud al máximo dado, con suelo.
#[allow(
    clippy::cast_possible_truncation,
    reason = "la similitud vive en [0, 1]"
)]
fn escalar(similitud: f64, maximo: f32) -> f32 {
    if similitud < rules::UMBRAL_SIMILITUD {
        return 0.0;
    }
    // Se reescala el tramo útil para que el umbral valga cero y 1.0 el máximo.
    let normalizada = (similitud - rules::UMBRAL_SIMILITUD) / (1.0 - rules::UMBRAL_SIMILITUD);
    (normalizada as f32).clamp(0.0, 1.0) * maximo
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn un_termino_no_coincide_dentro_de_otra_palabra() {
        // El bug que este cambio corrige: con búsqueda por subcadena, "Stayin'
        // Alive" se habría penalizado como grabación en directo, descartando la
        // versión correcta sin que nadie se enterara.
        assert!(!contiene_termino("stayin alive", "live"));
        assert!(!contiene_termino("oliver twist", "live"));
        assert!(!contiene_termino("sandstorm extended remix", "mix"));
        assert!(!contiene_termino("discovery", "cover"));
        assert!(!contiene_termino("instrumentales", "instrumental"));
    }

    #[test]
    fn un_termino_coincide_como_palabra_completa() {
        assert!(contiene_termino("bohemian rhapsody live", "live"));
        assert!(contiene_termino("live at wembley", "live"));
        assert!(contiene_termino("dj mix 2024", "mix"));
        assert!(contiene_termino("sandstorm extended remix", "remix"));
    }

    #[test]
    fn un_termino_de_varias_palabras_exige_que_sean_contiguas() {
        assert!(contiene_termino("teardrop slowed reverb", "slowed"));
        assert!(contiene_termino(
            "queen greatest hits 1981",
            "greatest hits"
        ));
        assert!(
            !contiene_termino("greatest songs and hits", "greatest hits"),
            "las palabras deben ir seguidas"
        );
    }

    #[test]
    fn los_casos_degenerados_no_revientan() {
        assert!(!contiene_termino("", "live"));
        assert!(!contiene_termino("live", ""));
        assert!(!contiene_termino("a", "una frase muy larga"));
    }

    #[test]
    fn la_escala_de_similitud_tiene_suelo_y_techo() {
        assert!(
            (escalar(0.3, 20.0) - 0.0).abs() < f32::EPSILON,
            "por debajo del umbral, nada"
        );
        assert!(
            (escalar(1.0, 20.0) - 20.0).abs() < 0.01,
            "coincidencia total, el máximo"
        );
        let media = escalar(0.775, 20.0);
        assert!(
            (3.0..17.0).contains(&media),
            "un valor intermedio da algo intermedio: {media}"
        );
    }
}

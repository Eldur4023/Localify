//! Qué clase de grabación es una canción: la original, un directo, un remix…
//!
//! ## Por qué vive en el dominio
//!
//! Este vocabulario nació dentro del emparejador de YouTube, que lo usa para
//! puntuar candidatos contra una canción conocida. Resulta que la búsqueda
//! necesita exactamente lo mismo para agrupar diez versiones de "Faint" en una
//! sola fila, y `localify-services` no depende —ni debe depender— del crate de
//! descargas.
//!
//! Copiarlo habría sido la salida fácil y la equivocada: dos listas de términos
//! que empiezan iguales terminan distintas, y la que se queda atrás falla en
//! silencio. Vive aquí, y el emparejador la reexporta.
//!
//! ## Qué no hace
//!
//! No decide si una grabación es "buena" ni si es la que el usuario quiere.
//! Solo lee lo que el título dice de sí mismo. Un título que miente —o que
//! calla— se clasifica mal, y por eso quien lo usa combina esto con otras
//! señales: el álbum, el artista, la duración.

use crate::text;

/// Qué clase de grabación declara ser un título.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaseDeVersion {
    /// Nada en el título sugiere que sea otra cosa que la grabación de estudio.
    Original,
    /// Grabada en directo: es otra interpretación, no la del disco.
    Directo,
    /// Cover, karaoke, remix, instrumental, acústica…
    Otra,
    /// El audio se ha manipulado: acelerado, con reverb, 8D…
    Manipulado,
    /// Un recopilatorio o una mezcla continua, no una canción.
    Recopilatorio,
}

impl ClaseDeVersion {
    /// `true` si no es la grabación original.
    #[must_use]
    pub const fn es_variante(self) -> bool {
        !matches!(self, Self::Original)
    }
}

/// Términos que delatan una grabación en directo.
///
/// Van normalizados (minúsculas, sin diacríticos) porque se comparan contra
/// texto que ha pasado por [`text::normalize`].
pub const TERMINOS_DIRECTO: &[&str] = &[
    "live",
    "en vivo",
    "en directo",
    "directo",
    "concert",
    "concierto",
    "unplugged",
    "session",
    "sessions",
    "tiny desk",
];

/// Términos que delatan otra versión de la misma canción.
///
/// `acoustic` vive aquí y no en los directos: una versión acústica de estudio
/// es otra grabación, esté o no tocada en directo.
///
/// `demo` y `version` entraron al agrupar resultados de búsqueda: YouTube Music
/// devuelve ediciones de aniversario llenas de maquetas, y una maqueta es otra
/// grabación aunque la publicara el sello.
pub const TERMINOS_VERSION: &[&str] = &[
    "cover",
    "karaoke",
    "instrumental",
    "remix",
    "mashup",
    "nightcore",
    "tribute",
    "parodia",
    "parody",
    "reaction",
    "backing track",
    "playback",
    "midi",
    "acoustic",
    "acustico",
    "demo",
    "rehearsal",
    "ensayo",
];

/// Términos que delatan audio manipulado.
pub const TERMINOS_MANIPULADO: &[&str] = &[
    "slowed",
    "reverb",
    "sped up",
    "speed up",
    "bass boosted",
    "bass boost",
    "8d",
    "9d",
    "16d",
    "lofi",
    "lo fi",
    "pitched",
    "daycore",
];

/// Términos que delatan un vídeo musical.
pub const TERMINOS_VIDEOCLIP: &[&str] = &[
    "official video",
    "music video",
    "videoclip",
    "video oficial",
    "video musical",
];

/// Términos que delatan un recopilatorio.
pub const TERMINOS_RECOPILATORIO: &[&str] = &[
    "full album",
    "album completo",
    "greatest hits",
    "grandes exitos",
    "mix",
    "megamix",
    "compilation",
    "recopilacion",
    "playlist",
    "best of",
    "todas sus canciones",
];

/// Clasifica un título ya normalizado.
///
/// El orden importa. Un "live remix" es las dos cosas, y se cuenta como remix:
/// entre dos motivos para descartar algo, manda el que más lo aleja de la
/// grabación original.
#[must_use]
pub fn clase_normalizada(titulo_norm: &str) -> ClaseDeVersion {
    let contiene = |terminos: &[&str]| terminos.iter().any(|t| contiene_termino(titulo_norm, t));

    if contiene(TERMINOS_RECOPILATORIO) {
        ClaseDeVersion::Recopilatorio
    } else if contiene(TERMINOS_MANIPULADO) {
        ClaseDeVersion::Manipulado
    } else if contiene(TERMINOS_VERSION) {
        ClaseDeVersion::Otra
    } else if contiene(TERMINOS_DIRECTO) {
        ClaseDeVersion::Directo
    } else {
        ClaseDeVersion::Original
    }
}

/// Clasifica un título tal cual venga.
#[must_use]
pub fn clase(titulo: &str) -> ClaseDeVersion {
    clase_normalizada(&text::normalize(titulo))
}

/// `true` si el texto contiene el término **como palabra completa**.
///
/// Con `contains` a secas, "mix" aparece dentro de "mixtape" y de "remix", y
/// cualquier canción que se llamara "Remixed" se clasificaría dos veces. Peor:
/// "8d" está dentro de un identificador cualquiera.
fn contiene_termino(texto: &str, termino: &str) -> bool {
    let mut desde = 0;
    while let Some(pos) = texto[desde..].find(termino) {
        let inicio = desde + pos;
        let fin = inicio + termino.len();

        let antes_ok = inicio == 0
            || !texto[..inicio]
                .chars()
                .next_back()
                .is_some_and(char::is_alphanumeric);
        let despues_ok = fin == texto.len()
            || !texto[fin..]
                .chars()
                .next()
                .is_some_and(char::is_alphanumeric);

        if antes_ok && despues_ok {
            return true;
        }
        desde = inicio + 1;
    }
    false
}

/// Título sin los añadidos entre paréntesis, corchetes o tras un guion.
///
/// Es lo que permite reconocer que "Faint", "Faint (Live)" y
/// "Faint - Remastered 2011" son la misma canción.
///
/// ## Solo se quita si el añadido dice ser una variante
///
/// "Jigga What / Faint" y "Say My Name (feat. Alguien)" no se recortan: quitar
/// todo paréntesis convertiría en la misma canción cosas que no lo son, y una
/// colaboración distinta es un tema distinto.
#[must_use]
pub fn titulo_canonico(titulo: &str) -> String {
    let mut base = titulo;

    // Se recorta por la cola mientras el último añadido sea una variante.
    while let Some(corte) = ultimo_corte(base) {
        let (izquierda, resto) = base.split_at(corte);
        if !clase(resto).es_variante() || izquierda.trim().is_empty() {
            break;
        }
        base = izquierda;
    }

    // `search_title` remata el trabajo con el ruido editorial —"remastered
    // 2011", "deluxe edition"— que no es una versión distinta pero tampoco
    // parte del título. Reimplementarlo aquí era duplicar su lista.
    text::search_title(base)
}

/// Posición donde empieza el último añadido de un título, si lo hay.
fn ultimo_corte(titulo: &str) -> Option<usize> {
    let candidatos = [titulo.rfind('('), titulo.rfind('['), titulo.rfind(" - ")];
    candidatos.into_iter().flatten().max()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn los_directos_y_las_versiones_se_distinguen() {
        assert_eq!(clase("Faint"), ClaseDeVersion::Original);
        assert_eq!(clase("Faint (Live)"), ClaseDeVersion::Directo);
        assert_eq!(clase("Faint (Instrumental)"), ClaseDeVersion::Otra);
        assert_eq!(
            clase("Faint (Meteora|20 Demo)"),
            ClaseDeVersion::Otra,
            "una maqueta es otra grabacion aunque la publique el sello"
        );
        assert_eq!(clase("Faint (slowed + reverb)"), ClaseDeVersion::Manipulado);
        assert_eq!(
            clase("Linkin Park Greatest Hits"),
            ClaseDeVersion::Recopilatorio
        );
    }

    #[test]
    fn los_terminos_se_buscan_como_palabra_entera() {
        // "mix" dentro de "mixtape" no convierte la cancion en recopilatorio, y
        // "live" dentro de "Oliver" no la convierte en directo.
        assert_eq!(clase("Oliver"), ClaseDeVersion::Original);
        assert_eq!(clase("Delivery"), ClaseDeVersion::Original);
        assert_eq!(clase("Mixtape"), ClaseDeVersion::Original);
    }

    #[test]
    fn entre_dos_motivos_manda_el_que_mas_aleja_del_original() {
        // Un directo remezclado esta mas lejos del disco que un directo a secas.
        assert_eq!(clase("Numb (Live Remix)"), ClaseDeVersion::Otra);
    }

    #[test]
    fn el_titulo_canonico_agrupa_las_variantes_de_una_cancion() {
        for variante in [
            "Faint",
            "Faint (Live)",
            "Faint (Instrumental)",
            "Faint (Live in Hamburg, 2011)",
            "Faint - Remastered 2011",
            "Faint [Karaoke Version]",
        ] {
            assert_eq!(
                titulo_canonico(variante),
                "faint",
                "'{variante}' deberia agruparse con las demas"
            );
        }
    }

    #[test]
    fn una_colaboracion_no_es_una_variante() {
        // Quitar todo parentesis juntaria temas que no son el mismo: un tema
        // con otro invitado es otro tema.
        assert_ne!(
            titulo_canonico("Say My Name (feat. Alguien)"),
            titulo_canonico("Say My Name"),
        );
        assert_ne!(
            titulo_canonico("Jigga What / Faint"),
            titulo_canonico("Faint"),
        );
    }

    #[test]
    fn un_titulo_que_solo_es_una_variante_no_se_queda_vacio() {
        // "(Live)" a secas no puede recortarse hasta la nada: quedarian todas
        // las canciones sin titulo agrupadas en una sola fila.
        assert!(!titulo_canonico("(Live)").is_empty());
    }
}

//! Normalización canónica de texto.
//!
//! **Esta es la única implementación de normalización del proyecto.** La usan
//! `MetadataService` (para poblar las columnas `*_norm`), el buscador local y
//! el scorer de YouTube. Si dos de ellos normalizaran distinto, el matching se
//! degradaría en silencio y sería muy difícil de diagnosticar: por eso vive en
//! `core` y no en ninguna implementación concreta.
//!
//! Pipeline: minúsculas → NFKD → eliminar diacríticos → eliminar signos →
//! colapsar espacios.

use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;

/// Forma canónica para comparación y búsqueda.
///
/// ```
/// use localify_core::text::normalize;
/// assert_eq!(normalize("Björk – Jóga  (Live)"), "bjork joga live");
/// assert_eq!(normalize("MØ feat. Diplo"), "mo feat diplo");
/// ```
#[must_use]
pub fn normalize(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut espacio_pendiente = false;

    for ch in input.nfkd() {
        if is_combining_mark(ch) {
            continue;
        }
        // Casos que NFKD no descompone: letras con trazo.
        let ch = match ch {
            'ø' | 'Ø' => 'o',
            'đ' | 'Đ' | 'ð' | 'Ð' => 'd',
            'ł' | 'Ł' => 'l',
            'ħ' | 'Ħ' => 'h',
            'ŧ' | 'Ŧ' => 't',
            'æ' | 'Æ' => 'a', // simplificación deliberada: 'ae' rompería la longitud esperada
            'œ' | 'Œ' => 'o',
            'ß' => 's',
            otro => otro,
        };

        if ch.is_alphanumeric() {
            if espacio_pendiente && !out.is_empty() {
                out.push(' ');
            }
            espacio_pendiente = false;
            out.extend(ch.to_lowercase());
        } else {
            // Cualquier no-alfanumérico (espacios, guiones, paréntesis, tipos
            // de guion Unicode) actúa como separador único.
            espacio_pendiente = true;
        }
    }
    out
}

/// Sufijos editoriales que Spotify añade al título y que no distinguen la
/// canción. Se eliminan antes de buscar en YouTube para no sesgar la consulta.
const SUFIJOS_RUIDO: &[&str] = &[
    "remaster",
    "remastered",
    "deluxe",
    "deluxe edition",
    "expanded edition",
    "special edition",
    "anniversary edition",
    "bonus track",
    "bonus track version",
    "single version",
    "album version",
    "radio edit",
    "explicit",
    "explicit version",
    "mono",
    "stereo",
];

/// Elimina paréntesis y sufijos editoriales, dejando el título "de búsqueda".
///
/// Conserva lo que sí distingue una versión (`live`, `remix`, `acoustic`),
/// porque eso es exactamente lo que el scorer necesita para exigir
/// coincidencia en vez de penalizarla.
///
/// ```
/// use localify_core::text::search_title;
/// assert_eq!(search_title("Bohemian Rhapsody - Remastered 2011"), "bohemian rhapsody");
/// assert_eq!(search_title("Smells Like Teen Spirit (Live)"), "smells like teen spirit live");
/// ```
#[must_use]
pub fn search_title(title: &str) -> String {
    let normalizado = normalize(title);
    let mut palabras: Vec<&str> = normalizado.split(' ').filter(|s| !s.is_empty()).collect();

    // Recorta por la cola mientras el sufijo sea ruido editorial. Se hace en
    // varias pasadas para cubrir "remastered 2011" (año + palabra).
    loop {
        let antes = palabras.len();

        if let Some(&last) = palabras.last() {
            // Un año suelto al final solo es ruido si le precede un término
            // editorial ("remastered 2011"), nunca por sí mismo ("1979").
            let es_anyo = last.len() == 4 && last.chars().all(|c| c.is_ascii_digit());
            if es_anyo
                && palabras.len() >= 2
                && let Some(&anterior) = palabras.get(palabras.len() - 2)
                && SUFIJOS_RUIDO.contains(&anterior)
            {
                palabras.pop();
            }
        }

        for n in (1..=3).rev() {
            if palabras.len() > n {
                let cola = palabras[palabras.len() - n..].join(" ");
                if SUFIJOS_RUIDO.contains(&cola.as_str()) {
                    palabras.truncate(palabras.len() - n);
                    break;
                }
            }
        }

        if palabras.len() == antes {
            break;
        }
    }

    palabras.join(" ")
}

/// Similitud Jaro-Winkler en `[0.0, 1.0]`. La usa el scorer para comparar
/// títulos y nombres de canal, donde las diferencias son tipográficas
/// (mayúsculas, puntuación, orden) y no semánticas.
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn similarity(a: &str, b: &str) -> f64 {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();

    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }

    let ventana = (a.len().max(b.len()) / 2).saturating_sub(1);
    let mut a_match = vec![false; a.len()];
    let mut b_match = vec![false; b.len()];
    let mut coincidencias = 0usize;

    for (i, &ca) in a.iter().enumerate() {
        let inicio = i.saturating_sub(ventana);
        let fin = (i + ventana + 1).min(b.len());
        for j in inicio..fin {
            if !b_match[j] && b[j] == ca {
                a_match[i] = true;
                b_match[j] = true;
                coincidencias += 1;
                break;
            }
        }
    }

    if coincidencias == 0 {
        return 0.0;
    }

    let mut transposiciones = 0usize;
    let mut k = 0usize;
    for (i, &matched) in a_match.iter().enumerate() {
        if !matched {
            continue;
        }
        while !b_match[k] {
            k += 1;
        }
        if a[i] != b[k] {
            transposiciones += 1;
        }
        k += 1;
    }

    let m = coincidencias as f64;
    let jaro =
        (m / a.len() as f64 + m / b.len() as f64 + (m - transposiciones as f64 / 2.0) / m) / 3.0;

    // Bonificación de Winkler: prefijo común de hasta 4 caracteres.
    let prefijo = a
        .iter()
        .zip(b.iter())
        .take(4)
        .take_while(|(x, y)| x == y)
        .count() as f64;

    jaro + prefijo * 0.1 * (1.0 - jaro)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_elimina_diacriticos_y_signos() {
        assert_eq!(normalize("Björk – Jóga  (Live)"), "bjork joga live");
        assert_eq!(normalize("Café  Tacvba"), "cafe tacvba");
        assert_eq!(normalize("AC/DC"), "ac dc");
        assert_eq!(normalize("  espacios   raros  "), "espacios raros");
    }

    #[test]
    fn normalize_maneja_letras_con_trazo() {
        assert_eq!(normalize("MØ"), "mo");
        assert_eq!(normalize("Motörhead"), "motorhead");
        assert_eq!(normalize("Straße"), "strase");
    }

    #[test]
    fn normalize_conserva_texto_no_latino() {
        // No transliteramos: el scorer compara con el título original de YouTube,
        // que también viene en su alfabeto.
        assert_eq!(normalize("君の名は"), "君の名は");
        assert_eq!(normalize("Тату"), "тату");
    }

    #[test]
    fn normalize_es_idempotente() {
        let entrada = "Björk – Jóga (Remastered 2011)";
        let una = normalize(entrada);
        assert_eq!(normalize(&una), una);
    }

    #[test]
    fn search_title_quita_sufijos_editoriales() {
        assert_eq!(
            search_title("Bohemian Rhapsody - Remastered 2011"),
            "bohemian rhapsody"
        );
        assert_eq!(
            search_title("Wish You Were Here (Album Version)"),
            "wish you were here"
        );
        assert_eq!(search_title("Hey Jude - Mono"), "hey jude");
    }

    #[test]
    fn search_title_conserva_lo_que_distingue_la_version() {
        // Crítico: si borráramos "live" o "remix", el scorer penalizaría la
        // versión correcta por contener un término prohibido.
        assert_eq!(
            search_title("Smells Like Teen Spirit (Live)"),
            "smells like teen spirit live"
        );
        assert_eq!(
            search_title("Around the World - Radio Edit"),
            "around the world"
        );
        assert!(search_title("Sandstorm (Extended Remix)").contains("remix"));
    }

    #[test]
    fn search_title_no_borra_un_anyo_que_es_el_titulo() {
        assert_eq!(search_title("1979"), "1979");
        assert_eq!(
            search_title("Smashing Pumpkins 1979"),
            "smashing pumpkins 1979"
        );
    }

    #[test]
    fn similarity_reconoce_identicos_y_distingue_distintos() {
        assert!((similarity("bohemian rhapsody", "bohemian rhapsody") - 1.0).abs() < 1e-9);
        assert!(similarity("bohemian rhapsody", "bohemian rapsody") > 0.9);
        assert!(similarity("bohemian rhapsody", "stairway to heaven") < 0.6);
        assert!((similarity("", "") - 1.0).abs() < 1e-9);
        assert!(similarity("algo", "") < 1e-9);
    }
}

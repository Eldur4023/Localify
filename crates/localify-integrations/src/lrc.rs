//! Análisis del formato LRC.
//!
//! Un fichero LRC es una lista de líneas `[mm:ss.cc] texto`. La especificación
//! no existe como tal —es un formato de facto de finales de los noventa— y los
//! ficheros reales traen de todo: etiquetas de metadatos, varias marcas de
//! tiempo por línea, centésimas o milésimas, y líneas sin texto.
//!
//! Este módulo no intenta ser tolerante "por si acaso": cada rareza que acepta
//! es una que aparece en LRCLIB y que, ignorada, produce una letra desalineada
//! o incompleta.
//!
//! ## Lo que se acepta y por qué
//!
//! - **Varias marcas en una línea** (`[00:12.00][01:30.00] estribillo`): así se
//!   escriben los estribillos sin repetir el texto. Ignorarlas dejaría el
//!   estribillo sin resaltar en todas sus repeticiones menos la primera.
//! - **Etiquetas de metadatos** (`[ar:Artista]`, `[length:03:21]`): se
//!   descartan. Colarlas como líneas de letra pondría "ar:Artista" en pantalla.
//! - **Fracciones de dos o tres dígitos**: `.34` son centésimas y `.340`,
//!   milésimas. Tratarlas igual desplazaría la letra casi un segundo.
//! - **Líneas vacías**: se conservan. Son los silencios instrumentales, y sin
//!   ellas el resaltado se queda clavado en el último verso durante el solo.

use localify_core::domain::audio::DurationMs;
use localify_core::domain::lyrics::LyricLine;

/// Convierte un LRC en líneas ordenadas por tiempo.
///
/// Devuelve `None` si no hay ni una línea con marca de tiempo: un fichero así
/// es letra plana, no sincronizada, y decir lo contrario haría que la interfaz
/// intentara resaltar algo que no avanza.
#[must_use]
pub fn analizar(texto: &str) -> Option<Vec<LyricLine>> {
    let mut lineas = Vec::new();

    for cruda in texto.lines() {
        let (marcas, resto) = marcas_de(cruda);
        if marcas.is_empty() {
            continue;
        }
        let texto = resto.trim().to_owned();
        for at_ms in marcas {
            lineas.push(LyricLine {
                at: DurationMs::new(at_ms),
                text: texto.clone(),
            });
        }
    }

    if lineas.is_empty() {
        return None;
    }

    // Con varias marcas por línea, el orden de lectura no es el cronológico.
    lineas.sort_by_key(|l| l.at.as_ms());
    Some(lineas)
}

/// Extrae las marcas de tiempo iniciales y devuelve el resto de la línea.
fn marcas_de(linea: &str) -> (Vec<u32>, &str) {
    let mut marcas = Vec::new();
    let mut resto = linea.trim_start();

    while resto.starts_with('[') {
        let Some(fin) = resto.find(']') else { break };
        let dentro = &resto[1..fin];

        // Una etiqueta de metadatos (`ar:`, `length:`) corta la cabecera: lo
        // que venga después ya no es una marca de tiempo.
        let Some(ms) = tiempo(dentro) else {
            if marcas.is_empty() {
                return (Vec::new(), "");
            }
            break;
        };
        marcas.push(ms);
        resto = &resto[fin + 1..];
    }

    (marcas, resto)
}

/// Convierte `mm:ss.cc` o `mm:ss.mmm` en milisegundos.
fn tiempo(s: &str) -> Option<u32> {
    let (min, resto) = s.split_once(':')?;
    let minutos: u32 = min.trim().parse().ok()?;

    let (seg, frac) = match resto.split_once(['.', ':']) {
        Some((a, b)) => (a, b),
        None => (resto, ""),
    };
    let segundos: u32 = seg.trim().parse().ok()?;
    if segundos >= 60 {
        return None;
    }

    // Dos dígitos son centésimas y tres, milésimas. Multiplicar por diez en el
    // primer caso es la diferencia entre una letra alineada y otra que va casi
    // un segundo por delante en cada verso.
    let milis = match frac.len() {
        0 => 0,
        2 => frac.parse::<u32>().ok()? * 10,
        3 => frac.parse::<u32>().ok()?,
        1 => frac.parse::<u32>().ok()? * 100,
        _ => return None,
    };

    Some(minutos * 60_000 + segundos * 1_000 + milis)
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "en un test, un `expect` que falla es el fallo"
)]
mod tests {
    use super::*;

    #[test]
    fn una_linea_normal_se_analiza() {
        let l = analizar("[00:12.34] Hola").expect("hay letra");
        assert_eq!(l.len(), 1);
        assert_eq!(l[0].at.as_ms(), 12_340);
        assert_eq!(l[0].text, "Hola");
    }

    #[test]
    fn las_centesimas_y_las_milesimas_se_distinguen() {
        // `.34` son 340 ms; `.340` también. Pero `.034` son 34 ms. Tratar los
        // dos casos igual desalinearía la letra casi un segundo por verso.
        assert_eq!(analizar("[00:01.34] a").expect("hay")[0].at.as_ms(), 1_340);
        assert_eq!(analizar("[00:01.340] a").expect("hay")[0].at.as_ms(), 1_340);
        assert_eq!(analizar("[00:01.034] a").expect("hay")[0].at.as_ms(), 1_034);
    }

    #[test]
    fn un_estribillo_con_varias_marcas_aparece_en_todas() {
        let l = analizar("[00:10.00][01:20.50][02:30.00] Estribillo").expect("hay letra");
        assert_eq!(l.len(), 3);
        assert_eq!(l[0].at.as_ms(), 10_000);
        assert_eq!(l[1].at.as_ms(), 80_500);
        assert_eq!(l[2].at.as_ms(), 150_000);
        assert!(l.iter().all(|x| x.text == "Estribillo"));
    }

    #[test]
    fn las_etiquetas_de_metadatos_no_son_letra() {
        let l = analizar("[ar:Artista]\n[ti:Titulo]\n[length:03:21]\n[00:05.00] Primera línea")
            .expect("hay letra");
        assert_eq!(l.len(), 1, "solo la línea con marca de tiempo es letra");
        assert_eq!(l[0].text, "Primera línea");
    }

    #[test]
    fn las_lineas_vacias_se_conservan() {
        // Son los silencios instrumentales. Sin ellas, el resaltado se queda
        // clavado en el último verso durante todo el solo.
        let l = analizar("[00:05.00] Verso\n[00:20.00]\n[00:40.00] Otro").expect("hay letra");
        assert_eq!(l.len(), 3);
        assert_eq!(l[1].text, "");
    }

    #[test]
    fn el_resultado_va_ordenado_por_tiempo() {
        let l = analizar("[01:00.00] segunda\n[00:10.00] primera").expect("hay letra");
        assert_eq!(l[0].text, "primera");
        assert_eq!(l[1].text, "segunda");
    }

    #[test]
    fn un_texto_sin_marcas_no_es_letra_sincronizada() {
        assert!(analizar("Solo texto plano\nsin marcas").is_none());
        assert!(analizar("").is_none());
    }

    #[test]
    fn los_segundos_fuera_de_rango_se_descartan() {
        // `[00:75.00]` no es un tiempo válido; aceptarlo colocaría la línea a
        // 1:15 en vez de descartarla, y el resto de la letra se desplazaría.
        assert!(analizar("[00:75.00] mal").is_none());
    }

    #[test]
    fn un_corchete_sin_cerrar_no_cuelga_el_analisis() {
        assert!(analizar("[00:10.00 sin cerrar").is_none());
    }
}

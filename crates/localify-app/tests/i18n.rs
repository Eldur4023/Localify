//! Los catálogos de idioma, comprobados desde Rust.
//!
//! No hay comprobador de tipos ni test runner en el frontend (ADR-019), así que
//! los invariantes de los ficheros de traducción se verifican aquí, que es donde
//! sí hay uno.
//!
//! ## Por qué merece un test
//!
//! Una clave que falta en un idioma no rompe nada: la interfaz muestra
//! `[settings.moving]` en su lugar y sigue funcionando. Eso significa que el
//! fallo **no se ve** salvo que alguien cambie de idioma y mire justo esa
//! pantalla. Es exactamente el tipo de error que se acumula en silencio durante
//! meses.
//!
//! Lo mismo con la codificación: una herramienta que reescriba un fichero en
//! latin1 convierte "Español" en "EspaÃ±ol" y el resultado sigue siendo JSON
//! válido. Solo se nota mirando.

#![allow(clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::path::PathBuf;

/// Ruta de un catálogo, relativa a la raíz del repositorio.
fn catalogo(idioma: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../frontend/src/i18n")
        .join(format!("{idioma}.json"))
}

fn leer(idioma: &str) -> BTreeMap<String, String> {
    let ruta = catalogo(idioma);
    let texto = std::fs::read_to_string(&ruta)
        .unwrap_or_else(|e| panic!("no se pudo leer {}: {e}", ruta.display()));
    serde_json::from_str(&texto)
        .unwrap_or_else(|e| panic!("{} no es JSON válido: {e}", ruta.display()))
}

#[test]
fn los_dos_idiomas_tienen_exactamente_las_mismas_claves() {
    let es = leer("es");
    let en = leer("en");

    let faltan_en_ingles: Vec<_> = es.keys().filter(|k| !en.contains_key(*k)).collect();
    let faltan_en_espanol: Vec<_> = en.keys().filter(|k| !es.contains_key(*k)).collect();

    assert!(
        faltan_en_ingles.is_empty(),
        "claves sin traducir al inglés: {faltan_en_ingles:?}"
    );
    assert!(
        faltan_en_espanol.is_empty(),
        "claves sin traducir al español: {faltan_en_espanol:?}"
    );
}

#[test]
fn ninguna_traduccion_esta_vacia_por_descuido() {
    for idioma in ["es", "en"] {
        for (clave, valor) in leer(idioma) {
            assert!(
                !valor.trim().is_empty(),
                "{idioma}.json: '{clave}' está vacía"
            );
        }
    }
}

#[test]
fn los_parametros_coinciden_entre_idiomas() {
    let es = leer("es");
    let en = leer("en");

    for (clave, texto_es) in &es {
        let Some(texto_en) = en.get(clave) else {
            continue; // lo cubre el test de claves
        };
        let mut a = parametros(texto_es);
        let mut b = parametros(texto_en);
        a.sort();
        b.sort();
        // Un `{count}` que se pierde al traducir deja un texto que dice
        // "canciones" sin decir cuántas, y no falla en ningún sitio.
        assert_eq!(a, b, "los parámetros de '{clave}' no coinciden");
    }
}

/// Nombres entre llaves de una plantilla: `"{count} canciones"` → `["count"]`.
fn parametros(texto: &str) -> Vec<String> {
    let mut salida = Vec::new();
    let mut resto = texto;
    while let Some(i) = resto.find('{') {
        let tras = &resto[i + 1..];
        match tras.find('}') {
            Some(j) => {
                salida.push(tras[..j].to_owned());
                resto = &tras[j + 1..];
            }
            None => break,
        }
    }
    salida
}

/// Prefijos cuyas claves se componen en tiempo de ejecución.
///
/// Son los únicos sitios donde una clave no aparece literalmente en el código, y
/// van enumerados a mano a propósito: la alternativa —aceptar cualquier clave
/// que empiece por algo conocido— convertiría el test en un colador.
const COMPUESTAS: &[&str] = &[
    // `t(`album.type.${detalle.albumType}`)` en views/album.ts
    "album.type.",
    // `pasos(["discord_step_1", …])` → `t(`settings.${clave}`)` en views/settings.ts
    "settings.discord_step_",
    "settings.lastfm_step_",
];

/// Ninguna clave sobra.
///
/// Una traducción que ya no usa nadie no se ve —no rompe nada, no aparece en
/// pantalla— y por eso se queda. Se habían acumulado dieciséis: la sección
/// "Escuchado hace poco" de Inicio que dejó de existir, una etiqueta de silenciar
/// para un botón que nunca se puso, "Ir al artista" para un menú que no puede
/// ofrecerlo porque la fila no trae el identificador del artista.
///
/// Leer `es.json` era hacerse una idea equivocada de lo que la aplicación hace.
#[test]
fn ninguna_clave_sobra() {
    let fuentes = fuentes_del_proyecto();

    let huerfanas: Vec<String> = leer("es")
        .into_keys()
        .filter(|c| !COMPUESTAS.iter().any(|p| c.starts_with(p)))
        .filter(|c| !fuentes.contains(&format!("\"{c}\"")))
        .collect();

    assert!(
        huerfanas.is_empty(),
        "claves que no usa nadie: {huerfanas:?}\n\
         Si se componen en tiempo de ejecución, añade su prefijo a COMPUESTAS."
    );
}

/// Todo el código del proyecto en una sola cadena.
///
/// El frontend y el backend juntos porque las claves salen de los dos: el
/// backend emite `message_key` y el frontend lo traduce (ADR-012).
fn fuentes_del_proyecto() -> String {
    let raiz = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut todo = String::new();
    for carpeta in ["frontend/src", "crates"] {
        recoger(&raiz.join(carpeta), &mut todo);
    }
    todo
}

fn recoger(dir: &std::path::Path, salida: &mut String) {
    let Ok(entradas) = std::fs::read_dir(dir) else {
        return;
    };
    for entrada in entradas.flatten() {
        let ruta = entrada.path();
        if ruta.is_dir() {
            recoger(&ruta, salida);
        } else if matches!(
            ruta.extension().and_then(|e| e.to_str()),
            Some("ts" | "rs" | "html")
        ) && let Ok(texto) = std::fs::read_to_string(&ruta)
        {
            salida.push_str(&texto);
        }
    }
}

#[test]
fn los_catalogos_estan_en_utf8_sin_doble_codificar() {
    for idioma in ["es", "en"] {
        let texto = std::fs::read_to_string(catalogo(idioma)).expect("lee");
        // "Ã" y "â" solo aparecen si un fichero UTF-8 se ha releído como
        // latin1 y vuelto a escribir. Ninguno de los dos idiomas los usa.
        for sospechoso in ["Ã", "â\u{80}", "Â"] {
            assert!(
                !texto.contains(sospechoso),
                "{idioma}.json parece doblemente codificado: contiene '{sospechoso}'"
            );
        }
    }
}

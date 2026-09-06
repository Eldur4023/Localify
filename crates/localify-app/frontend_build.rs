//! Compilación del frontend sin Node.js (ADR-019).
//!
//! Recorre `frontend/`, transpila cada `.ts` a `.js` con oxc y copia el resto
//! de assets a `frontend/dist/`. No empaqueta: WebView2 es Chromium y carga
//! módulos ES nativos, y bundlear activos que se sirven desde el propio proceso
//! no ahorraría nada.
//!
//! Convertir TypeScript a JavaScript es, para nuestro caso, **borrado de
//! tipos**: una transformación puramente sintáctica. La comprobación de tipos
//! es un paso aparte y opcional (`tsc --noEmit`), no una puerta del build.

use std::error::Error;
use std::path::{Path, PathBuf};

use oxc_allocator::Allocator;
use oxc_codegen::Codegen;
use oxc_parser::{ParseOptions, Parser};
use oxc_semantic::SemanticBuilder;
use oxc_span::SourceType;
use oxc_transformer::{TransformOptions, Transformer};

type Resultado<T> = Result<T, Box<dyn Error>>;

pub(crate) fn build() -> Resultado<()> {
    let raiz_crate = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let raiz_proyecto = raiz_crate
        .parent()
        .and_then(Path::parent)
        .ok_or("no se pudo localizar la raíz del proyecto")?;

    let origen = raiz_proyecto.join("frontend");
    let destino = origen.join("dist");

    if !origen.is_dir() {
        return Err(format!("no existe la carpeta frontend: {}", origen.display()).into());
    }

    // Solo se recompila si cambia algo del frontend.
    println!("cargo::rerun-if-changed={}", origen.join("src").display());
    println!(
        "cargo::rerun-if-changed={}",
        origen.join("index.html").display()
    );

    std::fs::create_dir_all(&destino)?;

    let mut procesados = 0_u32;
    let mut generados = std::collections::HashSet::new();
    recorrer(&origen, &origen, &destino, &mut procesados, &mut generados)?;

    let borrados = podar(&destino, &generados)?;
    if borrados > 0 {
        println!("cargo::warning=frontend: {borrados} ficheros obsoletos eliminados");
    }

    println!("cargo::warning=frontend: {procesados} ficheros generados");
    Ok(())
}

/// Borra de `dist/` lo que ya no tiene origen.
///
/// Sin esto, renombrar o eliminar un módulo deja su `.js` viejo ahí para
/// siempre. No se carga —nadie lo importa— pero al depurar aparece en la lista
/// de ficheros y en las búsquedas, y termina costando tiempo a alguien que cree
/// estar mirando código vivo.
fn podar(destino: &Path, generados: &std::collections::HashSet<PathBuf>) -> Resultado<u32> {
    let mut borrados = 0_u32;
    let mut pendientes = vec![destino.to_path_buf()];

    while let Some(dir) = pendientes.pop() {
        let Ok(entradas) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entrada in entradas.flatten() {
            let ruta = entrada.path();
            if ruta.is_dir() {
                pendientes.push(ruta);
            } else if !generados.contains(&ruta) {
                std::fs::remove_file(&ruta)?;
                borrados += 1;
            }
        }
    }
    Ok(borrados)
}

fn recorrer(
    base: &Path,
    dir: &Path,
    destino: &Path,
    contador: &mut u32,
    generados: &mut std::collections::HashSet<PathBuf>,
) -> Resultado<()> {
    for entrada in std::fs::read_dir(dir)? {
        let entrada = entrada?;
        let ruta = entrada.path();
        let nombre = entrada.file_name();
        let nombre = nombre.to_string_lossy();

        // No recursar en la propia salida ni en artefactos ajenos.
        if nombre == "dist" || nombre == "node_modules" || nombre.starts_with('.') {
            continue;
        }

        if ruta.is_dir() {
            recorrer(base, &ruta, destino, contador, generados)?;
            continue;
        }

        let relativa = ruta.strip_prefix(base)?;
        match ruta.extension().and_then(|e| e.to_str()) {
            // Los ficheros de declaración no generan código.
            Some("ts") if nombre.ends_with(".d.ts") => {}
            Some("ts") => {
                let salida = destino.join(relativa).with_extension("js");
                transpilar(&ruta, &salida)?;
                generados.insert(salida);
                *contador += 1;
            }
            // Los tsconfig y demás configuración no se copian a la salida.
            Some("json") if nombre.starts_with("tsconfig") => {}
            _ => {
                let salida = destino.join(relativa);
                copiar_si_cambio(&ruta, &salida)?;
                generados.insert(salida);
                *contador += 1;
            }
        }
    }
    Ok(())
}

fn transpilar(origen: &Path, destino: &Path) -> Resultado<()> {
    let fuente = std::fs::read_to_string(origen)?;

    let allocator = Allocator::default();
    let tipo = SourceType::ts();

    let mut resultado = Parser::new(&allocator, &fuente, tipo)
        .with_options(ParseOptions {
            parse_regular_expression: false,
            ..ParseOptions::default()
        })
        .parse();

    if !resultado.diagnostics.is_empty() {
        let detalle = resultado
            .diagnostics
            .iter()
            .map(std::string::ToString::to_string)
            .collect::<Vec<_>>()
            .join("; ");
        return Err(format!("error de sintaxis en {}: {detalle}", origen.display()).into());
    }

    // El transformer es quien borra los tipos. Codegen por sí solo reimprimiría
    // el TypeScript tal cual, anotaciones incluidas.
    let scoping = SemanticBuilder::new()
        .build(&resultado.program)
        .semantic
        .into_scoping();

    // `TransformOptions::default()` no activa ningún downlevel de sintaxis: los
    // targets van a `None`. Eso deja la salida en ESNext, que es exactamente lo
    // que queremos porque WebView2 es Chromium. Lo único que se aplica es el
    // borrado de tipos, que depende del `SourceType`, no de las opciones.
    let transformacion = Transformer::new(&allocator, origen, &TransformOptions::default())
        .build_with_scoping(scoping, &mut resultado.program);

    if !transformacion.diagnostics.is_empty() {
        let detalle = transformacion
            .diagnostics
            .iter()
            .map(std::string::ToString::to_string)
            .collect::<Vec<_>>()
            .join("; ");
        return Err(format!("error al transpilar {}: {detalle}", origen.display()).into());
    }

    // La comprobación va **después** de transformar, sobre el programa ya sin
    // tipos. Antes, `Record` o `Partial` aparecían como nombres sueltos: son
    // tipos de TypeScript, no valores, y no existen en el JavaScript que sale.
    let semantica = SemanticBuilder::new().build(&resultado.program).semantic;
    comprobar_referencias(origen, &semantica)?;

    let js = Codegen::new().build(&resultado.program).code;

    escribir_si_cambio(destino, js.as_bytes())
}

/// Nombres que existen en el navegador y no los declara nadie en el código.
///
/// Es una lista y no una heurística porque el objetivo es que **cualquier otro
/// nombre suelto pare la compilación**. Añadir uno cuando haga falta cuesta una
/// línea; no tenerla cuesta un `ReferenceError` en las manos del usuario.
const GLOBALES: &[&str] = &[
    // Entorno
    "globalThis",
    "undefined",
    "WeakRef",
    "window",
    "document",
    "console",
    "navigator",
    "location",
    "history",
    "performance",
    "requestAnimationFrame",
    "cancelAnimationFrame",
    "setTimeout",
    "clearTimeout",
    "setInterval",
    "clearInterval",
    "queueMicrotask",
    "fetch",
    "matchMedia",
    "getComputedStyle",
    "structuredClone",
    "customElements",
    // Tipos y constructores del lenguaje
    "Object",
    "Array",
    "String",
    "Number",
    "Boolean",
    "BigInt",
    "Symbol",
    "Math",
    "JSON",
    "Date",
    "RegExp",
    "Map",
    "Set",
    "WeakMap",
    "WeakSet",
    "Promise",
    "Proxy",
    "Reflect",
    "Error",
    "TypeError",
    "RangeError",
    "Intl",
    "AbortController",
    "AbortSignal",
    "TextEncoder",
    "TextDecoder",
    "URL",
    "URLSearchParams",
    // `CSS.escape`, para construir selectores a partir de un identificador que
    // puede llevar caracteres que no son válidos sueltos en un selector.
    "CSS",
    "isNaN",
    "isFinite",
    "parseInt",
    "parseFloat",
    "encodeURIComponent",
    "decodeURIComponent",
    // Tipos del DOM que el código nombra en `instanceof` y en genéricos
    "Node",
    "Element",
    "HTMLElement",
    "HTMLInputElement",
    "HTMLButtonElement",
    "HTMLAnchorElement",
    "HTMLSelectElement",
    "HTMLTextAreaElement",
    "HTMLImageElement",
    "HTMLDialogElement",
    "DocumentFragment",
    "DOMRect",
    "Event",
    "CustomEvent",
    "MouseEvent",
    "KeyboardEvent",
    "PointerEvent",
    "DragEvent",
    "DataTransfer",
    "IntersectionObserver",
    "ResizeObserver",
    "MutationObserver",
    "SVGElement",
    "SVGSVGElement",
];

/// Falla si un fichero usa un nombre que nadie declara ni importa.
///
/// ## Por qué esto vive en el build
///
/// No hay comprobador de tipos en la cadena (ADR-019): oxc **borra** los tipos,
/// no los verifica. Eso deja pasar una clase concreta de error que no da la
/// cara hasta que el usuario pulsa el sitio exacto: una variable que se dejó de
/// declarar pero se sigue usando. Pasó de verdad —al quitar el indicador de
/// disponibilidad quedó un `disponibilidad.clear()` huérfano— y el fallo salió
/// al añadir una canción a una playlist, no al compilar.
///
/// El analizador semántico de oxc ya sabe qué referencias quedan sin resolver;
/// solo hacía falta mirarlas. No es comprobación de tipos, pero cubre el error
/// que más caro sale por no tenerla.
fn comprobar_referencias(origen: &Path, semantica: &oxc_semantic::Semantic<'_>) -> Resultado<()> {
    let sueltas: Vec<&str> = semantica
        .scoping()
        .root_unresolved_references()
        .iter()
        .map(|(nombre, _)| nombre.as_str())
        .filter(|n| !GLOBALES.contains(n))
        .collect();

    if sueltas.is_empty() {
        return Ok(());
    }

    Err(format!(
        "{}: nombres sin declarar ni importar: {}. \
         Si alguno es un global legítimo del navegador, añádelo a GLOBALES en frontend_build.rs",
        origen.display(),
        sueltas.join(", ")
    )
    .into())
}

/// Escribe solo si el contenido cambió.
///
/// Evita tocar la marca de tiempo de ficheros idénticos, que dispararía
/// recargas innecesarias del WebView durante el desarrollo.
fn escribir_si_cambio(destino: &Path, contenido: &[u8]) -> Resultado<()> {
    if let Ok(actual) = std::fs::read(destino)
        && actual == contenido
    {
        return Ok(());
    }
    if let Some(padre) = destino.parent() {
        std::fs::create_dir_all(padre)?;
    }
    std::fs::write(destino, contenido)?;
    Ok(())
}

fn copiar_si_cambio(origen: &Path, destino: &Path) -> Resultado<()> {
    let contenido = std::fs::read(origen)?;
    escribir_si_cambio(destino, &contenido)
}

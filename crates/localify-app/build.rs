//! Script de compilación.
//!
//! Hace dos cosas: preparar Tauri y **compilar el frontend**.
//!
//! Lo segundo es lo inusual: no hay Node, ni npm, ni bundler. El TypeScript se
//! transpila con oxc, desde Rust, y se sirve como módulos ES nativos (ADR-019).
//! Así `cargo build` compila el proyecto entero con una sola cadena de
//! herramientas.

mod frontend_build;

fn main() {
    // Debe ir antes de `tauri_build`: este comprueba que `frontendDist` exista.
    if let Err(e) = frontend_build::build() {
        // Un fallo aquí es un error de compilación real: sin frontend no hay
        // aplicación.
        println!("cargo::error=no se pudo compilar el frontend: {e}");
        std::process::exit(1);
    }

    tauri_build::build();
}

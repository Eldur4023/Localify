//! Búsqueda contra MusicBrainz de verdad.
//!
//! Hermano de `localify-ytmusic --example explorar`, y por el mismo motivo: la
//! única forma de saber qué devuelve un catálogo es preguntárselo. Los tests con
//! JSON fijo comprueban el parseo; esto comprueba que lo que llega tiene la
//! forma que el parseo espera.
//!
//! ```text
//! cargo run -p localify-musicbrainz --example buscar -- "casey edwards bury the light"
//! ```

#![allow(
    clippy::print_stdout,
    reason = "es una herramienta de linea de comandos"
)]

use localify_core::ports::metadata_provider::MetadataProvider;
use localify_musicbrainz::MusicBrainzProvider;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let consulta = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "casey edwards bury the light".to_owned());

    let proveedor = MusicBrainzProvider::nuevo()?;
    let inicio = std::time::Instant::now();
    let pagina = proveedor.search_tracks(&consulta, 10, 0).await?;

    println!(
        "«{consulta}» — {} de {} ({} ms)",
        pagina.items.len(),
        pagina.total.unwrap_or(0),
        inicio.elapsed().as_millis()
    );

    for t in &pagina.items {
        let artistas: Vec<&str> = t.artists.iter().map(|a| a.name.as_str()).collect();
        println!("  · {} — {}", t.title, artistas.join(", "));
        println!(
            "    {} s · isrc {:?} · album {:?}",
            t.duration.as_ms() / 1000,
            t.isrc,
            t.album.as_ref().map(|a| &a.title)
        );
        println!("    id {}", t.id.as_str());
    }

    Ok(())
}

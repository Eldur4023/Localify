//! Lee una playlist pública de Spotify sin credenciales.
//!
//! Comprueba contra el servicio real lo que los tests comprueban contra JSON
//! fijo: que la página sigue teniendo la forma que el parseo espera.
//!
//! ```text
//! cargo run -p localify-spotify --example lista_publica -- 00ew3gyVcZCkCJyOW5tSZR
//! ```

#![allow(
    clippy::print_stdout,
    clippy::expect_used,
    reason = "es una herramienta de linea de comandos"
)]

use std::sync::Arc;

use localify_spotify::{TransporteHttp, publica};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let id = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "00ew3gyVcZCkCJyOW5tSZR".to_owned());

    let transporte: Arc<dyn localify_spotify::Transporte> = Arc::new(TransporteHttp::nuevo()?);
    let avisar = |hechas: u32, total: u32| println!("  progreso: {hechas}/{total}");

    let lista = publica::leer(transporte.as_ref(), &id, &avisar).await?;

    println!("\nnombre      : {}", lista.name);
    println!("descripción : {:?}", lista.description);
    println!("portada     : {:?}", lista.cover_url);
    println!("canciones   : {}\n", lista.tracks.len());

    for t in &lista.tracks {
        let artistas: Vec<&str> = t.artists.iter().map(|a| a.name.as_str()).collect();
        println!(
            "  {:>3} s  {} — {}",
            t.duration.as_ms() / 1000,
            t.title,
            artistas.join(", ")
        );
    }

    Ok(())
}

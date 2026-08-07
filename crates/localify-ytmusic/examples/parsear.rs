//! Ejecuta el parser sobre respuestas reales de InnerTube.
//!
//! Los tests unitarios usan fixtures, que comprueban la lógica pero no que la
//! API siga devolviendo lo que se supone. Esto sí: sale a la red y enseña qué
//! sale por el otro lado.
//!
//! ```text
//! cargo run -p localify-ytmusic --example parsear -- "bohemian rhapsody"
//! ```

#![allow(
    clippy::print_stdout,
    reason = "es una herramienta de linea de comandos"
)]

use localify_ytmusic::innertube::{ClienteInnerTube, Filtro, estanterias};
use localify_ytmusic::parseo;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let consulta = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "bohemian rhapsody".to_owned());

    let cliente = ClienteInnerTube::nuevo("es", "ES")?;
    let ahora = chrono::Utc::now();

    println!("── Canciones ──");
    let resp = cliente.buscar(&consulta, Filtro::Canciones).await?;
    let mut totales = (0_u32, 0_u32, 0_u32, 0_u32);
    for (_, elementos) in estanterias(&resp) {
        totales.0 += u32::try_from(elementos.len()).unwrap_or(0);
        for e in elementos {
            match parseo::cancion(e, ahora) {
                Some(t) => {
                    totales.1 += 1;
                    if !t.artists.is_empty() {
                        totales.2 += 1;
                    }
                    if t.album.is_some() {
                        totales.3 += 1;
                    }
                    println!(
                        "  {:<52} {:>7} ms  {:<22} {}",
                        recortar(&t.title, 50),
                        t.duration.as_ms(),
                        recortar(
                            &t.artists.first().map_or("—".into(), |a| a.name.clone()),
                            20
                        ),
                        t.album
                            .as_ref()
                            .map_or("—".into(), |a| recortar(&a.title, 30)),
                    );
                }
                None => println!("  (descartada)"),
            }
        }
    }
    println!(
        "\n  {} elementos → {} pistas · {} con artista · {} con álbum",
        totales.0, totales.1, totales.2, totales.3
    );

    println!("\n── Álbumes ──");
    let resp = cliente.buscar(&consulta, Filtro::Albumes).await?;
    for (_, elementos) in estanterias(&resp) {
        for e in elementos.iter().take(5) {
            match parseo::album(e) {
                Some(a) => println!(
                    "  {:<45} {:?}  {:<20} {}",
                    recortar(&a.title, 43),
                    a.album_type,
                    recortar(
                        &a.artists.first().map_or("—".into(), |x| x.name.clone()),
                        18
                    ),
                    a.release_date
                        .map_or("—".into(), |d| d.format("%Y").to_string()),
                ),
                None => println!("  (descartado)"),
            }
        }
    }

    println!("\n── Artistas ──");
    let resp = cliente.buscar(&consulta, Filtro::Artistas).await?;
    for (_, elementos) in estanterias(&resp) {
        for e in elementos.iter().take(5) {
            match parseo::artista(e) {
                Some(a) => println!("  {:<30} {}", recortar(&a.name, 28), a.id),
                None => println!("  (descartado)"),
            }
        }
    }

    Ok(())
}

fn recortar(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        return s.to_owned();
    }
    format!("{}…", s.chars().take(n - 1).collect::<String>())
}

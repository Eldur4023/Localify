//! Explorador de la respuesta de InnerTube.
//!
//! No es parte de la aplicación: es la herramienta con la que se averigua qué
//! devuelve de verdad cada filtro, en vez de suponerlo. La API no está
//! documentada, así que la única fuente de verdad es lo que llega por el cable.
//!
//! ```text
//! cargo run -p localify-ytmusic --example explorar -- "bohemian rhapsody"
//! cargo run -p localify-ytmusic --example explorar -- "bury the light" 20
//! ```
//!
//! El segundo argumento es cuántos elementos enseñar de cada estantería. Por
//! defecto tres, que basta para ver la forma de la respuesta; con la lista
//! entera se ve **el orden**, que es lo que hay que mirar cuando la queja es
//! "busco esto y no aparece".

#![allow(
    clippy::print_stdout,
    reason = "es una herramienta de linea de comandos"
)]

use localify_ytmusic::innertube::{
    ClienteInnerTube, Filtro, browse_id, columna, estanterias, tramos, video_id,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let consulta = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "bohemian rhapsody".to_owned());

    let cuantos: usize = std::env::args()
        .nth(2)
        .and_then(|n| n.parse().ok())
        .unwrap_or(3);

    // El idioma y el país cambian **mucho** lo que devuelve InnerTube, así que
    // se pueden pasar: es la única forma de comparar el efecto del locale en
    // vez de suponerlo.
    let idioma = std::env::args().nth(3).unwrap_or_else(|| "es".to_owned());
    let pais = std::env::args().nth(4).unwrap_or_else(|| "ES".to_owned());

    let cliente = ClienteInnerTube::nuevo(&idioma, &pais)?;

    for filtro in [
        Filtro::Canciones,
        Filtro::Albumes,
        Filtro::Artistas,
        Filtro::ListasDeReproduccion,
    ] {
        println!("\n══════ {filtro:?} ══════");
        let inicio = std::time::Instant::now();
        let resp = cliente.buscar(&consulta, filtro).await?;
        println!("  ({} ms)", inicio.elapsed().as_millis());

        let shelves = estanterias(&resp);
        if shelves.is_empty() {
            println!("  (sin estanterías)");
            continue;
        }

        for (titulo, elementos) in shelves {
            println!("  estantería «{titulo}» — {} elementos", elementos.len());
            for e in elementos.iter().take(cuantos) {
                println!("    · col0: {:?}", columna(e, 0));
                println!("      col1: {:?}", columna(e, 1));
                println!("      col2: {:?}", columna(e, 2));
                println!("      videoId : {:?}", video_id(e));
                println!("      browseId: {:?}", browse_id(e));
                println!("      tramos col1:");
                for (texto, id) in tramos(e, 1) {
                    println!("         {texto:?} -> {id:?}");
                }
            }
        }
    }

    Ok(())
}

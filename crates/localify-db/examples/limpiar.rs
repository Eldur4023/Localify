//! Quita de la base de datos real la biblioteca sintética de `sembrar`.
//!
//! El sembrador escribe sobre la base de datos del usuario a propósito —medir
//! la aplicación de verdad y no una copia— y eso deja un rastro que no se va
//! solo: decenas de miles de canciones inventadas, con historial y favoritos,
//! que salen en Inicio y en la biblioteca como si fueran música.
//!
//! ```text
//! cargo run -p localify-db --example limpiar          # dice qué hay
//! cargo run -p localify-db --example limpiar -- borrar
//! ```
//!
//! ## Por qué no vale con borrar el fichero
//!
//! `%APPDATA%\Localify\localify.db` guarda también lo descargado de verdad. Sin
//! sus filas, los ficheros de `audio/` quedan huérfanos y el reconciliador los
//! descarta al escanear: reconoce el identificador, pero no encuentra la pista
//! en el catálogo (ver `biblioteca.rs`). Borrar la base de datos entera es
//! perder la música buena para quitar la falsa.
//!
//! ## Cómo se distingue lo sembrado
//!
//! Por la forma del identificador, que `sembrar` construye con un prefijo y
//! dieciocho dígitos: `seed000000000000000042`. No basta con el prefijo —un
//! `videoId` de YouTube podría empezar por `seed`—, así que se exige además la
//! longitud exacta y que el resto sean todo dígitos. Un identificador real que
//! cumpliera las tres cosas a la vez no existe.

#![allow(
    clippy::print_stdout,
    reason = "es una herramienta de linea de comandos"
)]

use localify_db::Pool;

/// Condición que identifica una fila sembrada, dado su prefijo.
///
/// `NOT GLOB '*[^0-9]*'` es "no contiene ningún carácter que no sea un dígito",
/// que es como se dice "todo dígitos" en SQLite sin expresiones regulares.
fn sembrada(prefijo: &str) -> String {
    format!("id LIKE '{prefijo}%' AND length(id) = 22 AND substr(id, 5) NOT GLOB '*[^0-9]*'")
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let borrar = std::env::args().nth(1).is_some_and(|a| a == "borrar");

    let appdata = std::env::var("APPDATA")?;
    let ruta = std::path::Path::new(&appdata)
        .join("Localify")
        .join("localify.db");
    println!("Base de datos: {}\n", ruta.display());

    let pool = Pool::abrir(&ruta)?;

    let resumen = |pool: Pool| async move {
        pool.leer(|c| {
            let uno = |sql: &str| -> rusqlite::Result<i64> { c.query_row(sql, [], |r| r.get(0)) };
            Ok((
                uno("SELECT COUNT(*) FROM tracks")?,
                uno(&format!(
                    "SELECT COUNT(*) FROM tracks WHERE {}",
                    sembrada("seed")
                ))?,
                uno("SELECT COUNT(*) FROM audio_files")?,
            ))
        })
        .await
    };

    let (total, falsas, con_fichero) = resumen(pool.clone()).await?;
    println!("  {total} pistas en el catálogo");
    println!("  {falsas} sembradas");
    println!("  {} de verdad", total - falsas);
    println!("  {con_fichero} con fichero descargado en disco");

    if !borrar {
        println!("\nNada se ha tocado. Para borrarlas: --example limpiar -- borrar");
        return Ok(());
    }

    println!("\nBorrando...");
    pool.escribir(move |tx| {
        // Las claves ajenas van en cascada (ver V1), así que borrar la pista se
        // lleva su historial, sus favoritos, su entrada de playlist y su
        // fichero. Los disparadores de V2 la sacan del índice de búsqueda.
        tx.execute_batch(&format!(
            "DELETE FROM tracks  WHERE {};
             DELETE FROM albums  WHERE {};
             DELETE FROM artists WHERE {};",
            sembrada("seed"),
            sembrada("albm"),
            sembrada("arti"),
        ))?;
        Ok(())
    })
    .await?;

    // El fichero no encoge solo: sin esto las páginas liberadas siguen ahí.
    // El checkpoint va primero, para vaciar antes lo que quedó en el WAL, y
    // ninguna de las dos cosas puede correr dentro de una transacción.
    pool.escribir_sin_transaccion(|c| {
        c.execute_batch("PRAGMA wal_checkpoint(TRUNCATE); VACUUM;")?;
        Ok(())
    })
    .await?;

    let (quedan, _, _) = resumen(pool).await?;
    println!("Listo: quedan {quedan} pistas, todas reales.");
    Ok(())
}

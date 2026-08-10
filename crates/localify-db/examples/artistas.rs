//! Enseña los artistas del catálogo agrupados por nombre.
//!
//! Existe para ver de un vistazo cuándo el mismo artista está guardado varias
//! veces. Pasa porque cada catálogo usa su propio identificador —un canal de
//! YouTube, un UUID de MusicBrainz— y porque importar una playlist de Spotify
//! inventa uno local por cada artista, sin mirar si ya estaba.
//!
//! ```text
//! cargo run -p localify-db --example artistas
//! ```

#![allow(
    clippy::print_stdout,
    clippy::expect_used,
    reason = "es una herramienta de linea de comandos"
)]

use localify_db::Pool;

#[tokio::main]
async fn main() {
    let ruta = std::env::args().nth(1).unwrap_or_else(|| {
        let base = std::env::var("APPDATA").unwrap_or_default();
        format!(r"{base}\Localify\localify.db")
    });

    let pool = Pool::abrir(std::path::Path::new(&ruta)).expect("abre");
    let filas = pool
        .leer(|conn| {
            let mut stmt = conn.prepare(
                "SELECT a.name_norm, a.id, a.name, a.image_url IS NOT NULL,
                        (SELECT COUNT(*) FROM track_artists ta WHERE ta.artist_id = a.id)
                 FROM artists a
                 ORDER BY a.name_norm, a.id",
            )?;
            let filas = stmt
                .query_map([], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, i64>(3)? != 0,
                        r.get::<_, i64>(4)?,
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(filas)
        })
        .await
        .expect("lee");

    let mut anterior = String::new();
    let mut duplicados = 0;
    let mut repetido = false;
    for (norm, id, nombre, con_foto, pistas) in &filas {
        if *norm == anterior {
            if !repetido {
                duplicados += 1;
                repetido = true;
            }
        } else {
            anterior.clone_from(norm);
            repetido = false;
            println!();
        }
        let foto = if *con_foto { "foto" } else { "-   " };
        println!("  {foto}  {pistas:>4} pistas  {id:<26} {nombre}");
    }

    println!("\n{} artistas, {duplicados} nombres repetidos", filas.len());
}

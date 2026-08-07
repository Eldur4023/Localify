//! Por qué una pista no se descarga.
//!
//! `find_best` falla con "sin candidatos" por dos motivos que desde fuera se ven
//! igual: que las consultas no devuelvan nada, o que **todo lo encontrado esté
//! excluido** por intentos anteriores. Esta herramienta distingue los dos casos,
//! que es lo primero que hay que saber para no arreglar el problema equivocado.
//!
//! ```text
//! cargo run -p localify-db --example emparejamientos -- 0578c31a-4ab4-4181-b05d-1a0a62e49bec
//! ```
//!
//! Sin argumento, lista las pistas con más vídeos rechazados: son las que están
//! atrapadas en el círculo de "falla, se excluye, vuelve a fallar".

#![allow(
    clippy::print_stdout,
    clippy::expect_used,
    reason = "es una herramienta de linea de comandos"
)]

use localify_db::Pool;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // La misma ruta que usan `sembrar` y `limpiar`: este crate no depende de
    // `localify-platform`, y para una herramienta de diagnóstico no compensa
    // añadir la dependencia.
    let appdata = std::env::var("APPDATA")?;
    let ruta = std::path::Path::new(&appdata)
        .join("Localify")
        .join("localify.db");
    println!("base de datos: {}", ruta.display());

    let pool = Pool::abrir(&ruta)?;

    match std::env::args().nth(1) {
        Some(pista) => detalle(&pool, &pista).await,
        None => atrapadas(&pool).await,
    }
}

/// Todo lo que se sabe de los intentos de una pista.
async fn detalle(pool: &Pool, pista: &str) -> Result<(), Box<dyn std::error::Error>> {
    let id = pista.to_owned();
    let filas: Vec<(String, String, f64, String, i64)> = pool
        .leer(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT video_id, title, score, confidence, rejected
                 FROM youtube_matches WHERE track_id = ?1
                 ORDER BY rejected, score DESC",
            )?;
            let filas = stmt
                .query_map([&id], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(filas)
        })
        .await?;

    if filas.is_empty() {
        println!("\nsin intentos registrados: nunca se llegó a emparejar");
        // Entonces lo que importa es con qué datos se construyó el plan de
        // consultas: sin título o sin artista, el plan sale vacío o inútil.
        pista_tal_como_esta(pool, pista).await?;
        return Ok(());
    }

    let rechazados = filas.iter().filter(|f| f.4 != 0).count();
    println!("\n{} intentos, {rechazados} rechazados\n", filas.len());
    for (video, titulo, score, confianza, rechazado) in &filas {
        let marca = if *rechazado != 0 {
            "RECHAZADO"
        } else {
            "         "
        };
        println!("  {marca}  {video}  {score:>5.1}  {confianza:<6}  {titulo}");
    }

    if rechazados == filas.len() {
        println!(
            "\nTodos excluidos. `find_best` no puede devolver nada aunque YouTube\n\
             sí tenga el vídeo: está descartando lo que encuentra."
        );
    }
    Ok(())
}

/// Los datos con los que se construye el plan de consultas.
///
/// Es lo que hay que mirar cuando no hay ni un intento: el emparejador se
/// alimenta del título, del artista principal y del ISRC, y si alguno falta el
/// plan sale vacío o busca otra cosa.
async fn pista_tal_como_esta(pool: &Pool, pista: &str) -> Result<(), Box<dyn std::error::Error>> {
    let id = pista.to_owned();
    let fila: Option<(String, String, i64, Option<String>)> = pool
        .leer(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT t.title, t.artist_display, t.duration_ms, t.isrc
                 FROM tracks t WHERE t.id = ?1",
            )?;
            let mut filas =
                stmt.query_map([&id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?;
            Ok(filas.next().transpose()?)
        })
        .await?;

    match fila {
        None => println!("\nLa pista NO está en `tracks`: no hay nada que emparejar."),
        Some((titulo, artistas, ms, isrc)) => {
            println!("\n  título   : {titulo:?}");
            println!("  artistas : {artistas:?}");
            println!("  duración : {ms} ms");
            println!("  isrc     : {isrc:?}");
            if artistas.trim().is_empty() {
                println!(
                    "\n  Sin artista, el plan solo puede buscar por título: es la\n  \
                     diferencia entre encontrar la grabación y encontrar cualquier cosa."
                );
            }
        }
    }
    Ok(())
}

/// Pistas atrapadas en el círculo de exclusiones.
async fn atrapadas(pool: &Pool) -> Result<(), Box<dyn std::error::Error>> {
    let filas: Vec<(String, String, i64, i64)> = pool
        .leer(|conn| {
            let mut stmt = conn.prepare(
                "SELECT m.track_id, COALESCE(t.title, '?'),
                        SUM(m.rejected), COUNT(*)
                 FROM youtube_matches m
                 LEFT JOIN tracks t ON t.id = m.track_id
                 GROUP BY m.track_id
                 HAVING SUM(m.rejected) > 0
                 ORDER BY SUM(m.rejected) DESC
                 LIMIT 20",
            )?;
            let filas = stmt
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(filas)
        })
        .await?;

    if filas.is_empty() {
        println!("\nninguna pista tiene vídeos rechazados");
        return Ok(());
    }

    println!("\nrechazados/total  pista");
    for (id, titulo, rechazados, total) in &filas {
        println!("  {rechazados:>3}/{total:<3}  {id}  {titulo}");
    }
    Ok(())
}

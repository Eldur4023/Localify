//! Verificación de los criterios de rendimiento de la Fase 3.
//!
//! Los objetivos del roadmap son:
//!
//! - 50 000 pistas insertadas en menos de 10 s
//! - búsqueda FTS5 en menos de 30 ms
//! - página 400 de la biblioteca en menos de 15 ms
//!
//! Estos tests están marcados `#[ignore]` porque tardan más que la suite normal
//! y su resultado depende del disco de la máquina. Se ejecutan a propósito:
//!
//! ```text
//! cargo test -p localify-db --release --test rendimiento -- --ignored --nocapture
//! ```
//!
//! **Deben correrse en `--release`**: en `debug` el propio mapeo a entidades
//! domina el tiempo y la medición no dice nada útil sobre SQLite.

// Un benchmark no es código de producción: imprime resultados, convierte tipos
// para formatearlos y falla con `assert!` cuando no se cumple un objetivo. Los
// lints que protegen el código de la aplicación aquí solo estorbarían.
#![allow(
    clippy::print_stdout,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::expect_used,
    clippy::indexing_slicing
)]

use std::time::Instant;

use localify_core::domain::audio::DurationMs;
use localify_core::domain::ids::{AlbumId, ArtistId, TrackId};
use localify_core::domain::track::{AlbumRef, ArtistRef, Track, TrackFilter, TrackSort};
use localify_core::page::{Cursor, PageRequest};
use localify_core::ports::database::{SearchRepository, TrackRepository};
use localify_db::Pool;
use localify_db::repositories::{SqliteSearchRepository, SqliteTrackRepository};

/// Tamaño del corpus. Es el objetivo declarado para "biblioteca grande".
const PISTAS: usize = 50_000;

/// Filas por transacción al poblar. Una transacción por pista multiplicaría por
/// mil el número de fsync; una sola transacción de 50 000 filas dispararía el
/// uso de memoria del WAL.
const LOTE: usize = 1_000;

/// Vocabulario para generar títulos plausibles. Importa que haya repetición de
/// términos: un índice donde cada palabra aparece una vez no se parece en nada
/// a una biblioteca real y haría la búsqueda artificialmente rápida.
const PALABRAS: &[&str] = &[
    "amor", "noche", "fuego", "cielo", "mar", "luz", "sombra", "camino", "tiempo", "sueño",
    "corazon", "viento", "lluvia", "estrella", "silencio", "danza", "verano", "invierno",
    "memoria", "libertad",
];

const ARTISTAS: &[&str] = &[
    "Queen",
    "Radiohead",
    "Bjork",
    "Daft Punk",
    "Portishead",
    "Massive Attack",
    "Air",
    "Boards of Canada",
    "Aphex Twin",
    "Burial",
    "Four Tet",
    "Caribou",
];

fn generar(indice: usize) -> Track {
    let a = PALABRAS[indice % PALABRAS.len()];
    let b = PALABRAS[(indice / PALABRAS.len()) % PALABRAS.len()];
    let artista = ARTISTAS[indice % ARTISTAS.len()];

    Track {
        id: TrackId::from_trusted(format!("bench{indice:017}")),
        title: format!("{a} {b} {indice}"),
        album: Some(AlbumRef {
            id: AlbumId::from_trusted(format!("albm{:018}", indice / 12)),
            title: format!("Album {}", indice / 12),
        }),
        artists: vec![ArtistRef {
            id: ArtistId::from_trusted(format!("arti{:018}", indice % ARTISTAS.len())),
            name: artista.to_owned(),
        }],
        duration: DurationMs::new(120_000 + (indice as u32 % 180_000)),
        track_number: Some((indice % 12 + 1) as u16),
        disc_number: Some(1),
        explicit: indice.is_multiple_of(7),
        isrc: Some(format!("ES{indice:010}")),
        release_date: None,
        popularity: Some((indice % 100) as u8),
        added_at: chrono::Utc::now(),
    }
}

/// Puebla la base de datos y devuelve cuánto tardó.
async fn poblar(repo: &SqliteTrackRepository) -> std::time::Duration {
    let inicio = Instant::now();
    let mut lote = Vec::with_capacity(LOTE);

    for i in 0..PISTAS {
        lote.push(generar(i));
        if lote.len() == LOTE {
            repo.upsert(&lote).await.expect("inserta lote");
            lote.clear();
        }
    }
    if !lote.is_empty() {
        repo.upsert(&lote).await.expect("inserta resto");
    }

    inicio.elapsed()
}

/// Mediana de varias mediciones. Con una sola medida, cualquier hipo del disco
/// o del planificador del sistema invalidaría el resultado.
fn mediana(mut muestras: Vec<u128>) -> u128 {
    muestras.sort_unstable();
    muestras[muestras.len() / 2]
}

#[tokio::test]
#[ignore = "benchmark: ejecutar con --release --ignored"]
async fn criterios_de_rendimiento_de_la_fase_3() {
    let (pool, _guard) = Pool::temporal().expect("abre");
    localify_db::ejecutar_migraciones(&pool)
        .await
        .expect("migra");

    let tracks = SqliteTrackRepository::new(pool.clone());
    let search = SqliteSearchRepository::new(pool.clone());

    // ── Inserción ────────────────────────────────────────────────────────────
    let insercion = poblar(&tracks).await;
    println!("Inserción de {PISTAS} pistas: {} ms", insercion.as_millis());

    let stats = tracks.stats().await.expect("stats");
    assert_eq!(stats.track_count as usize, PISTAS);
    assert!(
        insercion.as_secs() < 10,
        "objetivo: < 10 s; medido: {} s",
        insercion.as_secs_f32()
    );

    // Las estadísticas del planificador importan: sin `optimize`, SQLite sigue
    // creyendo que las tablas están vacías y elige planes malos.
    pool.escribir_sin_transaccion(|conn| {
        conn.execute_batch("PRAGMA optimize;")?;
        Ok(())
    })
    .await
    .expect("optimize");

    // ── Búsqueda FTS5 ────────────────────────────────────────────────────────
    let consultas = ["amor", "noche fuego", "bjor", "estrella silencio", "cie"];
    let mut medidas = Vec::new();

    for consulta in consultas {
        for _ in 0..5 {
            let t = Instant::now();
            let pagina = search
                .search_tracks(consulta, &PageRequest::new(0, 50))
                .await
                .expect("busca");
            medidas.push(t.elapsed().as_micros());
            assert!(!pagina.items.is_empty(), "'{consulta}' no devolvió nada");
        }
    }

    let busqueda_us = mediana(medidas);
    println!("Búsqueda FTS5 (mediana): {busqueda_us} µs");
    assert!(
        busqueda_us < 30_000,
        "objetivo: < 30 ms; medido: {} ms",
        busqueda_us as f64 / 1000.0
    );

    // ── Scroll completo con cursor ───────────────────────────────────────────
    //
    // Recorre las 500 páginas de la biblioteca como lo haría la lista
    // virtualizada. Lo que se comprueba no es solo que cada página sea rápida,
    // sino que **el coste no crece con la profundidad**: es justo lo que
    // distingue el keyset del `OFFSET`, y lo que hace usable el scroll.
    let mut medidas = Vec::new();
    let mut primeras = Vec::new();
    let mut ultimas = Vec::new();
    let mut cursor = None;
    let mut paginas = 0_u32;
    let mut filas_vistas = 0_usize;

    loop {
        let peticion = match &cursor {
            Some(c) => PageRequest::from_cursor(Cursor::clone(c), 100),
            None => PageRequest::new(0, 100),
        };

        let t = Instant::now();
        let pagina = tracks
            .list_rows(&TrackFilter::default(), TrackSort::AddedDesc, &peticion)
            .await
            .expect("lista");
        let us = t.elapsed().as_micros();

        medidas.push(us);
        if paginas < 20 {
            primeras.push(us);
        }
        if paginas >= 480 {
            ultimas.push(us);
        }

        filas_vistas += pagina.items.len();
        paginas += 1;

        match pagina.next_cursor {
            Some(c) => cursor = Some(c),
            None => break,
        }
        assert!(paginas < 1000, "la paginación no termina");
    }

    assert_eq!(
        filas_vistas, PISTAS,
        "el scroll completo debe recorrer toda la biblioteca"
    );

    let pagina_us = mediana(medidas);
    let primeras_us = mediana(primeras);
    let ultimas_us = mediana(ultimas);

    println!("Scroll: {paginas} páginas de 100");
    println!("  página (mediana global): {pagina_us} µs");
    println!("  primeras 20 páginas:     {primeras_us} µs");
    println!("  últimas 20 páginas:      {ultimas_us} µs");

    assert!(
        pagina_us < 15_000,
        "objetivo: < 15 ms por página; medido: {} ms",
        pagina_us as f64 / 1000.0
    );

    // El coste al final no debe dispararse respecto al principio. Se admite un
    // factor 3 de margen para el ruido del sistema; con `OFFSET` la diferencia
    // sería de dos órdenes de magnitud.
    assert!(
        ultimas_us <= primeras_us.max(1) * 3,
        "el coste crece con la profundidad: primeras {primeras_us} µs, últimas {ultimas_us} µs"
    );

    // ── Tamaño en disco ──────────────────────────────────────────────────────
    let bytes = std::fs::metadata(pool.ruta()).map_or(0, |m| m.len());
    println!("Base de datos: {:.1} MB", bytes as f64 / 1_048_576.0);
}

/// Ninguna consulta de las rutas calientes debe recorrer una tabla entera.
///
/// Es el criterio "toda consulta usa índice" del roadmap, comprobado con el
/// propio planificador en lugar de por inspección visual.
#[tokio::test]
#[ignore = "benchmark: ejecutar con --release --ignored"]
async fn las_consultas_calientes_usan_indice() {
    let (pool, _guard) = Pool::temporal().expect("abre");
    localify_db::ejecutar_migraciones(&pool)
        .await
        .expect("migra");

    let tracks = SqliteTrackRepository::new(pool.clone());
    // Bastan unas miles para que el planificador tome decisiones realistas.
    let lote: Vec<Track> = (0..5_000).map(generar).collect();
    for trozo in lote.chunks(LOTE) {
        tracks.upsert(trozo).await.expect("inserta");
    }
    pool.escribir_sin_transaccion(|conn| {
        conn.execute_batch("PRAGMA optimize;")?;
        Ok(())
    })
    .await
    .expect("optimize");

    let consultas: Vec<(&str, &str)> = vec![
        (
            "biblioteca por fecha",
            "SELECT t.id FROM tracks t ORDER BY t.added_at DESC, t.id DESC LIMIT 100",
        ),
        (
            "pistas de un álbum",
            "SELECT t.id FROM tracks t WHERE t.album_id = 'x'
             ORDER BY t.disc_number, t.track_number",
        ),
        (
            "pistas de un artista",
            "SELECT ta.track_id FROM track_artists ta WHERE ta.artist_id = 'x'",
        ),
        (
            "favoritos por fecha",
            "SELECT f.track_id FROM favorites f ORDER BY f.added_at DESC LIMIT 100",
        ),
        (
            "historial reciente",
            "SELECT h.track_id FROM play_history h ORDER BY h.played_at DESC LIMIT 50",
        ),
        (
            "entradas de playlist",
            "SELECT pi.id FROM playlist_items pi WHERE pi.playlist_id = 'x' ORDER BY pi.position",
        ),
        (
            "pista por ISRC",
            "SELECT t.id FROM tracks t WHERE t.isrc = 'ES0000000001'",
        ),
        (
            "metadatos caducados",
            "SELECT t.id FROM tracks t WHERE t.metadata_at < 0 ORDER BY t.metadata_at LIMIT 50",
        ),
    ];

    let planes: Vec<(String, Vec<String>)> = pool
        .leer(move |conn| {
            let mut resultado = Vec::new();
            for (nombre, sql) in consultas {
                let mut stmt = conn.prepare(&format!("EXPLAIN QUERY PLAN {sql}"))?;
                let filas: Vec<String> = stmt
                    .query_map([], |r| r.get::<_, String>(3))?
                    .collect::<Result<Vec<_>, _>>()?;
                resultado.push((nombre.to_owned(), filas));
            }
            Ok(resultado)
        })
        .await
        .expect("planes");

    let mut fallos = Vec::new();
    for (nombre, plan) in &planes {
        println!("── {nombre}");
        for linea in plan {
            println!("   {linea}");
            // "SCAN <tabla>" sin "USING INDEX" es un recorrido completo.
            if linea.starts_with("SCAN") && !linea.contains("USING") {
                fallos.push(format!("{nombre}: {linea}"));
            }
        }
    }

    assert!(
        fallos.is_empty(),
        "estas consultas recorren la tabla entera:\n{}",
        fallos.join("\n")
    );
}

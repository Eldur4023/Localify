//! Puebla la base de datos real de Localify con una biblioteca sintética.
//!
//! Existe para poder medir la interfaz con datos del tamaño que declara el
//! objetivo —50 000 pistas— sin descargar nada. Es una herramienta de
//! desarrollo, no parte de la aplicación: por eso vive en `examples/` y no se
//! compila con el binario.
//!
//! ```text
//! cargo run -p localify-db --example sembrar -- 50000
//! ```
//!
//! Además del catálogo siembra **historial y favoritos**. No es un extra: las
//! secciones de Inicio se omiten cuando no hay datos suficientes (ver
//! `recomendaciones.rs`), así que sin historial esa pantalla está vacía por
//! diseño y no habría manera de mirarla con contenido.
//!
//! **Escribe en la base de datos real del usuario.** Es deliberado: el objetivo
//! es medir la aplicación de verdad, no una copia. Lo que deja **no se va
//! solo**: son pistas normales del catálogo, con su historial y sus favoritos,
//! y salen en Inicio y en la biblioteca como cualquier otra.
//!
//! Para quitarlas, `examples/limpiar.rs`, que borra exactamente estas filas y
//! respeta lo descargado de verdad. Borrar `localify.db` entero también
//! funciona, pero se lleva por delante la música buena.

#![allow(
    clippy::print_stdout,
    reason = "es una herramienta de linea de comandos"
)]

use localify_core::domain::audio::DurationMs;
use localify_core::domain::ids::{AlbumId, ArtistId, TrackId};
use localify_core::domain::track::{AlbumRef, ArtistRef, Track};
use localify_core::ports::database::TrackRepository;
use localify_db::Pool;
use localify_db::repositories::SqliteTrackRepository;

/// Filas por transacción. Una por pista multiplicaría los `fsync`; una sola de
/// 50 000 dispararía la memoria del WAL.
const LOTE: usize = 1_000;

const PALABRAS: &[&str] = &[
    "amor", "noche", "fuego", "cielo", "mar", "luz", "sombra", "camino", "tiempo", "sueño",
    "corazon", "viento", "lluvia", "estrella", "silencio", "danza", "verano", "invierno",
    "memoria", "libertad",
];

const ARTISTAS: &[&str] = &[
    "Queen",
    "Radiohead",
    "Björk",
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

#[allow(
    clippy::cast_possible_truncation,
    reason = "los indices estan acotados por el tamano del corpus"
)]
fn generar(indice: usize) -> Track {
    let a = PALABRAS[indice % PALABRAS.len()];
    let b = PALABRAS[(indice / PALABRAS.len()) % PALABRAS.len()];

    Track {
        id: TrackId::from_trusted(format!("seed{indice:018}")),
        title: format!("{a} {b} {indice}"),
        album: Some(AlbumRef {
            id: AlbumId::from_trusted(format!("albm{:018}", indice / 12)),
            title: format!("Album {}", indice / 12),
        }),
        artists: vec![ArtistRef {
            id: ArtistId::from_trusted(format!("arti{:018}", indice % ARTISTAS.len())),
            name: ARTISTAS[indice % ARTISTAS.len()].to_owned(),
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

/// Escuchas que se simulan. Suficientes para que las secciones de Inicio
/// superen su mínimo sin tardar en escribirse.
const ESCUCHAS: usize = 400;

/// Favoritos que se marcan.
const FAVORITOS: usize = 60;

/// Siembra historial y favoritos sobre un catálogo ya escrito.
///
/// Las escuchas se reparten entre hoy y hace cuatro meses: "lo que más
/// escuchas" mira los últimos 30 días y "Redescubre" pide 90 sin tocar, así que
/// una ventana corta dejaría una de las dos secciones vacía.
async fn sembrar_actividad(pool: &Pool, catalogo: usize) -> Result<(), Box<dyn std::error::Error>> {
    use localify_core::domain::library::PlayHistoryEntry;
    use localify_core::ports::database::{FavoriteRepository, HistoryRepository};
    use localify_db::repositories::{SqliteFavoriteRepository, SqliteHistoryRepository};

    let historial = SqliteHistoryRepository::new(pool.clone());
    let favoritos = SqliteFavoriteRepository::new(pool.clone());

    let ahora = chrono::Utc::now();

    for i in 0..ESCUCHAS.min(catalogo) {
        // Se concentran en las primeras pistas del catálogo para que unas pocas
        // acumulen reproducciones: un reparto uniforme sobre 50 000 daría una
        // escucha por canción y ningún "más escuchado".
        let indice = (i * 7) % catalogo.min(600);
        let dias = i64::try_from(i % 120).unwrap_or(0);

        historial
            .record(&PlayHistoryEntry {
                track_id: TrackId::from_trusted(format!("seed{indice:018}")),
                played_at: ahora - chrono::Duration::days(dias),
                ms_played: 200_000,
                completed: i % 5 != 0,
                context: Some("library".to_owned()),
            })
            .await?;
    }

    for i in 0..FAVORITOS.min(catalogo) {
        favoritos
            .set(&TrackId::from_trusted(format!("seed{:018}", i * 11)), true)
            .await?;
    }

    println!("  {ESCUCHAS} escuchas, {FAVORITOS} favoritos");
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Un argumento que no es un número se rechaza en vez de caer al valor por
    // defecto. Escribir `--help` esperando ayuda y recibir 50 000 canciones
    // sintéticas en la base de datos real no es un uso razonable de un valor
    // por defecto: solo se aplica cuando no se pidió nada.
    let cuantas: usize = match std::env::args().nth(1) {
        None => 50_000,
        Some(arg) => arg.parse().map_err(|_| {
            format!("uso: cargo run -p localify-db --example sembrar -- [cuantas]\n       '{arg}' no es un número")
        })?,
    };

    let appdata = std::env::var("APPDATA")?;
    let ruta = std::path::Path::new(&appdata)
        .join("Localify")
        .join("localify.db");

    println!("Base de datos: {}", ruta.display());
    println!("Sembrando {cuantas} pistas...");

    let pool = Pool::abrir(&ruta)?;
    localify_db::ejecutar_migraciones(&pool).await?;
    let repo = SqliteTrackRepository::new(pool.clone());

    let inicio = std::time::Instant::now();
    let mut lote = Vec::with_capacity(LOTE);
    for i in 0..cuantas {
        lote.push(generar(i));
        if lote.len() == LOTE {
            repo.upsert(&lote).await?;
            lote.clear();
            print!("\r  {} / {cuantas}", i + 1);
        }
    }
    if !lote.is_empty() {
        repo.upsert(&lote).await?;
    }

    sembrar_actividad(&pool, cuantas).await?;

    let stats = repo.stats().await?;
    println!("\rListo en {:.1?}", inicio.elapsed());
    println!(
        "  {} pistas, {} álbumes, {} artistas",
        stats.track_count, stats.album_count, stats.artist_count
    );
    Ok(())
}

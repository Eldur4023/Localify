//! Empareja una pista contra YouTube de verdad, enseñando el porqué.
//!
//! Hermano de `localify-ytmusic --example explorar`: cuando una descarga falla
//! con "sin candidatos", desde fuera no se distingue si el plan salió mal, si
//! las consultas no devolvieron nada o si lo devuelto se descartó al
//! interpretarlo. Esto enseña las tres cosas.
//!
//! ```text
//! cargo run -p localify-ytdlp --example emparejar -- "Bury the Light" "Casey Edwards" 582 JPE102003410
//! ```

#![allow(
    clippy::print_stdout,
    clippy::expect_used,
    reason = "es una herramienta de linea de comandos"
)]

use std::sync::Arc;

use localify_core::domain::audio::DurationMs;
use localify_core::domain::ids::{ArtistId, TrackId};
use localify_core::domain::track::{ArtistRef, Track};
use localify_ytdlp::proceso::{Ejecutor, EjecutorReal};
use localify_ytdlp::search::plan_de_consultas;
use localify_ytdlp::{ClienteYtDlp, search::Consulta};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Las trazas de `localify-ytdlp` son media herramienta: dicen por qué se
    // descartó cada candidato.
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .init();

    let mut args = std::env::args().skip(1);
    let titulo = args.next().unwrap_or_else(|| "Bury the Light".to_owned());
    let artista = args.next().unwrap_or_else(|| "Casey Edwards".to_owned());
    let segundos: u32 = args.next().and_then(|s| s.parse().ok()).unwrap_or(582);
    let isrc = args.next();

    let pista = Track {
        id: TrackId::nuevo_local(),
        title: titulo.clone(),
        album: None,
        artists: vec![ArtistRef {
            id: ArtistId::nuevo_local(),
            name: artista.clone(),
        }],
        duration: DurationMs::from_secs(segundos),
        track_number: None,
        disc_number: None,
        explicit: false,
        isrc,
        release_date: None,
        popularity: None,
        added_at: chrono::Utc::now(),
    };

    let binarios = std::path::Path::new(&std::env::var("APPDATA")?)
        .join("Localify")
        .join("bin");
    let cliente = ClienteYtDlp::nuevo(Arc::new(EjecutorReal::nuevo(binarios)), Arc::default());

    let plan = plan_de_consultas(&pista, None);
    println!("plan: {} consultas\n", plan.len());

    for consulta in &plan {
        let inicio = std::time::Instant::now();
        match cliente.buscar(consulta).await {
            Ok(candidatos) => {
                println!(
                    "  {:<12} {:>5} ms  {} candidatos  «{}»",
                    consulta.origen,
                    inicio.elapsed().as_millis(),
                    candidatos.len(),
                    consulta.texto
                );
                for c in candidatos.iter().take(3) {
                    println!(
                        "      {} · {} s · {:?}",
                        c.video_id,
                        c.duration.as_ms() / 1000,
                        c.channel
                    );
                }
            }
            Err(e) => println!("  {:<12} FALLÓ: {e}", consulta.origen),
        }
    }

    if let Some(ultima) = plan.last() {
        crudo(ultima).await?;
    }

    Ok(())
}

/// La salida sin interpretar de una consulta.
///
/// Distingue los dos casos que desde fuera se ven igual: que yt-dlp no
/// devolviera nada, o que devolviera algo que nosotros descartamos. El segundo
/// es el que costó encontrar —un `duplicate field` tragado por un `.ok()`— y el
/// motivo de que esta herramienta exista.
async fn crudo(ultima: &Consulta) -> Result<(), Box<dyn std::error::Error>> {
    {
        let ejecutor = EjecutorReal::nuevo(
            std::path::Path::new(&std::env::var("APPDATA")?)
                .join("Localify")
                .join("bin"),
        );
        let args: Vec<String> = vec![
            format!("ytsearch10:{}", ultima.texto),
            "--dump-json".into(),
            "--flat-playlist".into(),
            "--no-warnings".into(),
            "--ignore-errors".into(),
            "--playlist-end".into(),
            "10".into(),
        ];
        let salida = ejecutor.ejecutar("yt-dlp", &args).await?;
        println!(
            "\ncrudo de «{}»: codigo={} stdout={} bytes stderr={} bytes",
            ultima.origen,
            salida.codigo,
            salida.stdout.len(),
            salida.stderr.len()
        );
        let lineas: Vec<&str> = salida.stdout.lines().collect();
        println!("  lineas={}", lineas.len());
        if let Some(primera) = lineas.first() {
            println!("  {}", &primera[..primera.len().min(200)]);
            match serde_json::from_str::<serde_json::Value>(primera) {
                Ok(v) => {
                    for campo in ["duration", "view_count", "channel", "uploader"] {
                        println!("    {campo} = {:?}", v.get(campo));
                    }
                }
                Err(e) => println!("    JSON inválido: {e}"),
            }
        }
        for linea in salida.stderr.lines().take(5) {
            println!("  ERR {}", &linea[..linea.len().min(300)]);
        }
    }

    Ok(())
}

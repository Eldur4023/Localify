//! Diagnóstico de Discord Rich Presence.
//!
//! Habla con Discord por la misma tubería que la aplicación, pero **enseña las
//! respuestas** en vez de descartarlas. Existe porque desde dentro de Localify
//! los dos fallos posibles se ven igual —el perfil no cambia— y ninguno deja
//! rastro: una tubería que no contesta y una actividad que Discord rechaza.
//!
//! ```text
//! cargo run -p localify-integrations --example discord -- <client_id>
//! ```

#![allow(
    clippy::print_stdout,
    clippy::print_stderr,
    clippy::expect_used,
    clippy::cast_possible_truncation,
    reason = "es una herramienta de linea de comandos"
)]

use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::windows::named_pipe::ClientOptions;

const ESPERA: Duration = Duration::from_secs(5);

#[tokio::main]
async fn main() {
    let client_id = std::env::args().nth(1).unwrap_or_else(|| {
        eprintln!("uso: cargo run -p localify-integrations --example discord -- <client_id>");
        std::process::exit(2);
    });

    let Some((ruta, mut tuberia)) = abrir() else {
        println!("ninguna tubería de Discord aceptó una conexión");
        return;
    };
    println!("conectado por {ruta}");

    let saludo = serde_json::json!({ "v": 1, "client_id": client_id });
    enviar(&mut tuberia, 0, &saludo).await;
    let Some((op, v)) = recibir(&mut tuberia).await else {
        println!("saludo -> la tubería no contestó en {ESPERA:?}");
        return;
    };
    println!("saludo -> op={op} {v}");

    // Con imagen. Es lo que publica la aplicación desde que se añadió la
    // carátula, y lo primero que hay que descartar.
    let con_imagen = serde_json::json!({
        "type": 2,
        "details": "Prueba",
        "state": "Localify",
        "assets": {
            "large_image": "https://i.scdn.co/image/ab67616d0000b273e8b066f70c206551210d902b",
            "large_text": "Prueba",
        },
        "timestamps": { "start": ahora(), "end": ahora() + 180 },
    });
    publicar(&mut tuberia, "con imagen", Some(con_imagen)).await;

    // Sin imagen. Si esta pasa y la otra no, la carátula es la culpable.
    let sin_imagen = serde_json::json!({
        "type": 2,
        "details": "Prueba",
        "state": "Localify",
        "timestamps": { "start": ahora(), "end": ahora() + 180 },
    });
    publicar(&mut tuberia, "sin imagen", Some(sin_imagen)).await;

    // Con `assets` a nulo. Es lo que manda la aplicación cuando la canción no
    // tiene carátula: `json!` **no omite** una clave cuyo valor es `None`, la
    // escribe como `null`, que no es lo mismo que no mandarla.
    let assets_nulo = serde_json::json!({
        "type": 2,
        "details": "Prueba",
        "state": "Localify",
        "assets": serde_json::Value::Null,
        "timestamps": { "start": ahora(), "end": ahora() + 180 },
    });
    publicar(&mut tuberia, "assets nulo", Some(assets_nulo)).await;

    // Y lo mínimo que Discord admite, para separar "el campo está mal" de "la
    // aplicación no puede publicar nada".
    let minima = serde_json::json!({ "details": "Prueba" });
    publicar(&mut tuberia, "mínima", Some(minima)).await;
}

async fn publicar(
    tuberia: &mut tokio::net::windows::named_pipe::NamedPipeClient,
    etiqueta: &str,
    actividad: Option<serde_json::Value>,
) {
    let orden = serde_json::json!({
        "cmd": "SET_ACTIVITY",
        "args": { "pid": std::process::id(), "activity": actividad },
        "nonce": format!("diag-{etiqueta}"),
    });
    enviar(tuberia, 1, &orden).await;
    match recibir(tuberia).await {
        Some((op, v)) => println!("{etiqueta} -> op={op} {v}"),
        None => println!("{etiqueta} -> sin respuesta en {ESPERA:?}"),
    }
    tokio::time::sleep(Duration::from_secs(2)).await;
}

fn abrir() -> Option<(String, tokio::net::windows::named_pipe::NamedPipeClient)> {
    for n in 0..10 {
        let ruta = format!(r"\\.\pipe\discord-ipc-{n}");
        match ClientOptions::new().open(&ruta) {
            Ok(t) => return Some((ruta, t)),
            Err(e) => println!("  {ruta}: {e}"),
        }
    }
    None
}

async fn enviar(
    tuberia: &mut tokio::net::windows::named_pipe::NamedPipeClient,
    opcode: u32,
    carga: &serde_json::Value,
) {
    let cuerpo = serde_json::to_vec(carga).expect("json válido");
    let mut trama = Vec::with_capacity(8 + cuerpo.len());
    trama.extend_from_slice(&opcode.to_le_bytes());
    trama.extend_from_slice(&(cuerpo.len() as u32).to_le_bytes());
    trama.extend_from_slice(&cuerpo);
    tuberia.write_all(&trama).await.expect("escribe");
    tuberia.flush().await.expect("vacía");
}

async fn recibir(
    tuberia: &mut tokio::net::windows::named_pipe::NamedPipeClient,
) -> Option<(u32, serde_json::Value)> {
    let leer = async {
        let mut cabecera = [0_u8; 8];
        tuberia.read_exact(&mut cabecera).await.ok()?;
        let opcode = u32::from_le_bytes([cabecera[0], cabecera[1], cabecera[2], cabecera[3]]);
        let largo = u32::from_le_bytes([cabecera[4], cabecera[5], cabecera[6], cabecera[7]]);
        let mut cuerpo = vec![0_u8; largo as usize];
        tuberia.read_exact(&mut cuerpo).await.ok()?;
        Some((
            opcode,
            serde_json::from_slice(&cuerpo).unwrap_or(serde_json::Value::Null),
        ))
    };
    tokio::time::timeout(ESPERA, leer).await.ok().flatten()
}

fn ahora() -> i64 {
    chrono::Utc::now().timestamp()
}

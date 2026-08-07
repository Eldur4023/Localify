//! Comprobación contra LRCLIB de verdad.
//!
//! Marcados `#[ignore]` porque salen a la red: en CI serían intermitentes por
//! razones que no tienen nada que ver con el código —un corte de red, un
//! mantenimiento del servicio— y un test que falla por motivos ajenos enseña a
//! ignorar los fallos.
//!
//! Lo que verifican no lo puede verificar un doble: que la forma real de la
//! respuesta sigue siendo la que espera el deserializador, y que la letra que
//! llega se analiza a líneas sincronizadas.
//!
//! ```text
//! cargo test -p localify-integrations --test lrclib_real -- --ignored --nocapture
//! ```

#![allow(clippy::expect_used, clippy::print_stdout)]

use std::time::Duration;

use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Respuesta {
    #[serde(default)]
    synced_lyrics: Option<String>,
}

fn cliente() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent("Localify/0.1.0 (test)")
        .timeout(Duration::from_secs(10))
        .build()
        .expect("cliente")
}

#[tokio::test]
#[ignore = "sale a la red: ejecutar con --ignored"]
async fn una_cancion_conocida_devuelve_letra_sincronizada() {
    let http = cliente();
    let resp = http
        .get("https://lrclib.net/api/get")
        .query(&[
            ("artist_name", "Radiohead"),
            ("track_name", "Creep"),
            ("album_name", "Pablo Honey"),
            ("duration", "238"),
        ])
        .send()
        .await
        .expect("responde");

    assert!(resp.status().is_success(), "estado: {}", resp.status());
    let cuerpo: Respuesta = resp.json().await.expect("json con la forma esperada");

    let sincronizada = cuerpo
        .synced_lyrics
        .as_deref()
        .expect("LRCLIB tiene esta canción sincronizada");
    let lineas = localify_integrations::lrc::analizar(sincronizada)
        .expect("el LRC real se analiza a líneas");

    println!("{} líneas, primera: {:?}", lineas.len(), lineas.first());
    assert!(
        lineas.len() > 10,
        "una canción entera tiene más de 10 líneas"
    );
    // Las marcas van en orden y la primera no puede estar en el segundo cero
    // exacto salvo casualidad: si lo estuviera, el análisis habría fallado.
    assert!(
        lineas
            .windows(2)
            .all(|p| p[0].at.as_ms() <= p[1].at.as_ms())
    );
}

#[tokio::test]
#[ignore = "sale a la red: ejecutar con --ignored"]
async fn una_cancion_inventada_devuelve_404() {
    // Es el caso mayoritario en una biblioteca real, y el que justifica la
    // caché negativa: sin ella, cada reproducción de una canción sin letra
    // sería una petición de red para recibir siempre esto.
    let http = cliente();
    let resp = http
        .get("https://lrclib.net/api/get")
        .query(&[
            ("artist_name", "Artista Que No Existe 9f3a"),
            ("track_name", "Cancion Inventada 9f3a"),
            ("duration", "180"),
        ])
        .send()
        .await
        .expect("responde");

    assert_eq!(resp.status(), reqwest::StatusCode::NOT_FOUND);
}

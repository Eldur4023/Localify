//! Comprobación contra la API de GitHub de verdad.
//!
//! Marcado `#[ignore]` por el mismo motivo que `lrclib_real.rs`: sale a la
//! red, y un fallo de GitHub o de la conexión no dice nada sobre si el código
//! está bien.
//!
//! Lo que verifica no lo puede verificar un doble: que `/releases/latest`
//! sigue teniendo la forma que espera el deserializador de `autoupdate`.
//!
//! ```text
//! cargo test -p localify-integrations --test github_real -- --ignored --nocapture
//! ```

#![allow(clippy::expect_used, clippy::print_stdout)]

use localify_integrations::autoupdate;

#[tokio::test]
#[ignore = "sale a la red: ejecutar con --ignored"]
async fn el_ultimo_release_del_repo_se_analiza_si_existe() {
    let http = autoupdate::cliente().expect("cliente");

    // "0.0.0" nunca es más nuevo que nada publicado: si el repositorio tiene
    // algún release, esto tiene que encontrarlo. Pero el repositorio puede
    // legítimamente no tener ninguno todavía —GitHub devuelve 404 en ese
    // caso—, y `comprobar` debe tratarlo como "no hay nada nuevo", no como un
    // fallo. Por eso el test no exige que aparezca una actualización: solo
    // que, si aparece, tenga la forma correcta.
    let resultado = autoupdate::comprobar(&http, "0.0.0").await;
    println!("{resultado:?}");

    if let Some(actualizacion) = resultado {
        assert!(actualizacion.url.starts_with("https://github.com/"));
    }
}

//! Enseña los ajustes guardados, tal cual están en la base de datos.
//!
//! Cuando una integración "no funciona", lo primero que hay que separar es si
//! está apagada o si está encendida y falla. Desde la interfaz no se distingue:
//! una casilla marcada en pantalla y un `false` en disco se ven igual si el
//! guardado no llegó a ocurrir.
//!
//! ```text
//! cargo run -p localify-db --example ajustes
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
    println!("base de datos: {ruta}\n");

    let pool = Pool::abrir(std::path::Path::new(&ruta)).expect("abre");
    let filas = pool
        .leer(|conn| {
            let mut stmt = conn.prepare("SELECT key, value FROM settings ORDER BY key")?;
            let filas = stmt
                .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(filas)
        })
        .await
        .expect("lee");

    if filas.is_empty() {
        println!("(no hay ninguna clave guardada: todo son valores por defecto)");
    }
    for (clave, valor) in filas {
        println!("── {clave}\n{valor}\n");
    }
}

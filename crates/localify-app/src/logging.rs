//! Registro de eventos.
//!
//! Salida a fichero rotado por día en `%APPDATA%/Localify/logs/`, más consola
//! en compilaciones de depuración.
//!
//! Regla estricta: **nunca se loguean credenciales, tokens ni URLs firmadas.**
//! El `client_secret` de Spotify no debe aparecer jamás en un fichero de log,
//! y hay un test en la Fase 5 que lo verifica.

use std::path::Path;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, fmt};

/// Filtro por defecto: informativo para nuestro código, silencioso para las
/// dependencias, que en `debug` inundarían la salida.
const FILTRO_POR_DEFECTO: &str = "localify=debug,info";

/// Inicializa el registro.
///
/// Devuelve un guard que **debe mantenerse vivo** durante toda la ejecución: al
/// soltarse, vuelca lo que quede pendiente en el buffer. Perderlo significaría
/// perder precisamente las últimas líneas, que son las que importan cuando algo
/// falla al cerrar.
#[must_use]
pub fn init(logs_dir: &Path) -> Option<WorkerGuard> {
    if let Err(e) = std::fs::create_dir_all(logs_dir) {
        // Todavía no hay logging, así que no hay dónde registrar esto. La
        // aplicación debe arrancar igualmente: quedarse sin logs es un
        // inconveniente, no un motivo para no funcionar.
        let _ = e;
        return None;
    }

    let appender = tracing_appender::rolling::daily(logs_dir, "localify.log");
    let (writer, guard) = tracing_appender::non_blocking(appender);

    let filtro = EnvFilter::try_from_env("LOCALIFY_LOG")
        .unwrap_or_else(|_| EnvFilter::new(FILTRO_POR_DEFECTO));

    let capa_fichero = fmt::layer()
        .with_writer(writer)
        .with_ansi(false)
        .with_target(true)
        .with_thread_ids(true);

    let registro = tracing_subscriber::registry()
        .with(filtro)
        .with(capa_fichero);

    #[cfg(debug_assertions)]
    let registro = registro.with(fmt::layer().with_ansi(true).with_target(true));

    // `try_init` en lugar de `init`: en los tests puede haber otro suscriptor
    // ya instalado, y eso no debe abortar el proceso.
    if registro.try_init().is_err() {
        return None;
    }

    Some(guard)
}

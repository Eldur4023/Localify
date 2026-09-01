//! Comandos de autoactualización.

use tauri::State;

use crate::context::AppContext;
use crate::dto::common::ApiError;

type Resultado<T> = Result<T, ApiError>;

/// Abre en el navegador la página del release que detectó la comprobación de
/// fondo.
///
/// No acepta una URL desde el frontend: es Rust quien la decide, con lo
/// último que encontró `localify_integrations::autoupdate`. Mismo motivo que
/// `settings_open_external` (ver su comentario): la interfaz nunca debe poder
/// mandar una URL arbitraria al manejador de protocolos del sistema, ni
/// siquiera una que en su día vino de aquí — un WebView comprometido podría
/// haberla cambiado por el camino.
#[tauri::command]
pub async fn updates_open_release_page(ctx: State<'_, AppContext>) -> Resultado<()> {
    let url = ctx
        .actualizacion_disponible
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    let Some(url) = url else {
        return Err(localify_core::error::CoreError::invalid(
            "no hay ninguna actualización pendiente de confirmar",
        )
        .into());
    };
    localify_platform::navegador::abrir(&url)?;
    Ok(())
}

//! Comandos de estadísticas de escucha.

use tauri::State;

use crate::context::AppContext;
use crate::dto::common::ApiError;
use crate::dto::stats::ListeningStatsDto;

type Resultado<T> = Result<T, ApiError>;

/// Cuántas canciones y artistas entran en los `top_*` de la respuesta.
///
/// Fijo y no un parámetro del frontend: la pantalla de Estadísticas siempre
/// pinta el mismo número de filas, y parametrizarlo solo movería esta
/// constante de un fichero a otro.
const TOP_LIMIT: u8 = 10;

#[tauri::command]
pub async fn stats_get(ctx: State<'_, AppContext>) -> Resultado<ListeningStatsDto> {
    Ok(ctx.library.listening_stats(TOP_LIMIT).await?.into())
}

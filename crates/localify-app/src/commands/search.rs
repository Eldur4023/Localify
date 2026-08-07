//! Comandos de búsqueda, inicio, recomendaciones y letras.

use localify_core::domain::ids::TrackId;
use tauri::State;

use crate::context::AppContext;
use crate::dto::catalog::TrackRowDto;
use crate::dto::common::{ApiError, PageRequestDto};
use crate::dto::library::{HomeSectionDto, LyricsDto, SearchResultsDto, ambito_desde_str};

type Resultado<T> = Result<T, ApiError>;

/// Busca.
///
/// **Siempre consulta lo local primero** y devuelve esos resultados de
/// inmediato. La consulta remota, si procede, se lanza en segundo plano y avisa
/// con `searchRemoteReady`.
///
/// Nótese que no existe ningún comando para buscar en YouTube: es una decisión
/// arquitectónica, no un olvido. YouTube es un detalle interno de la capa de
/// descarga y no forma parte de la superficie pública.
#[tauri::command]
pub async fn search_query(
    ctx: State<'_, AppContext>,
    q: String,
    scope: String,
    page: PageRequestDto,
) -> Resultado<SearchResultsDto> {
    let ambito = ambito_desde_str(&scope)?;
    Ok(ctx.search.search(&q, ambito, &page.into()).await?.into())
}

#[tauri::command]
pub async fn search_suggest(
    ctx: State<'_, AppContext>,
    prefix: String,
    limit: u8,
) -> Resultado<Vec<String>> {
    Ok(ctx.search.suggest(&prefix, limit).await?)
}

/// Secciones de Inicio.
///
/// Se generan **solo con datos locales**: artistas, géneros, álbumes, playlists
/// e historial. Nada de esto sale a la red.
#[tauri::command]
pub async fn home_sections(ctx: State<'_, AppContext>) -> Resultado<Vec<HomeSectionDto>> {
    Ok(ctx
        .recommendations
        .home()
        .await?
        .into_iter()
        .map(Into::into)
        .collect())
}

#[tauri::command]
pub async fn reco_similar_to_track(
    ctx: State<'_, AppContext>,
    track_id: String,
    limit: u8,
) -> Resultado<Vec<TrackRowDto>> {
    let id = TrackId::parse(track_id)?;
    Ok(ctx
        .recommendations
        .similar_to_track(&id, limit)
        .await?
        .into_iter()
        .map(Into::into)
        .collect())
}

/// Letra de una pista.
///
/// `null` significa que no existe, y **no es un error**: la interfaz oculta el
/// panel sin decir nada.
#[tauri::command]
pub async fn lyrics_get(
    ctx: State<'_, AppContext>,
    track_id: String,
) -> Resultado<Option<LyricsDto>> {
    let id = TrackId::parse(track_id)?;
    Ok(ctx.lyrics.get(&id).await?.map(Into::into))
}

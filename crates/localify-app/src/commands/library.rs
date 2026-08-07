//! Comandos de biblioteca, álbumes y artistas.

use localify_core::domain::album::AlbumFilter;
use localify_core::domain::ids::{AlbumId, ArtistId, TrackId};
use localify_core::domain::track::TrackFilter;
use tauri::State;

use crate::context::AppContext;
use crate::dto::catalog::{
    AlbumDetailDto, AlbumRowDto, ArtistDetailDto, ArtistRowDto, TrackRowDto,
};
use crate::dto::common::{ApiError, AvailabilityDto, PageDto, PageRequestDto};
use crate::dto::library::{LibraryStatsDto, TrackFilterDto, orden_desde_str};

type Resultado<T> = Result<T, ApiError>;

#[tauri::command]
pub async fn library_tracks(
    ctx: State<'_, AppContext>,
    filter: TrackFilterDto,
    sort: String,
    page: PageRequestDto,
) -> Resultado<PageDto<TrackRowDto>> {
    let filtro: TrackFilter = filter.try_into()?;
    let orden = orden_desde_str(&sort)?;
    let pagina = ctx.library.tracks(&filtro, orden, &page.into()).await?;
    Ok(PageDto::desde(pagina, Into::into))
}

#[tauri::command]
pub async fn library_albums(
    ctx: State<'_, AppContext>,
    page: PageRequestDto,
) -> Resultado<PageDto<AlbumRowDto>> {
    let pagina = ctx
        .library
        .albums(&AlbumFilter::default(), &page.into())
        .await?;
    Ok(PageDto::desde(pagina, Into::into))
}

#[tauri::command]
pub async fn library_artists(
    ctx: State<'_, AppContext>,
    page: PageRequestDto,
) -> Resultado<PageDto<ArtistRowDto>> {
    let pagina = ctx.library.artists(&page.into()).await?;
    Ok(PageDto::desde(pagina, Into::into))
}

#[tauri::command]
pub async fn library_favorites(
    ctx: State<'_, AppContext>,
    page: PageRequestDto,
) -> Resultado<PageDto<TrackRowDto>> {
    let pagina = ctx.library.favorites(&page.into()).await?;
    Ok(PageDto::desde(pagina, Into::into))
}

#[tauri::command]
pub async fn library_set_favorite(
    ctx: State<'_, AppContext>,
    track_id: String,
    enabled: bool,
) -> Resultado<()> {
    let id = TrackId::parse(track_id)?;
    ctx.library.set_favorite(&id, enabled).await?;
    Ok(())
}

#[tauri::command]
pub async fn library_recent(ctx: State<'_, AppContext>, limit: u16) -> Resultado<Vec<TrackRowDto>> {
    Ok(ctx
        .library
        .recent(limit)
        .await?
        .into_iter()
        .map(Into::into)
        .collect())
}

/// Estado de varias pistas de golpe.
///
/// Existe para que la lista virtualizada pida las ~40 filas visibles en **una**
/// llamada. Sin esto habría una petición por fila al hacer scroll, que es la
/// diferencia entre 60 fps y una lista que se atasca.
#[tauri::command]
pub async fn library_availability(
    ctx: State<'_, AppContext>,
    track_ids: Vec<String>,
) -> Resultado<Vec<(String, AvailabilityDto)>> {
    let ids = track_ids
        .into_iter()
        .map(TrackId::parse)
        .collect::<Result<Vec<_>, _>>()?;

    Ok(ctx
        .downloads
        .statuses(&ids)
        .await?
        .into_iter()
        .map(|(id, a)| (id.into_string(), a.into()))
        .collect())
}

#[tauri::command]
pub async fn library_stats(ctx: State<'_, AppContext>) -> Resultado<LibraryStatsDto> {
    Ok(ctx.library.stats().await?.into())
}

/// Reconcilia disco y base de datos. Corre en segundo plano con progreso por
/// eventos; aquí solo se devuelve el identificador para seguirlo.
#[tauri::command]
pub async fn library_rescan(ctx: State<'_, AppContext>) -> Resultado<String> {
    Ok(ctx.library.rescan().await?.to_string())
}

/// Borra el fichero descargado de una pista.
///
/// La pista sigue en el catálogo, en sus playlists y en los favoritos: lo que se
/// va es el audio, y se vuelve a bajar al reproducirla. Es la marcha atrás de un
/// emparejamiento malo, que hasta ahora no la tenía.
#[tauri::command]
pub async fn library_delete_download(
    ctx: State<'_, AppContext>,
    track_id: String,
) -> Resultado<()> {
    let id = TrackId::parse(&track_id)?;
    ctx.library.delete_download(&id).await?;
    Ok(())
}

/// Borra **todo** lo descargado. Devuelve cuántas pistas.
///
/// Se van los ficheros de audio; se quedan el catálogo, las playlists, los
/// favoritos y el historial. Lo que se pierde es tiempo de descarga.
#[tauri::command]
pub async fn library_wipe_downloads(ctx: State<'_, AppContext>) -> Resultado<u32> {
    Ok(ctx.library.wipe_downloads().await?)
}

#[tauri::command]
pub async fn album_detail(
    ctx: State<'_, AppContext>,
    album_id: String,
) -> Resultado<AlbumDetailDto> {
    let id = AlbumId::parse(album_id)?;
    Ok(ctx.library.album_detail(&id).await?.into())
}

#[tauri::command]
pub async fn artist_detail(
    ctx: State<'_, AppContext>,
    artist_id: String,
) -> Resultado<ArtistDetailDto> {
    let id = ArtistId::parse(artist_id)?;
    Ok(ctx.library.artist_detail(&id).await?.into())
}

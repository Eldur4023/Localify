//! Comandos de biblioteca, álbumes y artistas.

use localify_core::domain::album::AlbumFilter;
use localify_core::domain::ids::{AlbumId, ArtistId, TrackId};
use localify_core::domain::track::TrackFilter;
use tauri::State;

use crate::context::AppContext;
use crate::dto::catalog::{
    AlbumDetailDto, AlbumRowDto, ArtistDetailDto, ArtistRowDto, TrackCandidateDto, TrackRowDto,
};
use crate::dto::common::{ApiError, AvailabilityDto, PageDto, PageRequestDto};
use crate::dto::library::{ImportReportDto, LibraryStatsDto, TrackFilterDto, orden_desde_str};

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

/// Vuelve a encolar las descargas que fallaron. Devuelve cuántas.
///
/// Existe porque un fallo de emparejamiento no tenía marcha atrás: la canción se
/// quedaba en `Failed` para siempre y lo único que podía hacer el usuario era
/// borrar una descarga que nunca llegó a existir. El servicio ya sabía
/// reintentar; lo que faltaba era una puerta.
#[tauri::command]
pub async fn library_retry_failed(ctx: State<'_, AppContext>) -> Resultado<u32> {
    Ok(ctx.downloads.retry_failed().await?)
}

/// Borra **todo** lo descargado. Devuelve cuántas pistas.
///
/// Se van los ficheros de audio; se quedan el catálogo, las playlists, los
/// favoritos y el historial. Lo que se pierde es tiempo de descarga.
#[tauri::command]
pub async fn library_wipe_downloads(ctx: State<'_, AppContext>) -> Resultado<u32> {
    Ok(ctx.library.wipe_downloads().await?)
}

/// Abre el selector nativo de ficheros de audio, para importar canciones
/// propias.
///
/// Filtrado a los formatos que la biblioteca sabe reproducir y catalogar
/// (`AudioFormat`): dejar pasar cualquier fichero solo movería el rechazo a
/// más tarde, cuando ya no hay diálogo que lo explique.
///
/// Devuelve una lista vacía si el usuario cancela. Cancelar no es un error.
#[tauri::command]
pub async fn library_pick_import_files(app: tauri::AppHandle) -> Resultado<Vec<String>> {
    use tauri_plugin_dialog::DialogExt;

    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .add_filter(
            "audio",
            &["opus", "webm", "m4a", "mp4", "aac", "mp3", "flac", "ogg", "oga", "wav", "aif", "aiff"],
        )
        .pick_files(move |rutas| {
            let _ = tx.send(rutas);
        });

    let elegidos = rx.await.map_err(|_| {
        localify_core::error::CoreError::internal("el selector de ficheros se cerró sin responder")
    })?;

    Ok(elegidos
        .unwrap_or_default()
        .into_iter()
        .map(|r| r.to_string())
        .collect())
}

/// Importa los ficheros elegidos a la biblioteca, para que convivan con lo
/// descargado.
#[tauri::command]
pub async fn library_import_files(
    ctx: State<'_, AppContext>,
    paths: Vec<String>,
) -> Resultado<ImportReportDto> {
    let rutas = paths.into_iter().map(std::path::PathBuf::from).collect();
    Ok(ctx.library.import_files(rutas).await?.into())
}

/// Borra la pista del catálogo entero: sus playlists, sus favoritos y su
/// historial se van con ella. El frontend debe pedir confirmación antes de
/// llamar a esto — a diferencia de `library_delete_download`, no hay marcha
/// atrás fácil.
#[tauri::command]
pub async fn library_delete_track(
    ctx: State<'_, AppContext>,
    track_id: String,
) -> Resultado<()> {
    let id = TrackId::parse(&track_id)?;
    ctx.library.delete_track(&id).await?;
    Ok(())
}

/// Vuelve una pista a "sin identificar": título del fichero si lo tiene
/// descargado, sin artista ni álbum. El audio no se toca.
#[tauri::command]
pub async fn library_reset_metadata(
    ctx: State<'_, AppContext>,
    track_id: String,
) -> Resultado<()> {
    let id = TrackId::parse(&track_id)?;
    ctx.metadata.reset_metadata(&id).await?;
    Ok(())
}

/// Busca candidatos en el proveedor activo para reasignar metadatos a mano.
///
/// No persiste nada: son candidatos a elegir, no resultados que ya entraron en
/// el catálogo.
#[tauri::command]
pub async fn library_search_candidates(
    ctx: State<'_, AppContext>,
    query: String,
    limit: u8,
) -> Resultado<Vec<TrackCandidateDto>> {
    Ok(ctx
        .metadata
        .search_candidates(&query, limit)
        .await?
        .into_iter()
        .map(Into::into)
        .collect())
}

/// Reasigna los metadatos de una pista al candidato elegido.
#[tauri::command]
pub async fn library_assign_metadata(
    ctx: State<'_, AppContext>,
    track_id: String,
    candidate: TrackCandidateDto,
) -> Resultado<()> {
    let id = TrackId::parse(&track_id)?;
    ctx.metadata.assign_metadata(&id, &candidate.into()).await?;
    Ok(())
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

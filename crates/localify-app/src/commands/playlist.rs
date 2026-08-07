//! Comandos de playlists.

use localify_core::domain::ids::{PlaylistEntryId, PlaylistId, TrackId};
use tauri::State;

use crate::context::AppContext;
use crate::dto::catalog::TrackRowDto;
use crate::dto::common::{ApiError, PageRequestDto};
use crate::dto::library::{PlaylistDetailDto, PlaylistSummaryDto};

type Resultado<T> = Result<T, ApiError>;

fn entrada(id: &str) -> Resultado<PlaylistEntryId> {
    uuid::Uuid::parse_str(id)
        .map(PlaylistEntryId::from_uuid)
        .map_err(|e| {
            localify_core::error::CoreError::invalid(format!("id de entrada inválido: {e}")).into()
        })
}

#[tauri::command]
pub async fn playlist_list(ctx: State<'_, AppContext>) -> Resultado<Vec<PlaylistSummaryDto>> {
    Ok(ctx
        .playlists
        .list()
        .await?
        .into_iter()
        .map(Into::into)
        .collect())
}

#[tauri::command]
pub async fn playlist_create(
    ctx: State<'_, AppContext>,
    name: String,
) -> Resultado<PlaylistSummaryDto> {
    Ok(ctx.playlists.create(&name).await?.into())
}

#[tauri::command]
pub async fn playlist_rename(
    ctx: State<'_, AppContext>,
    playlist_id: String,
    name: String,
) -> Resultado<()> {
    let id = PlaylistId::parse(&playlist_id)?;
    ctx.playlists.rename(&id, &name).await?;
    Ok(())
}

#[tauri::command]
pub async fn playlist_delete(ctx: State<'_, AppContext>, playlist_id: String) -> Resultado<()> {
    let id = PlaylistId::parse(&playlist_id)?;
    ctx.playlists.delete(&id).await?;
    Ok(())
}

#[tauri::command]
pub async fn playlist_detail(
    ctx: State<'_, AppContext>,
    playlist_id: String,
    page: PageRequestDto,
) -> Resultado<PlaylistDetailDto> {
    let id = PlaylistId::parse(&playlist_id)?;
    Ok(ctx.playlists.detail(&id, &page.into()).await?.into())
}

#[tauri::command]
pub async fn playlist_add_tracks(
    ctx: State<'_, AppContext>,
    playlist_id: String,
    track_ids: Vec<String>,
    at_index: Option<usize>,
) -> Resultado<()> {
    let id = PlaylistId::parse(&playlist_id)?;
    let pistas = track_ids
        .into_iter()
        .map(TrackId::parse)
        .collect::<Result<Vec<_>, _>>()?;
    ctx.playlists.add_tracks(&id, &pistas, at_index).await?;
    Ok(())
}

#[tauri::command]
pub async fn playlist_remove_entries(
    ctx: State<'_, AppContext>,
    playlist_id: String,
    entry_ids: Vec<String>,
) -> Resultado<()> {
    let id = PlaylistId::parse(&playlist_id)?;
    let entradas = entry_ids
        .iter()
        .map(|e| entrada(e))
        .collect::<Result<Vec<_>, _>>()?;
    ctx.playlists.remove_entries(&id, &entradas).await?;
    Ok(())
}

/// Reordena una entrada.
///
/// Con claves fraccionarias esto es **un solo `UPDATE`**, sea la playlist de 10
/// pistas o de 5 000. Por eso el frontend puede aplicar el movimiento de forma
/// optimista y revertir solo si esto falla.
#[tauri::command]
pub async fn playlist_reorder(
    ctx: State<'_, AppContext>,
    playlist_id: String,
    entry_id: String,
    to_index: usize,
) -> Resultado<()> {
    let id = PlaylistId::parse(&playlist_id)?;
    ctx.playlists
        .reorder(&id, entrada(&entry_id)?, to_index)
        .await?;
    Ok(())
}

/// Abre el selector de imágenes del sistema.
///
/// Devuelve la ruta elegida, o `None` si se cancela. Es lo **único** que sale
/// de aquí con forma de ruta, y va en sentido contrario al habitual: la elige
/// el usuario en un diálogo nativo y vuelve para que `playlist_set_cover` la
/// copie a la biblioteca. Nada de la biblioteca sale por este camino.
#[tauri::command]
pub async fn playlist_pick_image(app: tauri::AppHandle) -> Resultado<Option<String>> {
    use tauri_plugin_dialog::DialogExt;

    // El diálogo nativo es bloqueante: se pide con callback y se espera al
    // canal, igual que el selector de carpetas de Ajustes.
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .add_filter("Imágenes", &["jpg", "jpeg", "png", "webp"])
        .pick_file(move |ruta| {
            let _ = tx.send(ruta);
        });

    let elegida = rx.await.map_err(|_| {
        localify_core::error::CoreError::internal("el selector de imágenes se cerró sin responder")
    })?;

    Ok(elegida.map(|r| r.to_string()))
}

/// Fija la portada de una playlist copiando la imagen a la biblioteca.
#[tauri::command]
pub async fn playlist_set_cover(
    ctx: State<'_, AppContext>,
    playlist_id: String,
    image_path: String,
) -> Resultado<()> {
    let id = PlaylistId::parse(&playlist_id)?;
    ctx.playlists
        .set_cover(&id, std::path::Path::new(&image_path))
        .await?;
    Ok(())
}

/// Quita la portada propia y devuelve la playlist al mosaico.
#[tauri::command]
pub async fn playlist_clear_cover(
    ctx: State<'_, AppContext>,
    playlist_id: String,
) -> Resultado<()> {
    let id = PlaylistId::parse(&playlist_id)?;
    ctx.playlists.clear_cover(&id).await?;
    Ok(())
}

#[tauri::command]
pub async fn playlist_set_description(
    ctx: State<'_, AppContext>,
    playlist_id: String,
    description: Option<String>,
) -> Resultado<()> {
    let id = PlaylistId::parse(&playlist_id)?;
    ctx.playlists
        .set_description(&id, description.as_deref())
        .await?;
    Ok(())
}

/// Importa una playlist pública de Spotify o de YouTube Music.
///
/// Se llamaba `playlist_import_spotify` y era el nombre equivocado por partida
/// doble: el destino lo decide **la URL** que pegue el usuario, no un catálogo
/// fijo, y YouTube Music también se puede importar.
///
/// **No descarga audio**: sería bajar cientos de canciones que quizá no se
/// escuchen nunca. Las descargas siguen siendo bajo demanda al reproducir.
#[tauri::command]
pub async fn playlist_import(ctx: State<'_, AppContext>, url_or_id: String) -> Resultado<String> {
    Ok(ctx
        .playlists
        .import_from_provider(&url_or_id)
        .await?
        .to_string())
}

#[tauri::command]
pub async fn playlist_suggestions(
    ctx: State<'_, AppContext>,
    playlist_id: String,
    limit: u8,
) -> Resultado<Vec<TrackRowDto>> {
    let id = PlaylistId::parse(&playlist_id)?;
    Ok(ctx
        .playlists
        .suggestions(&id, limit)
        .await?
        .into_iter()
        .map(Into::into)
        .collect())
}

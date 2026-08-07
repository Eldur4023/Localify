//! Comandos del reproductor y de la cola.

use localify_core::domain::audio::{DurationMs, Volume};
use localify_core::domain::ids::{QueueEntryId, TrackId};
use tauri::State;

use crate::context::AppContext;
use crate::dto::common::ApiError;
use crate::dto::player::{
    PlaybackContextDto, PlayerStateDto, PositionDto, QueueSnapshotDto, repeticion_desde_str,
};

type Resultado<T> = Result<T, ApiError>;

/// Convierte identificadores del cliente, que no son de fiar.
fn pistas(ids: Vec<String>) -> Resultado<Vec<TrackId>> {
    ids.into_iter()
        .map(TrackId::parse)
        .collect::<Result<Vec<_>, _>>()
        .map_err(Into::into)
}

fn entrada(id: &str) -> Resultado<QueueEntryId> {
    uuid::Uuid::parse_str(id)
        .map(QueueEntryId::from_uuid)
        .map_err(|e| {
            localify_core::error::CoreError::invalid(format!("id de entrada inválido: {e}")).into()
        })
}

// ─────────────────────────────────────────────────────────────────────────────
// Reproductor
// ─────────────────────────────────────────────────────────────────────────────

/// Punto de entrada principal de la aplicación.
///
/// Si la pista no está en local, la descarga arranca sola y la reproducción
/// empieza en cuanto hay buffer. El usuario no ve ninguna de las dos cosas.
#[tauri::command]
pub async fn player_play_track(
    ctx: State<'_, AppContext>,
    track_id: String,
    context: PlaybackContextDto,
) -> Resultado<PlayerStateDto> {
    let id = TrackId::parse(track_id)?;
    let contexto = context.try_into()?;
    Ok(ctx.playback.play_track(&id, contexto).await?.into())
}

#[tauri::command]
pub async fn player_toggle(ctx: State<'_, AppContext>) -> Resultado<PlayerStateDto> {
    Ok(ctx.playback.toggle().await?.into())
}

#[tauri::command]
pub async fn player_pause(ctx: State<'_, AppContext>) -> Resultado<PlayerStateDto> {
    Ok(ctx.playback.pause().await?.into())
}

#[tauri::command]
pub async fn player_resume(ctx: State<'_, AppContext>) -> Resultado<PlayerStateDto> {
    Ok(ctx.playback.resume().await?.into())
}

#[tauri::command]
pub async fn player_next(ctx: State<'_, AppContext>) -> Resultado<PlayerStateDto> {
    Ok(ctx.playback.next().await?.into())
}

/// Anterior. La regla de los tres segundos vive en el servicio, no aquí.
#[tauri::command]
pub async fn player_previous(ctx: State<'_, AppContext>) -> Resultado<PlayerStateDto> {
    Ok(ctx.playback.previous().await?.into())
}

#[tauri::command]
pub async fn player_seek(
    ctx: State<'_, AppContext>,
    position_ms: u32,
) -> Resultado<PlayerStateDto> {
    Ok(ctx
        .playback
        .seek(DurationMs::new(position_ms))
        .await?
        .into())
}

#[tauri::command]
pub async fn player_set_volume(
    ctx: State<'_, AppContext>,
    volume: f32,
) -> Resultado<PlayerStateDto> {
    // `Volume::new` acota en lugar de fallar: un volumen fuera de rango es un
    // error de cliente sin consecuencias, y silenciar la app por ello sería
    // peor que corregirlo.
    Ok(ctx.playback.set_volume(Volume::new(volume)).await?.into())
}

#[tauri::command]
pub async fn player_set_repeat(
    ctx: State<'_, AppContext>,
    mode: String,
) -> Resultado<PlayerStateDto> {
    Ok(ctx
        .playback
        .set_repeat(repeticion_desde_str(&mode)?)
        .await?
        .into())
}

#[tauri::command]
pub async fn player_set_shuffle(
    ctx: State<'_, AppContext>,
    enabled: bool,
) -> Resultado<PlayerStateDto> {
    Ok(ctx.playback.set_shuffle(enabled).await?.into())
}

/// Estado completo. **Comando de resincronización** cuando el frontend pierde
/// eventos.
#[tauri::command]
pub async fn player_get_state(ctx: State<'_, AppContext>) -> Resultado<PlayerStateDto> {
    Ok(ctx.playback.state().await.into())
}

/// Posición y buffer.
///
/// Se sondea a 4 Hz desde el frontend en lugar de emitirse como evento: la
/// posición cambia 44 100 veces por segundo y serializarla por IPC sería
/// absurdo. Esto lee atómicos y responde en microsegundos.
#[tauri::command]
pub async fn player_position(ctx: State<'_, AppContext>) -> Resultado<PositionDto> {
    let (posicion, buffer) = ctx.playback.position();
    Ok(PositionDto {
        position_ms: posicion.as_ms(),
        buffered_ms: buffer.as_ms(),
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// Cola
// ─────────────────────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn queue_get(ctx: State<'_, AppContext>) -> Resultado<QueueSnapshotDto> {
    Ok(ctx.queue.snapshot().await.into())
}

/// "Reproducir a continuación".
#[tauri::command]
pub async fn queue_add_next(
    ctx: State<'_, AppContext>,
    track_ids: Vec<String>,
) -> Resultado<QueueSnapshotDto> {
    ctx.queue.add_next(&pistas(track_ids)?).await?;
    Ok(ctx.queue.snapshot().await.into())
}

/// "Añadir a la cola".
#[tauri::command]
pub async fn queue_add_last(
    ctx: State<'_, AppContext>,
    track_ids: Vec<String>,
) -> Resultado<QueueSnapshotDto> {
    ctx.queue.add_last(&pistas(track_ids)?).await?;
    Ok(ctx.queue.snapshot().await.into())
}

#[tauri::command]
pub async fn queue_remove(
    ctx: State<'_, AppContext>,
    entry_id: String,
) -> Resultado<QueueSnapshotDto> {
    ctx.queue.remove(entrada(&entry_id)?).await?;
    Ok(ctx.queue.snapshot().await.into())
}

#[tauri::command]
pub async fn queue_move(
    ctx: State<'_, AppContext>,
    entry_id: String,
    to_index: usize,
) -> Resultado<QueueSnapshotDto> {
    ctx.queue.move_entry(entrada(&entry_id)?, to_index).await?;
    Ok(ctx.queue.snapshot().await.into())
}

#[tauri::command]
pub async fn queue_clear_user(ctx: State<'_, AppContext>) -> Resultado<QueueSnapshotDto> {
    ctx.queue.clear_user_queue().await?;
    Ok(ctx.queue.snapshot().await.into())
}

#[tauri::command]
pub async fn queue_jump_to(
    ctx: State<'_, AppContext>,
    entry_id: String,
) -> Resultado<PlayerStateDto> {
    Ok(ctx.playback.jump_to(entrada(&entry_id)?).await?.into())
}

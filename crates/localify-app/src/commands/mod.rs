//! Comandos de Tauri: la superficie pública de la aplicación.
//!
//! Cada handler es **deliberadamente trivial**: convierte DTO → dominio,
//! delega en un trait y convierte dominio → DTO. Si alguno contiene una
//! decisión de negocio (un `if` sobre el estado, un orden de operaciones), está
//! mal ubicado y debe bajar a un servicio.
//!
//! Esa disciplina es lo que hace que la API sirva a otros frontends: toda la
//! lógica está por debajo de esta capa, no repartida en ella.

pub mod library;
pub mod player;
pub mod playlist;
pub mod search;
pub mod settings;

/// Registra todos los comandos en el constructor de Tauri.
///
/// Está en un solo sitio para que añadir un comando y olvidarse de registrarlo
/// sea imposible: no compila si el nombre no existe, y la lista es la
/// documentación viva de la superficie de la API.
#[macro_export]
macro_rules! registrar_comandos {
    ($builder:expr) => {
        $builder.invoke_handler(tauri::generate_handler![
            // ── Reproductor ─────────────────────────────────────────────────
            $crate::commands::player::player_play_track,
            $crate::commands::player::player_toggle,
            $crate::commands::player::player_pause,
            $crate::commands::player::player_resume,
            $crate::commands::player::player_next,
            $crate::commands::player::player_previous,
            $crate::commands::player::player_seek,
            $crate::commands::player::player_set_volume,
            $crate::commands::player::player_set_repeat,
            $crate::commands::player::player_set_shuffle,
            $crate::commands::player::player_get_state,
            $crate::commands::player::player_position,
            // ── Cola ────────────────────────────────────────────────────────
            $crate::commands::player::queue_get,
            $crate::commands::player::queue_add_next,
            $crate::commands::player::queue_add_last,
            $crate::commands::player::queue_remove,
            $crate::commands::player::queue_move,
            $crate::commands::player::queue_clear_user,
            $crate::commands::player::queue_jump_to,
            // ── Biblioteca ──────────────────────────────────────────────────
            $crate::commands::library::library_tracks,
            $crate::commands::library::library_albums,
            $crate::commands::library::library_artists,
            $crate::commands::library::library_favorites,
            $crate::commands::library::library_set_favorite,
            $crate::commands::library::library_recent,
            $crate::commands::library::library_availability,
            $crate::commands::library::library_stats,
            $crate::commands::library::library_rescan,
            $crate::commands::library::library_delete_download,
            $crate::commands::library::library_wipe_downloads,
            $crate::commands::library::album_detail,
            $crate::commands::library::artist_detail,
            // ── Búsqueda ────────────────────────────────────────────────────
            $crate::commands::search::search_query,
            $crate::commands::search::search_suggest,
            // ── Playlists ───────────────────────────────────────────────────
            $crate::commands::playlist::playlist_list,
            $crate::commands::playlist::playlist_create,
            $crate::commands::playlist::playlist_rename,
            $crate::commands::playlist::playlist_set_description,
            $crate::commands::playlist::playlist_delete,
            $crate::commands::playlist::playlist_detail,
            $crate::commands::playlist::playlist_add_tracks,
            $crate::commands::playlist::playlist_remove_entries,
            $crate::commands::playlist::playlist_reorder,
            $crate::commands::playlist::playlist_import,
            $crate::commands::playlist::playlist_pick_image,
            $crate::commands::playlist::playlist_set_cover,
            $crate::commands::playlist::playlist_clear_cover,
            $crate::commands::playlist::playlist_suggestions,
            // ── Inicio, letras y ajustes ────────────────────────────────────
            $crate::commands::search::home_sections,
            $crate::commands::search::reco_similar_to_track,
            $crate::commands::search::lyrics_get,
            $crate::commands::settings::settings_get,
            $crate::commands::settings::settings_patch,
            $crate::commands::settings::settings_audio_devices,
            $crate::commands::settings::settings_eq_profiles,
            $crate::commands::settings::settings_set_spotify_credentials,
            $crate::commands::settings::settings_test_spotify,
            $crate::commands::settings::settings_open_external,
            $crate::commands::settings::settings_set_lastfm_credentials,
            $crate::commands::settings::settings_lastfm_begin_auth,
            $crate::commands::settings::settings_lastfm_complete_auth,
            $crate::commands::settings::settings_lastfm_disconnect,
            $crate::commands::settings::settings_lastfm_pending,
            $crate::commands::settings::settings_change_library_path,
            $crate::commands::settings::settings_pick_folder,
            $crate::commands::settings::settings_preview_eq,
            $crate::commands::settings::api_version,
        ])
    };
}

//! Los servicios que quedan cuando falta la pieza de la que dependen.
//!
//! ## Por qué existen
//!
//! Hay dos cosas que pueden faltar al arrancar y ninguna debe cerrar la
//! aplicación:
//!
//! - **La base de datos.** Si no abre o el esquema no es utilizable, cerrarse
//!   dejaría al usuario sin forma de ver qué ha pasado ni de recuperar su
//!   biblioteca. Se arranca, y cada operación dice que no hay biblioteca.
//! - **El dispositivo de audio.** Un portátil con la tarjeta deshabilitada debe
//!   poder abrir su biblioteca, buscar y ordenar playlists. Lo único que no hará
//!   es sonar.
//!
//! ## Por qué no fingen
//!
//! Aquí hubo trece servicios sobre un almacén en memoria con un catálogo de
//! ejemplo —Queen, Radiohead, Björk—. Servían para construir el frontend
//! mientras no existían los proveedores reales, y para eso estaban bien. Cuando
//! esos proveedores llegaron, lo que quedó fue un modo degradado que **enseñaba
//! música inventada como si fuera la del usuario**: quien abría la aplicación
//! con la base de datos rota veía una biblioteca ajena, no un aviso.
//!
//! Un error es más útil que un dato falso. Es la misma regla que ya sigue el
//! resto del proyecto: `a_estado_descarga` falla en vez de degradarse, y la
//! búsqueda sin bloque de datos devuelve error en vez de una lista vacía que
//! parece "esto no tiene canciones".
//!
//! ## Qué hace cada operación
//!
//! Leer devuelve `Storage`, que el frontend traduce y enseña. Escribir, lo
//! mismo: fallar es lo correcto cuando no hay dónde escribir. Lo único que
//! responde de verdad es `SettingsService::get`, porque su firma no admite
//! error y porque los ajustes por defecto son ciertos —es lo que rige— y dejan
//! la pantalla de Ajustes abierta, que es donde está la ruta de la biblioteca.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use localify_core::domain::album::{AlbumDetail, AlbumFilter, AlbumRow};
use localify_core::domain::artist::{ArtistDetail, ArtistRow};
use localify_core::domain::audio::{AudioDevice, DurationMs, EqProfile, Volume};
use localify_core::domain::availability::Availability;
use localify_core::domain::download::Priority;
use localify_core::domain::ids::{
    AlbumId, ArtistId, PlaylistEntryId, PlaylistId, QueueEntryId, TrackId,
};
use localify_core::domain::library::{LibraryStats, ScanReport};
use localify_core::domain::lyrics::Lyrics;
use localify_core::domain::playlist::{PlaylistDetail, PlaylistSummary};
use localify_core::domain::queue::{
    AdvanceReason, PlayStatus, PlaybackContext, PlayerState, QueueSnapshot, RepeatMode,
};
use localify_core::domain::settings::{Settings, SettingsPatch};
use localify_core::domain::track::{TrackFilter, TrackRow, TrackSort};
use localify_core::error::{CoreError, CoreResult};
use localify_core::events::{DomainEvent, EventPublisher, ProviderStatus};
use localify_core::page::{Page, PageRequest};
use localify_core::ports::services::{
    DownloadHandle, DownloadService, HomeSection, LibraryService, LyricsService, MetadataService,
    NotificationService, PlaybackService, PlaylistService, QueueService, RecommendationService,
    SearchResults, SearchScope, SearchService, SettingsService, ToastLevel,
};
use uuid::Uuid;

/// El error de todo lo que necesita la base de datos.
fn sin_biblioteca<T>() -> CoreResult<T> {
    Err(CoreError::storage(
        "la base de datos no se pudo abrir: la aplicacion arranco sin biblioteca",
    ))
}

/// El error de todo lo que necesita la tarjeta de sonido.
fn sin_audio<T>() -> CoreResult<T> {
    Err(CoreError::Audio(
        "no hay dispositivo de audio disponible".into(),
    ))
}

// ─────────────────────────────────────────────────────────────────────────────
// Sin base de datos
// ─────────────────────────────────────────────────────────────────────────────

/// Todos los servicios que dependen de la persistencia, sin ella.
///
/// Lleva la ruta de la biblioteca porque es el único dato cierto que queda y el
/// que el usuario necesita: es lo que la pantalla de Ajustes enseña, y con eso
/// puede ir a mirar qué le ha pasado a la carpeta.
#[derive(Debug, Clone)]
pub struct SinBiblioteca {
    ruta: PathBuf,
}

impl SinBiblioteca {
    #[must_use]
    pub const fn nuevo(ruta: PathBuf) -> Self {
        Self { ruta }
    }
}

#[async_trait]
impl LibraryService for SinBiblioteca {
    async fn tracks(
        &self,
        _filter: &TrackFilter,
        _sort: TrackSort,
        _page: &PageRequest,
    ) -> CoreResult<Page<TrackRow>> {
        sin_biblioteca()
    }
    async fn albums(&self, _f: &AlbumFilter, _p: &PageRequest) -> CoreResult<Page<AlbumRow>> {
        sin_biblioteca()
    }
    async fn artists(&self, _p: &PageRequest) -> CoreResult<Page<ArtistRow>> {
        sin_biblioteca()
    }
    async fn album_detail(&self, _id: &AlbumId) -> CoreResult<AlbumDetail> {
        sin_biblioteca()
    }
    async fn artist_detail(&self, _id: &ArtistId) -> CoreResult<ArtistDetail> {
        sin_biblioteca()
    }
    async fn set_favorite(&self, _id: &TrackId, _enabled: bool) -> CoreResult<()> {
        sin_biblioteca()
    }
    async fn favorites(&self, _p: &PageRequest) -> CoreResult<Page<TrackRow>> {
        sin_biblioteca()
    }
    async fn record_play(&self, _id: &TrackId, _ms: u32, _completed: bool) -> CoreResult<()> {
        sin_biblioteca()
    }
    async fn recent(&self, _limit: u16) -> CoreResult<Vec<TrackRow>> {
        sin_biblioteca()
    }
    async fn stats(&self) -> CoreResult<LibraryStats> {
        sin_biblioteca()
    }
    async fn delete_download(&self, _id: &TrackId) -> CoreResult<()> {
        sin_biblioteca()
    }
    async fn wipe_downloads(&self) -> CoreResult<u32> {
        sin_biblioteca()
    }
    async fn rescan(&self) -> CoreResult<Uuid> {
        sin_biblioteca()
    }
    async fn last_scan_report(&self) -> CoreResult<Option<ScanReport>> {
        sin_biblioteca()
    }
}

#[async_trait]
impl SearchService for SinBiblioteca {
    async fn search(
        &self,
        _query: &str,
        _scope: SearchScope,
        _page: &PageRequest,
    ) -> CoreResult<SearchResults> {
        sin_biblioteca()
    }
    async fn suggest(&self, _prefix: &str, _limit: u8) -> CoreResult<Vec<String>> {
        sin_biblioteca()
    }
}

#[async_trait]
impl PlaylistService for SinBiblioteca {
    async fn list(&self) -> CoreResult<Vec<PlaylistSummary>> {
        sin_biblioteca()
    }
    async fn create(&self, _name: &str) -> CoreResult<PlaylistSummary> {
        sin_biblioteca()
    }
    async fn rename(&self, _id: &PlaylistId, _name: &str) -> CoreResult<()> {
        sin_biblioteca()
    }
    async fn set_description(&self, _id: &PlaylistId, _d: Option<&str>) -> CoreResult<()> {
        sin_biblioteca()
    }
    async fn delete(&self, _id: &PlaylistId) -> CoreResult<()> {
        sin_biblioteca()
    }
    async fn detail(&self, _id: &PlaylistId, _p: &PageRequest) -> CoreResult<PlaylistDetail> {
        sin_biblioteca()
    }
    async fn add_tracks(
        &self,
        _id: &PlaylistId,
        _tracks: &[TrackId],
        _at: Option<usize>,
    ) -> CoreResult<()> {
        sin_biblioteca()
    }
    async fn remove_entries(&self, _id: &PlaylistId, _e: &[PlaylistEntryId]) -> CoreResult<()> {
        sin_biblioteca()
    }
    async fn reorder(
        &self,
        _id: &PlaylistId,
        _entry: PlaylistEntryId,
        _to: usize,
    ) -> CoreResult<()> {
        sin_biblioteca()
    }
    async fn set_cover(&self, _id: &PlaylistId, _image: &Path) -> CoreResult<()> {
        sin_biblioteca()
    }
    async fn clear_cover(&self, _id: &PlaylistId) -> CoreResult<()> {
        sin_biblioteca()
    }
    async fn cover_file(&self, _id: &PlaylistId) -> CoreResult<Option<PathBuf>> {
        sin_biblioteca()
    }
    async fn import_from_provider(&self, _url_or_id: &str) -> CoreResult<Uuid> {
        sin_biblioteca()
    }
    async fn suggestions(&self, _id: &PlaylistId, _limit: u8) -> CoreResult<Vec<TrackRow>> {
        sin_biblioteca()
    }
}

#[async_trait]
impl RecommendationService for SinBiblioteca {
    async fn home(&self) -> CoreResult<Vec<HomeSection>> {
        sin_biblioteca()
    }
    async fn similar_to_track(&self, _id: &TrackId, _limit: u8) -> CoreResult<Vec<TrackRow>> {
        sin_biblioteca()
    }
    async fn for_playlist(&self, _id: &PlaylistId, _limit: u8) -> CoreResult<Vec<TrackRow>> {
        sin_biblioteca()
    }
}

#[async_trait]
impl DownloadService for SinBiblioteca {
    async fn ensure(&self, _track: &TrackId, _priority: Priority) -> CoreResult<DownloadHandle> {
        sin_biblioteca()
    }
    async fn status(&self, _track: &TrackId) -> CoreResult<Availability> {
        sin_biblioteca()
    }
    async fn statuses(&self, _tracks: &[TrackId]) -> CoreResult<Vec<(TrackId, Availability)>> {
        sin_biblioteca()
    }
    async fn retry_failed(&self) -> CoreResult<u32> {
        sin_biblioteca()
    }
}

#[async_trait]
impl QueueService for SinBiblioteca {
    async fn snapshot(&self) -> QueueSnapshot {
        QueueSnapshot::default()
    }
    async fn set_context(&self, _ctx: PlaybackContext, _start: usize) -> CoreResult<()> {
        sin_biblioteca()
    }
    async fn add_next(&self, _tracks: &[TrackId]) -> CoreResult<()> {
        sin_biblioteca()
    }
    async fn add_last(&self, _tracks: &[TrackId]) -> CoreResult<()> {
        sin_biblioteca()
    }
    async fn remove(&self, _entry: QueueEntryId) -> CoreResult<()> {
        sin_biblioteca()
    }
    async fn move_entry(&self, _entry: QueueEntryId, _to: usize) -> CoreResult<()> {
        sin_biblioteca()
    }
    async fn clear_user_queue(&self) -> CoreResult<()> {
        sin_biblioteca()
    }
    async fn advance(&self, _reason: AdvanceReason) -> CoreResult<Option<TrackId>> {
        sin_biblioteca()
    }
    async fn go_back(&self) -> CoreResult<Option<TrackId>> {
        sin_biblioteca()
    }
    async fn peek_next(&self) -> CoreResult<Option<TrackId>> {
        sin_biblioteca()
    }
    async fn set_shuffle(&self, _enabled: bool) -> CoreResult<()> {
        sin_biblioteca()
    }
    async fn set_repeat(&self, _mode: RepeatMode) -> CoreResult<()> {
        sin_biblioteca()
    }
}

#[async_trait]
impl MetadataService for SinBiblioteca {
    async fn ensure_track(&self, _id: &TrackId) -> CoreResult<()> {
        sin_biblioteca()
    }
    async fn ensure_album(&self, _id: &AlbumId) -> CoreResult<()> {
        sin_biblioteca()
    }
    async fn ensure_artist(&self, _id: &ArtistId) -> CoreResult<()> {
        sin_biblioteca()
    }
    async fn ensure_cover(&self, _album: &AlbumId) -> CoreResult<Option<PathBuf>> {
        sin_biblioteca()
    }
    async fn ensure_track_thumbnail(&self, _track: &TrackId) -> CoreResult<Option<PathBuf>> {
        sin_biblioteca()
    }
    async fn ensure_artist_image(&self, _artist: &ArtistId) -> CoreResult<Option<PathBuf>> {
        sin_biblioteca()
    }
    async fn refresh_stale(&self, _limit: u32) -> CoreResult<u32> {
        sin_biblioteca()
    }
}

#[async_trait]
impl SettingsService for SinBiblioteca {
    /// Los ajustes por defecto sobre la ruta real, que es exactamente lo que
    /// rige sin base de datos. La firma no admite error, y devolverlos deja
    /// abierta la pantalla de Ajustes: es donde se ve dónde debería estar la
    /// biblioteca.
    async fn get(&self) -> Settings {
        Settings::por_defecto_en(self.ruta.clone())
    }
    async fn patch(&self, _patch: SettingsPatch) -> CoreResult<Settings> {
        sin_biblioteca()
    }
    async fn set_spotify_credentials(
        &self,
        _id: &str,
        _secret: &str,
    ) -> CoreResult<ProviderStatus> {
        sin_biblioteca()
    }
    async fn test_spotify(&self) -> CoreResult<ProviderStatus> {
        sin_biblioteca()
    }
    async fn set_lastfm_session(&self, _user: Option<String>) -> CoreResult<Settings> {
        sin_biblioteca()
    }
    async fn change_library_path(&self, _path: &Path, _mover: bool) -> CoreResult<Uuid> {
        sin_biblioteca()
    }
    async fn audio_devices(&self) -> CoreResult<Vec<AudioDevice>> {
        sin_biblioteca()
    }
    async fn eq_profiles(&self) -> CoreResult<Vec<EqProfile>> {
        sin_biblioteca()
    }
    async fn preview_eq(&self, _profile: &EqProfile) -> CoreResult<()> {
        sin_biblioteca()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Sin tarjeta de sonido
// ─────────────────────────────────────────────────────────────────────────────

/// El reproductor cuando no hay dispositivo de audio.
///
/// Antes fingía: marcaba la canción como sonando, publicaba `TrackChanged` y se
/// quedaba con la barra a cero. El usuario pulsaba una canción, la veía puesta
/// en el reproductor y no oía nada, sin ninguna explicación. Ahora falla, y el
/// fallo dice qué pasa.
#[derive(Debug, Clone, Copy, Default)]
pub struct SinAudio;

#[async_trait]
impl PlaybackService for SinAudio {
    async fn play_track(&self, _id: &TrackId, _ctx: PlaybackContext) -> CoreResult<PlayerState> {
        sin_audio()
    }
    async fn toggle(&self) -> CoreResult<PlayerState> {
        sin_audio()
    }
    async fn pause(&self) -> CoreResult<PlayerState> {
        sin_audio()
    }
    async fn resume(&self) -> CoreResult<PlayerState> {
        sin_audio()
    }
    async fn next(&self) -> CoreResult<PlayerState> {
        sin_audio()
    }
    async fn previous(&self) -> CoreResult<PlayerState> {
        sin_audio()
    }
    async fn seek(&self, _position: DurationMs) -> CoreResult<PlayerState> {
        sin_audio()
    }
    async fn set_volume(&self, _volume: Volume) -> CoreResult<PlayerState> {
        sin_audio()
    }
    async fn set_repeat(&self, _mode: RepeatMode) -> CoreResult<PlayerState> {
        sin_audio()
    }
    async fn set_shuffle(&self, _enabled: bool) -> CoreResult<PlayerState> {
        sin_audio()
    }
    async fn jump_to(&self, _entry: QueueEntryId) -> CoreResult<PlayerState> {
        sin_audio()
    }

    /// El estado sí se responde: la interfaz lo pide al arrancar para pintar el
    /// reproductor, y un error ahí dejaría la barra inferior rota en vez de
    /// simplemente parada.
    async fn state(&self) -> PlayerState {
        PlayerState {
            status: PlayStatus::Stopped,
            ..PlayerState::default()
        }
    }

    fn position(&self) -> (DurationMs, DurationMs) {
        (DurationMs::ZERO, DurationMs::ZERO)
    }

    async fn persist_now(&self) -> CoreResult<()> {
        Ok(())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Piezas opcionales
// ─────────────────────────────────────────────────────────────────────────────

/// Sin cliente HTTP no hay letras.
///
/// `Ok(None)` y no un error: que una canción no tenga letra es normal, y la
/// interfaz ya sabe ocultar el panel sin decir nada. Ver [`LyricsService::get`].
#[derive(Debug, Clone, Copy, Default)]
pub struct SinLetras;

#[async_trait]
impl LyricsService for SinLetras {
    async fn get(&self, _track: &TrackId) -> CoreResult<Option<Lyrics>> {
        Ok(None)
    }
}

/// Los avisos de la aplicación, publicados por el bus.
///
/// No es un doble ni un apaño: es **la** implementación. Un aviso in-app es un
/// evento hacia el frontend y nada más. El panel multimedia del sistema lo lleva
/// `localify-app::multimedia`, atado al `HWND` de la ventana, que es algo que
/// esta capa no conoce ni debe conocer.
pub struct AvisosPorBus(pub Arc<dyn EventPublisher>);

impl std::fmt::Debug for AvisosPorBus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AvisosPorBus").finish_non_exhaustive()
    }
}

#[async_trait]
impl NotificationService for AvisosPorBus {
    async fn now_playing(&self, _track: &TrackId) -> CoreResult<()> {
        Ok(())
    }

    async fn playback_status(&self, _playing: bool) -> CoreResult<()> {
        Ok(())
    }

    async fn toast(&self, level: ToastLevel, key: &str, params: &[(String, String)]) {
        self.0.publish(DomainEvent::Toast {
            level: match level {
                ToastLevel::Info => localify_core::events::ToastLevel::Info,
                ToastLevel::Warn => localify_core::events::ToastLevel::Warn,
                ToastLevel::Error => localify_core::events::ToastLevel::Error,
            },
            message_key: key.to_owned(),
            params: params.to_vec(),
        });
    }
}

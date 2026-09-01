//! Puertos de persistencia.
//!
//! `core` define **repositorios**, no una conexión.
//!
//! El motivo es de diseño, no de estilo: un trait `Database` con métodos
//! genéricos (`read<T, F>(&self, f: F)`) no sería apto para `dyn` y no se
//! podría inyectar como `Arc<dyn _>`. Además, exponer "ejecuta esta consulta"
//! filtraría SQLite a la capa de negocio. Con repositorios, `core` describe
//! *qué datos necesita* y `localify-db` decide *cómo* obtenerlos.
//!
//! El manejo de conexiones, el pool y las transacciones son internos de
//! `localify-db` (ver ADR-004).

use async_trait::async_trait;

use crate::domain::album::{Album, AlbumFilter, AlbumRow};
use crate::domain::artist::{Artist, ArtistRow};
use crate::domain::audio::{DurationMs, Volume};
use crate::domain::availability::Availability;
use crate::domain::download::{DownloadJob, MatchResult, YoutubeCandidate};
use crate::domain::ids::{AlbumId, ArtistId, PlaylistEntryId, PlaylistId, TrackId};
use crate::domain::library::{AudioFileRecord, LibraryStats, PlayHistoryEntry, ScanReport};
use crate::domain::lyrics::Lyrics;
use crate::domain::playlist::{Playlist, PlaylistEntry, PlaylistSummary};
use crate::domain::queue::{PlaybackContext, RepeatMode};
use crate::domain::track::{Track, TrackFilter, TrackRow, TrackSort};
use crate::error::CoreResult;
use crate::page::{Page, PageRequest};

/// Operaciones atómicas sobre el catálogo.
#[async_trait]
pub trait TrackRepository: Send + Sync + 'static {
    async fn get(&self, id: &TrackId) -> CoreResult<Option<Track>>;
    async fn get_many(&self, ids: &[TrackId]) -> CoreResult<Vec<Track>>;

    /// Inserta o actualiza pista, álbum y artistas **en una sola transacción**.
    ///
    /// Es el único camino de escritura del catálogo, y por eso la columna
    /// denormalizada `artist_display` no puede desincronizarse (ADR-011).
    async fn upsert(&self, tracks: &[Track]) -> CoreResult<()>;

    async fn list_rows(
        &self,
        filter: &TrackFilter,
        sort: TrackSort,
        page: &PageRequest,
    ) -> CoreResult<Page<TrackRow>>;

    async fn rows_by_ids(&self, ids: &[TrackId]) -> CoreResult<Vec<TrackRow>>;

    /// Pistas cuyos metadatos llevan más de `older_than_secs` sin refrescarse.
    async fn stale(&self, older_than_secs: u64, limit: u32) -> CoreResult<Vec<TrackId>>;

    async fn stats(&self) -> CoreResult<LibraryStats>;
}

#[async_trait]
pub trait AlbumRepository: Send + Sync + 'static {
    async fn get(&self, id: &AlbumId) -> CoreResult<Option<Album>>;
    async fn upsert(&self, albums: &[Album]) -> CoreResult<()>;
    async fn list_rows(
        &self,
        filter: &AlbumFilter,
        page: &PageRequest,
    ) -> CoreResult<Page<AlbumRow>>;
    async fn tracks_of(&self, id: &AlbumId) -> CoreResult<Vec<TrackRow>>;
    async fn set_cover_cached(&self, id: &AlbumId, cached: bool) -> CoreResult<()>;
}

#[async_trait]
pub trait ArtistRepository: Send + Sync + 'static {
    async fn get(&self, id: &ArtistId) -> CoreResult<Option<Artist>>;
    async fn upsert(&self, artists: &[Artist]) -> CoreResult<()>;
    async fn list_rows(&self, page: &PageRequest) -> CoreResult<Page<ArtistRow>>;
    async fn albums_of(&self, id: &ArtistId) -> CoreResult<Vec<AlbumRow>>;
    async fn top_tracks_of(&self, id: &ArtistId, limit: u8) -> CoreResult<Vec<TrackRow>>;
}

/// Ficheros de audio en disco. La existencia de una fila **es** la definición
/// de que la pista está en la biblioteca.
#[async_trait]
pub trait AudioFileRepository: Send + Sync + 'static {
    async fn get(&self, track: &TrackId) -> CoreResult<Option<AudioFileRecord>>;
    async fn availability(&self, tracks: &[TrackId]) -> CoreResult<Vec<(TrackId, Availability)>>;
    async fn insert(&self, record: &AudioFileRecord) -> CoreResult<()>;
    async fn delete(&self, track: &TrackId) -> CoreResult<()>;
    /// Para `rescan`: recorre todos los registros en páginas.
    async fn list_all(&self, page: &PageRequest) -> CoreResult<Page<AudioFileRecord>>;
}

#[async_trait]
pub trait FavoriteRepository: Send + Sync + 'static {
    async fn set(&self, track: &TrackId, enabled: bool) -> CoreResult<()>;
    async fn is_favorite(&self, track: &TrackId) -> CoreResult<bool>;
    async fn list(&self, page: &PageRequest) -> CoreResult<Page<TrackRow>>;
    async fn count(&self) -> CoreResult<u64>;
}

#[async_trait]
pub trait HistoryRepository: Send + Sync + 'static {
    async fn record(&self, entry: &PlayHistoryEntry) -> CoreResult<()>;
    async fn recent_tracks(&self, limit: u16) -> CoreResult<Vec<TrackRow>>;
    async fn play_count(&self, track: &TrackId) -> CoreResult<u32>;
    /// Artistas más escuchados en los últimos `days`, para Inicio.
    async fn top_artists(&self, days: u16, limit: u8) -> CoreResult<Vec<ArtistRow>>;

    /// Canciones más escuchadas en los últimos `days`.
    async fn top_tracks(&self, days: u16, limit: u8) -> CoreResult<Vec<TrackRow>>;

    /// Álbumes más escuchados en los últimos `days`.
    ///
    /// Se cuenta por **canciones distintas** oídas del álbum, no por escuchas
    /// totales: repetir cien veces el single de un disco no significa que el
    /// disco guste, y sin esta distinción un solo éxito arrasaría la sección.
    async fn top_albums(&self, days: u16, limit: u8) -> CoreResult<Vec<AlbumRow>>;

    /// Favoritos sin escuchar desde hace `days`, para "Redescubre".
    async fn rediscover(&self, days: u16, limit: u8) -> CoreResult<Vec<TrackRow>>;

    /// Borra el historial entero. Devuelve cuántas escuchas había.
    ///
    /// Existe porque vaciar la biblioteca sin vaciar esto deja Inicio lleno de
    /// canciones que ya no están: el historial es lo único que alimenta esa
    /// pantalla, y sobrevivir al borrado la convierte en un museo de lo que
    /// hubo.
    async fn clear(&self) -> CoreResult<u32>;
}

#[async_trait]
pub trait PlaylistRepository: Send + Sync + 'static {
    async fn create(&self, playlist: &Playlist) -> CoreResult<()>;
    async fn get(&self, id: &PlaylistId) -> CoreResult<Option<Playlist>>;
    async fn rename(&self, id: &PlaylistId, name: &str) -> CoreResult<()>;
    /// Cambia la descripción. `None` la borra.
    async fn set_description(&self, id: &PlaylistId, description: Option<&str>) -> CoreResult<()>;
    async fn delete(&self, id: &PlaylistId) -> CoreResult<()>;

    /// Fija la portada, **relativa** a la biblioteca (ADR-018).
    ///
    /// `None` la quita y devuelve la playlist al mosaico compuesto con las
    /// portadas de sus primeras pistas.
    async fn set_cover(&self, id: &PlaylistId, rel_path: Option<&str>) -> CoreResult<()>;
    async fn list_summaries(&self) -> CoreResult<Vec<PlaylistSummary>>;

    /// Playlists que más se han **puesto** en los últimos `days`.
    ///
    /// Se mide por el contexto guardado en el historial —desde dónde se lanzó
    /// cada canción—, no por si la playlist contiene canciones que se han oído.
    /// Son cosas distintas: con lo segundo, una canción popular metida en diez
    /// listas las subiría las diez a la vez sin que el usuario haya abierto
    /// ninguna.
    async fn most_played(&self, days: u16, limit: u8) -> CoreResult<Vec<PlaylistSummary>>;
    async fn entries(&self, id: &PlaylistId, page: &PageRequest)
    -> CoreResult<Page<PlaylistEntry>>;

    /// Añade con claves de ordenación ya calculadas por el servicio.
    async fn add_entries(
        &self,
        id: &PlaylistId,
        entries: &[(PlaylistEntryId, TrackId, f64)],
    ) -> CoreResult<()>;

    async fn remove_entries(&self, id: &PlaylistId, entries: &[PlaylistEntryId]) -> CoreResult<()>;

    /// Reordena actualizando **una sola fila** (ADR-009).
    async fn set_position(
        &self,
        id: &PlaylistId,
        entry: PlaylistEntryId,
        position: f64,
    ) -> CoreResult<()>;

    /// Claves de los vecinos de un índice, para calcular el punto medio.
    async fn neighbors(
        &self,
        id: &PlaylistId,
        index: usize,
    ) -> CoreResult<(Option<f64>, Option<f64>)>;

    /// Renumera a enteros cuando los huecos se estrechan demasiado.
    async fn rebalance(&self, id: &PlaylistId) -> CoreResult<()>;
}

/// Caché de emparejamientos con YouTube. Borrarla solo cuesta tiempo.
#[async_trait]
pub trait YoutubeMatchRepository: Send + Sync + 'static {
    async fn best_for(&self, track: &TrackId) -> CoreResult<Option<YoutubeCandidate>>;
    async fn save(&self, result: &MatchResult) -> CoreResult<()>;
    /// Marca un vídeo como incorrecto para que no vuelva a elegirse.
    async fn reject(&self, track: &TrackId, video_id: &str) -> CoreResult<()>;
    async fn rejected_ids(&self, track: &TrackId) -> CoreResult<Vec<String>>;
}

#[async_trait]
pub trait DownloadJobRepository: Send + Sync + 'static {
    async fn upsert(&self, job: &DownloadJob) -> CoreResult<()>;
    async fn get(&self, track: &TrackId) -> CoreResult<Option<DownloadJob>>;
    async fn delete(&self, track: &TrackId) -> CoreResult<()>;
    /// Trabajos que quedaron a medias en la sesión anterior.
    async fn interrupted(&self) -> CoreResult<Vec<DownloadJob>>;
    async fn failed(&self) -> CoreResult<Vec<DownloadJob>>;
}

/// Almacén clave-valor de la configuración.
#[async_trait]
pub trait SettingsRepository: Send + Sync + 'static {
    async fn get_raw(&self, key: &str) -> CoreResult<Option<String>>;
    async fn set_raw(&self, key: &str, value: &str) -> CoreResult<()>;
    async fn get_all(&self) -> CoreResult<Vec<(String, String)>>;
    async fn delete(&self, key: &str) -> CoreResult<()>;
}

/// Caché con caducidad, respaldada en la base de datos.
#[async_trait]
pub trait CacheRepository: Send + Sync + 'static {
    async fn get(&self, namespace: &str, key: &str) -> CoreResult<Option<Vec<u8>>>;
    async fn put(&self, namespace: &str, key: &str, value: &[u8], ttl_secs: u64) -> CoreResult<()>;
    async fn invalidate(&self, namespace: &str, key: &str) -> CoreResult<()>;
    async fn purge_expired(&self) -> CoreResult<u64>;
}

#[async_trait]
pub trait LyricsRepository: Send + Sync + 'static {
    async fn get(&self, track: &TrackId) -> CoreResult<Option<Lyrics>>;
    async fn save(&self, track: &TrackId, lyrics: &Lyrics) -> CoreResult<()>;
    /// Registra que no existe letra, para no volver a preguntar en un tiempo.
    async fn mark_not_found(&self, track: &TrackId) -> CoreResult<()>;
    async fn is_marked_not_found(&self, track: &TrackId) -> CoreResult<bool>;
}

/// Informes del reconciliador de biblioteca.
///
/// Se guardan para poder enseñar en Ajustes qué pasó en el último escaneo sin
/// tener que repetirlo, que en una biblioteca grande cuesta minutos.
#[async_trait]
pub trait ScanReportRepository: Send + Sync + 'static {
    async fn save(&self, report: &ScanReport) -> CoreResult<()>;
    async fn last(&self) -> CoreResult<Option<ScanReport>>;
}

/// Estado del reproductor persistido entre sesiones.
#[async_trait]
pub trait PlayerStateRepository: Send + Sync + 'static {
    async fn load(&self) -> CoreResult<Option<PersistedPlayerState>>;
    async fn save(&self, state: &PersistedPlayerState) -> CoreResult<()>;

    /// Olvida la sesión guardada.
    ///
    /// Tras vaciar la biblioteca, restaurarla dejaría la aplicación abriendo con
    /// una cola de canciones cuyos ficheros acaban de borrarse.
    async fn clear(&self) -> CoreResult<()>;
}

/// Instantánea serializable de la sesión.
///
/// La cola va como un bloque, no normalizada en una tabla: se lee y se escribe
/// **siempre entera**, nunca se consulta por partes, y se actualiza a menudo.
/// Normalizarla solo añadiría escrituras.
///
/// ## Por qué solo identificadores
///
/// Lleva `TrackId` y no `TrackRow`. Los metadatos de una pista pueden haber
/// cambiado entre sesiones —o haberse enriquecido desde Spotify—, así que
/// rehidratarlos desde el catálogo al arrancar es más correcto que restaurar
/// una copia obsoleta. Además mantiene el JSON pequeño.
///
/// El tipo refleja exactamente lo que hay en la tabla. Guardar aquí un
/// `PlayerState` completo obligaría al repositorio a hacer un `JOIN` para
/// rellenar filas que quien restaura va a volver a consultar de todas formas.
#[derive(Debug, Clone, PartialEq)]
pub struct PersistedPlayerState {
    pub track_id: Option<TrackId>,
    pub position: DurationMs,
    pub volume: Volume,
    pub repeat: RepeatMode,
    pub shuffle: bool,
    /// Semilla de la permutación de aleatorio, para reproducirla al arrancar.
    pub shuffle_seed: Option<u64>,
    pub context: Option<PlaybackContext>,
    /// Pistas del contexto, en su orden natural.
    pub context_queue: Vec<TrackId>,
    /// Cola de usuario pendiente.
    pub user_queue: Vec<TrackId>,
    pub queue_index: usize,
}

/// Índice de búsqueda de texto completo (FTS5).
#[async_trait]
pub trait SearchRepository: Send + Sync + 'static {
    /// Búsqueda local. Es la primera parada de toda búsqueda; nunca se salta.
    async fn search_tracks(&self, query: &str, page: &PageRequest) -> CoreResult<Page<TrackRow>>;
    async fn search_albums(&self, query: &str, limit: u8) -> CoreResult<Vec<AlbumRow>>;
    async fn search_artists(&self, query: &str, limit: u8) -> CoreResult<Vec<ArtistRow>>;
    async fn search_playlists(&self, query: &str, limit: u8) -> CoreResult<Vec<PlaylistSummary>>;
}

/// Consultas de similitud para las recomendaciones locales.
///
/// Vive en el puerto de persistencia porque se resuelve íntegramente en SQL:
/// traer 50 000 pistas a memoria para calcular un coseno sería absurdo.
#[async_trait]
pub trait SimilarityRepository: Send + Sync + 'static {
    /// Pistas similares a una dada, con su puntuación.
    async fn similar_to_track(&self, track: &TrackId, limit: u8)
    -> CoreResult<Vec<(TrackId, f32)>>;

    /// Pistas afines al conjunto de una playlist, excluyendo las que ya están.
    async fn similar_to_set(
        &self,
        tracks: &[TrackId],
        limit: u8,
    ) -> CoreResult<Vec<(TrackId, f32)>>;

    /// Pistas del mismo artista o álbum que las escuchadas recientemente.
    async fn because_you_listened(&self, limit: u8) -> CoreResult<Vec<(TrackId, TrackId, f32)>>;

    /// Lo que encaja contigo y **todavía no has puesto nunca**.
    ///
    /// `days` acota qué escuchas cuentan como gusto actual. El resultado excluye
    /// todo lo que aparezca en el historial: es lo que separa una recomendación
    /// de un resumen de lo ya oído.
    async fn discover(&self, days: u16, limit: u8) -> CoreResult<Vec<(TrackId, f32)>>;
}

/// Tamaño de WAL a partir del cual conviene forzar un checkpoint.
///
/// El WAL crece con cada escritura y solo se recorta al integrarse. Con la
/// posición de reproducción guardándose cada cinco segundos, una sesión larga
/// puede acumular cientos de MB si nadie lo comprueba.
///
/// Vive con el puerto y no con el adaptador para que quien decide —la tarea de
/// mantenimiento— pueda leerlo sin conocer SQLite.
pub const WAL_MAXIMO_BYTES: u64 = 64 * 1024 * 1024;

/// Mantenimiento de la base de datos. Todo se ejecuta en segundo plano y nunca
/// en el arranque bloqueante.
#[async_trait]
pub trait MaintenanceRepository: Send + Sync + 'static {
    async fn optimize(&self) -> CoreResult<()>;
    async fn incremental_vacuum(&self) -> CoreResult<()>;
    async fn checkpoint_wal(&self) -> CoreResult<()>;

    /// Tamaño actual del registro de escritura anticipada, en bytes.
    ///
    /// Se consulta para decidir si toca un checkpoint. Forzarlo cada pocos
    /// minutos bloquearía a los escritores para no recortar nada.
    fn wal_bytes(&self) -> u64;

    /// Borra pistas sin fichero, sin playlist, sin favorito, sin historial y
    /// que no estén en la cola guardada.
    async fn purge_orphans(&self, older_than_days: u16) -> CoreResult<u64>;
}

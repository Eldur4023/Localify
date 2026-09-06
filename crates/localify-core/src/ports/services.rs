//! Traits de los servicios de negocio.
//!
//! Es el contrato que consume la capa de comandos de Tauri. Está pensado para
//! servir a **cualquier** frontend: nada aquí presupone un WebView.
//!
//! Los servicios con estado (`Playback`, `Queue`, `Download`) se implementan
//! como actores; lo que se inyecta es un handle clonable que envía mensajes
//! (ADR-008). Desde fuera son indistinguibles de un servicio sin estado, que es
//! justo el objetivo.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use uuid::Uuid;

use crate::domain::album::{AlbumDetail, AlbumFilter, AlbumRow};
use crate::domain::artist::{ArtistDetail, ArtistRow};
use crate::domain::audio::{AudioDevice, DurationMs, EqProfile, Volume};
use crate::domain::availability::Availability;
use crate::domain::download::Priority;
use crate::domain::ids::{AlbumId, ArtistId, PlaylistEntryId, PlaylistId, QueueEntryId, TrackId};
use crate::domain::library::{ImportReport, LibraryStats, ScanReport};
use crate::domain::lyrics::Lyrics;
use crate::domain::playlist::{PlaylistDetail, PlaylistSummary};
use crate::domain::queue::{
    AdvanceReason, PlaybackContext, PlayerState, QueueSnapshot, RepeatMode,
};
use crate::domain::settings::{Settings, SettingsPatch};
use crate::domain::track::{Track, TrackFilter, TrackRow, TrackSort};
use crate::error::CoreResult;
use crate::events::ProviderStatus;
use crate::page::{Page, PageRequest};

// ─────────────────────────────────────────────────────────────────────────────
// Configuración
// ─────────────────────────────────────────────────────────────────────────────

#[async_trait]
pub trait SettingsService: Send + Sync + 'static {
    /// Instantánea completa. Es barata: se sirve de memoria.
    async fn get(&self) -> Settings;

    /// Aplica un cambio parcial.
    ///
    /// Valida **todo** antes de escribir **nada**: un patch inválido devuelve
    /// error sin dejar la configuración a medias.
    async fn patch(&self, patch: SettingsPatch) -> CoreResult<Settings>;

    /// Guarda las credenciales de Spotify. El secreto va al almacén del sistema
    /// y jamás se devuelve.
    async fn set_spotify_credentials(
        &self,
        client_id: &str,
        client_secret: &str,
    ) -> CoreResult<ProviderStatus>;

    /// Comprueba las credenciales contra el proveedor.
    async fn test_spotify(&self) -> CoreResult<ProviderStatus>;

    /// Cambia la carpeta de biblioteca.
    ///
    /// No es un `patch` cualquiera: es una migración con progreso. Devuelve el
    /// identificador para seguirla por eventos.
    async fn change_library_path(&self, path: &Path, move_existing: bool) -> CoreResult<Uuid>;

    async fn audio_devices(&self) -> CoreResult<Vec<AudioDevice>>;
    async fn eq_profiles(&self) -> CoreResult<Vec<EqProfile>>;

    /// Aplica un ecualizador **sin persistirlo**.
    ///
    /// Existe porque escuchar y guardar son cosas distintas. Mientras se
    /// arrastra un deslizador hay que oír el cambio en cada movimiento —un
    /// ecualizador que solo se aplica al soltar es un ecualizador a ciegas—,
    /// pero escribir en disco a ese ritmo serían decenas de transacciones por
    /// segundo. El ajuste se guarda cuando la mano se detiene.
    ///
    /// Lo aplicado se pierde al reiniciar, que es exactamente lo que debe pasar
    /// con algo que el usuario no llegó a confirmar.
    ///
    /// # Errors
    /// Si el perfil tiene ganancias fuera de rango.
    async fn preview_eq(&self, profile: &EqProfile) -> CoreResult<()>;
}

// ─────────────────────────────────────────────────────────────────────────────
// Metadatos y búsqueda
// ─────────────────────────────────────────────────────────────────────────────

#[async_trait]
pub trait MetadataService: Send + Sync + 'static {
    /// Garantiza metadatos completos y frescos en local. Si ya los hay y no han
    /// caducado, no toca la red.
    async fn ensure_track(&self, id: &TrackId) -> CoreResult<()>;
    async fn ensure_album(&self, id: &AlbumId) -> CoreResult<()>;
    async fn ensure_artist(&self, id: &ArtistId) -> CoreResult<()>;

    /// Descarga y cachea la portada en los tres tamaños. Devuelve la ruta de
    /// la mayor.
    async fn ensure_cover(&self, album: &AlbumId) -> CoreResult<Option<PathBuf>>;

    /// Portada de una pista **que no tiene álbum**.
    ///
    /// ## Para qué hace falta
    ///
    /// Casi toda la música lleva su portada por el álbum. Pero hay pistas que
    /// no tienen ninguno: las importadas de una lista pública de Spotify llegan
    /// con título, artista y duración y nada más, porque su página no da el
    /// disco. Sin esto se quedan con el icono genérico para siempre, y una lista
    /// de trece notas musicales iguales no se distingue de un fallo de carga.
    ///
    /// ## De dónde sale
    ///
    /// De la miniatura del vídeo que se emparejó para descargarla. En las
    /// subidas oficiales —los canales `- Topic`, que es lo que el emparejador
    /// prefiere— esa miniatura **es** la carátula del disco.
    ///
    /// Devuelve `None` si la pista tiene álbum —entonces la portada es la suya y
    /// pedirla por aquí sería una segunda fuente para lo mismo—, si aún no se ha
    /// emparejado, o si la miniatura no se pudo traer.
    async fn ensure_track_thumbnail(&self, track: &TrackId) -> CoreResult<Option<PathBuf>>;

    /// Lo mismo para la foto de un artista.
    ///
    /// Va aparte de [`MetadataService::ensure_cover`] y no como un parámetro
    /// porque el camino para conseguir la URL es distinto: la portada de un
    /// álbum se pide con `album()`, la foto de un artista con `artist()`, y el
    /// fichero se cachea en otra carpeta. Un solo método con un enum obligaría
    /// a ramificar entero por dentro sin compartir nada más que el `fetch`.
    async fn ensure_artist_image(&self, artist: &ArtistId) -> CoreResult<Option<PathBuf>>;

    /// Refresca metadatos caducados en segundo plano. Devuelve cuántos.
    async fn refresh_stale(&self, limit: u32) -> CoreResult<u32>;

    /// Busca candidatos en el proveedor activo, para reasignar los metadatos
    /// de una pista a mano.
    ///
    /// A diferencia de la búsqueda normal, **no persiste nada**: son
    /// candidatos a elegir, no resultados que ya entraron en el catálogo.
    /// Solo `assign_metadata` escribe, y solo con el que el usuario elija.
    async fn search_candidates(&self, query: &str, limit: u8) -> CoreResult<Vec<Track>>;

    /// Sobreescribe los metadatos de una pista con los de `candidate`.
    ///
    /// Conserva el identificador de la pista (así no rompe playlists,
    /// favoritos ni historial) y su fecha de alta; todo lo demás —título,
    /// artistas, álbum, ISRC…— viene del candidato elegido. Bloquea la pista
    /// frente al refresco automático (ver
    /// [`crate::ports::database::TrackRepository::stale`]) y
    /// olvida cualquier emparejamiento de YouTube anterior, que ya no
    /// significa nada con la identidad nueva.
    ///
    /// # Errors
    /// Si la pista no existe.
    async fn assign_metadata(&self, id: &TrackId, candidate: &Track) -> CoreResult<()>;

    /// Vuelve una pista a un estado "sin identificar": el título cae al
    /// nombre de su fichero si lo tiene descargado, sin artista ni álbum. El
    /// audio, si lo hay, no se toca.
    ///
    /// Como `assign_metadata`, bloquea frente al refresco automático y olvida
    /// el emparejamiento de YouTube — es el primer paso antes de reasignar a
    /// mano, no un borrado.
    ///
    /// # Errors
    /// Si la pista no existe.
    async fn reset_metadata(&self, id: &TrackId) -> CoreResult<()>;
}

/// Ámbito de una búsqueda.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SearchScope {
    #[default]
    All,
    Tracks,
    Albums,
    Artists,
    Playlists,
}

/// Estado de la mitad remota de una búsqueda.
#[derive(Debug, Clone, PartialEq)]
pub enum RemoteResults {
    /// No se ha consultado al proveedor.
    NotAttempted,
    /// En curso. Llegará `SearchRemoteReady` con el mismo `query_id`.
    ///
    /// Los resultados que ya hay son válidos: la lista puede crecer y
    /// reordenarse cuando llegue la respuesta, no vaciarse.
    Loading,
    /// El proveedor ya contestó y sus resultados están en `tracks`.
    Ready,
    Unavailable {
        reason_key: String,
    },
}

/// Una canción y sus otras versiones.
///
/// ## Por qué se agrupa
///
/// YouTube tiene de cada canción la de estudio, el directo, la instrumental,
/// la maqueta del aniversario y tres covers. Buscar "Faint" devolvía diez filas
/// que dicen lo mismo, y la que el usuario quería estaba entre ellas sin forma
/// de distinguirla.
///
/// Agrupar en vez de filtrar es deliberado: **no se pierde nada**. Las otras
/// versiones siguen ahí, a un despliegue de distancia. Un filtro que se
/// equivoca hace desaparecer una canción sin decirlo.
#[derive(Debug, Clone, PartialEq)]
pub struct GrupoDeVersiones {
    /// La que se muestra: la grabación original si la hay.
    pub principal: TrackRow,
    /// Las demás, de más a menos parecida a la original.
    pub versiones: Vec<TrackRow>,
}

/// Lo que mejor responde a la consulta, sea del tipo que sea.
///
/// Se destaca aparte porque casi siempre se busca **una cosa concreta**, y esa
/// cosa no siempre es una canción: quien escribe "radiohead" quiere el artista,
/// y quien escribe "ok computer" quiere el disco. Con listas separadas por tipo,
/// lo que se buscaba queda enterrado en la segunda o la tercera.
#[derive(Debug, Clone, PartialEq)]
pub enum PrimeraCoincidencia {
    Track(TrackRow),
    Album(AlbumRow),
    Artist(ArtistRow),
}

/// El resultado de una búsqueda.
///
/// ## Una sola lista de canciones, no dos
///
/// Hubo un tiempo en que esto devolvía "lo local" y "lo remoto" por separado, y
/// la interfaz los pintaba en dos bloques: *En tu biblioteca* y *Más
/// resultados*. Era una mentira con dos capas.
///
/// La primera: **el catálogo local no es una biblioteca**. Cada búsqueda
/// persiste sus resultados, así que "lo local" es exactamente lo que devolvió
/// el proveedor la última vez que se buscó algo parecido. Encabezarlo con "en
/// tu biblioteca" le atribuía al usuario una colección que no había hecho.
///
/// La segunda: **los dos bloques se solapaban**. Buscar dos veces lo mismo
/// enseñaba las mismas canciones arriba y abajo, con distinto título encima.
///
/// Ahora `tracks` es la respuesta, y `remote` dice solo si queda algo por
/// llegar. De dónde salió cada fila es un detalle de implementación.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchResults {
    /// Monótono. Permite descartar respuestas de pulsaciones ya superadas.
    pub query_id: u64,
    /// Lo que mejor responde, destacado. `None` si no hay nada claro.
    pub top: Option<PrimeraCoincidencia>,
    /// Canciones, en el mejor orden disponible y agrupadas por versiones.
    pub tracks: Vec<GrupoDeVersiones>,
    pub albums: Vec<AlbumRow>,
    pub artists: Vec<ArtistRow>,
    pub playlists: Vec<PlaylistSummary>,
    /// Si el proveedor va a aportar algo más a `tracks`.
    pub remote: RemoteResults,
}

#[async_trait]
pub trait SearchService: Send + Sync + 'static {
    /// Busca.
    ///
    /// **Siempre consulta SQLite primero** y devuelve lo local de inmediato. La
    /// consulta remota, si procede, se lanza en segundo plano y avisa por
    /// evento. Nunca se busca directamente en YouTube: no hay forma de hacerlo
    /// desde esta API.
    async fn search(
        &self,
        query: &str,
        scope: SearchScope,
        page: &PageRequest,
    ) -> CoreResult<SearchResults>;

    async fn suggest(&self, prefix: &str, limit: u8) -> CoreResult<Vec<String>>;
}

// ─────────────────────────────────────────────────────────────────────────────
// Descargas
// ─────────────────────────────────────────────────────────────────────────────

/// Lo que necesita el reproductor para empezar a sonar.
#[derive(Debug, Clone, PartialEq)]
pub struct DownloadHandle {
    /// Ruta ya reproducible: el `.part` en crecimiento o el fichero definitivo.
    pub playable_path: PathBuf,
    /// `true` si el fichero está completo.
    pub complete: bool,
}

#[async_trait]
pub trait DownloadService: Send + Sync + 'static {
    /// Garantiza que la pista se pueda reproducir.
    ///
    /// Idempotente: si ya es local, vuelve al instante; si ya hay una descarga
    /// en curso, se engancha a ella en lugar de duplicarla.
    ///
    /// **Una pista descargada nunca se vuelve a descargar.**
    async fn ensure(&self, track: &TrackId, priority: Priority) -> CoreResult<DownloadHandle>;

    async fn status(&self, track: &TrackId) -> CoreResult<Availability>;

    /// Estado de varias pistas de golpe.
    ///
    /// Existe para que una lista virtualizada pida las ~40 filas visibles en
    /// una sola llamada. Sin esto habría una llamada por fila al hacer scroll.
    async fn statuses(&self, tracks: &[TrackId]) -> CoreResult<Vec<(TrackId, Availability)>>;

    /// Reintenta las descargas fallidas. Es explícito porque el reintento
    /// automático de un emparejamiento sin confianza no aportaría nada
    /// (ADR-017).
    async fn retry_failed(&self) -> CoreResult<u32>;

    // Nótese que aquí NO hay `cancel` ni `pause`: no existen en el diseño
    // (ADR-016).
}

// ─────────────────────────────────────────────────────────────────────────────
// Cola y reproducción
// ─────────────────────────────────────────────────────────────────────────────

#[async_trait]
pub trait QueueService: Send + Sync + 'static {
    async fn snapshot(&self) -> QueueSnapshot;

    /// Establece el contexto y la posición inicial. Reemplaza la cola de
    /// contexto pero **conserva la de usuario**, igual que Spotify.
    async fn set_context(&self, ctx: PlaybackContext, start_index: usize) -> CoreResult<()>;

    /// "Reproducir a continuación".
    async fn add_next(&self, tracks: &[TrackId]) -> CoreResult<()>;
    /// "Añadir a la cola".
    async fn add_last(&self, tracks: &[TrackId]) -> CoreResult<()>;

    async fn remove(&self, entry: QueueEntryId) -> CoreResult<()>;
    async fn move_entry(&self, entry: QueueEntryId, to_index: usize) -> CoreResult<()>;
    async fn clear_user_queue(&self) -> CoreResult<()>;

    /// Avanza y devuelve lo siguiente. Consume primero la cola de usuario.
    async fn advance(&self, reason: AdvanceReason) -> CoreResult<Option<TrackId>>;
    async fn go_back(&self) -> CoreResult<Option<TrackId>>;

    /// Siguiente pista sin consumirla, para precargar y preparar el crossfade.
    async fn peek_next(&self) -> CoreResult<Option<TrackId>>;

    async fn set_shuffle(&self, enabled: bool) -> CoreResult<()>;
    async fn set_repeat(&self, mode: RepeatMode) -> CoreResult<()>;
}

#[async_trait]
pub trait PlaybackService: Send + Sync + 'static {
    /// Reproduce una pista en un contexto.
    ///
    /// Si no está en local, arranca la descarga y empieza a sonar en cuanto hay
    /// buffer. El usuario no ve ninguna de estas dos cosas.
    async fn play_track(&self, id: &TrackId, ctx: PlaybackContext) -> CoreResult<PlayerState>;

    async fn toggle(&self) -> CoreResult<PlayerState>;
    async fn pause(&self) -> CoreResult<PlayerState>;
    async fn resume(&self) -> CoreResult<PlayerState>;
    async fn next(&self) -> CoreResult<PlayerState>;

    /// Anterior. Por debajo de tres segundos reproducidos va a la pista
    /// previa; por encima reinicia la actual, como Spotify.
    async fn previous(&self) -> CoreResult<PlayerState>;

    async fn seek(&self, position: DurationMs) -> CoreResult<PlayerState>;
    async fn set_volume(&self, volume: Volume) -> CoreResult<PlayerState>;
    async fn set_repeat(&self, mode: RepeatMode) -> CoreResult<PlayerState>;
    async fn set_shuffle(&self, enabled: bool) -> CoreResult<PlayerState>;

    /// Salta a una entrada concreta de la cola.
    async fn jump_to(&self, entry: QueueEntryId) -> CoreResult<PlayerState>;

    /// Estado completo. Es el comando de resincronización cuando el frontend
    /// pierde eventos.
    async fn state(&self) -> PlayerState;

    /// Posición y buffer. Se sondea varias veces por segundo, así que lee
    /// atómicos y no toca la base de datos.
    fn position(&self) -> (DurationMs, DurationMs);

    /// Vuelca el estado a disco. Se llama al cerrar, antes de destruir la
    /// ventana, para no perder el segundo exacto.
    async fn persist_now(&self) -> CoreResult<()>;
}

// ─────────────────────────────────────────────────────────────────────────────
// Biblioteca y playlists
// ─────────────────────────────────────────────────────────────────────────────

#[async_trait]
pub trait LibraryService: Send + Sync + 'static {
    async fn tracks(
        &self,
        filter: &TrackFilter,
        sort: TrackSort,
        page: &PageRequest,
    ) -> CoreResult<Page<TrackRow>>;

    async fn albums(&self, filter: &AlbumFilter, page: &PageRequest) -> CoreResult<Page<AlbumRow>>;
    async fn artists(&self, page: &PageRequest) -> CoreResult<Page<ArtistRow>>;

    async fn album_detail(&self, id: &AlbumId) -> CoreResult<AlbumDetail>;
    async fn artist_detail(&self, id: &ArtistId) -> CoreResult<ArtistDetail>;

    async fn set_favorite(&self, id: &TrackId, enabled: bool) -> CoreResult<()>;
    async fn favorites(&self, page: &PageRequest) -> CoreResult<Page<TrackRow>>;

    /// Registra una escucha. `completed` alimenta el historial y las
    /// recomendaciones locales.
    async fn record_play(&self, id: &TrackId, ms_played: u32, completed: bool) -> CoreResult<()>;

    async fn recent(&self, limit: u16) -> CoreResult<Vec<TrackRow>>;
    async fn stats(&self) -> CoreResult<LibraryStats>;

    /// Borra el fichero descargado de una pista.
    ///
    /// La pista **no desaparece**: sigue en el catálogo, en sus playlists y en
    /// los favoritos, y vuelve a descargarse al reproducirla. Lo que se borra es
    /// el audio.
    ///
    /// Es la salida al invariante "lo descargado no se vuelve a descargar", que
    /// existe para que nadie baje cien veces la misma canción y que hasta ahora
    /// no tenía marcha atrás: un emparejamiento malo se quedaba para siempre.
    async fn delete_download(&self, id: &TrackId) -> CoreResult<()>;

    /// Borra **todo** lo descargado. Devuelve cuántas pistas.
    ///
    /// Mismo trato que la anterior, a lo grande: se van los ficheros de audio y
    /// se quedan el catálogo, las playlists, los favoritos y el historial. Lo
    /// que se pierde es tiempo de descarga, no decisiones del usuario.
    async fn wipe_downloads(&self) -> CoreResult<u32>;

    /// Reconcilia disco y base de datos en ambos sentidos. Corre en segundo
    /// plano con progreso; nunca bloquea el arranque.
    async fn rescan(&self) -> CoreResult<Uuid>;
    async fn last_scan_report(&self) -> CoreResult<Option<ScanReport>>;

    /// Importa ficheros que el usuario ya tenía, para que convivan con lo
    /// descargado.
    ///
    /// A diferencia de `rescan`, que solo recupera ficheros de pistas que el
    /// catálogo ya conoce, esto da de alta una pista **nueva** por fichero,
    /// leyendo título, artista y álbum de sus propias etiquetas. Se ejecuta
    /// síncrono: es una selección manual de decenas de ficheros, no un barrido
    /// de biblioteca entera.
    async fn import_files(&self, paths: Vec<PathBuf>) -> CoreResult<ImportReport>;

    /// Borra la pista del catálogo entero, no solo su audio.
    ///
    /// A diferencia de [`LibraryService::delete_download`], esto **sí** se
    /// lleva sus playlists, sus favoritos y su historial — quien llama debe
    /// pedir confirmación antes de invocarlo. El fichero de audio, si lo hay,
    /// se borra primero del disco.
    async fn delete_track(&self, id: &TrackId) -> CoreResult<()>;
}

#[async_trait]
pub trait PlaylistService: Send + Sync + 'static {
    async fn list(&self) -> CoreResult<Vec<PlaylistSummary>>;
    async fn create(&self, name: &str) -> CoreResult<PlaylistSummary>;
    async fn rename(&self, id: &PlaylistId, name: &str) -> CoreResult<()>;

    /// Cambia la descripción. `None` la quita.
    ///
    /// Va aparte de [`PlaylistService::rename`] y no como un `patch` con los dos
    /// campos: renombrar y describir son gestos distintos —uno se hace al vuelo
    /// y el otro escribiendo un párrafo— y unirlos obligaría a mandar el nombre
    /// entero cada vez que alguien corrige una coma de la descripción.
    async fn set_description(&self, id: &PlaylistId, description: Option<&str>) -> CoreResult<()>;
    async fn delete(&self, id: &PlaylistId) -> CoreResult<()>;
    async fn detail(&self, id: &PlaylistId, page: &PageRequest) -> CoreResult<PlaylistDetail>;

    async fn add_tracks(
        &self,
        id: &PlaylistId,
        tracks: &[TrackId],
        at_index: Option<usize>,
    ) -> CoreResult<()>;

    async fn remove_entries(&self, id: &PlaylistId, entries: &[PlaylistEntryId]) -> CoreResult<()>;

    /// Reordena. Con claves fraccionarias esto es **un solo `UPDATE`**, sea la
    /// playlist de 10 pistas o de 5 000 (ADR-009).
    async fn reorder(
        &self,
        id: &PlaylistId,
        entry: PlaylistEntryId,
        to_index: usize,
    ) -> CoreResult<()>;

    async fn set_cover(&self, id: &PlaylistId, image: &Path) -> CoreResult<()>;

    /// Quita la portada propia y devuelve la playlist al mosaico.
    async fn clear_cover(&self, id: &PlaylistId) -> CoreResult<()>;

    /// Ruta absoluta de la portada propia, para servirla.
    ///
    /// Devuelve `None` si no hay ninguna o si el fichero desapareció. La ruta
    /// **no cruza el puente**: la usa el manejador del esquema `cover`, que
    /// vive en Rust y responde con los bytes (ADR-018).
    async fn cover_file(&self, id: &PlaylistId) -> CoreResult<Option<PathBuf>>;

    /// Importa una playlist pública. Devuelve el identificador de la
    /// importación; el progreso llega por eventos.
    ///
    /// **No descarga audio**: sería descargar cientos de canciones que quizá no
    /// se escuchen nunca. Las descargas siguen siendo bajo demanda.
    async fn import_from_provider(&self, url_or_id: &str) -> CoreResult<Uuid>;

    /// Sugerencias para seguir añadiendo, generadas localmente.
    async fn suggestions(&self, id: &PlaylistId, limit: u8) -> CoreResult<Vec<TrackRow>>;
}

// ─────────────────────────────────────────────────────────────────────────────
// Recomendaciones, letras, caché y avisos
// ─────────────────────────────────────────────────────────────────────────────

/// Una sección de la pantalla de Inicio.
#[derive(Debug, Clone, PartialEq)]
pub struct HomeSection {
    /// Clave i18n del título.
    pub key: String,
    pub params: Vec<(String, String)>,
    pub items: HomeItems,
}

#[derive(Debug, Clone, PartialEq)]
pub enum HomeItems {
    Tracks(Vec<TrackRow>),
    Albums(Vec<AlbumRow>),
    Artists(Vec<ArtistRow>),
    Playlists(Vec<PlaylistSummary>),
}

#[async_trait]
pub trait RecommendationService: Send + Sync + 'static {
    /// Secciones de Inicio. **Nada de esto sale a la red**: se genera con
    /// artistas, géneros, álbumes, playlists e historial locales.
    async fn home(&self) -> CoreResult<Vec<HomeSection>>;
    async fn similar_to_track(&self, id: &TrackId, limit: u8) -> CoreResult<Vec<TrackRow>>;
    async fn for_playlist(&self, id: &PlaylistId, limit: u8) -> CoreResult<Vec<TrackRow>>;
}

#[async_trait]
pub trait LyricsService: Send + Sync + 'static {
    /// Letra de una pista.
    ///
    /// `Ok(None)` significa que no existe, y **no es un error**: la UI oculta
    /// el panel sin decir nada.
    async fn get(&self, track: &TrackId) -> CoreResult<Option<Lyrics>>;
}

// La caché no tiene puerto de servicio. Hubo un `CacheService` con su propia
// enumeración de espacios de nombres y su propia tabla de caducidades, pero
// nunca llegó a conectarse: quien cachea de verdad —las recomendaciones— usa
// `CacheRepository`, que recibe el espacio y el TTL como argumentos. Dos puertos
// para lo mismo, uno de ellos con una única implementación que devolvía `None` a
// todo y estaba cableada en producción.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastLevel {
    Info,
    Warn,
    Error,
}

#[async_trait]
pub trait NotificationService: Send + Sync + 'static {
    /// Actualiza el panel multimedia del sistema.
    async fn now_playing(&self, track: &TrackId) -> CoreResult<()>;
    async fn playback_status(&self, playing: bool) -> CoreResult<()>;

    /// Aviso in-app, con clave i18n. Localify **nunca** notifica descargas: son
    /// invisibles por diseño.
    async fn toast(&self, level: ToastLevel, key: &str, params: &[(String, String)]);
}

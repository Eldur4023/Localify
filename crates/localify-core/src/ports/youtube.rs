//! Puertos de obtención de audio.
//!
//! Se separan a propósito **emparejar** y **descargar**:
//!
//! - [`YoutubeMatcher`] es lógica determinista sobre datos. Se prueba con
//!   fixtures JSON, sin red, y es donde vive la calidad de la biblioteca.
//! - [`YoutubeDownloader`] es I/O sobre un proceso externo.
//!
//! Mezclarlos haría imposible testear el scorer, que es justo la pieza con más
//! reglas y más consecuencias (un mal emparejamiento queda grabado para
//! siempre, porque lo descargado no se vuelve a descargar).

use std::path::{Path, PathBuf};

use async_trait::async_trait;

use crate::domain::audio::DurationMs;
use crate::domain::download::{DownloadProgress, MatchResult};
use crate::domain::settings::FormatPreference;
use crate::domain::track::Track;
use crate::error::CoreResult;

/// Candidato crudo, tal como lo devuelve la búsqueda, **antes** de puntuar.
#[derive(Debug, Clone, PartialEq)]
pub struct RawCandidate {
    pub video_id: String,
    pub title: String,
    pub channel: Option<String>,
    pub channel_id: Option<String>,
    pub description: Option<String>,
    pub duration: DurationMs,
    pub view_count: Option<u64>,
    pub from_youtube_music: bool,
    /// `true` si la descripción contiene "Provided to YouTube by", marca
    /// inequívoca de subida por la discográfica.
    pub provided_to_youtube: bool,
}

/// Busca candidatos. Es el único punto del sistema que consulta YouTube.
#[async_trait]
pub trait YoutubeSearch: Send + Sync + 'static {
    /// Consulta libre (`ytsearch`).
    async fn search(&self, query: &str, limit: u8) -> CoreResult<Vec<RawCandidate>>;
    /// Consulta contra YouTube Music, que es la fuente preferente.
    async fn search_music(&self, query: &str, limit: u8) -> CoreResult<Vec<RawCandidate>>;
}

/// Elige la mejor versión disponible.
///
/// Nunca se expone al frontend: no existe ningún comando para "buscar en
/// YouTube". Es una decisión arquitectónica (ver `06-api.md`), no un olvido.
#[async_trait]
pub trait YoutubeMatcher: Send + Sync + 'static {
    /// Empareja una pista.
    ///
    /// `exclude` son vídeos ya rechazados por el usuario o que fallaron al
    /// descargar.
    ///
    /// `conocido` es el vídeo que el catálogo asocia a esta grabación, si lo
    /// sabe (ver [`crate::ports::metadata_provider::MetadataProvider::youtube_video_id`]).
    /// **No es una orden**: entra como un candidato más y se puntúa con el resto.
    /// Un enlace equivocado en el catálogo no puede meter una canción errónea en
    /// la biblioteca, porque lo descargado no se vuelve a descargar.
    async fn find_best(
        &self,
        track: &Track,
        exclude: &[String],
        conocido: Option<&str>,
    ) -> CoreResult<MatchResult>;
}

/// Fichero ya descargado y verificado, listo para entrar en la biblioteca.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadedFile {
    /// Ruta definitiva del temporal, tras verificar y remuxear.
    pub path: PathBuf,
    /// Metadatos técnicos medidos en el propio fichero.
    pub info: MediaInfo,
    /// Extensión final, que puede diferir de la descargada si hubo remux.
    pub extension: String,
}

/// Notificación de avance de una descarga.
pub trait DownloadObserver: Send + Sync {
    fn on_progress(&self, progress: &DownloadProgress);
    /// Hay bytes suficientes para empezar a sonar.
    fn on_playable(&self, path: &Path);
}

/// Metadatos técnicos medidos en el fichero ya descargado.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaInfo {
    pub duration: DurationMs,
    pub codec: String,
    pub bitrate_kbps: Option<u32>,
    pub sample_rate: Option<u32>,
    pub channels: Option<u8>,
    /// Para M4A: si el átomo `moov` está al principio, se puede reproducir
    /// progresivamente; si está al final, hay que esperar al fichero completo.
    pub seekable_from_start: bool,
}

/// Descarga el audio.
///
/// **No hay `cancel` ni `pause`, y no es un olvido**: no existen en el diseño
/// (ADR-016). Codificar la regla en el tipo la hace imposible de violar por
/// accidente. Un trabajo solo termina completándose o fallando.
#[async_trait]
pub trait YoutubeDownloader: Send + Sync + 'static {
    /// Descarga a `dest` (un `.part` en `.tmp/`), informando por `observer`,
    /// y deja el fichero verificado y en su contenedor definitivo.
    ///
    /// **No lo mueve a la biblioteca**: verificar la integridad es cosa suya,
    /// pero etiquetar, renombrar y registrar en la base de datos es del
    /// servicio de descargas, que es quien conoce esas reglas.
    ///
    /// No existe `select_format` a propósito: yt-dlp elige el formato a partir
    /// de la preferencia en la misma invocación que descarga. Preguntárselo
    /// antes duplicaría las peticiones a YouTube sin aportar nada, porque lo
    /// que realmente se obtuvo se sabe midiendo el fichero.
    async fn download(
        &self,
        video_id: &str,
        preference: FormatPreference,
        dest: &Path,
        expected: DurationMs,
        observer: &dyn DownloadObserver,
    ) -> CoreResult<DownloadedFile>;

    /// Inspecciona un fichero de audio.
    async fn probe(&self, path: &Path) -> CoreResult<MediaInfo>;
}

/// Etiquetado de ficheros de audio.
///
/// Escribir los tags hace la biblioteca **portable**: sigue siendo válida
/// aunque se borre la base de datos, y permite reconstruirla escaneando la
/// carpeta.
#[async_trait]
pub trait TagWriter: Send + Sync + 'static {
    /// Escribe metadatos y portada embebida.
    async fn write(&self, path: &Path, track: &Track, cover: Option<&[u8]>) -> CoreResult<()>;

    /// Lee el `LOCALIFY_SPOTIFY_ID` de un fichero, para recuperar la identidad
    /// de una pista durante un `rescan`.
    async fn read_track_id(&self, path: &Path) -> CoreResult<Option<String>>;
}

/// Gestión de los binarios externos (yt-dlp, ffmpeg).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidecarStatus {
    pub name: &'static str,
    pub path: Option<PathBuf>,
    pub version: Option<String>,
    pub available: bool,
}

#[async_trait]
pub trait SidecarManager: Send + Sync + 'static {
    async fn status(&self) -> CoreResult<Vec<SidecarStatus>>;
    /// Descarga o actualiza los binarios que falten o estén obsoletos.
    async fn ensure_available(&self) -> CoreResult<Vec<SidecarStatus>>;
    /// yt-dlp se rompe cuando YouTube cambia; actualizarlo es la reparación
    /// habitual y debe poder hacerse sin publicar versión de Localify.
    async fn update(&self, name: &str) -> CoreResult<SidecarStatus>;
}

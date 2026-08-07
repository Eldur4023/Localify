//! Puertos específicos del sistema operativo.
//!
//! Todo lo que sea Win32, DPAPI o rutas del SO vive detrás de estos traits y se
//! implementa en un único crate, `localify-platform` (ADR-013). Portar a Linux
//! es escribir otra implementación; ningún servicio se entera.

use std::path::{Path, PathBuf};

use async_trait::async_trait;

use crate::domain::audio::DurationMs;
use crate::domain::queue::PlayStatus;
use crate::error::CoreResult;

/// Rutas de la aplicación.
///
/// Las devuelve la plataforma y no una constante porque `%APPDATA%` y
/// `%USERPROFILE%` no se conocen hasta ejecutar.
pub trait AppPaths: Send + Sync + 'static {
    /// Configuración y base de datos.
    fn config_dir(&self) -> &Path;
    /// Raíz de la biblioteca (configurable por el usuario).
    fn library_dir(&self) -> &Path;
    fn audio_dir(&self) -> PathBuf;
    fn covers_dir(&self) -> PathBuf;
    /// Fotos de artista cacheadas.
    ///
    /// Carpeta propia y no `covers/` a secas: los identificadores de álbum y de
    /// artista salen del mismo proveedor y comparten alfabeto, así que un día
    /// coincidirían y el artista se quedaría con la portada de un disco. Además,
    /// separarlas permite vaciar unas sin tocar las otras.
    fn artists_dir(&self) -> PathBuf;
    /// Descargas en curso. Se purga al arrancar: un `.part` huérfano nunca
    /// debe confundirse con biblioteca.
    fn temp_dir(&self) -> PathBuf;
    fn logs_dir(&self) -> PathBuf;
    fn binaries_dir(&self) -> PathBuf;
    fn database_path(&self) -> PathBuf;
    fn settings_path(&self) -> PathBuf;

    /// Ruta absoluta a partir de una relativa de la biblioteca.
    ///
    /// **Único punto donde se resuelven rutas de audio.** Las rutas se
    /// persisten relativas (ADR-018); si alguien concatenara por su cuenta,
    /// cambiar la carpeta de biblioteca rompería en sitios impredecibles.
    fn resolve(&self, rel: &Path) -> PathBuf;
}

/// Almacén de secretos del sistema (DPAPI en Windows).
///
/// El `client_secret` de Spotify y la sesión de Last.fm nunca se guardan en
/// claro ni cruzan el puente IPC.
#[async_trait]
pub trait SecretStore: Send + Sync + 'static {
    async fn get(&self, key: &str) -> CoreResult<Option<String>>;
    async fn set(&self, key: &str, value: &str) -> CoreResult<()>;
    async fn delete(&self, key: &str) -> CoreResult<()>;
}

/// Claves del almacén de secretos.
///
/// Están aquí, y no en quien las escribe, porque casi todas tienen **dos**
/// dueños: `localify-services` guarda la sesión de Last.fm y decide si la
/// aplicación se considera conectada, y `localify-integrations` la lee para
/// firmar. Con una constante en cada crate, cambiar el nombre en uno deja al
/// otro leyendo una clave que ya no existe, y el síntoma —"aparece conectado
/// pero no scrobblea"— no señala a ningún sitio.
pub mod claves {
    /// `client_id` de Spotify. No es secreto, pero acompaña al que sí lo es.
    pub const SPOTIFY_ID: &str = "spotify.client_id";
    /// `client_secret` de Spotify. **Nunca se lee para devolverlo.**
    pub const SPOTIFY_SECRETO: &str = "spotify.client_secret";
    /// Clave de API de Last.fm.
    pub const LASTFM_API_KEY: &str = "lastfm.api_key";
    /// Secreto de la aplicación de Last.fm, con el que se firma cada llamada.
    pub const LASTFM_API_SECRET: &str = "lastfm.api_secret";
    /// Sesión concedida por el usuario. No caduca.
    pub const LASTFM_SESION: &str = "lastfm.session_key";
}

/// Metadatos que el sistema muestra en su panel multimedia.
#[derive(Debug, Clone, PartialEq)]
pub struct NowPlaying {
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub duration: DurationMs,
    /// Ruta local a la portada. El SO necesita un fichero, no una URL.
    pub cover_path: Option<PathBuf>,
}

/// Órdenes que llegan **desde** el sistema: teclas multimedia, panel del SO,
/// botones de la barra de tareas.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaCommand {
    Play,
    Pause,
    Toggle,
    Next,
    Previous,
    Stop,
    Seek { position_ms: u32 },
}

/// Integración con los controles multimedia del sistema.
///
/// En Windows: `SystemMediaTransportControls` y la barra de miniaturas de la
/// barra de tareas. En Linux sería MPRIS; en macOS, `MPNowPlayingInfoCenter`.
/// La implementación por defecto en plataformas no soportadas no hace nada, y
/// la aplicación funciona igual.
#[async_trait]
pub trait SystemMediaIntegration: Send + Sync + 'static {
    async fn set_now_playing(&self, info: &NowPlaying) -> CoreResult<()>;
    async fn set_status(&self, status: PlayStatus) -> CoreResult<()>;
    async fn set_position(&self, position: DurationMs, duration: DurationMs) -> CoreResult<()>;
    async fn clear(&self) -> CoreResult<()>;

    /// Registra el receptor de las órdenes del sistema. Se llama una vez al
    /// arrancar.
    fn set_command_handler(&self, handler: Box<dyn Fn(MediaCommand) + Send + Sync>);
}

/// Operaciones de ficheros con las garantías que exige el proyecto.
#[async_trait]
pub trait FileSystem: Send + Sync + 'static {
    /// Mueve `from` a `to` de forma atómica, sobrescribiendo si hace falta.
    ///
    /// En Windows debe usar `MOVEFILE_REPLACE_EXISTING` y funcionar aunque el
    /// motor de audio tenga el fichero abierto (por eso los orígenes se abren
    /// con `FILE_SHARE_DELETE`). Es el último paso de toda descarga y lo que
    /// garantiza que en la biblioteca no aparezca jamás un fichero incompleto.
    async fn atomic_rename(&self, from: &Path, to: &Path) -> CoreResult<()>;

    /// Copia `from` a `to` conservando el original. Devuelve los bytes escritos.
    ///
    /// Existe además de [`FileSystem::atomic_rename`] porque la migración de
    /// carpeta de biblioteca **necesita el original intacto** hasta que la copia
    /// entera está verificada: mover fichero a fichero deja la biblioteca
    /// partida entre dos carpetas si el proceso muere a mitad, y ninguna de las
    /// dos está completa. Copiar cuesta espacio y tiempo; es el precio de que
    /// cualquier interrupción deje una biblioteca íntegra en un sitio conocido.
    ///
    /// Como todo lo demás, escribe en un temporal y renombra: un destino
    /// presente es, por definición, una copia completa.
    async fn copy_file(&self, from: &Path, to: &Path) -> CoreResult<u64>;

    /// Escribe y hace `fsync` antes de devolver.
    async fn write_synced(&self, path: &Path, bytes: &[u8]) -> CoreResult<()>;

    async fn ensure_dir(&self, path: &Path) -> CoreResult<()>;
    async fn remove_file(&self, path: &Path) -> CoreResult<()>;

    /// Borra el contenido de un directorio sin borrarlo. Para purgar `.tmp/`.
    async fn clear_dir(&self, path: &Path) -> CoreResult<u32>;

    async fn exists(&self, path: &Path) -> bool;
    async fn file_size(&self, path: &Path) -> CoreResult<u64>;

    /// Espacio libre en el volumen que contiene `path`.
    async fn available_space(&self, path: &Path) -> CoreResult<u64>;

    /// Comprueba que se puede escribir. Se usa al validar un cambio de carpeta
    /// de biblioteca, **antes** de aceptar el ajuste.
    async fn is_writable(&self, path: &Path) -> bool;
}

/// Detección del locale del sistema, para elegir idioma en el primer arranque.
pub trait LocaleProvider: Send + Sync + 'static {
    /// Locale preferido (`"es-ES"`, `"en-US"`).
    fn system_locale(&self) -> String;
}

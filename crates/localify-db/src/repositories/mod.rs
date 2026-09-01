//! Repositorios: la implementación de los puertos de persistencia.
//!
//! Cada repositorio recibe el [`crate::Pool`] y traduce entre consultas SQL y
//! entidades del dominio. Ninguno contiene lógica de negocio: si un repositorio
//! empieza a decidir *cuándo* hacer algo en vez de solo *cómo* obtenerlo, esa
//! decisión pertenece a un servicio.

pub mod albums;
pub mod artists;
pub mod audio_files;
pub mod downloads;
pub mod favorites;
pub mod history;
pub mod maintenance;
pub mod player_state;
pub mod playlists;
pub mod scans;
pub mod search;
pub mod similarity;
pub mod system;
pub mod tracks;

pub use albums::SqliteAlbumRepository;
pub use artists::SqliteArtistRepository;
pub use audio_files::SqliteAudioFileRepository;
pub use downloads::{SqliteDownloadJobRepository, SqliteYoutubeMatchRepository};
pub use favorites::SqliteFavoriteRepository;
pub use history::SqliteHistoryRepository;
pub use maintenance::SqliteMaintenanceRepository;
pub use player_state::SqlitePlayerStateRepository;
pub use playlists::SqlitePlaylistRepository;
pub use scans::SqliteScanReportRepository;
pub use search::SqliteSearchRepository;
pub use similarity::SqliteSimilarityRepository;
pub use system::{SqliteCacheRepository, SqliteLyricsRepository, SqliteSettingsRepository};
pub use tracks::SqliteTrackRepository;

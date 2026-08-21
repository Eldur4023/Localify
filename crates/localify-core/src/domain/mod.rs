//! Entidades y value objects del dominio.
//!
//! Tipos puros: sin I/O, sin dependencias de infraestructura, con la validación
//! y los invariantes que definen el modelo. Toda la lógica que sea "una regla
//! sobre los datos" vive aquí y no en un servicio.

pub mod album;
pub mod artist;
pub mod audio;
pub mod availability;
pub mod download;
pub mod ids;
pub mod library;
pub mod lyrics;
pub mod playlist;
pub mod queue;
pub mod scrobble;
pub mod settings;
pub mod track;
pub mod versiones;

pub use album::{Album, AlbumDetail, AlbumFilter, AlbumRow, AlbumType, CoverSet};
pub use artist::{Artist, ArtistDetail, ArtistRow};
pub use audio::{
    AudioDevice, AudioFormat, BANDAS_EQ_HZ, DurationMs, EqProfile, GANANCIA_MAX_DB, Volume,
};
pub use availability::Availability;
pub use download::{
    Confidence, DownloadJob, DownloadProgress, DownloadState, MatchResult, Priority,
    ScoreBreakdown, YoutubeCandidate,
};
pub use ids::{AlbumId, ArtistId, PlaylistEntryId, PlaylistId, QueueEntryId, TrackId};
pub use library::{AudioFileRecord, AudioSource, LibraryStats, PlayHistoryEntry, ScanReport};
pub use lyrics::{LyricLine, Lyrics};
pub use playlist::{
    ImportProgress, Playlist, PlaylistDetail, PlaylistEntry, PlaylistSource, PlaylistSummary,
};
pub use queue::{
    AdvanceReason, ChangeSource, PlayStatus, PlaybackContext, PlayerState, QueueEntry,
    QueueSnapshot, RepeatMode,
};
pub use scrobble::{PendingScrobble, merece_scrobble};
pub use settings::{
    AudioSettings, CookieSource, CookiesVigentes, DownloadSettings, FormatPreference,
    IntegrationSettings, Language, Settings, SettingsPatch, SettingsSection, SpotifySettings,
    UiSettings,
};
pub use track::{AlbumRef, ArtistRef, Track, TrackFilter, TrackRow, TrackSort};
pub use versiones::{ClaseDeVersion, clase, titulo_canonico};

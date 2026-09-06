//! DTOs de la API pública.
//!
//! Son tipos **distintos** de las entidades del dominio, no envoltorios. Esa
//! separación es lo que permite que el dominio evolucione sin romper a los
//! clientes, y que el contrato IPC se mantenga estable y plano: aquí no hay
//! `Option<Box<dyn ...>>` ni genéricos con vida, solo `String`, números y
//! enumeraciones etiquetadas.
//!
//! ## Tipos de TypeScript
//!
//! Se **generan**, no se escriben a mano (ADR-014). `ts-rs` deriva
//! `frontend/src/ipc/types.gen.ts` desde estas definiciones al ejecutar los
//! tests, y la CI exige que el diff quede vacío. Rust es la única fuente de
//! verdad de lo que cruza el puente.
//!
//! ## Convenciones
//!
//! - `#[serde(rename_all = "camelCase")]` en todo.
//! - Duraciones en milisegundos (`u32`), instantes en unix segundos (`i64`).
//! - Las uniones llevan discriminante explícito (`kind`, `type`, `state`).
//! - Ningún campo declarado se omite: `Option<T>` viaja como `T | null`.

pub mod catalog;
pub mod common;
pub mod events;
pub mod library;
pub mod player;
pub mod settings;

pub use catalog::{
    AlbumDetailDto, AlbumRefDto, AlbumRowDto, ArtistDetailDto, ArtistRefDto, ArtistRowDto,
    TrackCandidateDto, TrackDetailDto, TrackRowDto,
};
pub use common::{ApiError, AvailabilityDto, PageDto, PageRequestDto};
pub use events::LocalifyEvent;
pub use library::{
    HomeSectionDto, ImportReportDto, LibraryStatsDto, LyricLineDto, LyricsDto, PlaylistDetailDto,
    PlaylistEntryDto, PlaylistSummaryDto, SearchResultsDto,
};
pub use player::{PlaybackContextDto, PlayerStateDto, QueueEntryDto, QueueSnapshotDto};
pub use settings::{ProviderStatusDto, SettingsDto, SettingsPatchDto};

/// Carpeta donde `ts-rs` deposita los tipos generados.
///
/// Se declara una vez para que todos los DTOs exporten al mismo sitio; si cada
/// tipo la repitiera, bastaría una errata para que un tipo acabara suelto.
pub const DIRECTORIO_TS: &str = "../../frontend/src/ipc/";

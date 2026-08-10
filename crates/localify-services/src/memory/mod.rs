//! Servicios provisionales en memoria.
//!
//! Existen por una razón concreta del roadmap: permitir que la capa de comandos
//! y el frontend se construyan **en paralelo** a los proveedores reales, en
//! lugar de esperar a que Spotify, yt-dlp y el motor de audio estén listos.
//!
//! Cada uno se sustituye por su implementación definitiva en la fase que le
//! corresponde (5 a 9). Cuando eso ocurra, `localify-app` no cambiará ni una
//! línea salvo el cableado del `AppContext`: es exactamente lo que la inversión
//! de dependencias debía comprar, y esto lo demuestra antes de que sea caro
//! descubrir que no funciona.
//!
//! No son código desechable sin valor: quedan como **dobles de test** para la
//! suite de integración, que debe correr sin red ni disco.

pub mod servicios;
pub mod store;

pub use servicios::{
    Contexto, DownloadEnMemoria, LibraryEnMemoria, LyricsEnMemoria, MetadataEnMemoria,
    NotificationEnMemoria, PlaybackEnMemoria, PlaylistEnMemoria, QueueEnMemoria,
    RecommendationEnMemoria, SearchEnMemoria, SettingsEnMemoria,
};
pub use store::{Datos, MemoryStore};

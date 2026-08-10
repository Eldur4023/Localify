//! # localify-services
//!
//! Los servicios de negocio de Localify.
//!
//! **Este crate no depende de ningún crate de infraestructura.** Solo conoce
//! `localify-core`: recibe sus dependencias como `Arc<dyn Trait>` y no sabe si
//! detrás hay SQLite o un doble en memoria. Es lo que permite probar la lógica
//! completa sin tocar disco ni red, y lo que verifica la CI con `cargo tree`.
//!
//! Los servicios sin estado son estructuras inmutables. Los que tienen estado
//! mutable con invariantes temporales (`Playback`, `Queue`, `Download`) se
//! implementan como actores: una tarea que posee el estado en exclusiva y
//! consume un canal de comandos (ADR-008). Desde fuera son indistinguibles de
//! los demás, porque lo que se inyecta es un handle que implementa el mismo
//! trait.
//!
//! El módulo [`inerte`] cubre lo que queda cuando falta una pieza de la que
//! todo lo demás depende —la base de datos, la tarjeta de sonido—: la
//! aplicación arranca igual y dice qué falta, en vez de cerrarse o de inventar.

// En los tests, `expect` con un mensaje es la forma correcta de fallar.
#![cfg_attr(test, allow(clippy::expect_used))]

pub mod actors;
pub mod ajustes;
pub mod biblioteca;
pub mod combinado;
pub mod inerte;
pub mod metadata;
pub mod playlists;
pub mod proveedor;
pub mod recomendaciones;
pub mod search;

pub use actors::{
    BACKOFF_POR_DEFECTO, DependenciasCola, DependenciasDescarga, DependenciasReproduccion,
    DownloadActor, PlaybackActor, QueueActor, conectar_eventos,
};
pub use biblioteca::{Dependencias as DependenciasBiblioteca, LibraryServiceImpl};
pub use combinado::ProveedorCombinado;
pub use metadata::MetadataServiceImpl;
pub use playlists::{Dependencias as DependenciasPlaylists, PlaylistServiceImpl};
pub use recomendaciones::{Dependencias as DependenciasRecomendaciones, RecommendationServiceImpl};
pub use search::SearchServiceImpl;

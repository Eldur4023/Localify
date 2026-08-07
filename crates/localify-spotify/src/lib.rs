//! # localify-spotify
//!
//! Cliente de la Web API de Spotify. Implementa
//! [`localify_core::ports::metadata_provider::MetadataProvider`].
//!
//! Es el **único** punto del proyecto que habla con Spotify. Se ocupa de la
//! autenticación (client credentials), la limitación de peticiones, los
//! reintentos con backoff y del mapeo de las respuestas crudas a entidades del
//! dominio.
//!
//! El usuario no inicia sesión: no hay cuenta de Spotify implicada. Las
//! credenciales son de aplicación y se aportan una vez desde Ajustes (ADR-005).
//! Si no están configuradas, el proveedor responde `NotConfigured` y la
//! aplicación sigue funcionando por completo sobre la biblioteca local: es un
//! modo de operación previsto, no un fallo.
//!
//! ## Sobre las pruebas
//!
//! El transporte HTTP va detrás de un trait, así que **la suite entera corre
//! sin red**: limitador, refresco de token, reintentos y mapeo se prueban con
//! respuestas preparadas, de forma determinista y en milisegundos.

// En los tests, `expect` y `panic!` con un mensaje son la forma correcta de
// fallar.
#![cfg_attr(test, allow(clippy::expect_used, clippy::panic))]

pub mod auth;
pub mod client;
pub mod error;
pub mod mapper;
pub mod models;
pub mod provider;
pub mod publica;
pub mod rate_limit;
pub mod transporte;
pub mod uri;

pub use auth::Credenciales;
pub use client::ClienteSpotify;
pub use error::{PROVEEDOR, SpotifyError, SpotifyResult};
pub use provider::SpotifyProvider;
pub use transporte::{Transporte, TransporteHttp};

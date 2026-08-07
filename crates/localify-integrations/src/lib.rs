//! # localify-integrations
//!
//! Integraciones opcionales: Discord Rich Presence, Last.fm y letras.
//!
//! ## Ninguna es una dependencia de nadie
//!
//! Todas son **consumidoras del bus de eventos**, nunca dependencias de un
//! servicio. La consecuencia práctica es que si Discord no está abierto, si
//! Last.fm no responde o si el usuario las desactiva, no hay nada más que se
//! entere: la música sigue sonando y ningún otro servicio cambia de
//! comportamiento.
//!
//! Es también la razón de que vivan en su propio crate. Si `localify-services`
//! las conociera, un fallo suyo podría propagarse a la reproducción.
//!
//! ## Estado
//!
//! - Letras (LRCLIB): implementadas.
//! - Discord Rich Presence: implementado.
//! - Last.fm: implementado, con cola persistente.

// En un test, un `expect` que falla **es** el fallo: es la forma más corta de
// decir qué se esperaba. Mismo criterio que en el resto del workspace.
#![cfg_attr(test, allow(clippy::expect_used))]

pub mod discord;
pub mod imagenes;
pub mod lastfm;
pub mod letras;
pub mod lrc;

pub use discord::Dependencias as DependenciasDiscord;
pub use imagenes::DescargadorDeImagenes;
pub use lastfm::{Dependencias as DependenciasLastfm, GestorLastfm};
pub use letras::{Dependencias as DependenciasLetras, LyricsServiceImpl};

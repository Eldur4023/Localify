//! # localify-integrations
//!
//! Integraciones opcionales: Discord Rich Presence y letras.
//!
//! ## Ninguna es una dependencia de nadie
//!
//! Todas son **consumidoras del bus de eventos**, nunca dependencias de un
//! servicio. La consecuencia práctica es que si Discord no está abierto o si
//! el usuario las desactiva, no hay nada más que se entere: la música sigue
//! sonando y ningún otro servicio cambia de comportamiento.
//!
//! Es también la razón de que vivan en su propio crate. Si `localify-services`
//! las conociera, un fallo suyo podría propagarse a la reproducción.
//!
//! ## Estado
//!
//! - Letras (LRCLIB): implementadas.
//! - Discord Rich Presence: implementado.
//!
//! Aparte está `autoupdate` (aviso de nuevas versiones contra los releases de
//! GitHub): no consume el bus, solo lo alimenta una vez por arranque, así que
//! no encaja del todo en la descripción de arriba —pero comparte el motivo de
//! vivir en este crate: es opcional y su fallo no debe propagarse a nada.

// En un test, un `expect` que falla **es** el fallo: es la forma más corta de
// decir qué se esperaba. Mismo criterio que en el resto del workspace.
#![cfg_attr(test, allow(clippy::expect_used))]

pub mod autoupdate;
pub mod discord;
pub mod imagenes;
pub mod letras;
pub mod lrc;

pub use discord::Dependencias as DependenciasDiscord;
pub use imagenes::DescargadorDeImagenes;
pub use letras::{Dependencias as DependenciasLetras, LyricsServiceImpl};

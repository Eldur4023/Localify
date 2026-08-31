//! # localify-platform
//!
//! **El único crate del proyecto con código específico del sistema operativo.**
//!
//! Todo lo que sea Win32, DPAPI o convenciones de rutas vive aquí, detrás de
//! los traits de [`localify_core::ports::platform`]. Ningún otro crate contiene
//! un `#[cfg(windows)]` (ADR-013).
//!
//! La consecuencia práctica es que portar a Linux consiste en escribir
//! `mpris.rs` y una implementación de secretos con libsecret, sin tocar una
//! sola línea de lógica de negocio. Lo que hoy no está soportado se degrada a
//! una implementación que no hace nada, y la aplicación funciona igual.
//!
//! ## Sobre `unsafe`
//!
//! Este crate y `localify-audio` son los dos únicos donde se permite. Aquí es
//! inevitable: las APIs de Win32 son FFI. Cada bloque lleva un comentario
//! `// SAFETY:` que justifica sus invariantes.

// Justificación del levantamiento del lint: ver la nota de arriba. El resto del
// workspace mantiene `unsafe_code = "deny"`.
#![allow(unsafe_code)]
// En los tests, `expect` con un mensaje es la forma correcta de fallar.
#![cfg_attr(test, allow(clippy::expect_used))]

pub mod fs;
pub mod locale;
pub mod media;
pub mod navegador;
pub mod paths;
pub mod secrets;
pub mod sidecars;
pub mod single_instance;

pub use fs::RealFileSystem;
pub use locale::SystemLocale;
pub use media::{SinIntegracion, integracion as integracion_multimedia};
pub use paths::{CoverSize, LocalifyPaths};
pub use secrets::AlmacenDeSecretos;
pub use sidecars::{Actualizacion, SIDECARS, SidecarLocator, actualizar_yt_dlp};
pub use single_instance::{InstanceGuard, adquirir as adquirir_instancia};

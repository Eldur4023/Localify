//! # localify-app
//!
//! El ensamblador. Es el **único** crate que conoce implementaciones concretas:
//! aquí se construye el [`context::AppContext`] cableando cada trait de
//! `localify-core` con su implementación real.
//!
//! Responsabilidades:
//!
//! - Registrar los comandos de Tauri (`commands/`), que son deliberadamente
//!   triviales: convierten DTO → dominio, delegan en un trait y vuelven.
//! - Convertir entre DTOs de la API y entidades del dominio (`dto/`).
//! - Puentear el bus de eventos hacia el WebView (`bridge.rs`).
//! - Orquestar el arranque por etapas (`bootstrap.rs`).
//!
//! Si un handler de comando contiene una decisión de negocio, está mal ubicado
//! y debe bajar a un servicio.

// En los tests, `expect` y `panic!` con un mensaje son la forma correcta de
// fallar: el mensaje es la explicación del fallo.
#![cfg_attr(test, allow(clippy::expect_used, clippy::panic))]

pub mod bootstrap;
pub mod bridge;
pub mod commands;
pub mod context;
pub mod credenciales;
pub mod dto;
pub mod logging;
pub mod multimedia;

/// Punto de entrada de la aplicación.
pub fn run() {
    bootstrap::run();
}

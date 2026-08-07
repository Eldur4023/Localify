//! # localify-db
//!
//! Persistencia en SQLite: pool de conexiones, migraciones y repositorios.
//!
//! Implementa los puertos de [`localify_core::ports::database`]. La capa de
//! negocio nunca ve una `Connection` ni una sentencia SQL: pide datos a un
//! repositorio y este decide cómo obtenerlos (ADR-004).
//!
//! ## Modelo de acceso
//!
//! SQLite en modo WAL admite N lectores concurrentes y **un** escritor. El pool
//! refleja eso literalmente: varias conexiones de solo lectura y una única
//! conexión de escritura tras una cola, lo que elimina por construcción los
//! `SQLITE_BUSY` y las transacciones entrelazadas.
//!
//! Todo el trabajo de SQLite ocurre en el pool de hilos bloqueantes: ninguna
//! consulta se ejecuta en un hilo del runtime asíncrono.
//!
//! Ver `docs/architecture/05-database.md` para el esquema completo.

// En los tests, `expect` con un mensaje es la forma correcta de fallar.
#![cfg_attr(test, allow(clippy::expect_used, clippy::panic))]

pub mod error;
pub mod mappers;
pub mod migrations;
pub mod pool;
pub mod pragmas;
pub mod repositories;

pub use error::{DbError, DbResult};
pub use migrations::{EstadoEsquema, ejecutar as ejecutar_migraciones};
pub use pool::Pool;

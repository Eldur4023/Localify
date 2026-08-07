//! # localify-core
//!
//! El centro de la arquitectura: entidades del dominio, puertos (traits) y
//! contratos compartidos.
//!
//! ## Regla de dependencias
//!
//! Este crate **no depende de nada del workspace**, ni de runtimes async, ni de
//! bases de datos, ni de Tauri. Todas las flechas de compilación del proyecto
//! apuntan hacia aquí. La infraestructura implementa estos traits; los
//! servicios los consumen. Ninguno se conoce entre sí.
//!
//! Consecuencia práctica: los servicios se pueden probar con dobles en memoria,
//! sin tocar disco ni red, y sustituir un proveedor (Spotify por MusicBrainz,
//! por ejemplo) no obliga a tocar lógica de negocio.
//!
//! Ver `docs/architecture/01-overview.md` y `02-modules.md`.

// En los tests, `expect` con un mensaje es la forma correcta de fallar: el
// mensaje es la explicación del fallo. En producción sigue estando restringido.
#![cfg_attr(test, allow(clippy::expect_used))]

pub mod domain;
pub mod error;
pub mod events;
pub mod page;
pub mod ports;
pub mod text;

pub use error::{CoreError, CoreResult};
pub use page::{Cursor, Page, PageRequest};

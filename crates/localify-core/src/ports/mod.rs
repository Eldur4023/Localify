//! Puertos: los traits que la infraestructura implementa y los servicios
//! consumen.
//!
//! Están **físicamente separados** de sus implementaciones a propósito. Si un
//! trait viviera junto a su implementación concreta, importar uno arrastraría
//! al otro y la regla de dependencias se erosionaría sin que nadie lo notara.
//!
//! Todos son `Send + Sync + 'static` y aptos para `dyn`, porque se inyectan
//! como `Arc<dyn Trait>`.

pub mod audio_engine;
pub mod clock;
pub mod database;
pub mod metadata_provider;
pub mod platform;
pub mod services;
pub mod youtube;

//! # localify-musicbrainz
//!
//! Adaptador de metadatos sobre la API pública de MusicBrainz.
//!
//! ## Por qué existe
//!
//! El catálogo de YouTube Music es *lo que hay subido a YouTube*, y eso tiene un
//! punto ciego concreto: la música editada que nadie ha subido como "canción".
//! Buscar la banda sonora de un juego devuelve cuarenta versiones de aficionados
//! y ninguna original. MusicBrainz conoce lo publicado —ediciones, ISRC,
//! duraciones exactas— y no conoce lo nativo de YouTube. Los dos juntos cubren
//! el hueco que ninguno cubre solo.
//!
//! ## Es un servicio gratuito de una fundación
//!
//! De ahí las dos reglas que el cliente aplica sin preguntar: una petición por
//! segundo y un `User-Agent` que dice quiénes somos. No son detalles de
//! cortesía, son la condición de uso.

// En un test, un `expect` que falla **es** el fallo.
#![cfg_attr(test, allow(clippy::expect_used))]

pub mod cliente;
pub mod parseo;
pub mod provider;

pub use cliente::ClienteMusicBrainz;
pub use provider::{MusicBrainzProvider, NOMBRE};

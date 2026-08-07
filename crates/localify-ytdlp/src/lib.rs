//! # localify-ytdlp
//!
//! Obtención de audio mediante los binarios externos yt-dlp y FFmpeg.
//!
//! Implementa los puertos de [`localify_core::ports::youtube`]. Se invocan como
//! procesos hijo y no como biblioteca (ADR-006): YouTube cambia su ofuscación
//! cada pocas semanas y yt-dlp publica correcciones en días, así que el
//! extractor debe poder actualizarse sin publicar una versión de Localify.
//!
//! ## Dos responsabilidades separadas
//!
//! - **Emparejar** ([`scoring`]): lógica determinista sobre datos, testeable con
//!   fixtures y sin red. Es donde se decide la calidad de la biblioteca, porque
//!   un mal emparejamiento queda grabado para siempre: lo descargado no se
//!   vuelve a descargar.
//! - **Descargar** ([`download`]): I/O sobre el proceso externo, con
//!   reproducción progresiva sobre el fichero `.part`.
//!
//! No existe cancelación ni pausa de descargas: no están en el diseño
//! (ADR-016).

// En los tests, `expect` y `panic!` con un mensaje son la forma correcta de
// fallar.
#![cfg_attr(test, allow(clippy::expect_used, clippy::panic))]

pub mod adaptadores;
pub mod download;
pub mod error;
pub mod formats;
pub mod proceso;
pub mod remux;
pub mod scoring;
pub mod search;
pub mod tags;
pub mod verify;

pub use adaptadores::{DescargadorYtDlp, MatcherYtDlp};
pub use download::ClienteYtDlp;
pub use error::{YtDlpError, YtDlpResult};
pub use remux::Remuxeador;
pub use scoring::{elegir_mejor, puntuar};
pub use search::{Consulta, RawCandidate, plan_de_consultas};
pub use tags::EtiquetadorLofty;
pub use verify::Inspector;

/// Detección de la marca de subida por el titular de los derechos.
pub mod rules_de_consulta {
    use crate::scoring::rules;

    /// `true` si la descripción declara subida por la discográfica.
    ///
    /// Es una de las señales más fiables que da YouTube: la pone el propio
    /// sistema de distribución, no quien sube el vídeo.
    #[must_use]
    pub fn detectar_provided(descripcion: Option<&str>) -> bool {
        descripcion
            .is_some_and(|d| localify_core::text::normalize(d).contains(rules::MARCA_PROVIDED))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn reconoce_la_marca_en_cualquier_capitalizacion() {
            assert!(detectar_provided(Some(
                "Provided to YouTube by Universal Music Group"
            )));
            assert!(detectar_provided(Some(
                "PROVIDED TO YOUTUBE BY Sony\n\nUnder Pressure"
            )));
        }

        #[test]
        fn no_se_confunde_con_otra_cosa() {
            assert!(!detectar_provided(Some("Subido por un fan")));
            assert!(!detectar_provided(Some("provided by a friend")));
            assert!(!detectar_provided(None));
        }
    }
}

//! Disponibilidad local de una pista.
//!
//! Es el estado que decide qué hace `play`, y el único punto donde la
//! aplicación distingue entre "tengo esto" y "esto es solo un resultado de
//! búsqueda". El usuario nunca ve esta distinción como una acción a realizar:
//! no hay botón de descargar (filosofía del proyecto).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::audio::AudioFormat;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Availability {
    /// Solo hay metadatos. Pulsar play iniciará la descarga.
    #[default]
    Absent,

    /// Hay un fichero temporal que ya se puede reproducir mientras crece.
    #[serde(rename_all = "camelCase")]
    Downloading {
        /// Progreso en `[0.0, 1.0]`.
        progress: f32,
        /// `true` cuando hay bytes suficientes para empezar a sonar.
        playable: bool,
    },

    /// Fichero completo, verificado y etiquetado. Reproducción inmediata.
    #[serde(rename_all = "camelCase")]
    Local {
        /// Ruta **relativa** a la raíz de la biblioteca (ADR-018).
        rel_path: PathBuf,
        format: AudioFormat,
        bytes: u64,
    },

    /// No se pudo obtener. Incluye el caso de que el matcher no encontrase una
    /// coincidencia fiable, que no se reintenta automáticamente (ADR-017).
    #[serde(rename_all = "camelCase")]
    Failed {
        /// Clave i18n del motivo; el backend no traduce (ADR-012).
        reason_key: String,
        attempts: u8,
    },
}

impl Availability {
    /// `true` si pulsar play produce sonido sin esperar a la red.
    #[must_use]
    pub const fn es_reproducible_ya(&self) -> bool {
        matches!(
            self,
            Self::Local { .. } | Self::Downloading { playable: true, .. }
        )
    }

    /// `true` si el fichero está completo en disco.
    #[must_use]
    pub const fn es_local(&self) -> bool {
        matches!(self, Self::Local { .. })
    }

    /// `true` si hace falta pedir una descarga.
    ///
    /// `Failed` **no** cuenta: reintentar en bucle una pista sin coincidencia
    /// fiable gastaría red y acabaría metiendo basura en la biblioteca. El
    /// reintento es explícito, vía `retry_failed`.
    #[must_use]
    pub const fn necesita_descarga(&self) -> bool {
        matches!(self, Self::Absent)
    }

    #[must_use]
    pub fn progreso(&self) -> f32 {
        match self {
            Self::Local { .. } => 1.0,
            Self::Downloading { progress, .. } => progress.clamp(0.0, 1.0),
            Self::Absent | Self::Failed { .. } => 0.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn una_descarga_con_buffer_ya_es_reproducible() {
        let a = Availability::Downloading {
            progress: 0.03,
            playable: true,
        };
        assert!(a.es_reproducible_ya());
        assert!(!a.es_local());
        assert!(
            !a.necesita_descarga(),
            "ya hay un job en curso; no se duplica"
        );
    }

    #[test]
    fn una_descarga_sin_buffer_todavia_no_suena() {
        let a = Availability::Downloading {
            progress: 0.0,
            playable: false,
        };
        assert!(!a.es_reproducible_ya());
    }

    #[test]
    fn un_fallo_no_reintenta_solo() {
        let a = Availability::Failed {
            reason_key: "download.no_match".into(),
            attempts: 3,
        };
        assert!(
            !a.necesita_descarga(),
            "ADR-017: el reintento debe ser explícito"
        );
        assert!(!a.es_reproducible_ya());
    }

    #[test]
    fn el_progreso_se_acota() {
        let a = Availability::Downloading {
            progress: 5.0,
            playable: true,
        };
        assert!((a.progreso() - 1.0).abs() < f32::EPSILON);
        assert!((Availability::Absent.progreso() - 0.0).abs() < f32::EPSILON);
    }
}

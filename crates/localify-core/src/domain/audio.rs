//! Value objects de audio: duración, volumen, formato, ecualizador.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::error::CoreError;

/// Duración en milisegundos.
///
/// No usamos `std::time::Duration` en la frontera serializable: su
/// representación (segundos + nanosegundos) es ruidosa en JSON y toda la
/// aplicación —SQLite, la API y el motor de audio— razona en milisegundos.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DurationMs(u32);

impl DurationMs {
    pub const ZERO: Self = Self(0);

    #[must_use]
    pub const fn new(ms: u32) -> Self {
        Self(ms)
    }

    #[must_use]
    pub const fn from_secs(s: u32) -> Self {
        Self(s.saturating_mul(1000))
    }

    #[must_use]
    pub const fn as_ms(self) -> u32 {
        self.0
    }

    #[must_use]
    pub const fn as_secs(self) -> u32 {
        self.0 / 1000
    }

    /// Diferencia absoluta. Es la operación central del scorer de YouTube:
    /// la duración es la señal más fiable para validar una coincidencia.
    #[must_use]
    pub const fn diff(self, otra: Self) -> Self {
        Self(self.0.abs_diff(otra.0))
    }

    #[must_use]
    pub const fn is_zero(self) -> bool {
        self.0 == 0
    }
}

impl fmt::Display for DurationMs {
    /// `m:ss`, o `h:mm:ss` si supera la hora.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let total = self.as_secs();
        let (h, m, s) = (total / 3600, (total % 3600) / 60, total % 60);
        if h > 0 {
            write!(f, "{h}:{m:02}:{s:02}")
        } else {
            write!(f, "{m}:{s:02}")
        }
    }
}

/// Volumen lineal en `[0.0, 1.0]`.
///
/// La curva perceptual se aplica en el motor de audio, no aquí: este tipo
/// representa lo que el usuario pide, no la ganancia que se aplica a la señal.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Volume(f32);

impl Volume {
    pub const MUTE: Self = Self(0.0);
    pub const MAX: Self = Self(1.0);

    /// Acota al rango válido en lugar de fallar: un volumen fuera de rango es
    /// un error de cliente sin consecuencias, y silenciar la app por ello
    /// sería peor que corregirlo.
    #[must_use]
    pub fn new(valor: f32) -> Self {
        Self(if valor.is_finite() {
            valor.clamp(0.0, 1.0)
        } else {
            1.0
        })
    }

    #[must_use]
    pub const fn as_f32(self) -> f32 {
        self.0
    }

    /// Ganancia perceptual. El oído responde de forma aproximadamente
    /// logarítmica: una rampa lineal se percibe concentrada en el extremo
    /// bajo. Elevar al cubo da una respuesta natural y es la aproximación
    /// habitual en reproductores.
    #[must_use]
    pub fn gain(self) -> f32 {
        self.0 * self.0 * self.0
    }
}

impl Default for Volume {
    fn default() -> Self {
        Self::MAX
    }
}

/// Formato contenedor del fichero de audio.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AudioFormat {
    Opus,
    M4a,
    Mp3,
    Flac,
    Ogg,
    Wav,
    Aiff,
}

impl AudioFormat {
    #[must_use]
    pub const fn extension(self) -> &'static str {
        match self {
            Self::Opus => "opus",
            Self::M4a => "m4a",
            Self::Mp3 => "mp3",
            Self::Flac => "flac",
            Self::Ogg => "ogg",
            Self::Wav => "wav",
            Self::Aiff => "aiff",
        }
    }

    /// `true` si el contenedor admite decodificación mientras el fichero
    /// todavía está creciendo.
    ///
    /// Es la propiedad que hace posible reproducir una descarga en curso
    /// (ADR-007). Matroska/WebM y Ogg intercalan datos en bloques secuenciales
    /// con cabecera al principio. MP3 es un flujo de tramas y también sirve.
    /// M4A es dudoso: solo funciona si el átomo `moov` está al inicio, cosa que
    /// hay que comprobar por fichero; se marca como no apto y se decide en la
    /// capa de descarga tras inspeccionarlo.
    #[must_use]
    pub const fn soporta_streaming_progresivo(self) -> bool {
        matches!(self, Self::Opus | Self::Ogg | Self::Mp3)
    }

    #[must_use]
    pub fn from_extension(ext: &str) -> Option<Self> {
        match ext.trim_start_matches('.').to_ascii_lowercase().as_str() {
            "opus" | "webm" => Some(Self::Opus),
            "m4a" | "mp4" | "aac" => Some(Self::M4a),
            "mp3" => Some(Self::Mp3),
            "flac" => Some(Self::Flac),
            "ogg" | "oga" => Some(Self::Ogg),
            "wav" => Some(Self::Wav),
            "aif" | "aiff" => Some(Self::Aiff),
            _ => None,
        }
    }
}

impl fmt::Display for AudioFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.extension())
    }
}

/// Frecuencias centrales del ecualizador, en Hz. Diez bandas en octavas,
/// el reparto estándar de los ecualizadores gráficos.
pub const BANDAS_EQ_HZ: [f32; 10] = [
    31.0, 62.0, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 16000.0,
];

/// Ganancia máxima por banda, en dB. Más allá, el limitador trabajaría de
/// forma constante y la mezcla sonaría comprimida.
pub const GANANCIA_MAX_DB: f32 = 12.0;

/// Perfil de ecualización: diez ganancias en dB.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EqProfile {
    /// Identificador estable (`flat`, `bass`, `custom`…).
    pub id: String,
    /// Clave i18n del nombre visible. Los perfiles de usuario llevan su
    /// nombre literal (ver [`EqProfile::es_predefinido`]).
    pub name_key: String,
    pub gains_db: [f32; 10],
}

impl EqProfile {
    /// # Errors
    /// Si alguna ganancia excede ±[`GANANCIA_MAX_DB`] o no es finita.
    pub fn new(
        id: impl Into<String>,
        name_key: impl Into<String>,
        gains_db: [f32; 10],
    ) -> Result<Self, CoreError> {
        for (i, g) in gains_db.iter().enumerate() {
            if !g.is_finite() || g.abs() > GANANCIA_MAX_DB {
                return Err(CoreError::invalid(format!(
                    "ganancia fuera de rango en la banda {} ({} Hz): {g} dB",
                    i, BANDAS_EQ_HZ[i]
                )));
            }
        }
        Ok(Self {
            id: id.into(),
            name_key: name_key.into(),
            gains_db,
        })
    }

    #[must_use]
    pub fn plano() -> Self {
        Self {
            id: "flat".into(),
            name_key: "eq.flat".into(),
            gains_db: [0.0; 10],
        }
    }

    /// Perfiles de fábrica. Se exponen como datos y no como código para que
    /// añadir uno nuevo no toque ninguna lógica.
    #[must_use]
    pub fn predefinidos() -> Vec<Self> {
        vec![
            Self::plano(),
            Self {
                id: "bass".into(),
                name_key: "eq.bass".into(),
                gains_db: [6.0, 5.0, 4.0, 2.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            },
            Self {
                id: "treble".into(),
                name_key: "eq.treble".into(),
                gains_db: [0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 3.0, 4.0, 5.0, 5.0],
            },
            Self {
                id: "vocal".into(),
                name_key: "eq.vocal".into(),
                gains_db: [-2.0, -1.0, 0.0, 2.0, 4.0, 4.0, 3.0, 1.0, 0.0, -1.0],
            },
            Self {
                id: "acoustic".into(),
                name_key: "eq.acoustic".into(),
                gains_db: [3.0, 3.0, 2.0, 1.0, 1.0, 1.0, 2.0, 2.0, 2.0, 1.0],
            },
            Self {
                id: "electronic".into(),
                name_key: "eq.electronic".into(),
                gains_db: [5.0, 4.0, 1.0, 0.0, -1.0, 1.0, 0.0, 1.0, 4.0, 5.0],
            },
            Self {
                id: "rock".into(),
                name_key: "eq.rock".into(),
                gains_db: [5.0, 4.0, 2.0, 0.0, -1.0, -1.0, 1.0, 3.0, 4.0, 4.0],
            },
        ]
    }

    #[must_use]
    pub fn es_predefinido(&self) -> bool {
        Self::predefinidos().iter().any(|p| p.id == self.id)
    }

    #[must_use]
    pub fn es_plano(&self) -> bool {
        self.gains_db.iter().all(|g| g.abs() < f32::EPSILON)
    }
}

impl Default for EqProfile {
    fn default() -> Self {
        Self::plano()
    }
}

/// Dispositivo de salida de audio.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioDevice {
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn la_duracion_se_formatea_como_en_spotify() {
        assert_eq!(DurationMs::new(0).to_string(), "0:00");
        assert_eq!(DurationMs::new(65_000).to_string(), "1:05");
        assert_eq!(DurationMs::new(354_000).to_string(), "5:54");
        assert_eq!(DurationMs::new(3_723_000).to_string(), "1:02:03");
    }

    #[test]
    fn diff_es_simetrica_y_no_desborda() {
        let a = DurationMs::new(1000);
        let b = DurationMs::new(4000);
        assert_eq!(a.diff(b), DurationMs::new(3000));
        assert_eq!(b.diff(a), DurationMs::new(3000));
    }

    #[test]
    fn el_volumen_se_acota_en_lugar_de_fallar() {
        assert_eq!(Volume::new(2.0), Volume::MAX);
        assert_eq!(Volume::new(-1.0), Volume::MUTE);
        assert!((Volume::new(f32::NAN).as_f32() - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn la_ganancia_es_monotona_y_respeta_los_extremos() {
        assert!((Volume::MUTE.gain() - 0.0).abs() < f32::EPSILON);
        assert!((Volume::MAX.gain() - 1.0).abs() < f32::EPSILON);
        assert!(
            Volume::new(0.5).gain() < 0.5,
            "la curva debe atenuar el tramo medio"
        );
    }

    #[test]
    fn solo_los_formatos_en_flujo_admiten_reproduccion_progresiva() {
        assert!(AudioFormat::Opus.soporta_streaming_progresivo());
        assert!(AudioFormat::Mp3.soporta_streaming_progresivo());
        // M4A depende de dónde esté el átomo `moov`: se decide por fichero.
        assert!(!AudioFormat::M4a.soporta_streaming_progresivo());
        assert!(!AudioFormat::Flac.soporta_streaming_progresivo());
    }

    #[test]
    fn webm_se_reconoce_como_opus() {
        assert_eq!(AudioFormat::from_extension("webm"), Some(AudioFormat::Opus));
        assert_eq!(
            AudioFormat::from_extension(".FLAC"),
            Some(AudioFormat::Flac)
        );
        assert_eq!(AudioFormat::from_extension("txt"), None);
    }

    #[test]
    fn el_eq_rechaza_ganancias_fuera_de_rango() {
        assert!(EqProfile::new("x", "x", [0.0; 10]).is_ok());
        let mut excesivo = [0.0; 10];
        excesivo[3] = GANANCIA_MAX_DB + 0.1;
        assert!(EqProfile::new("x", "x", excesivo).is_err());
    }

    #[test]
    fn los_perfiles_predefinidos_son_validos_y_unicos() {
        let perfiles = EqProfile::predefinidos();
        let ids: std::collections::HashSet<_> = perfiles.iter().map(|p| &p.id).collect();
        assert_eq!(ids.len(), perfiles.len(), "hay ids de perfil duplicados");
        for p in &perfiles {
            assert!(
                EqProfile::new(&p.id, &p.name_key, p.gains_db).is_ok(),
                "el perfil predefinido '{}' excede el rango permitido",
                p.id
            );
        }
    }
}

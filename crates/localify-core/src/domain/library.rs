//! Materialización local de la biblioteca: ficheros, estadísticas e historial.

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::audio::{AudioFormat, DurationMs};
use super::ids::TrackId;

/// Procedencia de un fichero de audio.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AudioSource {
    #[default]
    Youtube,
    /// Fichero que ya tenía el usuario y se incorporó al escanear.
    Imported,
}

/// Un fichero de audio presente en disco.
///
/// **Su existencia es la definición de "la pista está en mi biblioteca".** Si
/// hay fila aquí, hay fichero completo y verificado; si no, no lo hay. No
/// existe un estado intermedio persistido: los ficheros a medias viven en
/// `.tmp/` y jamás se registran.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioFileRecord {
    pub track_id: TrackId,
    /// **Relativa** a la raíz de la biblioteca (ADR-018). Nunca absoluta:
    /// cambiar de carpeta no debe obligar a reescribir 50 000 filas.
    pub rel_path: PathBuf,
    pub format: AudioFormat,
    pub codec: String,
    pub bitrate_kbps: Option<u32>,
    pub sample_rate: Option<u32>,
    pub channels: Option<u8>,
    pub size_bytes: u64,
    /// Duración **real medida en el fichero**, que puede diferir de la que
    /// declara Spotify. Comparar ambas es la verificación post-descarga.
    pub duration: DurationMs,
    pub source: AudioSource,
    pub youtube_id: Option<String>,
    pub verified_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryStats {
    pub track_count: u64,
    pub local_count: u64,
    pub album_count: u64,
    pub artist_count: u64,
    pub total_duration_ms: u64,
    pub total_bytes: u64,
    /// Canciones cuya descarga falló y sigue fallada.
    ///
    /// Es el único sitio donde ese número aparece. Las descargas son invisibles
    /// por diseño y las listas no dicen si una canción está en disco, así que sin
    /// esto un fallo de emparejamiento no se veía en ninguna parte: la canción
    /// simplemente no sonaba, y no había forma de pedir que se reintentara.
    pub failed_count: u64,
}

/// Resultado de reconciliar disco y base de datos.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanReport {
    pub files_scanned: u32,
    /// Ficheros en disco que faltaban en la base de datos y se han recuperado
    /// leyendo sus etiquetas.
    pub recovered: u32,
    /// Filas cuyo fichero ya no existe; la pista pasa a `Absent`.
    pub missing: u32,
    /// Ficheros ilegibles o sin metadatos utilizables.
    pub unreadable: u32,
    pub duration_ms: u64,
}

/// Una reproducción registrada.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayHistoryEntry {
    pub track_id: TrackId,
    pub played_at: DateTime<Utc>,
    pub ms_played: u32,
    /// `true` si se escuchó prácticamente entera. Es la señal positiva del
    /// motor de recomendaciones; su ausencia, la negativa.
    pub completed: bool,
    pub context: Option<String>,
}

/// Fracción de la pista a partir de la cual se considera escuchada por completo.
pub const UMBRAL_COMPLETADA: f32 = 0.9;

impl PlayHistoryEntry {
    /// Decide si una escucha cuenta como completada.
    #[must_use]
    #[allow(clippy::cast_precision_loss)]
    pub fn cuenta_como_completada(ms_played: u32, duration: DurationMs) -> bool {
        if duration.is_zero() {
            return false;
        }
        (ms_played as f32 / duration.as_ms() as f32) >= UMBRAL_COMPLETADA
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn una_escucha_casi_entera_cuenta_como_completada() {
        let dur = DurationMs::new(200_000);
        assert!(PlayHistoryEntry::cuenta_como_completada(190_000, dur));
        assert!(PlayHistoryEntry::cuenta_como_completada(200_000, dur));
    }

    #[test]
    fn un_salto_temprano_no_cuenta() {
        let dur = DurationMs::new(200_000);
        assert!(!PlayHistoryEntry::cuenta_como_completada(20_000, dur));
        assert!(!PlayHistoryEntry::cuenta_como_completada(179_000, dur));
    }

    #[test]
    fn una_duracion_desconocida_nunca_cuenta() {
        assert!(!PlayHistoryEntry::cuenta_como_completada(
            10_000,
            DurationMs::ZERO
        ));
    }
}

//! Emparejamiento con YouTube y estado de descarga.
//!
//! Todo lo de este módulo es un **detalle de la capa de obtención de audio**.
//! El ID de YouTube nunca asciende a clave del dominio: la pista se identifica
//! por su ID de Spotify, y esto es una caché de "dónde conseguir sus bytes".

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::audio::DurationMs;
use super::ids::TrackId;

/// Cuánta confianza merece un emparejamiento.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Confidence {
    /// Por debajo del umbral: **no se descarga** (ADR-017). Descargar aquí
    /// metería karaokes y covers en la biblioteca de forma permanente, porque
    /// una pista ya descargada nunca se vuelve a descargar.
    Low,
    /// Se descarga y se registra por si el usuario quiere revisarlo.
    Medium,
    /// Se descarga sin más.
    High,
}

/// Umbrales de puntuación. Son datos, no código: ajustarlos no toca lógica.
pub const UMBRAL_ALTA: f32 = 75.0;
pub const UMBRAL_MEDIA: f32 = 55.0;

impl Confidence {
    #[must_use]
    pub fn desde_puntuacion(score: f32) -> Self {
        if score >= UMBRAL_ALTA {
            Self::High
        } else if score >= UMBRAL_MEDIA {
            Self::Medium
        } else {
            Self::Low
        }
    }

    /// `true` si el emparejamiento es suficientemente bueno para descargar sin
    /// preguntar.
    #[must_use]
    pub const fn permite_descarga_automatica(self) -> bool {
        matches!(self, Self::High | Self::Medium)
    }
}

/// Desglose de la puntuación, para poder explicar por qué ganó un candidato.
///
/// Se persiste en `youtube_matches.breakdown`. Sin esto, depurar un mal
/// emparejamiento es adivinar.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScoreBreakdown {
    /// Multiplicador por diferencia de duración, en `[0.0, 1.0]`.
    pub duration_factor: f32,
    pub duration_diff_ms: u32,
    pub source_bonus: f32,
    pub title_bonus: f32,
    pub artist_bonus: f32,
    pub album_bonus: f32,
    /// Suma de penalizaciones, siempre negativa o cero.
    pub penalties: f32,
    /// Etiquetas de las penalizaciones aplicadas (`live`, `karaoke`…).
    pub penalty_reasons: Vec<String>,
    pub total: f32,
}

/// Un candidato de YouTube ya puntuado.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct YoutubeCandidate {
    pub video_id: String,
    pub title: String,
    pub channel: Option<String>,
    pub duration: DurationMs,
    pub view_count: Option<u64>,
    /// `true` si procede de music.youtube.com.
    pub from_youtube_music: bool,
    pub score: f32,
    pub breakdown: ScoreBreakdown,
}

/// Resultado del emparejamiento.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MatchResult {
    pub track_id: TrackId,
    pub best: YoutubeCandidate,
    pub confidence: Confidence,
    pub candidates_considered: u16,
}

/// Carril de descarga. Reproducir tiene prioridad sobre precargar; nunca al
/// revés.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Priority {
    /// El usuario pulsó play y está esperando.
    Immediate,
    /// Siguiente en la cola. Cede ancho de banda al carril inmediato.
    #[default]
    Prefetch,
}

/// Estado de un trabajo de descarga.
///
/// Nótese que **no hay `Paused` ni `Cancelled`**: no existen en el diseño
/// (ADR-016). Un trabajo solo termina completándose o fallando.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DownloadState {
    Queued,
    /// Buscando el mejor candidato en YouTube.
    Matching,
    Downloading,
    /// Descargado; verificando, etiquetando y renombrando.
    Finalizing,
    Done,
    Failed,
}

impl DownloadState {
    #[must_use]
    pub const fn es_terminal(self) -> bool {
        matches!(self, Self::Done | Self::Failed)
    }

    /// `true` si el trabajo estaba a medias cuando se cerró la aplicación y
    /// hay que reencolarlo al arrancar.
    #[must_use]
    pub const fn debe_reencolarse_al_arrancar(self) -> bool {
        matches!(self, Self::Matching | Self::Downloading | Self::Finalizing)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadJob {
    pub track_id: TrackId,
    pub state: DownloadState,
    pub priority: Priority,
    pub video_id: Option<String>,
    pub tmp_path: Option<PathBuf>,
    pub bytes_done: u64,
    pub bytes_total: Option<u64>,
    pub attempts: u8,
    /// Clave i18n del error, no texto para el usuario (ADR-012).
    pub last_error_key: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadProgress {
    pub bytes_done: u64,
    pub bytes_total: Option<u64>,
    /// `true` cuando hay buffer suficiente para empezar a sonar.
    pub playable: bool,
    pub state: DownloadState,
}

impl DownloadProgress {
    /// Progreso en `[0.0, 1.0]`, o `None` si aún se desconoce el tamaño total.
    #[must_use]
    #[allow(clippy::cast_precision_loss)]
    pub fn fraccion(&self) -> Option<f32> {
        let total = self.bytes_total?;
        if total == 0 {
            return None;
        }
        Some((self.bytes_done as f32 / total as f32).clamp(0.0, 1.0))
    }
}

/// Bytes mínimos antes de considerar reproducible un fichero parcial.
///
/// A ~160 kbps son unos 15 segundos de audio: margen suficiente para absorber
/// una red irregular sin que el usuario espere de más.
pub const BYTES_MINIMOS_REPRODUCIBLE: u64 = 300 * 1024;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn los_umbrales_de_confianza_son_los_documentados() {
        assert_eq!(Confidence::desde_puntuacion(91.0), Confidence::High);
        assert_eq!(Confidence::desde_puntuacion(UMBRAL_ALTA), Confidence::High);
        assert_eq!(Confidence::desde_puntuacion(60.0), Confidence::Medium);
        assert_eq!(
            Confidence::desde_puntuacion(UMBRAL_MEDIA),
            Confidence::Medium
        );
        assert_eq!(Confidence::desde_puntuacion(54.9), Confidence::Low);
        assert_eq!(Confidence::desde_puntuacion(0.0), Confidence::Low);
    }

    #[test]
    fn la_confianza_baja_nunca_descarga_sola() {
        assert!(!Confidence::Low.permite_descarga_automatica());
        assert!(Confidence::Medium.permite_descarga_automatica());
        assert!(Confidence::High.permite_descarga_automatica());
    }

    #[test]
    fn inmediato_tiene_mas_prioridad_que_prefetch() {
        assert!(
            Priority::Immediate < Priority::Prefetch,
            "el orden ordena la cola de trabajos"
        );
    }

    #[test]
    fn los_estados_a_medias_se_reencolan_al_arrancar() {
        assert!(DownloadState::Downloading.debe_reencolarse_al_arrancar());
        assert!(DownloadState::Finalizing.debe_reencolarse_al_arrancar());
        assert!(!DownloadState::Done.debe_reencolarse_al_arrancar());
        assert!(!DownloadState::Queued.debe_reencolarse_al_arrancar());
    }

    #[test]
    fn la_fraccion_es_none_sin_tamanyo_conocido() {
        let p = DownloadProgress {
            bytes_done: 1024,
            bytes_total: None,
            playable: false,
            state: DownloadState::Downloading,
        };
        assert_eq!(p.fraccion(), None);
    }

    #[test]
    fn la_fraccion_se_acota_aunque_el_total_mienta() {
        let p = DownloadProgress {
            bytes_done: 200,
            bytes_total: Some(100),
            playable: true,
            state: DownloadState::Downloading,
        };
        assert_eq!(p.fraccion(), Some(1.0));
    }
}

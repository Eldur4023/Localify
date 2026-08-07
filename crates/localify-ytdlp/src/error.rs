//! Errores de la capa de obtención de audio.

use localify_core::error::CoreError;

pub type YtDlpResult<T> = Result<T, YtDlpError>;

#[derive(Debug, thiserror::Error)]
pub enum YtDlpError {
    /// El binario no está disponible.
    ///
    /// Es accionable: se descarga en la primera ejecución o con
    /// `scripts/fetch-sidecars.ps1`.
    #[error("no se encontró el binario '{0}'")]
    SinBinario(&'static str),

    /// El proceso terminó con error.
    #[error("{binario} terminó con código {codigo}: {detalle}")]
    Proceso {
        binario: &'static str,
        codigo: i32,
        detalle: String,
    },

    /// yt-dlp no encontró el vídeo, o YouTube lo retiró.
    #[error("el vídeo no está disponible")]
    VideoNoDisponible,

    /// yt-dlp ya no entiende la respuesta de YouTube.
    ///
    /// Se distingue porque tiene una reparación concreta: actualizar el
    /// sidecar, que es lo que hace la aplicación antes de reintentar.
    #[error("yt-dlp parece desactualizado")]
    ExtractorObsoleto,

    /// La salida no tiene la forma esperada.
    #[error("salida de {binario} ilegible: {detalle}")]
    Salida {
        binario: &'static str,
        detalle: String,
    },

    /// No se encontró ningún candidato con confianza suficiente.
    ///
    /// **No se descarga nada** (ADR-017): meter un karaoke en la biblioteca
    /// sería permanente, porque lo descargado no se vuelve a descargar.
    #[error("sin coincidencia fiable para la pista")]
    SinCoincidencia,

    /// El fichero descargado no supera la verificación.
    #[error("el fichero descargado no es válido: {0}")]
    VerificacionFallida(String),

    #[error("error de entrada/salida")]
    Io(#[from] std::io::Error),
}

impl YtDlpError {
    /// `true` si reintentar puede funcionar.
    #[must_use]
    pub const fn es_reintentable(&self) -> bool {
        matches!(
            self,
            Self::Proceso { .. } | Self::Io(_) | Self::ExtractorObsoleto
        )
    }

    /// Clave i18n del motivo, para mostrarlo en la interfaz.
    ///
    /// El backend no traduce (ADR-012).
    #[must_use]
    pub const fn clave(&self) -> &'static str {
        match self {
            Self::SinBinario(_) => "download.no_sidecar",
            Self::VideoNoDisponible => "download.video_unavailable",
            Self::ExtractorObsoleto => "download.extractor_outdated",
            Self::SinCoincidencia => "download.no_match",
            Self::VerificacionFallida(_) => "download.corrupt",
            Self::Proceso { .. } | Self::Salida { .. } | Self::Io(_) => "download.failed",
        }
    }
}

impl From<YtDlpError> for CoreError {
    fn from(e: YtDlpError) -> Self {
        match e {
            YtDlpError::SinBinario(nombre) => Self::NotConfigured(match nombre {
                "yt-dlp" => "sidecar.yt_dlp",
                _ => "sidecar.ffmpeg",
            }),
            YtDlpError::SinCoincidencia => Self::not_found("youtube_match", "sin candidatos"),
            otro => Self::ProviderUnavailable {
                provider: "youtube",
                source: Some(Box::new(otro)),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cada_error_tiene_su_clave_de_traduccion() {
        let casos = [
            (YtDlpError::SinBinario("yt-dlp"), "download.no_sidecar"),
            (YtDlpError::VideoNoDisponible, "download.video_unavailable"),
            (YtDlpError::ExtractorObsoleto, "download.extractor_outdated"),
            (YtDlpError::SinCoincidencia, "download.no_match"),
            (
                YtDlpError::VerificacionFallida("x".into()),
                "download.corrupt",
            ),
        ];
        for (error, clave) in casos {
            assert_eq!(error.clave(), clave);
        }
    }

    #[test]
    fn un_extractor_obsoleto_es_reintentable_tras_actualizar() {
        assert!(YtDlpError::ExtractorObsoleto.es_reintentable());
    }

    #[test]
    fn la_falta_de_coincidencia_no_se_reintenta_sola() {
        // Reintentar en bucle gastaría red y acabaría metiendo basura.
        assert!(!YtDlpError::SinCoincidencia.es_reintentable());
        assert!(!YtDlpError::VideoNoDisponible.es_reintentable());
    }

    #[test]
    fn la_falta_del_binario_es_accionable_por_el_usuario() {
        let core: CoreError = YtDlpError::SinBinario("yt-dlp").into();
        assert_eq!(core.code(), "NOT_CONFIGURED");
        assert!(core.is_user_actionable());
    }
}

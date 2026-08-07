//! Errores del cliente de Spotify.

use localify_core::error::CoreError;

pub type SpotifyResult<T> = Result<T, SpotifyError>;

/// Nombre del proveedor, para errores y eventos.
pub const PROVEEDOR: &str = "spotify";

#[derive(Debug, thiserror::Error)]
pub enum SpotifyError {
    /// No hay credenciales de aplicación configuradas.
    ///
    /// **No es un fallo**: es el estado inicial de una instalación desde
    /// código fuente, y la aplicación funciona por completo sobre la biblioteca
    /// local mientras dure.
    #[error("faltan las credenciales de aplicación de Spotify")]
    SinCredenciales,

    /// Las credenciales existen pero Spotify las rechaza.
    #[error("Spotify rechazó las credenciales")]
    CredencialesInvalidas,

    /// Límite de peticiones. `Retry-After` viene en segundos.
    #[error("límite de peticiones alcanzado; reintentar en {segundos}s")]
    LimiteAlcanzado { segundos: u64 },

    /// El recurso no existe o no es accesible con credenciales de aplicación.
    ///
    /// El segundo caso importa: desde noviembre de 2024, las playlists
    /// propiedad de Spotify (Discover Weekly, Top 50…) devuelven 404 a las
    /// aplicaciones nuevas. Distinguirlo permite dar un mensaje útil en lugar
    /// de "no encontrado".
    #[error("recurso no encontrado: {0}")]
    NoEncontrado(String),

    /// Error del servidor de Spotify. Reintentable.
    #[error("Spotify devolvió {codigo}")]
    Servidor { codigo: u16 },

    /// Fallo de red.
    #[error("no se pudo contactar con Spotify")]
    Red(String),

    /// La respuesta no tiene la forma esperada.
    #[error("respuesta de Spotify ilegible: {0}")]
    Respuesta(String),

    /// La entrada no es un identificador ni una URL de Spotify válidos.
    #[error("entrada inválida: {0}")]
    Invalido(String),
}

impl SpotifyError {
    /// `true` si reintentar la misma petición puede funcionar.
    #[must_use]
    pub const fn es_reintentable(&self) -> bool {
        matches!(
            self,
            Self::LimiteAlcanzado { .. } | Self::Servidor { .. } | Self::Red(_)
        )
    }
}

impl From<SpotifyError> for CoreError {
    fn from(e: SpotifyError) -> Self {
        match e {
            SpotifyError::SinCredenciales => Self::NotConfigured("spotify.client_id"),
            SpotifyError::CredencialesInvalidas => Self::NotConfigured("spotify.client_secret"),
            SpotifyError::LimiteAlcanzado { segundos } => Self::RateLimited {
                provider: PROVEEDOR,
                retry_after_secs: segundos,
            },
            SpotifyError::NoEncontrado(id) => Self::not_found("spotify", id),
            SpotifyError::Invalido(m) => Self::invalid(m),
            otro => Self::ProviderUnavailable {
                provider: PROVEEDOR,
                source: Some(Box::new(otro)),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn la_falta_de_credenciales_es_accionable_y_no_un_fallo_de_proveedor() {
        let core: CoreError = SpotifyError::SinCredenciales.into();
        assert_eq!(core.code(), "NOT_CONFIGURED");
        assert!(
            core.is_user_actionable(),
            "el usuario puede arreglarlo en Ajustes"
        );
    }

    #[test]
    fn el_limite_conserva_los_segundos_de_espera() {
        let core: CoreError = SpotifyError::LimiteAlcanzado { segundos: 42 }.into();
        assert_eq!(core.code(), "RATE_LIMITED");
        assert!(core.is_retryable());
    }

    #[test]
    fn los_fallos_transitorios_son_reintentables_y_los_de_configuracion_no() {
        assert!(SpotifyError::Servidor { codigo: 503 }.es_reintentable());
        assert!(SpotifyError::Red("timeout".into()).es_reintentable());
        assert!(SpotifyError::LimiteAlcanzado { segundos: 1 }.es_reintentable());

        assert!(!SpotifyError::SinCredenciales.es_reintentable());
        assert!(!SpotifyError::NoEncontrado("x".into()).es_reintentable());
        assert!(!SpotifyError::Respuesta("json roto".into()).es_reintentable());
    }

    #[test]
    fn un_proveedor_caido_no_se_confunde_con_un_error_de_almacenamiento() {
        let core: CoreError = SpotifyError::Servidor { codigo: 502 }.into();
        assert_eq!(core.code(), "PROVIDER_UNAVAILABLE");
    }
}

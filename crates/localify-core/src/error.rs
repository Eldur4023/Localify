//! Modelo de errores del dominio.
//!
//! Cada crate de infraestructura define su propio error con `thiserror` y lo
//! convierte a [`CoreError`] en la frontera del puerto. En la frontera Tauri,
//! `localify-app` convierte [`CoreError`] a un DTO estable.
//!
//! Los mensajes son para desarrolladores (logs). Lo que ve el usuario se
//! resuelve a partir de [`CoreError::message_key`], que el frontend traduce
//! (ADR-012: el backend no traduce).

use std::error::Error as StdError;

pub type CoreResult<T> = Result<T, CoreError>;

/// Error boxeado y transportable entre hilos.
pub type BoxError = Box<dyn StdError + Send + Sync + 'static>;

#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    /// La entidad solicitada no existe.
    #[error("no encontrado: {entity} '{id}'")]
    NotFound { entity: &'static str, id: String },

    /// La entrada del usuario o del cliente no es válida. No se ha modificado
    /// nada: las validaciones ocurren antes de cualquier escritura.
    #[error("entrada inválida: {0}")]
    Invalid(String),

    /// La operación choca con el estado actual (duplicado, transición
    /// imposible, recurso ocupado).
    #[error("conflicto: {0}")]
    Conflict(String),

    /// Un proveedor externo (Spotify, YouTube, LRCLIB) no responde. No es un
    /// fallo de la aplicación: es un modo de operación degradado previsto.
    #[error("proveedor '{provider}' no disponible")]
    ProviderUnavailable {
        provider: &'static str,
        #[source]
        source: Option<BoxError>,
    },

    /// El proveedor ha aplicado limitación de peticiones.
    #[error("límite de peticiones de '{provider}'; reintentar en {retry_after_secs}s")]
    RateLimited {
        provider: &'static str,
        retry_after_secs: u64,
    },

    /// Falta configuración obligatoria (p. ej. credenciales de Spotify).
    /// Es accionable por el usuario, no un error del programa.
    #[error("falta configuración: {0}")]
    NotConfigured(&'static str),

    /// Fallo de persistencia o de sistema de ficheros.
    #[error("error de almacenamiento")]
    Storage(#[source] BoxError),

    /// Fallo del subsistema de audio.
    #[error("error de audio")]
    Audio(#[source] BoxError),

    /// La operación fue abortada porque el componente se está cerrando.
    #[error("operación cancelada: la aplicación se está cerrando")]
    ShuttingDown,

    /// Error no clasificado. Su aparición en logs es señal de que falta una
    /// variante específica.
    #[error("error interno")]
    Internal(#[source] BoxError),
}

impl CoreError {
    /// Código estable para el cliente. Nunca cambia sin un `major` de la API.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::NotFound { .. } => "NOT_FOUND",
            Self::Invalid(_) => "INVALID",
            Self::Conflict(_) => "CONFLICT",
            Self::ProviderUnavailable { .. } => "PROVIDER_UNAVAILABLE",
            Self::RateLimited { .. } => "RATE_LIMITED",
            Self::NotConfigured(_) => "NOT_CONFIGURED",
            Self::Storage(_) => "STORAGE",
            Self::Audio(_) => "AUDIO",
            Self::ShuttingDown => "SHUTTING_DOWN",
            Self::Internal(_) => "INTERNAL",
        }
    }

    /// Clave i18n que el frontend traduce. El backend nunca produce texto
    /// destinado al usuario final (ADR-012).
    #[must_use]
    pub const fn message_key(&self) -> &'static str {
        match self {
            Self::NotFound { .. } => "error.not_found",
            Self::Invalid(_) => "error.invalid",
            Self::Conflict(_) => "error.conflict",
            Self::ProviderUnavailable { .. } => "error.provider_unavailable",
            Self::RateLimited { .. } => "error.rate_limited",
            Self::NotConfigured(_) => "error.not_configured",
            Self::Storage(_) => "error.storage",
            Self::Audio(_) => "error.audio",
            Self::ShuttingDown => "error.shutting_down",
            Self::Internal(_) => "error.internal",
        }
    }

    /// Si `true`, reintentar la misma operación puede tener éxito.
    #[must_use]
    pub const fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::ProviderUnavailable { .. } | Self::RateLimited { .. } | Self::Storage(_)
        )
    }

    /// Si `true`, el usuario puede resolverlo desde Ajustes. La UI lo usa para
    /// ofrecer un enlace directo en lugar de un mensaje de error genérico.
    #[must_use]
    pub const fn is_user_actionable(&self) -> bool {
        matches!(self, Self::NotConfigured(_) | Self::Invalid(_))
    }

    pub fn not_found(entity: &'static str, id: impl Into<String>) -> Self {
        Self::NotFound {
            entity,
            id: id.into(),
        }
    }

    pub fn invalid(msg: impl Into<String>) -> Self {
        Self::Invalid(msg.into())
    }

    pub fn conflict(msg: impl Into<String>) -> Self {
        Self::Conflict(msg.into())
    }

    pub fn storage(err: impl Into<BoxError>) -> Self {
        Self::Storage(err.into())
    }

    pub fn internal(err: impl Into<BoxError>) -> Self {
        Self::Internal(err.into())
    }

    pub fn provider_unavailable(provider: &'static str, err: impl Into<BoxError>) -> Self {
        Self::ProviderUnavailable {
            provider,
            source: Some(err.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn los_codigos_son_estables_y_distintos() {
        let variantes = [
            CoreError::not_found("track", "abc"),
            CoreError::invalid("x"),
            CoreError::conflict("x"),
            CoreError::ProviderUnavailable {
                provider: "spotify",
                source: None,
            },
            CoreError::RateLimited {
                provider: "spotify",
                retry_after_secs: 1,
            },
            CoreError::NotConfigured("spotify.client_id"),
            CoreError::ShuttingDown,
        ];
        let codigos: Vec<_> = variantes.iter().map(CoreError::code).collect();
        let unicos: std::collections::HashSet<_> = codigos.iter().collect();
        assert_eq!(
            codigos.len(),
            unicos.len(),
            "dos variantes comparten código"
        );
    }

    #[test]
    fn falta_de_configuracion_es_accionable_pero_no_reintentable() {
        let err = CoreError::NotConfigured("spotify.client_id");
        assert!(err.is_user_actionable());
        assert!(!err.is_retryable());
    }
}

//! Tipos comunes de la API: errores, paginación y disponibilidad.

use localify_core::domain::availability::Availability;
use localify_core::error::CoreError;
use localify_core::page::{Cursor, Page, PageRequest};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Error tal como lo ve el cliente.
///
/// **No lleva texto para el usuario final**: lleva un código estable y una
/// clave i18n que el frontend traduce (ADR-012). Así el backend permanece
/// agnóstico del idioma y la API sirve a cualquier cliente.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "types.gen.ts")]
#[serde(rename_all = "camelCase")]
pub struct ApiError {
    /// Código estable. No cambia sin un `major` de la API.
    pub code: String,
    /// Clave i18n del mensaje.
    pub message_key: String,
    /// Parámetros de interpolación del mensaje.
    pub params: Vec<(String, String)>,
    /// `true` si el usuario puede resolverlo desde Ajustes.
    pub actionable: bool,
    /// `true` si reintentar tiene sentido.
    pub retryable: bool,
    /// Detalle técnico. **Solo en compilaciones de depuración**: en release
    /// podría filtrar rutas o fragmentos de credenciales a la interfaz.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl From<CoreError> for ApiError {
    fn from(e: CoreError) -> Self {
        let params = match &e {
            CoreError::NotFound { entity, id } => vec![
                ("entity".to_owned(), (*entity).to_owned()),
                ("id".to_owned(), id.clone()),
            ],
            CoreError::RateLimited {
                provider,
                retry_after_secs,
            } => vec![
                ("provider".to_owned(), (*provider).to_owned()),
                ("retryAfter".to_owned(), retry_after_secs.to_string()),
            ],
            CoreError::ProviderUnavailable { provider, .. } => {
                vec![("provider".to_owned(), (*provider).to_owned())]
            }
            CoreError::NotConfigured(que) => vec![("setting".to_owned(), (*que).to_owned())],
            _ => Vec::new(),
        };

        Self {
            code: e.code().to_owned(),
            message_key: e.message_key().to_owned(),
            params,
            actionable: e.is_user_actionable(),
            retryable: e.is_retryable(),
            detail: if cfg!(debug_assertions) {
                Some(e.to_string())
            } else {
                None
            },
        }
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message_key)
    }
}

impl std::error::Error for ApiError {}

/// Página de resultados.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "types.gen.ts")]
#[serde(rename_all = "camelCase")]
pub struct PageDto<T> {
    pub items: Vec<T>,
    /// `null` cuando contar sería caro y no aporta. Solo viene en la primera
    /// página; al seguir desplazándose, el cliente ya lo conoce.
    pub total: Option<u64>,
    /// `null` cuando no hay más resultados.
    pub next_cursor: Option<String>,
}

impl<T> PageDto<T> {
    /// Convierte una página del dominio aplicando `f` a cada elemento.
    pub fn desde<U>(page: Page<U>, f: impl FnMut(U) -> T) -> Self {
        Self {
            items: page.items.into_iter().map(f).collect(),
            total: page.total,
            next_cursor: page.next_cursor.map(|c| c.0),
        }
    }

    #[must_use]
    pub const fn vacia() -> Self {
        Self {
            items: Vec::new(),
            total: Some(0),
            next_cursor: None,
        }
    }
}

/// Petición de paginación.
#[derive(Debug, Clone, Default, Deserialize, TS)]
#[ts(export, export_to = "types.gen.ts")]
#[serde(rename_all = "camelCase", default)]
pub struct PageRequestDto {
    pub offset: u32,
    pub limit: Option<u32>,
    pub cursor: Option<String>,
}

impl From<PageRequestDto> for PageRequest {
    fn from(d: PageRequestDto) -> Self {
        Self {
            offset: d.offset,
            limit: d.limit,
            cursor: d.cursor.map(Cursor::new),
        }
    }
}

impl From<&PageRequestDto> for PageRequest {
    fn from(d: &PageRequestDto) -> Self {
        d.clone().into()
    }
}

/// Disponibilidad local de una pista.
#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[ts(export, export_to = "types.gen.ts")]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum AvailabilityDto {
    /// Solo metadatos. Pulsar play iniciará la descarga, de forma invisible.
    Absent,
    #[serde(rename_all = "camelCase")]
    Downloading { progress: f32, playable: bool },
    #[serde(rename_all = "camelCase")]
    Local { format: String, bytes: u64 },
    #[serde(rename_all = "camelCase")]
    Failed { reason_key: String, attempts: u8 },
}

impl From<Availability> for AvailabilityDto {
    fn from(a: Availability) -> Self {
        match a {
            Availability::Absent => Self::Absent,
            Availability::Downloading { progress, playable } => {
                Self::Downloading { progress, playable }
            }
            // La ruta **no** se expone: el WebView no accede al sistema de
            // ficheros (ver capabilities), así que dársela solo serviría para
            // filtrar la estructura del disco del usuario.
            Availability::Local { format, bytes, .. } => Self::Local {
                format: format.extension().to_owned(),
                bytes,
            },
            Availability::Failed {
                reason_key,
                attempts,
            } => Self::Failed {
                reason_key,
                attempts,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn el_error_conserva_codigo_clave_y_parametros() {
        let api: ApiError = CoreError::not_found("track", "abc123").into();
        assert_eq!(api.code, "NOT_FOUND");
        assert_eq!(api.message_key, "error.not_found");
        assert!(api.params.iter().any(|(k, v)| k == "id" && v == "abc123"));
        assert!(!api.retryable);
    }

    #[test]
    fn falta_de_configuracion_llega_como_accionable() {
        let api: ApiError = CoreError::NotConfigured("spotify.client_id").into();
        assert!(
            api.actionable,
            "la UI debe poder ofrecer un enlace a Ajustes"
        );
        assert!(!api.retryable);
    }

    #[test]
    fn el_limite_de_peticiones_llega_como_reintentable_con_su_espera() {
        let api: ApiError = CoreError::RateLimited {
            provider: "spotify",
            retry_after_secs: 30,
        }
        .into();
        assert!(api.retryable);
        assert!(
            api.params
                .iter()
                .any(|(k, v)| k == "retryAfter" && v == "30")
        );
    }

    #[test]
    fn la_disponibilidad_local_no_expone_la_ruta_en_disco() {
        let dto: AvailabilityDto = Availability::Local {
            rel_path: std::path::PathBuf::from("audio/3z/secreto.opus"),
            format: localify_core::domain::audio::AudioFormat::Opus,
            bytes: 4_000_000,
        }
        .into();

        let json = serde_json::to_string(&dto).expect("serializa");
        assert!(
            !json.contains("secreto"),
            "la ruta no debe cruzar el puente: {json}"
        );
        assert!(json.contains("opus"));
    }

    #[test]
    fn las_variantes_de_disponibilidad_llevan_discriminante() {
        let json = serde_json::to_string(&AvailabilityDto::Absent).expect("serializa");
        assert_eq!(json, r#"{"kind":"absent"}"#);
    }

    #[test]
    fn la_pagina_conserva_total_y_cursor() {
        let page = Page::new(vec![1_u32, 2, 3], Some(42), Some(Cursor::new("x")));
        let dto = PageDto::desde(page, |n| n.to_string());
        assert_eq!(dto.items, vec!["1", "2", "3"]);
        assert_eq!(dto.total, Some(42));
        assert_eq!(dto.next_cursor.as_deref(), Some("x"));
    }

    #[test]
    fn la_peticion_de_pagina_acota_el_limite() {
        let dto = PageRequestDto {
            offset: 0,
            limit: Some(100_000),
            cursor: None,
        };
        let req: PageRequest = dto.into();
        assert_eq!(req.limit(), localify_core::page::LIMITE_MAXIMO);
    }
}

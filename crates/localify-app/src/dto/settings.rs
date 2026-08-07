//! DTOs de configuración.
//!
//! **Ningún secreto cruza el puente.** El `client_secret` de Spotify y la
//! sesión de Last.fm viven en el almacén del sistema; aquí solo se informa de
//! si están configurados.

use localify_core::domain::audio::{AudioDevice, EqProfile};
use localify_core::domain::settings::{
    AudioSettings, DownloadSettings, FormatPreference, IntegrationSettings, Language, ListDensity,
    MetadataProviderKind, Settings, SettingsPatch, UiSettings,
};
use localify_core::events::ProviderStatus;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "types.gen.ts")]
#[serde(rename_all = "camelCase")]
pub struct EqProfileDto {
    pub id: String,
    pub name_key: String,
    pub gains_db: Vec<f32>,
}

impl From<EqProfile> for EqProfileDto {
    fn from(p: EqProfile) -> Self {
        Self {
            id: p.id,
            name_key: p.name_key,
            gains_db: p.gains_db.to_vec(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export, export_to = "types.gen.ts")]
#[serde(rename_all = "camelCase")]
pub struct EqProfileInputDto {
    pub id: String,
    pub name_key: String,
    pub gains_db: Vec<f32>,
}

impl TryFrom<EqProfileInputDto> for EqProfile {
    type Error = localify_core::error::CoreError;

    fn try_from(d: EqProfileInputDto) -> Result<Self, Self::Error> {
        let bandas: [f32; 10] = d.gains_db.try_into().map_err(|v: Vec<f32>| {
            localify_core::error::CoreError::invalid(format!(
                "el ecualizador tiene 10 bandas, recibidas {}",
                v.len()
            ))
        })?;
        // La validación de rango vive en el dominio, no aquí: repetirla sería
        // arriesgarse a que las dos copias divergieran.
        Self::new(d.id, d.name_key, bandas)
    }
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "types.gen.ts")]
#[serde(rename_all = "camelCase")]
pub struct AudioDeviceDto {
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

impl From<AudioDevice> for AudioDeviceDto {
    fn from(d: AudioDevice) -> Self {
        Self {
            id: d.id,
            name: d.name,
            is_default: d.is_default,
        }
    }
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "types.gen.ts")]
#[serde(rename_all = "camelCase")]
pub struct AudioSettingsDto {
    /// `0` desactiva el crossfade y activa la reproducción sin huecos.
    pub crossfade_ms: u32,
    pub gapless: bool,
    pub eq_profile: EqProfileDto,
    pub normalize_volume: bool,
    /// `null` = dispositivo predeterminado del sistema.
    pub output_device_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export, export_to = "types.gen.ts")]
#[serde(rename_all = "camelCase")]
pub struct AudioSettingsInputDto {
    pub crossfade_ms: u32,
    pub gapless: bool,
    pub eq_profile: EqProfileInputDto,
    pub normalize_volume: bool,
    pub output_device_id: Option<String>,
}

impl TryFrom<AudioSettingsInputDto> for AudioSettings {
    type Error = localify_core::error::CoreError;

    fn try_from(d: AudioSettingsInputDto) -> Result<Self, Self::Error> {
        Ok(Self {
            crossfade_ms: d.crossfade_ms,
            gapless: d.gapless,
            eq_profile: d.eq_profile.try_into()?,
            normalize_volume: d.normalize_volume,
            output_device_id: d.output_device_id,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "types.gen.ts")]
#[serde(rename_all = "camelCase")]
pub struct DownloadSettingsDto {
    pub preferred_format: String,
    /// Descargas simultáneas por carril (inmediato y prefetch).
    pub max_concurrent: u8,
    pub max_retries: u8,
}

const fn formato_a_str(f: FormatPreference) -> &'static str {
    match f {
        FormatPreference::Opus => "opus",
        FormatPreference::M4a => "m4a",
        FormatPreference::Best => "best",
    }
}

fn formato_desde_str(s: &str) -> Result<FormatPreference, localify_core::error::CoreError> {
    Ok(match s {
        "opus" => FormatPreference::Opus,
        "m4a" => FormatPreference::M4a,
        "best" => FormatPreference::Best,
        otro => {
            return Err(localify_core::error::CoreError::invalid(format!(
                "formato preferido desconocido: '{otro}'"
            )));
        }
    })
}

impl From<DownloadSettings> for DownloadSettingsDto {
    fn from(d: DownloadSettings) -> Self {
        Self {
            preferred_format: formato_a_str(d.preferred_format).to_owned(),
            max_concurrent: d.max_concurrent,
            max_retries: d.max_retries,
        }
    }
}

impl TryFrom<DownloadSettingsDto> for DownloadSettings {
    type Error = localify_core::error::CoreError;

    fn try_from(d: DownloadSettingsDto) -> Result<Self, Self::Error> {
        Ok(Self {
            preferred_format: formato_desde_str(&d.preferred_format)?,
            max_concurrent: d.max_concurrent,
            max_retries: d.max_retries,
        })
    }
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "types.gen.ts")]
#[serde(rename_all = "camelCase")]
pub struct SpotifySettingsDto {
    pub configured: bool,
    /// El `clientId` no es secreto y sirve para que el usuario reconozca cuál
    /// puso. El `clientSecret` **nunca** aparece.
    pub client_id: Option<String>,
}

/// Lo que hace falta para el primer paso de la autenticación de Last.fm.
///
/// El token no es un secreto: caduca en una hora y solo vale para esta
/// autorización. Viaja al frontend porque el segundo comando lo necesita y
/// guardarlo en el backend obligaría a mantener estado de una conversación que
/// puede quedarse a medias.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "types.gen.ts")]
#[serde(rename_all = "camelCase")]
pub struct LastfmAuthDto {
    pub token: String,
    /// Página de Last.fm donde el usuario autoriza la aplicación.
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "types.gen.ts")]
#[serde(rename_all = "camelCase")]
pub struct IntegrationSettingsDto {
    pub discord_enabled: bool,
    pub discord_client_id: Option<String>,
    pub lastfm_enabled: bool,
    pub lastfm_user: Option<String>,
    /// Solo de lectura: dice si hay sesión de Last.fm. Se ignora al recibirlo,
    /// porque conectarse no es un ajuste que se cambie con un interruptor.
    #[serde(default)]
    pub lastfm_connected: bool,
}

impl From<IntegrationSettings> for IntegrationSettingsDto {
    fn from(i: IntegrationSettings) -> Self {
        Self {
            discord_enabled: i.discord_enabled,
            discord_client_id: i.discord_client_id,
            lastfm_enabled: i.lastfm_enabled,
            lastfm_user: i.lastfm_user,
            lastfm_connected: i.lastfm_connected,
        }
    }
}

impl From<IntegrationSettingsDto> for IntegrationSettings {
    fn from(i: IntegrationSettingsDto) -> Self {
        Self {
            discord_enabled: i.discord_enabled,
            discord_client_id: i.discord_client_id,
            lastfm_enabled: i.lastfm_enabled,
            lastfm_user: i.lastfm_user,
            // Nunca viene del frontend: lo decide el almacén de secretos y lo
            // rellena el servicio de ajustes al leer.
            lastfm_connected: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "types.gen.ts")]
#[serde(rename_all = "camelCase")]
pub struct UiSettingsDto {
    pub list_density: String,
    pub start_view: String,
}

impl From<UiSettings> for UiSettingsDto {
    fn from(u: UiSettings) -> Self {
        Self {
            list_density: match u.list_density {
                ListDensity::Comfortable => "comfortable".to_owned(),
                ListDensity::Compact => "compact".to_owned(),
            },
            start_view: u.start_view,
        }
    }
}

impl From<UiSettingsDto> for UiSettings {
    fn from(u: UiSettingsDto) -> Self {
        Self {
            list_density: if u.list_density == "compact" {
                ListDensity::Compact
            } else {
                ListDensity::Comfortable
            },
            start_view: u.start_view,
        }
    }
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "types.gen.ts")]
#[serde(rename_all = "camelCase")]
pub struct SettingsDto {
    pub language: String,
    pub library_path: String,
    /// Catálogo de metadatos activo: `"ytmusic"` o `"spotify"`.
    pub metadata_provider: String,
    pub audio: AudioSettingsDto,
    pub download: DownloadSettingsDto,
    pub spotify: SpotifySettingsDto,
    pub integrations: IntegrationSettingsDto,
    pub ui: UiSettingsDto,
}

impl From<Settings> for SettingsDto {
    fn from(s: Settings) -> Self {
        Self {
            language: s.language.code().to_owned(),
            library_path: s.library_path.display().to_string(),
            metadata_provider: s.metadata_provider.code().to_owned(),
            audio: AudioSettingsDto {
                crossfade_ms: s.audio.crossfade_ms,
                gapless: s.audio.gapless,
                eq_profile: s.audio.eq_profile.into(),
                normalize_volume: s.audio.normalize_volume,
                output_device_id: s.audio.output_device_id,
            },
            download: s.download.into(),
            spotify: SpotifySettingsDto {
                configured: s.spotify.configured,
                client_id: s.spotify.client_id,
            },
            integrations: s.integrations.into(),
            ui: s.ui.into(),
        }
    }
}

/// Modificación parcial. Solo los campos presentes se aplican.
#[derive(Debug, Clone, Default, Deserialize, TS)]
#[ts(export, export_to = "types.gen.ts")]
#[serde(rename_all = "camelCase", default)]
pub struct SettingsPatchDto {
    pub language: Option<String>,
    pub metadata_provider: Option<String>,
    pub audio: Option<AudioSettingsInputDto>,
    pub download: Option<DownloadSettingsDto>,
    pub integrations: Option<IntegrationSettingsDto>,
    pub ui: Option<UiSettingsDto>,
}

impl TryFrom<SettingsPatchDto> for SettingsPatch {
    type Error = localify_core::error::CoreError;

    fn try_from(d: SettingsPatchDto) -> Result<Self, Self::Error> {
        let language = match d.language {
            Some(c) => Some(Language::from_code(&c).ok_or_else(|| {
                localify_core::error::CoreError::invalid(format!("idioma no soportado: '{c}'"))
            })?),
            None => None,
        };

        let metadata_provider = match d.metadata_provider {
            Some(c) => Some(MetadataProviderKind::from_code(&c).ok_or_else(|| {
                localify_core::error::CoreError::invalid(format!("proveedor no soportado: '{c}'"))
            })?),
            None => None,
        };

        Ok(Self {
            language,
            metadata_provider,
            audio: d.audio.map(TryInto::try_into).transpose()?,
            download: d.download.map(TryInto::try_into).transpose()?,
            integrations: d.integrations.map(Into::into),
            ui: d.ui.map(Into::into),
        })
    }
}

/// Estado de un proveedor externo.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "types.gen.ts")]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum ProviderStatusDto {
    Ready,
    /// Faltan credenciales. Es accionable desde Ajustes, y la aplicación sigue
    /// funcionando por completo sobre la biblioteca local.
    NotConfigured,
    #[serde(rename_all = "camelCase")]
    Unavailable {
        reason_key: String,
    },
}

impl From<ProviderStatus> for ProviderStatusDto {
    fn from(s: ProviderStatus) -> Self {
        match s {
            ProviderStatus::Ready => Self::Ready,
            ProviderStatus::NotConfigured => Self::NotConfigured,
            ProviderStatus::Unavailable { reason_key } => Self::Unavailable { reason_key },
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn los_ajustes_no_exponen_el_secreto_de_spotify() {
        let mut s = Settings::por_defecto_en(PathBuf::from(r"D:\Musica"));
        s.spotify.configured = true;
        s.spotify.client_id = Some("un-client-id-publico".into());

        let json = serde_json::to_string(&SettingsDto::from(s)).expect("serializa");

        assert!(json.contains("un-client-id-publico"));
        assert!(
            !json.to_lowercase().contains("secret"),
            "ningún campo de secreto debe aparecer en el DTO: {json}"
        );
    }

    #[test]
    fn el_patch_rechaza_un_idioma_no_soportado() {
        let dto = SettingsPatchDto {
            language: Some("fr".into()),
            ..Default::default()
        };
        assert!(SettingsPatch::try_from(dto).is_err());
    }

    #[test]
    fn el_patch_acepta_los_idiomas_soportados() {
        for codigo in ["es", "en"] {
            let dto = SettingsPatchDto {
                language: Some(codigo.to_owned()),
                ..Default::default()
            };
            assert!(
                SettingsPatch::try_from(dto).is_ok(),
                "'{codigo}' debería valer"
            );
        }
    }

    #[test]
    fn el_ecualizador_exige_exactamente_diez_bandas() {
        let corto = EqProfileInputDto {
            id: "custom".into(),
            name_key: "eq.custom".into(),
            gains_db: vec![0.0; 5],
        };
        assert!(EqProfile::try_from(corto).is_err());

        let justo = EqProfileInputDto {
            id: "custom".into(),
            name_key: "eq.custom".into(),
            gains_db: vec![0.0; 10],
        };
        assert!(EqProfile::try_from(justo).is_ok());
    }

    #[test]
    fn el_ecualizador_delega_la_validacion_de_rango_en_el_dominio() {
        let mut bandas = vec![0.0_f32; 10];
        bandas[3] = 99.0;
        let dto = EqProfileInputDto {
            id: "custom".into(),
            name_key: "eq.custom".into(),
            gains_db: bandas,
        };
        assert!(EqProfile::try_from(dto).is_err());
    }

    #[test]
    fn el_formato_preferido_hace_ida_y_vuelta() {
        for f in [
            FormatPreference::Opus,
            FormatPreference::M4a,
            FormatPreference::Best,
        ] {
            assert_eq!(formato_desde_str(formato_a_str(f)).expect("válido"), f);
        }
        assert!(formato_desde_str("flac").is_err());
    }

    #[test]
    fn el_estado_del_proveedor_lleva_discriminante() {
        let json = serde_json::to_value(ProviderStatusDto::from(ProviderStatus::NotConfigured))
            .expect("serializa");
        assert_eq!(json["state"], "notConfigured");
    }
}

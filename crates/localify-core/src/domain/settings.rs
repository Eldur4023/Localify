//! Configuración de la aplicación.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::audio::EqProfile;
use crate::error::CoreError;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    #[default]
    Es,
    En,
}

impl Language {
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Es => "es",
            Self::En => "en",
        }
    }

    #[must_use]
    pub fn from_code(code: &str) -> Option<Self> {
        match code.to_ascii_lowercase().as_str() {
            "es" => Some(Self::Es),
            "en" => Some(Self::En),
            _ => None,
        }
    }

    /// Detección desde el locale del sistema (`es-ES`, `en-US`…). Cualquier
    /// otro idioma cae en inglés, que es la lengua franca de la interfaz.
    #[must_use]
    pub fn from_locale(locale: &str) -> Self {
        match locale
            .split(['-', '_'])
            .next()
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("es") => Self::Es,
            _ => Self::En,
        }
    }
}

/// De dónde salen los metadatos y los resultados de búsqueda.
///
/// No es lo mismo que de dónde sale el audio: ese siempre es YouTube, vía
/// yt-dlp. Esto decide qué catálogo describe la música.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MetadataProviderKind {
    /// YouTube Music y MusicBrainz a la vez. **Es el valor por defecto.**
    ///
    /// Ninguno de los dos pide credenciales y no se solapan: YouTube Music sabe
    /// lo que hay subido y MusicBrainz lo que se ha publicado. Elegir uno solo
    /// obliga a acertar antes de buscar, y cuál es el correcto depende de la
    /// canción — que es justo lo que todavía no sabes cuando escribes.
    #[default]
    Combinado,
    /// Solo YouTube Music. Sin credenciales, y con lo nativo de la plataforma
    /// —remezclas, subidas de canal— que no está editado en ningún sitio.
    YtMusic,
    /// Solo MusicBrainz. Sin credenciales. Conoce la música editada: bandas
    /// sonoras, ediciones, ISRC y duraciones exactas. No conoce lo que solo
    /// existe en YouTube.
    MusicBrainz,
    /// Spotify. Mejores metadatos —agrupa álbumes mucho mejor y da ISRC y
    /// géneros—, pero exige que el usuario aporte sus propias credenciales.
    Spotify,
}

impl MetadataProviderKind {
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Combinado => "combinado",
            Self::YtMusic => "ytmusic",
            Self::MusicBrainz => "musicbrainz",
            Self::Spotify => "spotify",
        }
    }

    #[must_use]
    pub fn from_code(code: &str) -> Option<Self> {
        match code {
            "combinado" => Some(Self::Combinado),
            "ytmusic" => Some(Self::YtMusic),
            "musicbrainz" => Some(Self::MusicBrainz),
            "spotify" => Some(Self::Spotify),
            _ => None,
        }
    }
}

/// Duración máxima del crossfade. Más allá, el solapamiento deja de percibirse
/// como transición y empieza a sonar a mezcla.
pub const CROSSFADE_MAX_MS: u32 = 12_000;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioSettings {
    /// `0` desactiva el crossfade y activa la reproducción sin huecos.
    pub crossfade_ms: u32,
    pub gapless: bool,
    pub eq_profile: EqProfile,
    pub normalize_volume: bool,
    /// `None` = dispositivo predeterminado del sistema.
    pub output_device_id: Option<String>,
}

impl Default for AudioSettings {
    fn default() -> Self {
        Self {
            crossfade_ms: 0,
            gapless: true,
            eq_profile: EqProfile::plano(),
            normalize_volume: false,
            output_device_id: None,
        }
    }
}

/// Formato de audio preferido al descargar.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FormatPreference {
    /// Opus/WebM: el mejor audio que sirve YouTube y, además, el único que
    /// permite reproducción progresiva fiable. Es el valor por defecto por
    /// ambas razones a la vez (ADR-003, ADR-007).
    #[default]
    Opus,
    /// M4A/AAC. Menor calidad, pero symphonia lo decodifica de forma nativa.
    /// Es la vía de escape si el decodificador Opus diera problemas.
    M4a,
    /// Lo que yt-dlp considere mejor, sin preferencia de contenedor.
    Best,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadSettings {
    pub preferred_format: FormatPreference,
    /// Descargas simultáneas **por carril** (inmediato y prefetch).
    pub max_concurrent: u8,
    pub max_retries: u8,
    /// De dónde salen las cookies de YouTube. Ver [`CookieSource`].
    ///
    /// `serde(default)` porque este campo llegó después: sin él, una
    /// configuración guardada por una versión anterior no se podría leer y la
    /// sección entera volvería a sus valores de fábrica.
    #[serde(default)]
    pub cookies: CookieSource,
}

impl Default for DownloadSettings {
    fn default() -> Self {
        Self {
            preferred_format: FormatPreference::Opus,
            max_concurrent: 2,
            max_retries: 3,
            cookies: CookieSource::Ninguna,
        }
    }
}

/// De dónde saca yt-dlp las cookies de YouTube.
///
/// ## Para qué hacen falta
///
/// YouTube pide cada vez más a menudo «Sign in to confirm you're not a bot», y
/// entonces la descarga falla sin que haya nada que reintentar: no es un fallo
/// transitorio, es una puerta cerrada. Con las cookies de una sesión iniciada,
/// yt-dlp pasa.
///
/// ## Lo que hay que saber antes de activarlo
///
/// `Navegador` le da a yt-dlp acceso de lectura al almacén de cookies de ese
/// navegador **entero**, no solo a las de YouTube: es como funciona
/// `--cookies-from-browser` y no hay forma de acotarlo. `Fichero` es lo
/// contrario: un fichero que el usuario exporta y que contiene exactamente lo
/// que él decidió meter.
///
/// Ninguna de las dos cosas se copia, se registra ni cruza el puente IPC más
/// allá de la ruta y el nombre del navegador.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CookieSource {
    #[default]
    Ninguna,
    /// Se leen del perfil del navegador, con `--cookies-from-browser`.
    Navegador(String),
    /// Fichero en formato Netscape, con `--cookies`.
    Fichero(PathBuf),
}

/// Navegadores que yt-dlp sabe leer.
///
/// La lista va aquí y no en el frontend porque es la que decide qué es válido:
/// un nombre que yt-dlp no conozca hace fallar **todas** las descargas, no solo
/// la primera, y el error no dice que la culpa sea de un ajuste.
pub const NAVEGADORES: [&str; 8] = [
    "firefox", "chrome", "chromium", "edge", "brave", "opera", "vivaldi", "safari",
];

impl CookieSource {
    /// `true` si el origen es utilizable tal como está.
    #[must_use]
    pub fn es_valido(&self) -> bool {
        match self {
            Self::Ninguna => true,
            Self::Navegador(n) => NAVEGADORES.contains(&n.as_str()),
            Self::Fichero(p) => p.is_file(),
        }
    }
}

/// El origen de cookies vigente, compartido con quien lanza yt-dlp.
///
/// Va por celda compartida y no por consulta al servicio de ajustes por el mismo
/// motivo que el crossfade: se lee en el camino de cada descarga, y cambiarlo
/// tiene que surtir efecto sin reiniciar la aplicación.
#[derive(Debug, Default)]
pub struct CookiesVigentes(std::sync::RwLock<CookieSource>);

impl CookiesVigentes {
    #[must_use]
    pub fn leer(&self) -> CookieSource {
        self.0
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub fn poner(&self, origen: CookieSource) {
        *self
            .0
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = origen;
    }
}

/// Estado de las credenciales de Spotify tal y como lo ve el frontend.
///
/// **El `client_secret` no aparece aquí y nunca cruza el puente IPC.** Solo se
/// informa de si hay credenciales configuradas.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpotifySettings {
    pub configured: bool,
    pub client_id: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IntegrationSettings {
    pub discord_enabled: bool,
    /// Identificador de la aplicación registrada en Discord.
    ///
    /// No es un secreto —aparece en el perfil de cualquiera que use esa
    /// aplicación— así que vive aquí y no en el almacén del sistema. Tampoco se
    /// puede incrustar uno por defecto: sería el de quien compiló el binario, y
    /// todos los usuarios aparecerían bajo su nombre.
    #[serde(default)]
    pub discord_client_id: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ListDensity {
    #[default]
    Comfortable,
    Compact,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UiSettings {
    pub list_density: ListDensity,
    pub start_view: String,
}

impl Default for UiSettings {
    fn default() -> Self {
        Self {
            list_density: ListDensity::default(),
            start_view: "home".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub language: Language,
    pub library_path: PathBuf,
    /// Catálogo que describe la música. Ver [`MetadataProviderKind`].
    pub metadata_provider: MetadataProviderKind,
    pub audio: AudioSettings,
    pub download: DownloadSettings,
    pub spotify: SpotifySettings,
    pub integrations: IntegrationSettings,
    pub ui: UiSettings,
}

impl Settings {
    /// Ajustes por defecto para una ruta de biblioteca dada.
    #[must_use]
    pub fn por_defecto_en(library_path: PathBuf) -> Self {
        Self {
            language: Language::default(),
            library_path,
            metadata_provider: MetadataProviderKind::default(),
            audio: AudioSettings::default(),
            download: DownloadSettings::default(),
            spotify: SpotifySettings::default(),
            integrations: IntegrationSettings::default(),
            ui: UiSettings::default(),
        }
    }
}

/// Secciones de la configuración. Los eventos `SettingsChanged` indican cuáles
/// cambiaron, para que cada consumidor decida si le afecta.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SettingsSection {
    Language,
    /// Proveedor de metadatos.
    Provider,
    LibraryPath,
    Audio,
    Download,
    Spotify,
    Integrations,
    Ui,
}

/// Modificación parcial. Solo los campos presentes se aplican.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsPatch {
    pub language: Option<Language>,
    pub metadata_provider: Option<MetadataProviderKind>,
    pub audio: Option<AudioSettings>,
    pub download: Option<DownloadSettings>,
    pub integrations: Option<IntegrationSettings>,
    pub ui: Option<UiSettings>,
}

impl SettingsPatch {
    /// Valida el patch **antes** de tocar nada.
    ///
    /// La validación previa es lo que garantiza que un patch inválido no deje
    /// la configuración a medio aplicar.
    ///
    /// # Errors
    /// Si algún valor está fuera de rango.
    pub fn validar(&self) -> Result<(), CoreError> {
        if let Some(audio) = &self.audio {
            if audio.crossfade_ms > CROSSFADE_MAX_MS {
                return Err(CoreError::invalid(format!(
                    "el crossfade debe estar entre 0 y {CROSSFADE_MAX_MS} ms, recibido {}",
                    audio.crossfade_ms
                )));
            }
            // Reutiliza la validación de rango del propio perfil.
            EqProfile::new(
                audio.eq_profile.id.clone(),
                audio.eq_profile.name_key.clone(),
                audio.eq_profile.gains_db,
            )?;
        }

        if let Some(download) = &self.download {
            if !(1..=4).contains(&download.max_concurrent) {
                return Err(CoreError::invalid(
                    "las descargas simultáneas por carril deben estar entre 1 y 4",
                ));
            }
            if download.max_retries > 10 {
                return Err(CoreError::invalid("como máximo 10 reintentos"));
            }
        }

        Ok(())
    }

    /// Secciones afectadas, para el evento `SettingsChanged`.
    #[must_use]
    pub fn secciones(&self) -> Vec<SettingsSection> {
        let mut s = Vec::new();
        if self.language.is_some() {
            s.push(SettingsSection::Language);
        }
        if self.metadata_provider.is_some() {
            s.push(SettingsSection::Provider);
        }
        if self.audio.is_some() {
            s.push(SettingsSection::Audio);
        }
        if self.download.is_some() {
            s.push(SettingsSection::Download);
        }
        if self.integrations.is_some() {
            s.push(SettingsSection::Integrations);
        }
        if self.ui.is_some() {
            s.push(SettingsSection::Ui);
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn el_locale_del_sistema_cae_en_ingles_si_no_es_espanyol() {
        assert_eq!(Language::from_locale("es-ES"), Language::Es);
        assert_eq!(Language::from_locale("es_MX"), Language::Es);
        assert_eq!(Language::from_locale("en-US"), Language::En);
        assert_eq!(Language::from_locale("fr-FR"), Language::En);
        assert_eq!(Language::from_locale(""), Language::En);
    }

    #[test]
    fn el_patch_rechaza_un_crossfade_excesivo() {
        let patch = SettingsPatch {
            audio: Some(AudioSettings {
                crossfade_ms: CROSSFADE_MAX_MS + 1,
                ..Default::default()
            }),
            ..Default::default()
        };
        assert!(patch.validar().is_err());
    }

    #[test]
    fn el_patch_rechaza_una_concurrencia_invalida() {
        for n in [0_u8, 5, 200] {
            let patch = SettingsPatch {
                download: Some(DownloadSettings {
                    max_concurrent: n,
                    ..Default::default()
                }),
                ..Default::default()
            };
            assert!(
                patch.validar().is_err(),
                "max_concurrent={n} debería ser inválido"
            );
        }
    }

    #[test]
    fn el_patch_vacio_es_valido_y_no_afecta_a_nada() {
        let patch = SettingsPatch::default();
        assert!(patch.validar().is_ok());
        assert!(patch.secciones().is_empty());
    }

    #[test]
    fn las_secciones_reflejan_lo_que_trae_el_patch() {
        let patch = SettingsPatch {
            language: Some(Language::En),
            ui: Some(UiSettings::default()),
            ..Default::default()
        };
        let secciones = patch.secciones();
        assert!(secciones.contains(&SettingsSection::Language));
        assert!(secciones.contains(&SettingsSection::Ui));
        assert!(!secciones.contains(&SettingsSection::Audio));
    }

    #[test]
    fn el_crossfade_por_defecto_es_cero_con_gapless() {
        // Es el comportamiento de Spotify: sin crossfade salvo que se pida.
        let a = AudioSettings::default();
        assert_eq!(a.crossfade_ms, 0);
        assert!(a.gapless);
    }
}

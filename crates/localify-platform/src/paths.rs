//! Rutas de la aplicación.
//!
//! Estructura en disco:
//!
//! ```text
//! %APPDATA%/Localify/            ← configuración y base de datos
//! ├── localify.db
//! ├── settings.json              ← lo mínimo para arrancar
//! ├── logs/
//! └── bin/                       ← sidecars auto-actualizables
//!
//! <carpeta de biblioteca>/       ← configurable
//! ├── audio/<xx>/<track_id>.opus ← sharding por prefijo del id
//! ├── covers/
//! └── .tmp/                      ← descargas en curso; se purga al arrancar
//! ```

use std::path::{Path, PathBuf};

use directories::{BaseDirs, UserDirs};
use localify_core::error::{CoreError, CoreResult};
use localify_core::ports::platform::AppPaths;

const NOMBRE_APP: &str = "Localify";

/// Longitud del prefijo con el que se reparte el audio en subcarpetas.
///
/// Un único directorio con 50 000 ficheros degrada el explorador de Windows y
/// las operaciones de listado. Dos caracteres dan 62² ≈ 3 800 carpetas para
/// IDs base62, es decir unos pocos ficheros por carpeta en bibliotecas grandes.
const LONGITUD_SHARD: usize = 2;

#[derive(Debug, Clone)]
pub struct LocalifyPaths {
    config_dir: PathBuf,
    library_dir: PathBuf,
}

impl LocalifyPaths {
    /// Construye las rutas con la biblioteca en su ubicación por defecto.
    ///
    /// # Errors
    /// Si el sistema no expone los directorios de usuario.
    pub fn detectar() -> CoreResult<Self> {
        let base = BaseDirs::new().ok_or_else(|| {
            CoreError::storage("no se pudieron determinar los directorios del usuario")
        })?;
        let config_dir = base.config_dir().join(NOMBRE_APP);

        // Por defecto, la música va a la carpeta Música del usuario. Es donde
        // un usuario espera encontrarla, y sobrevive a una reinstalación.
        let library_dir = UserDirs::new()
            .and_then(|u| u.audio_dir().map(Path::to_path_buf))
            .unwrap_or_else(|| base.home_dir().join("Music"))
            .join(NOMBRE_APP);

        Ok(Self {
            config_dir,
            library_dir,
        })
    }

    /// Rutas con una biblioteca en una ubicación concreta.
    #[must_use]
    pub fn con_biblioteca(config_dir: PathBuf, library_dir: PathBuf) -> Self {
        Self {
            config_dir,
            library_dir,
        }
    }

    /// Cambia la raíz de la biblioteca. Solo lo usa la migración de carpeta.
    pub fn set_library_dir(&mut self, dir: PathBuf) {
        self.library_dir = dir;
    }

    /// Crea los directorios que deban existir antes de arrancar.
    ///
    /// # Errors
    /// Si alguno no se puede crear.
    pub fn crear_estructura(&self) -> CoreResult<()> {
        for dir in [
            self.config_dir.clone(),
            self.logs_dir(),
            self.binaries_dir(),
            self.library_dir.clone(),
            self.audio_dir(),
            self.covers_dir(),
            self.artists_dir(),
            self.temp_dir(),
        ] {
            std::fs::create_dir_all(&dir).map_err(|e| {
                CoreError::storage(format!("no se pudo crear '{}': {e}", dir.display()))
            })?;
        }
        Ok(())
    }

    /// Ruta relativa de un fichero de audio, con sharding por prefijo del ID.
    ///
    /// Devuelve algo como `audio/3z/3z8h0TU7ReDPLIbEnYhWZb.opus`. Es
    /// **relativa** a propósito: es lo que se persiste (ADR-018).
    #[must_use]
    pub fn audio_rel_path(track_id: &str, extension: &str) -> PathBuf {
        let shard: String = track_id
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .take(LONGITUD_SHARD)
            .collect();
        // Un ID demasiado corto o raro no debe producir una ruta vacía.
        let shard = if shard.len() < LONGITUD_SHARD {
            String::from("_")
        } else {
            shard
        };
        PathBuf::from("audio")
            .join(shard)
            .join(format!("{track_id}.{extension}"))
    }

    /// Ruta del fichero temporal de una descarga en curso.
    #[must_use]
    pub fn temp_download_path(&self, track_id: &str, extension: &str) -> PathBuf {
        self.temp_dir().join(format!("{track_id}.{extension}.part"))
    }

    /// Ruta de una portada cacheada.
    #[must_use]
    pub fn cover_path(&self, album_id: &str, tamano: CoverSize) -> PathBuf {
        self.covers_dir()
            .join(format!("{album_id}_{}.jpg", tamano.sufijo()))
    }
}

/// Tamaños de portada que se cachean.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoverSize {
    /// Rejillas y filas de lista.
    Small,
    /// Cabeceras.
    Medium,
    /// Vista ampliada del álbum.
    Large,
}

impl CoverSize {
    #[must_use]
    pub const fn sufijo(self) -> &'static str {
        match self {
            Self::Small => "sm",
            Self::Medium => "md",
            Self::Large => "lg",
        }
    }

    #[must_use]
    pub const fn pixeles(self) -> u32 {
        match self {
            Self::Small => 64,
            Self::Medium => 300,
            Self::Large => 640,
        }
    }
}

impl AppPaths for LocalifyPaths {
    fn config_dir(&self) -> &Path {
        &self.config_dir
    }

    fn library_dir(&self) -> &Path {
        &self.library_dir
    }

    fn audio_dir(&self) -> PathBuf {
        self.library_dir.join("audio")
    }

    fn covers_dir(&self) -> PathBuf {
        self.library_dir.join("covers")
    }

    fn artists_dir(&self) -> PathBuf {
        self.library_dir.join("artists")
    }

    fn temp_dir(&self) -> PathBuf {
        self.library_dir.join(".tmp")
    }

    fn logs_dir(&self) -> PathBuf {
        self.config_dir.join("logs")
    }

    fn binaries_dir(&self) -> PathBuf {
        self.config_dir.join("bin")
    }

    fn database_path(&self) -> PathBuf {
        self.config_dir.join("localify.db")
    }

    fn settings_path(&self) -> PathBuf {
        self.config_dir.join("settings.json")
    }

    fn resolve(&self, rel: &Path) -> PathBuf {
        // Una ruta absoluta persistida sería un error de programación, pero
        // unirla produciría una ruta silenciosamente incorrecta. Devolverla tal
        // cual al menos falla de forma visible al abrir el fichero.
        if rel.is_absolute() {
            rel.to_path_buf()
        } else {
            self.library_dir.join(rel)
        }
    }

    fn audio_rel_path(&self, track_id: &str, extension: &str) -> PathBuf {
        Self::audio_rel_path(track_id, extension)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rutas() -> LocalifyPaths {
        LocalifyPaths::con_biblioteca(
            PathBuf::from(r"C:\cfg\Localify"),
            PathBuf::from(r"D:\Musica\Localify"),
        )
    }

    #[test]
    fn el_audio_se_reparte_por_prefijo_del_id() {
        let p = LocalifyPaths::audio_rel_path("3z8h0TU7ReDPLIbEnYhWZb", "opus");
        assert_eq!(
            p,
            PathBuf::from("audio")
                .join("3z")
                .join("3z8h0TU7ReDPLIbEnYhWZb.opus")
        );
    }

    #[test]
    fn los_ids_locales_no_generan_rutas_con_dos_puntos() {
        // 'local:0193...' contiene ':', que en Windows no es válido en un
        // nombre de carpeta. El filtro a alfanuméricos lo evita.
        let p = LocalifyPaths::audio_rel_path("local:0193abc", "opus");
        let shard = p.parent().and_then(Path::file_name).expect("hay shard");
        assert_eq!(shard, "lo");
        assert!(!shard.to_string_lossy().contains(':'));
    }

    #[test]
    fn un_id_demasiado_corto_no_produce_una_ruta_vacia() {
        let p = LocalifyPaths::audio_rel_path("a", "mp3");
        assert_eq!(p, PathBuf::from("audio").join("_").join("a.mp3"));
    }

    #[test]
    fn las_rutas_relativas_se_resuelven_contra_la_biblioteca() {
        let r = rutas();
        let abs = r.resolve(Path::new("audio/3z/x.opus"));
        assert!(abs.starts_with(r"D:\Musica\Localify"));
    }

    #[test]
    fn el_temporal_vive_fuera_de_la_carpeta_de_audio() {
        let r = rutas();
        let tmp = r.temp_download_path("abc", "webm");
        assert!(tmp.starts_with(r.temp_dir()));
        assert!(
            !tmp.starts_with(r.audio_dir()),
            "un .part jamás debe caer en audio/"
        );
        assert!(tmp.to_string_lossy().ends_with(".part"));
    }

    #[test]
    fn la_base_de_datos_vive_en_config_no_en_la_biblioteca() {
        // Mover la biblioteca a otro disco no debe llevarse la base de datos.
        let r = rutas();
        assert!(r.database_path().starts_with(r"C:\cfg\Localify"));
        assert!(!r.database_path().starts_with(r"D:\Musica"));
    }
}

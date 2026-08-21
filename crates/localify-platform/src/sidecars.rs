//! Localización y actualización de los binarios externos (yt-dlp, FFmpeg).
//!
//! No se empaquetan con la aplicación. Dos razones, ambas de peso.
//!
//! 1. **yt-dlp se rompe cuando YouTube cambia**, cada pocas semanas. Poder
//!    actualizarlo sin publicar una versión de Localify es la diferencia entre
//!    "se arregla solo" y "la app está rota hasta la siguiente release".
//! 2. Respeta las licencias de ambos proyectos y mantiene el instalador
//!    pequeño.
//!
//! Instalarlos la primera vez lo hace `scripts/fetch-sidecars.ps1`. Mantener
//! yt-dlp al día lo hace la propia aplicación, en una tarea de fondo al
//! arrancar: ver [`actualizar_yt_dlp`]. Durante mucho tiempo ese segundo punto
//! fue solo una intención escrita en este comentario —había un puerto para ello
//! y ningún implementador—, y el síntoma acabó siendo el previsto: descargas que
//! empiezan a fallar sin motivo aparente.
//!
//! Orden de búsqueda: carpeta `bin/` de la aplicación → `PATH` del sistema. Lo
//! propio antes que lo del sistema, para que una versión del sistema
//! desactualizada no tenga prioridad sobre la que gestionamos.

use std::path::{Path, PathBuf};
use std::time::Duration;

use localify_core::error::CoreResult;
use localify_core::ports::youtube::SidecarStatus;

/// Lanza el proceso sin ventana de consola.
///
/// Sin esto, cada invocación hace parpadear una consola negra sobre la ventana.
/// Es la misma política que en el ejecutor de yt-dlp, y hay que repetirla porque
/// son procesos distintos.
#[allow(unused_variables, reason = "fuera de Windows no hay nada que hacer")]
fn sin_consola(cmd: &mut tokio::process::Command) {
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
}

/// Binarios que necesita Localify.
pub const SIDECARS: [&str; 2] = ["yt-dlp", "ffmpeg"];

#[derive(Debug, Clone)]
pub struct SidecarLocator {
    binaries_dir: PathBuf,
}

impl SidecarLocator {
    #[must_use]
    pub fn new(binaries_dir: PathBuf) -> Self {
        Self { binaries_dir }
    }

    /// Localiza un binario, o `None` si no está disponible.
    #[must_use]
    pub fn localizar(&self, nombre: &str) -> Option<PathBuf> {
        let con_extension = if cfg!(windows) {
            format!("{nombre}.exe")
        } else {
            nombre.to_owned()
        };

        let propio = self.binaries_dir.join(&con_extension);
        if propio.is_file() {
            return Some(propio);
        }

        buscar_en_path(&con_extension)
    }

    /// `true` si el binario es el que gestionamos nosotros.
    ///
    /// Lo que viene del `PATH` es de otro —un gestor de paquetes, otra
    /// aplicación— y no se toca: ver [`actualizar_yt_dlp`].
    #[must_use]
    pub fn es_propio(&self, ruta: &Path) -> bool {
        ruta.parent() == Some(self.binaries_dir.as_path())
    }

    /// Estado de todos los sidecars, con su versión si se pueden ejecutar.
    ///
    /// # Errors
    /// No falla: un binario ausente se refleja como `available: false`, que es
    /// información y no un error. La aplicación arranca igual y solo se ve
    /// afectada la descarga de audio.
    pub async fn estado(&self) -> CoreResult<Vec<SidecarStatus>> {
        let mut resultado = Vec::with_capacity(SIDECARS.len());

        for nombre in SIDECARS {
            let path = self.localizar(nombre);
            let version = match &path {
                Some(p) => leer_version(p, nombre).await,
                None => None,
            };
            resultado.push(SidecarStatus {
                name: nombre_estatico(nombre),
                available: path.is_some(),
                path,
                version,
            });
        }

        Ok(resultado)
    }
}

fn nombre_estatico(nombre: &str) -> &'static str {
    match nombre {
        "yt-dlp" => "yt-dlp",
        "ffmpeg" => "ffmpeg",
        _ => "desconocido",
    }
}

fn buscar_en_path(nombre_fichero: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(nombre_fichero))
        .find(|p| p.is_file())
}

/// Ejecuta el binario para leer su versión.
///
/// Devuelve `None` en lugar de error: que un binario esté pero no responda es
/// exactamente igual de accionable para el usuario que si no estuviera, y no
/// merece propagar un fallo.
async fn leer_version(path: &Path, nombre: &str) -> Option<String> {
    let mut cmd = tokio::process::Command::new(path);
    cmd.arg("--version").kill_on_drop(true);
    sin_consola(&mut cmd);

    let salida = cmd.output().await.ok()?;

    if !salida.status.success() {
        return None;
    }

    let texto = String::from_utf8_lossy(&salida.stdout);
    let primera = texto.lines().next()?.trim();

    Some(match nombre {
        // yt-dlp imprime solo la versión: "2026.07.21".
        "yt-dlp" => primera.to_owned(),
        // ffmpeg imprime "ffmpeg version 7.1 Copyright (c) ...".
        "ffmpeg" => primera
            .split_whitespace()
            .nth(2)
            .unwrap_or(primera)
            .to_owned(),
        _ => primera.to_owned(),
    })
}

/// Resultado de intentar actualizar yt-dlp.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Actualizacion {
    /// Ya estaba en la última versión.
    AlDia(String),
    /// Se instaló una versión nueva.
    Actualizado { antes: String, ahora: String },
    /// No se pudo comprobar. No es un fallo de la aplicación: sin red, o con
    /// GitHub caído, lo correcto es seguir con la versión que haya.
    NoSePudo(String),
    /// El binario no es nuestro y no se toca. Ver [`actualizar_yt_dlp`].
    NoEsNuestro,
}

/// Pone yt-dlp al día usando su propio actualizador.
///
/// ## Por qué hace falta
///
/// yt-dlp se rompe cuando YouTube cambia, cada pocas semanas, y hasta ahora
/// nadie lo actualizaba: el binario se descargaba a mano con
/// `scripts/fetch-sidecars.ps1` y se quedaba con la versión de ese día. El
/// resultado, meses después, es que las descargas empiezan a fallar sin motivo
/// aparente y sin nada que el usuario pueda hacer.
///
/// ## Por qué `--update` y no descargarlo nosotros
///
/// Porque yt-dlp ya sabe hacerlo: comprueba la última publicación, verifica lo
/// que baja y se sustituye a sí mismo, que en Windows no es trivial —el fichero
/// está en uso—. Reimplementarlo sería escribir peor lo que ya funciona.
///
/// ## Solo si el binario es nuestro
///
/// Si yt-dlp viene del `PATH` del sistema, es de otro: puede haberlo instalado
/// el gestor de paquetes, estar compartido con otras aplicaciones o no ser
/// escribible. Actualizar eso es meterse donde no nos llaman, y además
/// `--update` fallaría con un mensaje confuso.
///
/// # Errors
/// No falla: todo lo que puede salir mal se devuelve como [`Actualizacion`].
pub async fn actualizar_yt_dlp(locator: &SidecarLocator, tope: Duration) -> Actualizacion {
    let Some(ruta) = locator.localizar("yt-dlp") else {
        return Actualizacion::NoSePudo("no está instalado".to_owned());
    };
    if !locator.es_propio(&ruta) {
        return Actualizacion::NoEsNuestro;
    }

    let antes = leer_version(&ruta, "yt-dlp").await.unwrap_or_default();

    let mut cmd = tokio::process::Command::new(&ruta);
    cmd.arg("--update").kill_on_drop(true);
    sin_consola(&mut cmd);

    // Con tope: sin red, yt-dlp puede quedarse esperando a GitHub, y esta tarea
    // vive durante toda la sesión.
    let salida = match tokio::time::timeout(tope, cmd.output()).await {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => return Actualizacion::NoSePudo(e.to_string()),
        Err(_) => return Actualizacion::NoSePudo("se agotó la espera".to_owned()),
    };

    if !salida.status.success() {
        // Un fallo aquí es normal y no es nuestro: sin red, sin permisos de
        // escritura, GitHub limitando peticiones. Se informa y se sigue con la
        // versión que haya, que es exactamente lo que hacía antes.
        let motivo = String::from_utf8_lossy(&salida.stderr).trim().to_owned();
        return Actualizacion::NoSePudo(motivo);
    }

    let ahora = leer_version(&ruta, "yt-dlp").await.unwrap_or_default();
    if ahora == antes {
        Actualizacion::AlDia(ahora)
    } else {
        Actualizacion::Actualizado { antes, ahora }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn la_carpeta_propia_tiene_prioridad_sobre_el_path() {
        let dir = std::env::temp_dir().join("localify-test-sidecars");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("crea dir");

        let nombre_fichero = if cfg!(windows) {
            "yt-dlp.exe"
        } else {
            "yt-dlp"
        };
        let propio = dir.join(nombre_fichero);
        std::fs::write(&propio, b"binario simulado").expect("escribe");

        let locator = SidecarLocator::new(dir.clone());
        assert_eq!(locator.localizar("yt-dlp"), Some(propio));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn un_binario_inexistente_devuelve_none() {
        let locator = SidecarLocator::new(PathBuf::from(r"C:\ruta\que\no\existe"));
        assert_eq!(locator.localizar("no-existe-este-binario-jamas"), None);
    }

    #[tokio::test]
    async fn el_estado_incluye_todos_los_sidecars_aunque_falten() {
        let locator = SidecarLocator::new(PathBuf::from(r"C:\ruta\que\no\existe"));
        let estado = locator.estado().await.expect("no debe fallar");
        assert_eq!(estado.len(), SIDECARS.len());
        // Sin binarios, la app debe poder arrancar igualmente.
        assert!(estado.iter().all(|s| !s.available));
    }
}

//! Aviso de nuevas versiones, contra los releases de GitHub.
//!
//! **Solo avisa.** No descarga nada ni sustituye el binario en marcha: eso
//! exigiría firmar el ejecutable y dejar que un proceso se reemplace a sí
//! mismo, que es una categoría de riesgo distinta a la de esta comprobación.
//! Encontrar una versión más nueva termina en un evento hacia el frontend; que
//! el usuario acepte termina en abrir el navegador en la página del release,
//! igual que cualquier otro enlace externo de Localify.
//!
//! ## Por qué una sola comprobación por arranque
//!
//! Mismo criterio que el actualizador de yt-dlp: comprobar tarda un segundo,
//! no hace falta más de una vez por sesión, y no vale la pena mantener una
//! tarea despierta horas para volver a preguntar algo que no ha cambiado.

use std::time::Duration;

use localify_core::events::{DomainEvent, EventPublisher};
use semver::Version;
use serde::Deserialize;
use tracing::{debug, warn};

/// Repositorio consultado. El mismo que declara `workspace.package.repository`.
const REPO: &str = "Eldur4023/Localify";

/// GitHub exige `User-Agent` y lo agradece: permite que avisen si algo va mal
/// por nuestra parte, igual que con LRCLIB.
const AGENTE: &str = concat!(
    "Localify/",
    env!("CARGO_PKG_VERSION"),
    " (https://github.com/Eldur4023/Localify)"
);

/// Tope por petición. Una comprobación de fondo no debe retrasar nada más si
/// GitHub tarda en responder.
const TIMEOUT: Duration = Duration::from_secs(10);

/// Los dos únicos campos que interesan del último release.
#[derive(Debug, Deserialize)]
struct Release {
    tag_name: String,
    html_url: String,
}

/// Una versión publicada más nueva que la que corre ahora mismo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Actualizacion {
    pub version: String,
    pub url: String,
}

/// Cliente HTTP con lo que la API de GitHub exige.
///
/// # Errors
/// Si no se puede construir el cliente (falta de entropía para TLS, por
/// ejemplo). No es recuperable: la comprobación simplemente no ocurre.
pub fn cliente() -> Result<reqwest::Client, reqwest::Error> {
    reqwest::Client::builder()
        .user_agent(AGENTE)
        .timeout(TIMEOUT)
        .build()
}

/// Compara el último release publicado con `version_actual`.
///
/// `None` cubre tres casos por igual, a propósito: sin release más nuevo, con
/// un tag que no es un semver válido, o con la petición fallada. Ninguno de
/// los tres es un error que deba llegar al usuario — GitHub caído un rato no
/// es un motivo para molestar a nadie, solo para que hoy no se sepa.
///
/// `/releases/latest` de GitHub ya excluye borradores y prerelease por su
/// cuenta: no hace falta filtrarlos aquí.
pub async fn comprobar(http: &reqwest::Client, version_actual: &str) -> Option<Actualizacion> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let respuesta = match http
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            debug!(error = %e, "no se pudo consultar el ultimo release de GitHub");
            return None;
        }
    };

    if !respuesta.status().is_success() {
        debug!(estado = %respuesta.status(), "GitHub no devolvio el ultimo release");
        return None;
    }

    let release: Release = match respuesta.json().await {
        Ok(r) => r,
        Err(e) => {
            warn!(error = %e, "la respuesta de GitHub no tiene la forma esperada");
            return None;
        }
    };

    let publicada = Version::parse(release.tag_name.trim_start_matches('v')).ok()?;
    let actual = Version::parse(version_actual).ok()?;

    (publicada > actual).then(|| Actualizacion {
        version: publicada.to_string(),
        url: release.html_url,
    })
}

/// Comprueba una vez y, si hay algo más nuevo, avisa.
///
/// `guardar_url` existe porque este crate no conoce `AppContext`: quien llama
/// decide dónde queda la URL para cuando el usuario acepte el aviso. El
/// evento en sí no la lleva —el frontend nunca debe poder devolver a Rust una
/// URL para que la abra; ver el comentario de `settings_open_external`—, así
/// que sin este cierre el "sí, actualizar" no tendría adónde ir.
pub async fn vigilar(
    http: reqwest::Client,
    eventos: std::sync::Arc<dyn EventPublisher>,
    guardar_url: impl FnOnce(String) + Send + 'static,
) {
    let Some(actualizacion) = comprobar(&http, env!("CARGO_PKG_VERSION")).await else {
        return;
    };
    guardar_url(actualizacion.url);
    eventos.publish(DomainEvent::UpdateAvailable {
        version: actualizacion.version,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn una_version_publicada_mas_alta_gana() {
        assert!(
            Version::parse("1.2.0").expect("parsea") > Version::parse("1.1.9").expect("parsea")
        );
    }

    #[test]
    fn el_prefijo_v_no_impide_comparar() {
        // El tag de un release suele llevar "v" delante ("v1.2.3"); el
        // `Cargo.toml` nunca lo lleva. La comparación real vive en
        // `comprobar`, pero lo que hace posible mezclar los dos formatos es
        // justo este `trim_start_matches`.
        let publicada = Version::parse("v1.2.3".trim_start_matches('v')).expect("parsea");
        let actual = Version::parse("1.2.0").expect("parsea");
        assert!(publicada > actual);
    }

    #[test]
    fn un_tag_que_no_es_semver_no_hace_panico_al_parsear() {
        assert!(Version::parse("release-final".trim_start_matches('v')).is_err());
    }
}

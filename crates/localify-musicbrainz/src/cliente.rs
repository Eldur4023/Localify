//! Cliente HTTP de la API de MusicBrainz.
//!
//! ## El límite de una petición por segundo no es negociable
//!
//! MusicBrainz lo pide explícitamente y bloquea por IP a quien no lo respeta.
//! No es una recomendación de cortesía: es la condición de uso de un servicio
//! que mantiene una fundación sin ánimo de lucro y que no cobra por esto.
//!
//! El freno vive **aquí** y no en quien llama, por el mismo motivo que el rebote
//! remoto vive en el servicio de búsqueda: si cada llamante tiene que acordarse,
//! el día que alguien añada una llamada nueva se olvidará, y el síntoma será que
//! la aplicación deja de encontrar nada sin que nadie sepa por qué.
//!
//! ## Y el `User-Agent` tampoco
//!
//! MusicBrainz rechaza con 403 a quien no se identifica. Mandar el nombre de la
//! aplicación y una URL de contacto es lo que permite que puedan avisar si algo
//! nuestro va mal, en vez de cortarnos sin más.

use std::time::Duration;

use localify_core::error::{CoreError, CoreResult};
use serde::de::DeserializeOwned;
use tokio::sync::Mutex;
use tokio::time::Instant;
use tracing::debug;

const BASE: &str = "https://musicbrainz.org/ws/2";

/// Portadas. Es un servicio hermano con su propio dominio y sin límite tan
/// estricto, pero se identifica igual.
pub const COVER_ART: &str = "https://coverartarchive.org";

/// Separación mínima entre dos peticiones. Ver la cabecera del módulo.
const INTERVALO: Duration = Duration::from_millis(1100);

/// Tope por petición.
const TIMEOUT: Duration = Duration::from_secs(15);

/// Quiénes somos. MusicBrainz exige identificarse y devuelve 403 si no.
const AGENTE: &str = concat!(
    "Localify/",
    env!("CARGO_PKG_VERSION"),
    " ( https://github.com/Eldur4023/Localify )"
);

pub struct ClienteMusicBrainz {
    http: reqwest::Client,
    /// Cuándo salió la última petición.
    ///
    /// Un `Mutex` asíncrono y no un atómico: el que llega mientras otro espera
    /// tiene que **esperar su turno**, no calcular su hueco y salir a la vez.
    /// Con un atómico, tres búsquedas simultáneas leerían la misma marca y las
    /// tres se creerían con derecho a salir.
    ultima: Mutex<Option<Instant>>,
}

impl std::fmt::Debug for ClienteMusicBrainz {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClienteMusicBrainz").finish_non_exhaustive()
    }
}

impl ClienteMusicBrainz {
    /// # Errors
    /// Si el cliente HTTP no se puede construir.
    pub fn nuevo() -> Result<Self, reqwest::Error> {
        Ok(Self {
            http: reqwest::Client::builder()
                .timeout(TIMEOUT)
                .user_agent(AGENTE)
                .build()?,
            ultima: Mutex::new(None),
        })
    }

    /// Espera hasta que toque la siguiente petición.
    ///
    /// El candado se mantiene durante la espera **a propósito**: es lo que hace
    /// que los que llegan detrás se pongan en fila en vez de despertarse todos a
    /// la vez y salir juntos.
    async fn esperar_turno(&self) {
        let mut ultima = self.ultima.lock().await;
        if let Some(cuando) = *ultima
            && let Some(resto) = INTERVALO.checked_sub(cuando.elapsed())
        {
            tokio::time::sleep(resto).await;
        }
        *ultima = Some(Instant::now());
    }

    /// Lanza una petición a la API y deserializa la respuesta.
    ///
    /// `ruta` es lo que va detrás de `/ws/2`, ya con sus parámetros salvo
    /// `fmt`, que lo pone esta función porque no es decisión de quien llama.
    ///
    /// # Errors
    /// Si la petición falla o la respuesta no encaja con `T`.
    pub async fn pedir<T: DeserializeOwned>(
        &self,
        ruta: &str,
        parametros: &[(&str, String)],
    ) -> CoreResult<T> {
        self.esperar_turno().await;

        let url = format!("{BASE}/{ruta}");
        let respuesta = self
            .http
            .get(&url)
            .query(parametros)
            .query(&[("fmt", "json")])
            .send()
            .await
            .map_err(|e| CoreError::provider_unavailable("musicbrainz", Box::new(e)))?;

        let estado = respuesta.status();
        if !estado.is_success() {
            debug!(%url, %estado, "MusicBrainz respondió con error");
            // Un 503 aquí suele significar "vas demasiado rápido". Se trata como
            // el resto de indisponibilidades: la aplicación sigue sobre lo local.
            return Err(CoreError::provider_unavailable(
                "musicbrainz",
                Box::new(std::io::Error::other(format!("{estado} en {ruta}"))),
            ));
        }

        respuesta
            .json::<T>()
            .await
            .map_err(|e| CoreError::provider_unavailable("musicbrainz", Box::new(e)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn las_peticiones_se_separan_un_segundo() {
        // Con el reloj de tokio pausado, `sleep` avanza el tiempo virtual: esto
        // comprueba la espera sin tardar tres segundos de verdad.
        let cliente = ClienteMusicBrainz::nuevo().expect("construye");
        let inicio = Instant::now();

        for _ in 0..3 {
            cliente.esperar_turno().await;
        }

        // Tres turnos son dos esperas: el primero sale al momento.
        assert!(
            inicio.elapsed() >= INTERVALO * 2,
            "transcurrido {:?}, se esperaban al menos {:?}",
            inicio.elapsed(),
            INTERVALO * 2
        );
    }

    #[tokio::test(start_paused = true)]
    async fn el_primero_no_espera() {
        let cliente = ClienteMusicBrainz::nuevo().expect("construye");
        let inicio = Instant::now();
        cliente.esperar_turno().await;
        assert_eq!(inicio.elapsed(), Duration::ZERO);
    }
}

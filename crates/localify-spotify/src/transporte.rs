//! Transporte HTTP.
//!
//! Va detrás de un trait por una razón concreta: **la suite de tests no debe
//! tocar la red**. Con un transporte falso alimentado por fixtures se pueden
//! probar el limitador, el refresco de token, los reintentos y el mapeo
//! completos, de forma determinista y en milisegundos.

use std::time::Duration;

use async_trait::async_trait;

use crate::error::{SpotifyError, SpotifyResult};

/// Respuesta cruda, independiente del cliente HTTP.
#[derive(Debug, Clone)]
pub struct Respuesta {
    pub estado: u16,
    pub cuerpo: Vec<u8>,
    /// Segundos de `Retry-After`, si vino.
    pub retry_after: Option<u64>,
}

impl Respuesta {
    #[must_use]
    pub const fn es_ok(&self) -> bool {
        self.estado >= 200 && self.estado < 300
    }

    /// Deserializa el cuerpo.
    ///
    /// # Errors
    /// Si el JSON no encaja con el tipo esperado.
    pub fn json<T: serde::de::DeserializeOwned>(&self) -> SpotifyResult<T> {
        serde_json::from_slice(&self.cuerpo).map_err(|e| {
            // El cuerpo no se incluye en el error: puede ser enorme y, en la
            // respuesta del token, contiene credenciales.
            SpotifyError::Respuesta(format!("no se pudo interpretar el JSON: {e}"))
        })
    }
}

#[async_trait]
pub trait Transporte: Send + Sync + 'static {
    /// `GET` autenticado con un token de portador.
    async fn get(&self, url: &str, token: &str) -> SpotifyResult<Respuesta>;

    /// `GET` sin autenticar, con `User-Agent` de navegador.
    ///
    /// Lo usa la lectura de una playlist pública, que no pasa por la API sino
    /// por la página de incrustación. El `User-Agent` no es opcional: sin él,
    /// Spotify devuelve una página distinta y sin datos.
    ///
    /// Va en el mismo trait que el resto para que la suite pueda probarlo con
    /// un transporte falso, igual que todo lo demás.
    async fn get_publico(&self, url: &str, agente: &str) -> SpotifyResult<Respuesta>;

    /// `POST` de formulario con autenticación básica. Solo lo usa el endpoint
    /// del token.
    async fn post_form(
        &self,
        url: &str,
        campos: &[(&str, &str)],
        basic_auth: &str,
    ) -> SpotifyResult<Respuesta>;
}

/// Transporte real sobre `reqwest`.
#[derive(Debug, Clone)]
pub struct TransporteHttp {
    cliente: reqwest::Client,
}

/// Tiempo máximo por petición. Spotify responde en decenas de milisegundos; diez
/// segundos es un margen amplio que evita que un cuelgue de red deje una
/// búsqueda esperando para siempre.
const TIMEOUT: Duration = Duration::from_secs(10);

impl TransporteHttp {
    /// # Errors
    /// Si el cliente HTTP no se puede construir.
    pub fn nuevo() -> SpotifyResult<Self> {
        let cliente = reqwest::Client::builder()
            .timeout(TIMEOUT)
            .user_agent(concat!("Localify/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| SpotifyError::Red(e.to_string()))?;
        Ok(Self { cliente })
    }
}

/// Extrae `Retry-After` de las cabeceras.
fn retry_after(headers: &reqwest::header::HeaderMap) -> Option<u64> {
    headers
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse()
        .ok()
}

#[async_trait]
impl Transporte for TransporteHttp {
    async fn get(&self, url: &str, token: &str) -> SpotifyResult<Respuesta> {
        let respuesta = self
            .cliente
            .get(url)
            .bearer_auth(token)
            .send()
            .await
            .map_err(|e| SpotifyError::Red(e.to_string()))?;

        let estado = respuesta.status().as_u16();
        let espera = retry_after(respuesta.headers());
        let cuerpo = respuesta
            .bytes()
            .await
            .map_err(|e| SpotifyError::Red(e.to_string()))?
            .to_vec();

        Ok(Respuesta {
            estado,
            cuerpo,
            retry_after: espera,
        })
    }

    async fn get_publico(&self, url: &str, agente: &str) -> SpotifyResult<Respuesta> {
        let respuesta = self
            .cliente
            .get(url)
            .header(reqwest::header::USER_AGENT, agente)
            .send()
            .await
            .map_err(|e| SpotifyError::Red(e.to_string()))?;

        let estado = respuesta.status().as_u16();
        let cuerpo = respuesta
            .bytes()
            .await
            .map_err(|e| SpotifyError::Red(e.to_string()))?
            .to_vec();

        Ok(Respuesta {
            estado,
            cuerpo,
            retry_after: None,
        })
    }

    async fn post_form(
        &self,
        url: &str,
        campos: &[(&str, &str)],
        basic_auth: &str,
    ) -> SpotifyResult<Respuesta> {
        let respuesta = self
            .cliente
            .post(url)
            .header(
                reqwest::header::AUTHORIZATION,
                format!("Basic {basic_auth}"),
            )
            .form(campos)
            .send()
            .await
            .map_err(|e| SpotifyError::Red(e.to_string()))?;

        let estado = respuesta.status().as_u16();
        let espera = retry_after(respuesta.headers());
        let cuerpo = respuesta
            .bytes()
            .await
            .map_err(|e| SpotifyError::Red(e.to_string()))?
            .to_vec();

        Ok(Respuesta {
            estado,
            cuerpo,
            retry_after: espera,
        })
    }
}

/// Transporte programable para tests.
///
/// No va tras `cfg(test)`: los servicios de otros crates también lo necesitan
/// para probarse sin red, y un `cfg(test)` solo aplica al propio crate.
pub mod falso {
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    use super::{Respuesta, SpotifyResult, Transporte, async_trait};

    /// Petición registrada, para poder afirmar sobre lo que se pidió.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum Peticion {
        Get { url: String, token: String },
        PostForm { url: String },
    }

    /// Transporte que devuelve respuestas preparadas de antemano.
    #[derive(Debug, Clone, Default)]
    pub struct TransporteFalso {
        respuestas: Arc<Mutex<VecDeque<Respuesta>>>,
        pub peticiones: Arc<Mutex<Vec<Peticion>>>,
    }

    impl TransporteFalso {
        #[must_use]
        pub fn nuevo() -> Self {
            Self::default()
        }

        /// Encola una respuesta correcta con cuerpo JSON.
        #[must_use]
        pub fn con_json(self, json: &str) -> Self {
            self.encolar(Respuesta {
                estado: 200,
                cuerpo: json.as_bytes().to_vec(),
                retry_after: None,
            })
        }

        /// Encola una respuesta de error.
        #[must_use]
        pub fn con_estado(self, estado: u16, retry_after: Option<u64>) -> Self {
            self.encolar(Respuesta {
                estado,
                cuerpo: b"{}".to_vec(),
                retry_after,
            })
        }

        #[must_use]
        pub fn encolar(self, r: Respuesta) -> Self {
            if let Ok(mut cola) = self.respuestas.lock() {
                cola.push_back(r);
            }
            self
        }

        /// Cuántas peticiones se han hecho.
        #[must_use]
        pub fn cuantas(&self) -> usize {
            self.peticiones.lock().map_or(0, |p| p.len())
        }

        #[must_use]
        pub fn registradas(&self) -> Vec<Peticion> {
            self.peticiones
                .lock()
                .map(|p| p.clone())
                .unwrap_or_default()
        }

        fn siguiente(&self, p: Peticion) -> SpotifyResult<Respuesta> {
            if let Ok(mut reg) = self.peticiones.lock() {
                reg.push(p);
            }
            let siguiente = self.respuestas.lock().ok().and_then(|mut c| c.pop_front());
            siguiente.map_or_else(
                || {
                    Err(super::SpotifyError::Red(
                        "el transporte falso se quedó sin respuestas preparadas".into(),
                    ))
                },
                Ok,
            )
        }
    }

    #[async_trait]
    impl Transporte for TransporteFalso {
        async fn get(&self, url: &str, token: &str) -> SpotifyResult<Respuesta> {
            self.siguiente(Peticion::Get {
                url: url.to_owned(),
                token: token.to_owned(),
            })
        }

        async fn get_publico(&self, url: &str, _agente: &str) -> SpotifyResult<Respuesta> {
            // Se registra como un `Get` sin token: lo que importa comprobar es a
            // qué URL se fue, y que no se pidió token para ir.
            self.siguiente(Peticion::Get {
                url: url.to_owned(),
                token: String::new(),
            })
        }

        async fn post_form(
            &self,
            url: &str,
            _campos: &[(&str, &str)],
            _basic_auth: &str,
        ) -> SpotifyResult<Respuesta> {
            self.siguiente(Peticion::PostForm {
                url: url.to_owned(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn una_respuesta_2xx_es_correcta_y_el_resto_no() {
        let hacer = |estado| Respuesta {
            estado,
            cuerpo: Vec::new(),
            retry_after: None,
        };
        assert!(hacer(200).es_ok());
        assert!(hacer(204).es_ok());
        assert!(!hacer(304).es_ok());
        assert!(!hacer(401).es_ok());
        assert!(!hacer(429).es_ok());
        assert!(!hacer(503).es_ok());
    }

    #[test]
    fn un_json_ilegible_no_filtra_el_cuerpo_en_el_error() {
        // El cuerpo de la respuesta del token contiene credenciales: no debe
        // acabar en un mensaje de error que se escriba en el log.
        let r = Respuesta {
            estado: 200,
            cuerpo: br#"{"access_token":"SECRETO_MUY_RECONOCIBLE"#.to_vec(),
            retry_after: None,
        };
        let error = r
            .json::<crate::models::TokenRespuesta>()
            .expect_err("el JSON está truncado");
        assert!(
            !error.to_string().contains("SECRETO_MUY_RECONOCIBLE"),
            "el error no debe incluir el cuerpo: {error}"
        );
    }
}

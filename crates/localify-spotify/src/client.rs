//! Cliente HTTP de Spotify: limitación, reintentos y refresco de token.

use std::sync::Arc;
use std::time::Duration;

use crate::auth::{Credenciales, GestorToken};
use crate::error::{SpotifyError, SpotifyResult};
use crate::rate_limit::Limitador;
use crate::transporte::{Respuesta, Transporte};

pub const BASE: &str = "https://api.spotify.com/v1";

/// Intentos por petición.
///
/// Tres cubre lo que cubre un reintento —un 503 puntual, un corte de red de un
/// segundo— sin convertir una caída prolongada en una espera larga: pasado eso,
/// es mejor decirle al usuario que el proveedor no responde y seguir sirviendo
/// la biblioteca local.
const INTENTOS: u32 = 3;

/// Espera base del backoff exponencial.
const BACKOFF_BASE: Duration = Duration::from_millis(400);

pub struct ClienteSpotify {
    transporte: Arc<dyn Transporte>,
    token: GestorToken,
    limitador: Limitador,
}

impl ClienteSpotify {
    /// El transporte, para lo que no pasa por la API.
    ///
    /// Lo usa la lectura de una playlist pública, que va contra la página de
    /// incrustación: no hay token que gestionar ni límite que respetar, así que
    /// pasar por `get` sería pedir un token que no hace falta.
    #[must_use]
    pub fn transporte(&self) -> &dyn Transporte {
        self.transporte.as_ref()
    }
}

impl std::fmt::Debug for ClienteSpotify {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClienteSpotify").finish_non_exhaustive()
    }
}

impl ClienteSpotify {
    #[must_use]
    pub fn nuevo(transporte: Arc<dyn Transporte>) -> Self {
        Self {
            token: GestorToken::nuevo(Arc::clone(&transporte)),
            transporte,
            limitador: Limitador::nuevo(),
        }
    }

    pub async fn set_credenciales(&self, credenciales: Option<Credenciales>) {
        self.token.set_credenciales(credenciales).await;
    }

    pub async fn hay_credenciales(&self) -> bool {
        self.token.hay_credenciales().await
    }

    /// `GET` sobre la API, con limitación, reintentos y refresco de token.
    ///
    /// # Errors
    /// El primer error no reintentable, o el último tras agotar los intentos.
    pub async fn get<T: serde::de::DeserializeOwned>(&self, ruta: &str) -> SpotifyResult<T> {
        let url = if ruta.starts_with("http") {
            ruta.to_owned()
        } else {
            format!("{BASE}{ruta}")
        };

        let mut ultimo: Option<SpotifyError> = None;

        for intento in 0..INTENTOS {
            if intento > 0 {
                // Backoff exponencial con desviación: sin ella, varias
                // peticiones que fallaron a la vez reintentarían a la vez y
                // volverían a chocar.
                let espera = BACKOFF_BASE * 2_u32.pow(intento - 1);
                tokio::time::sleep(espera + desviacion(espera)).await;
            }

            self.limitador.adquirir().await;

            let token = match self.token.token().await {
                Ok(t) => t,
                // Un problema de credenciales no mejora reintentando.
                Err(e @ (SpotifyError::SinCredenciales | SpotifyError::CredencialesInvalidas)) => {
                    return Err(e);
                }
                Err(e) => {
                    ultimo = Some(e);
                    continue;
                }
            };

            let respuesta = match self.transporte.get(&url, &token).await {
                Ok(r) => r,
                Err(e) => {
                    ultimo = Some(e);
                    continue;
                }
            };

            match self.clasificar(&respuesta, &url).await {
                Ok(()) => return respuesta.json(),
                Err(e) if e.es_reintentable() => ultimo = Some(e),
                Err(e) => return Err(e),
            }
        }

        Err(ultimo.unwrap_or(SpotifyError::Servidor { codigo: 0 }))
    }

    /// Traduce el estado HTTP y actualiza el limitador.
    async fn clasificar(&self, respuesta: &Respuesta, url: &str) -> SpotifyResult<()> {
        match respuesta.estado {
            200..=299 => Ok(()),
            401 => {
                // El token caducó antes de lo previsto. Se invalida para que el
                // siguiente intento pida uno nuevo.
                self.token.invalidar().await;
                Err(SpotifyError::Servidor { codigo: 401 })
            }
            403 => Err(SpotifyError::NoEncontrado(format!(
                "{url} (sin acceso con credenciales de aplicación)"
            ))),
            404 => Err(SpotifyError::NoEncontrado(url.to_owned())),
            429 => {
                let segundos = respuesta.retry_after.unwrap_or(5);
                self.limitador.registrar_limite(segundos).await;
                Err(SpotifyError::LimiteAlcanzado { segundos })
            }
            codigo => Err(SpotifyError::Servidor { codigo }),
        }
    }
}

/// Desviación aleatoria de hasta un 25 %, para desincronizar reintentos.
///
/// No hace falta un generador de calidad: basta con que dos procesos que
/// fallaron a la vez no vuelvan a coincidir.
fn desviacion(base: Duration) -> Duration {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    base.mul_f64(f64::from(nanos % 1000) / 4000.0)
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    use super::*;
    use crate::transporte::falso::TransporteFalso;

    #[derive(Debug, Deserialize)]
    struct Carga {
        valor: String,
    }

    fn token_ok() -> String {
        r#"{"access_token":"tok","token_type":"Bearer","expires_in":3600}"#.to_owned()
    }

    async fn cliente(transporte: Arc<TransporteFalso>) -> ClienteSpotify {
        let c = ClienteSpotify::nuevo(transporte);
        c.set_credenciales(Some(Credenciales {
            client_id: "id".into(),
            client_secret: "secreto".into(),
        }))
        .await;
        c
    }

    #[tokio::test]
    async fn una_peticion_correcta_devuelve_el_cuerpo() {
        let t = Arc::new(
            TransporteFalso::nuevo()
                .con_json(&token_ok())
                .con_json(r#"{"valor":"hola"}"#),
        );
        let c = cliente(t).await;

        let carga: Carga = c.get("/tracks/abc").await.expect("responde");
        assert_eq!(carga.valor, "hola");
    }

    #[tokio::test]
    async fn sin_credenciales_falla_sin_reintentar() {
        let t = Arc::new(TransporteFalso::nuevo());
        let c = ClienteSpotify::nuevo(t.clone());

        let error = c
            .get::<Carga>("/tracks/abc")
            .await
            .expect_err("debe fallar");
        assert!(matches!(error, SpotifyError::SinCredenciales));
        assert_eq!(t.cuantas(), 0, "reintentar sin credenciales sería inútil");
    }

    #[tokio::test(start_paused = true)]
    async fn un_error_de_servidor_se_reintenta() {
        let t = Arc::new(
            TransporteFalso::nuevo()
                .con_json(&token_ok())
                .con_estado(503, None)
                .con_estado(503, None)
                .con_json(r#"{"valor":"al tercer intento"}"#),
        );
        let c = cliente(t.clone()).await;

        let carga: Carga = c.get("/tracks/abc").await.expect("acaba respondiendo");
        assert_eq!(carga.valor, "al tercer intento");
    }

    #[tokio::test(start_paused = true)]
    async fn tras_agotar_los_intentos_se_devuelve_el_ultimo_error() {
        let t = Arc::new(
            TransporteFalso::nuevo()
                .con_json(&token_ok())
                .con_estado(503, None)
                .con_estado(503, None)
                .con_estado(503, None),
        );
        let c = cliente(t).await;

        let error = c
            .get::<Carga>("/tracks/abc")
            .await
            .expect_err("debe fallar");
        assert!(matches!(error, SpotifyError::Servidor { codigo: 503 }));
    }

    #[tokio::test]
    async fn un_404_no_se_reintenta() {
        let t = Arc::new(
            TransporteFalso::nuevo()
                .con_json(&token_ok())
                .con_estado(404, None),
        );
        let c = cliente(t.clone()).await;

        let error = c
            .get::<Carga>("/tracks/noexiste")
            .await
            .expect_err("debe fallar");
        assert!(matches!(error, SpotifyError::NoEncontrado(_)));
        // Token + una petición: no debe haber reintentos.
        assert_eq!(t.cuantas(), 2);
    }

    #[tokio::test]
    async fn un_403_se_distingue_como_falta_de_acceso() {
        // Es el caso de las playlists propiedad de Spotify desde 2024: existen,
        // pero no son accesibles con credenciales de aplicación. Dar "no
        // encontrado" a secas sería desconcertante.
        let t = Arc::new(
            TransporteFalso::nuevo()
                .con_json(&token_ok())
                .con_estado(403, None),
        );
        let c = cliente(t).await;

        match c
            .get::<Carga>("/playlists/xyz")
            .await
            .expect_err("debe fallar")
        {
            SpotifyError::NoEncontrado(m) => {
                assert!(m.contains("credenciales de aplicación"), "{m}");
            }
            otro => panic!("se esperaba NoEncontrado, llegó {otro:?}"),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn un_401_invalida_el_token_y_pide_uno_nuevo() {
        let t = Arc::new(
            TransporteFalso::nuevo()
                .con_json(&token_ok())
                .con_estado(401, None)
                .con_json(&token_ok())
                .con_json(r#"{"valor":"con token nuevo"}"#),
        );
        let c = cliente(t.clone()).await;

        let carga: Carga = c.get("/tracks/abc").await.expect("responde");
        assert_eq!(carga.valor, "con token nuevo");
        assert_eq!(
            t.cuantas(),
            4,
            "debe pedirse un token nuevo tras el 401, no reusar el caducado"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn un_429_registra_el_limite_y_reintenta_tras_la_espera() {
        let t = Arc::new(
            TransporteFalso::nuevo()
                .con_json(&token_ok())
                .con_estado(429, Some(2))
                .con_json(r#"{"valor":"tras esperar"}"#),
        );
        let c = cliente(t).await;

        let inicio = tokio::time::Instant::now();
        let carga: Carga = c.get("/search").await.expect("responde");

        assert_eq!(carga.valor, "tras esperar");
        assert!(
            inicio.elapsed() >= Duration::from_secs(2),
            "debe respetarse el Retry-After, esperó {:?}",
            inicio.elapsed()
        );
    }

    #[tokio::test]
    async fn las_rutas_relativas_se_completan_con_la_base() {
        let t = Arc::new(
            TransporteFalso::nuevo()
                .con_json(&token_ok())
                .con_json(r#"{"valor":"x"}"#),
        );
        let c = cliente(t.clone()).await;
        let _: Carga = c.get("/tracks/abc").await.expect("responde");

        let peticiones = t.registradas();
        let url = peticiones
            .iter()
            .find_map(|p| match p {
                crate::transporte::falso::Peticion::Get { url, .. } => Some(url.clone()),
                crate::transporte::falso::Peticion::PostForm { .. } => None,
            })
            .expect("hubo un GET");
        assert_eq!(url, "https://api.spotify.com/v1/tracks/abc");
    }

    #[tokio::test]
    async fn una_url_absoluta_se_respeta() {
        // Spotify devuelve `next` como URL completa al paginar.
        let t = Arc::new(
            TransporteFalso::nuevo()
                .con_json(&token_ok())
                .con_json(r#"{"valor":"x"}"#),
        );
        let c = cliente(t.clone()).await;
        let _: Carga = c
            .get("https://api.spotify.com/v1/playlists/x/tracks?offset=100")
            .await
            .expect("responde");

        let url = t
            .registradas()
            .iter()
            .find_map(|p| match p {
                crate::transporte::falso::Peticion::Get { url, .. } => Some(url.clone()),
                crate::transporte::falso::Peticion::PostForm { .. } => None,
            })
            .expect("hubo un GET");
        assert!(url.ends_with("offset=100"), "{url}");
    }
}

//! Autenticación: flujo de credenciales de cliente.
//!
//! **El usuario no inicia sesión.** No hay cuenta de Spotify implicada, ni
//! redirección OAuth, ni navegador: solo un `client_id` y un `client_secret` de
//! aplicación, que se obtienen gratis en el panel de desarrollador y se pegan
//! una vez en Ajustes (ADR-005).
//!
//! El token dura una hora y se refresca solo. Se renueva con margen para que
//! una petición no se encuentre con un token que acaba de caducar entre la
//! comprobación y el envío.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Mutex;
use tokio::time::Instant;

use crate::error::{SpotifyError, SpotifyResult};
use crate::models::TokenRespuesta;
use crate::transporte::Transporte;

const URL_TOKEN: &str = "https://accounts.spotify.com/api/token";

/// Margen de renovación anticipada.
///
/// Sin él, una petición podría comprobar que el token es válido, tardar unos
/// milisegundos en salir y encontrarse un 401 al llegar.
const MARGEN: Duration = Duration::from_secs(60);

/// Credenciales de aplicación.
#[derive(Clone)]
pub struct Credenciales {
    pub client_id: String,
    pub client_secret: String,
}

impl std::fmt::Debug for Credenciales {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // El secreto nunca debe aparecer en un log ni en un volcado de
        // depuración, y `Debug` se invoca en sitios difíciles de auditar.
        f.debug_struct("Credenciales")
            .field("client_id", &self.client_id)
            .field("client_secret", &"<oculto>")
            .finish()
    }
}

impl Credenciales {
    /// Cabecera `Authorization: Basic` en base64.
    #[must_use]
    pub fn basic_auth(&self) -> String {
        base64(format!("{}:{}", self.client_id, self.client_secret).as_bytes())
    }
}

/// Codificación base64 estándar.
///
/// Se implementa aquí en lugar de añadir una dependencia: son veinte líneas,
/// se usa en un único sitio y evita arrastrar un crate más al árbol.
fn base64(datos: &[u8]) -> String {
    const ALFABETO: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    let mut salida = String::with_capacity(datos.len().div_ceil(3) * 4);
    for trozo in datos.chunks(3) {
        let b = [
            trozo[0],
            trozo.get(1).copied().unwrap_or(0),
            trozo.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);

        salida.push(ALFABETO[((n >> 18) & 63) as usize] as char);
        salida.push(ALFABETO[((n >> 12) & 63) as usize] as char);
        salida.push(if trozo.len() > 1 {
            ALFABETO[((n >> 6) & 63) as usize] as char
        } else {
            '='
        });
        salida.push(if trozo.len() > 2 {
            ALFABETO[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    salida
}

#[derive(Debug)]
struct TokenVigente {
    valor: String,
    caduca: Instant,
}

/// Gestor del token de acceso.
pub struct GestorToken {
    credenciales: Mutex<Option<Credenciales>>,
    token: Mutex<Option<TokenVigente>>,
    transporte: Arc<dyn Transporte>,
}

impl std::fmt::Debug for GestorToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GestorToken").finish_non_exhaustive()
    }
}

impl GestorToken {
    #[must_use]
    pub fn nuevo(transporte: Arc<dyn Transporte>) -> Self {
        Self {
            credenciales: Mutex::new(None),
            token: Mutex::new(None),
            transporte,
        }
    }

    /// Establece las credenciales y descarta el token anterior.
    pub async fn set_credenciales(&self, credenciales: Option<Credenciales>) {
        *self.credenciales.lock().await = credenciales;
        // Un token obtenido con las credenciales viejas ya no representa nada.
        *self.token.lock().await = None;
    }

    pub async fn hay_credenciales(&self) -> bool {
        self.credenciales.lock().await.is_some()
    }

    /// Devuelve un token válido, obteniéndolo si hace falta.
    ///
    /// # Errors
    /// [`SpotifyError::SinCredenciales`] si no están configuradas, o el error
    /// del intercambio.
    pub async fn token(&self) -> SpotifyResult<String> {
        // El bloqueo se mantiene durante la petición a propósito: si diez
        // búsquedas arrancan a la vez con el token caducado, solo una lo
        // renueva y las demás esperan a su resultado. Sin esto, serían diez
        // intercambios simultáneos y un `429` casi seguro.
        let mut guard = self.token.lock().await;

        if let Some(t) = guard.as_ref()
            && Instant::now() + MARGEN < t.caduca
        {
            return Ok(t.valor.clone());
        }

        let credenciales = self
            .credenciales
            .lock()
            .await
            .clone()
            .ok_or(SpotifyError::SinCredenciales)?;

        let nuevo = self.intercambiar(&credenciales).await?;
        let valor = nuevo.valor.clone();
        *guard = Some(nuevo);
        Ok(valor)
    }

    /// Invalida el token actual. Se llama al recibir un `401`.
    pub async fn invalidar(&self) {
        *self.token.lock().await = None;
    }

    async fn intercambiar(&self, credenciales: &Credenciales) -> SpotifyResult<TokenVigente> {
        let respuesta = self
            .transporte
            .post_form(
                URL_TOKEN,
                &[("grant_type", "client_credentials")],
                &credenciales.basic_auth(),
            )
            .await?;

        match respuesta.estado {
            200 => {
                let cuerpo: TokenRespuesta = respuesta.json()?;
                if cuerpo.access_token.is_empty() {
                    return Err(SpotifyError::Respuesta("token vacío".into()));
                }
                tracing::debug!(expira_en = cuerpo.expires_in, "token de Spotify obtenido");
                Ok(TokenVigente {
                    valor: cuerpo.access_token,
                    caduca: Instant::now() + Duration::from_secs(cuerpo.expires_in),
                })
            }
            400 | 401 => Err(SpotifyError::CredencialesInvalidas),
            429 => Err(SpotifyError::LimiteAlcanzado {
                segundos: respuesta.retry_after.unwrap_or(5),
            }),
            codigo => Err(SpotifyError::Servidor { codigo }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transporte::falso::TransporteFalso;

    fn credenciales() -> Credenciales {
        Credenciales {
            client_id: "id-de-prueba".into(),
            client_secret: "secreto-de-prueba".into(),
        }
    }

    fn respuesta_token(expira_en: u64) -> String {
        format!(r#"{{"access_token":"tok-abc","token_type":"Bearer","expires_in":{expira_en}}}"#)
    }

    #[test]
    fn el_base64_coincide_con_el_estandar() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foob"), "Zm9vYg==");
        assert_eq!(base64(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64(b"id:secreto"), "aWQ6c2VjcmV0bw==");
    }

    #[test]
    fn el_debug_de_las_credenciales_oculta_el_secreto() {
        let texto = format!("{:?}", credenciales());
        assert!(texto.contains("id-de-prueba"));
        assert!(
            !texto.contains("secreto-de-prueba"),
            "el secreto no debe aparecer en un volcado de depuración: {texto}"
        );
    }

    #[tokio::test]
    async fn sin_credenciales_no_se_pide_token() {
        let transporte = Arc::new(TransporteFalso::nuevo());
        let gestor = GestorToken::nuevo(transporte.clone());

        let error = gestor.token().await.expect_err("debe fallar");
        assert!(matches!(error, SpotifyError::SinCredenciales));
        assert_eq!(transporte.cuantas(), 0, "no debe salir ninguna petición");
    }

    #[tokio::test]
    async fn el_token_se_obtiene_una_vez_y_se_reutiliza() {
        let transporte = Arc::new(TransporteFalso::nuevo().con_json(&respuesta_token(3600)));
        let gestor = GestorToken::nuevo(transporte.clone());
        gestor.set_credenciales(Some(credenciales())).await;

        assert_eq!(gestor.token().await.expect("token"), "tok-abc");
        assert_eq!(gestor.token().await.expect("token"), "tok-abc");
        assert_eq!(gestor.token().await.expect("token"), "tok-abc");

        assert_eq!(transporte.cuantas(), 1, "el token debe reutilizarse");
    }

    #[tokio::test]
    async fn un_token_a_punto_de_caducar_se_renueva_por_anticipado() {
        // 30 segundos está dentro del margen de 60: debe renovarse ya.
        let transporte = Arc::new(
            TransporteFalso::nuevo()
                .con_json(&respuesta_token(30))
                .con_json(&respuesta_token(3600)),
        );
        let gestor = GestorToken::nuevo(transporte.clone());
        gestor.set_credenciales(Some(credenciales())).await;

        gestor.token().await.expect("primero");
        gestor.token().await.expect("segundo");

        assert_eq!(
            transporte.cuantas(),
            2,
            "un token que caduca dentro del margen debe renovarse antes de usarse"
        );
    }

    #[tokio::test]
    async fn cambiar_las_credenciales_descarta_el_token_anterior() {
        let transporte = Arc::new(
            TransporteFalso::nuevo()
                .con_json(&respuesta_token(3600))
                .con_json(&respuesta_token(3600)),
        );
        let gestor = GestorToken::nuevo(transporte.clone());

        gestor.set_credenciales(Some(credenciales())).await;
        gestor.token().await.expect("primero");

        gestor
            .set_credenciales(Some(Credenciales {
                client_id: "otro".into(),
                client_secret: "otro".into(),
            }))
            .await;
        gestor.token().await.expect("segundo");

        assert_eq!(transporte.cuantas(), 2);
    }

    #[tokio::test]
    async fn unas_credenciales_rechazadas_dan_un_error_accionable() {
        let transporte = Arc::new(TransporteFalso::nuevo().con_estado(400, None));
        let gestor = GestorToken::nuevo(transporte);
        gestor.set_credenciales(Some(credenciales())).await;

        let error = gestor.token().await.expect_err("debe fallar");
        assert!(matches!(error, SpotifyError::CredencialesInvalidas));
        assert!(
            !error.es_reintentable(),
            "reintentar con las mismas no sirve"
        );
    }

    #[tokio::test]
    async fn un_429_en_el_token_conserva_el_retry_after() {
        let transporte = Arc::new(TransporteFalso::nuevo().con_estado(429, Some(17)));
        let gestor = GestorToken::nuevo(transporte);
        gestor.set_credenciales(Some(credenciales())).await;

        match gestor.token().await.expect_err("debe fallar") {
            SpotifyError::LimiteAlcanzado { segundos } => assert_eq!(segundos, 17),
            otro => panic!("se esperaba LimiteAlcanzado, llegó {otro:?}"),
        }
    }

    #[tokio::test]
    async fn peticiones_concurrentes_solo_intercambian_un_token() {
        // Sin el bloqueo durante la petición, diez búsquedas simultáneas al
        // arrancar harían diez intercambios y provocarían un 429.
        let transporte = Arc::new(TransporteFalso::nuevo().con_json(&respuesta_token(3600)));
        let gestor = Arc::new(GestorToken::nuevo(transporte.clone()));
        gestor.set_credenciales(Some(credenciales())).await;

        let tareas: Vec<_> = (0..10)
            .map(|_| {
                let g = Arc::clone(&gestor);
                tokio::spawn(async move { g.token().await })
            })
            .collect();

        for t in tareas {
            assert_eq!(t.await.expect("join").expect("token"), "tok-abc");
        }
        assert_eq!(transporte.cuantas(), 1);
    }

    #[tokio::test]
    async fn invalidar_fuerza_una_renovacion() {
        let transporte = Arc::new(
            TransporteFalso::nuevo()
                .con_json(&respuesta_token(3600))
                .con_json(&respuesta_token(3600)),
        );
        let gestor = GestorToken::nuevo(transporte.clone());
        gestor.set_credenciales(Some(credenciales())).await;

        gestor.token().await.expect("primero");
        gestor.invalidar().await;
        gestor.token().await.expect("segundo");

        assert_eq!(transporte.cuantas(), 2);
    }
}

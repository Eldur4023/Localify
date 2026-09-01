//! API de control local, para procesos externos.
//!
//! Servidor HTTP en `127.0.0.1`, con verbos sobre `/player`: pausar,
//! reanudar, saltar de pista, retroceder y leer el estado. Es el mismo
//! reproductor que ya controlan SMTC en Windows y MPRIS en Linux, pero por un
//! canal que no depende de ninguno de los dos: un script, un Stream Deck o un
//! atajo de teclado personalizado pueden hablarle con un `curl`, sin librería
//! de D-Bus ni de sesiones multimedia de por medio.
//!
//! También sirve `/window/show` y `/app/quit`: son el mismo puerto por el
//! que un segundo lanzamiento de `localify` —desde un widget, o
//! `localify --quit`— le habla a la instancia que ya está corriendo en vez
//! de arrancar una segunda (ver `bootstrap::avisar_a_la_instancia_en_marcha`).
//!
//! ## Solo loopback, sin autenticación
//!
//! Escucha en `127.0.0.1` y en ningún otro sitio: `Ipv4Addr::LOCALHOST` es
//! explícito y no un `0.0.0.0` que además abriera la red local. Dentro de esa
//! frontera no hay contraseña ni token — es la misma confianza que ya existe
//! entre procesos de un mismo usuario en la misma máquina, y exigir una
//! credencial ahí solo estorbaría al caso de uso que esto sirve. Lo que sí
//! importa es que un navegador **no pueda** usarlo desde una pestaña: sin
//! cabeceras CORS, la política de origen del navegador bloquea la petición
//! antes de que llegue aquí.
//!
//! ## Por qué no falla el arranque
//!
//! Es una superficie opcional, igual que Discord o MPRIS. Si el puerto ya
//! está ocupado, se avisa y la aplicación sigue: la reproducción nunca debe
//! depender de que este servidor consiga arrancar.

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;

use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{get, post};
use localify_core::domain::audio::DurationMs;
use localify_core::ports::services::PlaybackService;
use serde::Deserialize;
use tracing::{info, warn};

use crate::dto::common::ApiError;
use crate::dto::player::PlayerStateDto;

/// Puerto en el que escucha. Alto y poco común a propósito, para no chocar
/// con nada que el usuario ya tenga corriendo.
///
/// `pub(crate)`: `bootstrap::avisar_a_la_instancia_en_marcha` necesita saber
/// a qué puerto llamar, y un segundo número mantenido a mano en otro fichero
/// sería la forma más tonta de que los dos se desincronizaran.
pub(crate) const PUERTO: u16 = 51000;

type Resultado = Result<Json<PlayerStateDto>, Error>;

/// Envoltorio de `ApiError` para poder implementarle `IntoResponse` aquí:
/// ambos son de este crate, pero `dto/common.rs` no tiene por qué conocer
/// axum, que es un detalle de transporte de esta API y no de la de Tauri.
#[derive(Debug)]
struct Error(ApiError);

impl From<localify_core::error::CoreError> for Error {
    fn from(e: localify_core::error::CoreError) -> Self {
        Self(e.into())
    }
}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        let estado = match self.0.code.as_str() {
            "NOT_FOUND" => StatusCode::NOT_FOUND,
            "INVALID" => StatusCode::BAD_REQUEST,
            "CONFLICT" => StatusCode::CONFLICT,
            "RATE_LIMITED" => StatusCode::TOO_MANY_REQUESTS,
            "NOT_CONFIGURED" => StatusCode::PRECONDITION_FAILED,
            "SHUTTING_DOWN" => StatusCode::SERVICE_UNAVAILABLE,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (estado, Json(self.0)).into_response()
    }
}

#[derive(Clone)]
struct Estado {
    playback: Arc<dyn PlaybackService>,
    /// Cierres sobre el `AppHandle`, no el `AppHandle` en sí.
    ///
    /// Este módulo no tiene por qué saber cómo se crea una ventana de Tauri
    /// de verdad, y guardarlo así es lo que permite probar el enrutador con
    /// dobles triviales en vez de tener que levantar una aplicación Tauri
    /// entera dentro de un test.
    mostrar_ventana: Arc<dyn Fn() -> tauri::Result<()> + Send + Sync>,
    pedir_salida: Arc<dyn Fn() + Send + Sync>,
}

/// Arranca el servidor. No hace nada si el puerto está ocupado.
pub async fn arrancar(playback: Arc<dyn PlaybackService>, app: tauri::AppHandle) {
    let direccion = SocketAddr::from((Ipv4Addr::LOCALHOST, PUERTO));

    let listener = match tokio::net::TcpListener::bind(direccion).await {
        Ok(l) => l,
        Err(e) => {
            warn!(error = %e, puerto = PUERTO, "API de control no disponible: puerto ocupado");
            return;
        }
    };

    info!(direccion = %direccion, "API de control local escuchando");

    let app_para_ventana = app.clone();
    let estado = Estado {
        playback,
        mostrar_ventana: Arc::new(move || crate::bootstrap::mostrar_ventana(&app_para_ventana)),
        pedir_salida: Arc::new(move || app.exit(0)),
    };

    let router = enrutador(estado);
    if let Err(e) = axum::serve(listener, router).await {
        warn!(error = %e, "la API de control terminó con un error");
    }
}

fn enrutador(estado: Estado) -> Router {
    Router::new()
        .route("/player/state", get(estado_actual))
        .route("/player/play", post(reanudar))
        .route("/player/pause", post(pausar))
        .route("/player/toggle", post(alternar))
        .route("/player/next", post(siguiente))
        .route("/player/previous", post(anterior))
        .route("/player/seek", post(saltar))
        .route("/window/show", post(pedir_ventana))
        .route("/app/quit", post(pedir_salida))
        .with_state(estado)
}

/// Crea o muestra la ventana principal. Es lo que atiende un segundo
/// lanzamiento de `localify` sin `--quit`, tanto si esta instancia arrancó
/// `--headless` como si el usuario ya había cerrado su única ventana.
async fn pedir_ventana(State(estado): State<Estado>) -> StatusCode {
    match (estado.mostrar_ventana)() {
        Ok(()) => StatusCode::NO_CONTENT,
        Err(e) => {
            warn!(error = %e, "no se pudo mostrar la ventana desde la API de control");
            StatusCode::INTERNAL_SERVER_ERROR
        }
    }
}

/// Cierra la aplicación de verdad. Es el único camino: cerrar la ventana ya
/// no lo hace (ver `RunEvent::ExitRequested` en `bootstrap::run`).
async fn pedir_salida(State(estado): State<Estado>) -> StatusCode {
    info!("cierre pedido desde la API de control (--quit)");
    (estado.pedir_salida)();
    StatusCode::NO_CONTENT
}

async fn estado_actual(State(estado): State<Estado>) -> Json<PlayerStateDto> {
    Json(estado.playback.state().await.into())
}

async fn reanudar(State(estado): State<Estado>) -> Resultado {
    Ok(Json(estado.playback.resume().await?.into()))
}

async fn pausar(State(estado): State<Estado>) -> Resultado {
    Ok(Json(estado.playback.pause().await?.into()))
}

async fn alternar(State(estado): State<Estado>) -> Resultado {
    Ok(Json(estado.playback.toggle().await?.into()))
}

async fn siguiente(State(estado): State<Estado>) -> Resultado {
    Ok(Json(estado.playback.next().await?.into()))
}

/// Por debajo de tres segundos reproducidos va a la pista previa; por
/// encima reinicia la actual. Es el mismo criterio de `PlaybackService`, y
/// cubre tanto "pista anterior" como "rebobinar" sin inventar un tercer
/// verbo.
async fn anterior(State(estado): State<Estado>) -> Resultado {
    Ok(Json(estado.playback.previous().await?.into()))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SaltarCuerpo {
    position_ms: u32,
}

async fn saltar(State(estado): State<Estado>, Json(cuerpo): Json<SaltarCuerpo>) -> Resultado {
    Ok(Json(
        estado
            .playback
            .seek(DurationMs::new(cuerpo.position_ms))
            .await?
            .into(),
    ))
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};

    use async_trait::async_trait;
    use axum::http::StatusCode;
    use localify_core::domain::audio::Volume;
    use localify_core::domain::ids::{QueueEntryId, TrackId};
    use localify_core::domain::queue::{PlayStatus, PlaybackContext, PlayerState, RepeatMode};
    use localify_core::error::{CoreError, CoreResult};

    use super::*;

    /// Doble mínimo de `PlaybackService`: guarda un estado y lo devuelve o,
    /// si `que_falla()` lo construyó así, falla siempre con el mismo error
    /// que da `SinAudio` en modo degradado. No modela transiciones reales
    /// —eso ya lo prueba `reproduccion.rs`—; aquí solo interesa que el
    /// enrutador HTTP llame al método correcto y traduzca bien la
    /// respuesta.
    struct Fake {
        estado: Mutex<PlayerState>,
        falla: AtomicBool,
    }

    impl Fake {
        fn nuevo() -> Self {
            Self {
                estado: Mutex::new(PlayerState::detenido()),
                falla: AtomicBool::new(false),
            }
        }

        fn que_falla() -> Self {
            Self {
                estado: Mutex::new(PlayerState::detenido()),
                falla: AtomicBool::new(true),
            }
        }

        fn leer(&self) -> CoreResult<PlayerState> {
            if self.falla.load(Ordering::Relaxed) {
                Err(CoreError::Audio("sin dispositivo de audio".into()))
            } else {
                Ok(self
                    .estado
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone())
            }
        }

        fn escribir(&self, s: PlayerState) {
            *self
                .estado
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = s;
        }
    }

    #[async_trait]
    impl PlaybackService for Fake {
        async fn play_track(
            &self,
            _id: &TrackId,
            _ctx: PlaybackContext,
        ) -> CoreResult<PlayerState> {
            self.leer()
        }
        async fn toggle(&self) -> CoreResult<PlayerState> {
            let mut s = self.leer()?;
            s.status = if s.status == PlayStatus::Playing {
                PlayStatus::Paused
            } else {
                PlayStatus::Playing
            };
            self.escribir(s.clone());
            Ok(s)
        }
        async fn pause(&self) -> CoreResult<PlayerState> {
            let mut s = self.leer()?;
            s.status = PlayStatus::Paused;
            self.escribir(s.clone());
            Ok(s)
        }
        async fn resume(&self) -> CoreResult<PlayerState> {
            let mut s = self.leer()?;
            s.status = PlayStatus::Playing;
            self.escribir(s.clone());
            Ok(s)
        }
        async fn next(&self) -> CoreResult<PlayerState> {
            self.leer()
        }
        async fn previous(&self) -> CoreResult<PlayerState> {
            self.leer()
        }
        async fn seek(&self, position: DurationMs) -> CoreResult<PlayerState> {
            let mut s = self.leer()?;
            s.position = position;
            self.escribir(s.clone());
            Ok(s)
        }
        async fn set_volume(&self, _volume: Volume) -> CoreResult<PlayerState> {
            self.leer()
        }
        async fn set_repeat(&self, _mode: RepeatMode) -> CoreResult<PlayerState> {
            self.leer()
        }
        async fn set_shuffle(&self, _enabled: bool) -> CoreResult<PlayerState> {
            self.leer()
        }
        async fn jump_to(&self, _entry: QueueEntryId) -> CoreResult<PlayerState> {
            self.leer()
        }
        async fn state(&self) -> PlayerState {
            self.estado
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }
        fn position(&self) -> (DurationMs, DurationMs) {
            (DurationMs::ZERO, DurationMs::ZERO)
        }
        async fn persist_now(&self) -> CoreResult<()> {
            Ok(())
        }
    }

    fn estado_con(playback: Fake) -> Estado {
        Estado {
            playback: Arc::new(playback),
            mostrar_ventana: Arc::new(|| Ok(())),
            pedir_salida: Arc::new(|| {}),
        }
    }

    #[tokio::test]
    async fn el_estado_se_lee_sin_tocar_nada() {
        let Json(dto) = estado_actual(State(estado_con(Fake::nuevo()))).await;
        assert_eq!(dto.status, "stopped");
    }

    #[tokio::test]
    async fn pausar_deja_el_estado_en_paused() {
        let Json(dto) = pausar(State(estado_con(Fake::nuevo())))
            .await
            .expect("pausa");
        assert_eq!(dto.status, "paused");
    }

    #[tokio::test]
    async fn reanudar_deja_el_estado_en_playing() {
        let Json(dto) = reanudar(State(estado_con(Fake::nuevo())))
            .await
            .expect("reanuda");
        assert_eq!(dto.status, "playing");
    }

    #[tokio::test]
    async fn alternar_invierte_el_estado_previo() {
        let estado = estado_con(Fake::nuevo());
        let Json(primero) = alternar(State(estado.clone())).await.expect("alterna");
        assert_eq!(primero.status, "playing");
        let Json(segundo) = alternar(State(estado)).await.expect("alterna otra vez");
        assert_eq!(segundo.status, "paused");
    }

    #[tokio::test]
    async fn saltar_mueve_la_posicion_al_valor_pedido() {
        let Json(dto) = saltar(
            State(estado_con(Fake::nuevo())),
            Json(SaltarCuerpo { position_ms: 5_000 }),
        )
        .await
        .expect("salta");
        assert_eq!(dto.position_ms, 5_000);
    }

    #[tokio::test]
    async fn un_reproductor_que_falla_se_traduce_a_500() {
        let error = pausar(State(estado_con(Fake::que_falla())))
            .await
            .expect_err("debe fallar");
        assert_eq!(
            error.into_response().status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }

    #[test]
    fn cada_codigo_de_error_tiene_su_estado_http() {
        let casos = [
            ("NOT_FOUND", StatusCode::NOT_FOUND),
            ("INVALID", StatusCode::BAD_REQUEST),
            ("CONFLICT", StatusCode::CONFLICT),
            ("RATE_LIMITED", StatusCode::TOO_MANY_REQUESTS),
            ("NOT_CONFIGURED", StatusCode::PRECONDITION_FAILED),
            ("SHUTTING_DOWN", StatusCode::SERVICE_UNAVAILABLE),
            ("AUDIO", StatusCode::INTERNAL_SERVER_ERROR),
        ];
        for (codigo, esperado) in casos {
            let api_error = ApiError {
                code: codigo.to_owned(),
                message_key: "x".into(),
                params: vec![],
                actionable: false,
                retryable: false,
                detail: None,
            };
            let respuesta = Error(api_error).into_response();
            assert_eq!(respuesta.status(), esperado, "código {codigo}");
        }
    }

    #[test]
    fn el_puerto_no_es_uno_de_los_habituales() {
        // No es una prueba de comportamiento, sino una barrera contra un
        // cambio accidental a un puerto que otra cosa suele usar (3000,
        // 8000, 8080...) y que rompería en silencio para quien ya lo tenga
        // ocupado.
        assert!(!matches!(PUERTO, 80 | 443 | 3000 | 5000 | 8000 | 8080));
    }

    #[tokio::test]
    async fn pedir_ventana_llama_al_cierre_y_devuelve_204() {
        let llamado = Arc::new(AtomicBool::new(false));
        let marca = Arc::clone(&llamado);
        let estado = Estado {
            playback: Arc::new(Fake::nuevo()),
            mostrar_ventana: Arc::new(move || {
                marca.store(true, Ordering::Relaxed);
                Ok(())
            }),
            pedir_salida: Arc::new(|| {}),
        };

        let respuesta = pedir_ventana(State(estado)).await;

        assert!(llamado.load(Ordering::Relaxed));
        assert_eq!(respuesta, StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn un_fallo_al_mostrar_la_ventana_se_traduce_a_500() {
        let estado = Estado {
            playback: Arc::new(Fake::nuevo()),
            mostrar_ventana: Arc::new(|| Err(tauri::Error::WindowNotFound)),
            pedir_salida: Arc::new(|| {}),
        };

        assert_eq!(
            pedir_ventana(State(estado)).await,
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }

    #[tokio::test]
    async fn pedir_salida_llama_al_cierre_y_devuelve_204() {
        let llamado = Arc::new(AtomicBool::new(false));
        let marca = Arc::clone(&llamado);
        let estado = Estado {
            playback: Arc::new(Fake::nuevo()),
            mostrar_ventana: Arc::new(|| Ok(())),
            pedir_salida: Arc::new(move || marca.store(true, Ordering::Relaxed)),
        };

        let respuesta = pedir_salida(State(estado)).await;

        assert!(llamado.load(Ordering::Relaxed));
        assert_eq!(respuesta, StatusCode::NO_CONTENT);
    }
}

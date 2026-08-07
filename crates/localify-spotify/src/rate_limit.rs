//! Limitación de peticiones.
//!
//! Spotify no publica sus límites exactos y responde `429` con `Retry-After`
//! cuando los superas. La estrategia tiene dos mitades, y ambas hacen falta:
//!
//! - **Preventiva**: un cubo de tokens conservador, para no llegar al `429`.
//! - **Reactiva**: al recibir un `429`, se respeta `Retry-After` al pie de la
//!   letra y se bloquean las peticiones siguientes hasta que expire.
//!
//! Solo con la reactiva, una ráfaga de veinte peticiones se comería el límite y
//! todas esperarían. Solo con la preventiva, un límite más estricto de lo
//! previsto degeneraría en un bucle de reintentos.

use std::time::Duration;

use tokio::sync::Mutex;
use tokio::time::Instant;

/// Peticiones que se permiten en ráfaga.
///
/// Deliberadamente conservador: importar una playlist son diez peticiones
/// seguidas, y agotar el margen ahí para que la siguiente búsqueda del usuario
/// espere sería el peor reparto posible.
const RAFAGA: f64 = 10.0;

/// Peticiones por segundo en régimen sostenido.
const POR_SEGUNDO: f64 = 3.0;

#[derive(Debug)]
struct Estado {
    tokens: f64,
    ultimo: Instant,
    /// Instante hasta el que Spotify pidió esperar.
    bloqueado_hasta: Option<Instant>,
}

/// Cubo de tokens con bloqueo por `Retry-After`.
#[derive(Debug)]
pub struct Limitador {
    estado: Mutex<Estado>,
    rafaga: f64,
    por_segundo: f64,
}

impl Limitador {
    #[must_use]
    pub fn nuevo() -> Self {
        Self::con_parametros(RAFAGA, POR_SEGUNDO)
    }

    #[must_use]
    pub fn con_parametros(rafaga: f64, por_segundo: f64) -> Self {
        Self {
            estado: Mutex::new(Estado {
                tokens: rafaga,
                ultimo: Instant::now(),
                bloqueado_hasta: None,
            }),
            rafaga,
            por_segundo,
        }
    }

    /// Espera hasta que se pueda hacer una petición.
    pub async fn adquirir(&self) {
        loop {
            let espera = {
                let mut e = self.estado.lock().await;

                // Un `Retry-After` en vigor manda sobre el cubo: Spotify ya ha
                // dicho explícitamente cuánto esperar.
                if let Some(hasta) = e.bloqueado_hasta {
                    let ahora = Instant::now();
                    if ahora < hasta {
                        Some(hasta - ahora)
                    } else {
                        e.bloqueado_hasta = None;
                        None
                    }
                } else {
                    None
                }
            };

            if let Some(d) = espera {
                tokio::time::sleep(d).await;
                continue;
            }

            let espera = {
                let mut e = self.estado.lock().await;
                let ahora = Instant::now();
                let transcurrido = ahora.duration_since(e.ultimo).as_secs_f64();
                e.ultimo = ahora;
                e.tokens = (e.tokens + transcurrido * self.por_segundo).min(self.rafaga);

                if e.tokens >= 1.0 {
                    e.tokens -= 1.0;
                    return;
                }
                // Tiempo que falta para recuperar un token entero.
                Duration::from_secs_f64((1.0 - e.tokens) / self.por_segundo)
            };

            tokio::time::sleep(espera).await;
        }
    }

    /// Registra un `429`.
    ///
    /// Bloquea todas las peticiones hasta que expire la espera pedida, en lugar
    /// de dejar que cada una descubra el límite por su cuenta.
    pub async fn registrar_limite(&self, segundos: u64) {
        // Un `Retry-After` desmesurado (por un proxy mal configurado, por
        // ejemplo) dejaría la aplicación inservible. Se acota a cinco minutos.
        let segundos = segundos.min(300);
        let mut e = self.estado.lock().await;
        e.bloqueado_hasta = Some(Instant::now() + Duration::from_secs(segundos));
        e.tokens = 0.0;
        tracing::warn!(segundos, "Spotify aplicó limitación de peticiones");
    }

    /// `true` si hay un `Retry-After` en vigor.
    pub async fn esta_bloqueado(&self) -> bool {
        let e = self.estado.lock().await;
        e.bloqueado_hasta.is_some_and(|h| Instant::now() < h)
    }
}

impl Default for Limitador {
    fn default() -> Self {
        Self::nuevo()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn la_rafaga_inicial_no_espera() {
        let l = Limitador::con_parametros(10.0, 3.0);
        let inicio = Instant::now();

        for _ in 0..10 {
            l.adquirir().await;
        }

        assert_eq!(
            inicio.elapsed(),
            Duration::ZERO,
            "las diez primeras deben pasar sin esperar"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn agotada_la_rafaga_se_espera_al_ritmo_sostenido() {
        let l = Limitador::con_parametros(10.0, 3.0);
        for _ in 0..10 {
            l.adquirir().await;
        }

        let inicio = Instant::now();
        l.adquirir().await;
        let esperado = Duration::from_secs_f64(1.0 / 3.0);

        assert!(
            inicio.elapsed() >= esperado.mul_f64(0.9),
            "la undécima debería esperar ~333 ms, esperó {:?}",
            inicio.elapsed()
        );
    }

    #[tokio::test(start_paused = true)]
    async fn los_tokens_se_recuperan_con_el_tiempo() {
        let l = Limitador::con_parametros(10.0, 3.0);
        for _ in 0..10 {
            l.adquirir().await;
        }

        // Cinco segundos dan para recuperar el cubo entero.
        tokio::time::sleep(Duration::from_secs(5)).await;

        let inicio = Instant::now();
        for _ in 0..10 {
            l.adquirir().await;
        }
        assert_eq!(inicio.elapsed(), Duration::ZERO);
    }

    #[tokio::test(start_paused = true)]
    async fn un_429_bloquea_hasta_que_expira_el_retry_after() {
        let l = Limitador::con_parametros(10.0, 3.0);
        l.registrar_limite(30).await;
        assert!(l.esta_bloqueado().await);

        let inicio = Instant::now();
        l.adquirir().await;

        assert!(
            inicio.elapsed() >= Duration::from_secs(30),
            "debe respetarse el Retry-After al pie de la letra, esperó {:?}",
            inicio.elapsed()
        );
        assert!(!l.esta_bloqueado().await);
    }

    #[tokio::test(start_paused = true)]
    async fn un_retry_after_desmesurado_se_acota() {
        // Un proxy mal configurado no debe dejar la aplicación inservible.
        let l = Limitador::con_parametros(10.0, 3.0);
        l.registrar_limite(86_400).await;

        let inicio = Instant::now();
        l.adquirir().await;

        assert!(
            inicio.elapsed() <= Duration::from_secs(301),
            "la espera debe acotarse a cinco minutos, esperó {:?}",
            inicio.elapsed()
        );
    }

    #[tokio::test(start_paused = true)]
    async fn varias_tareas_concurrentes_comparten_el_limite() {
        let l = std::sync::Arc::new(Limitador::con_parametros(5.0, 2.0));

        let tareas: Vec<_> = (0..5)
            .map(|_| {
                let l = std::sync::Arc::clone(&l);
                tokio::spawn(async move { l.adquirir().await })
            })
            .collect();

        for t in tareas {
            t.await.expect("join");
        }

        // El cubo debe estar agotado: la siguiente espera.
        let inicio = Instant::now();
        l.adquirir().await;
        assert!(inicio.elapsed() > Duration::ZERO);
    }
}

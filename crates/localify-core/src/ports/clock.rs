//! Reloj inyectable.
//!
//! Que el tiempo sea una dependencia y no una llamada estática es lo que hace
//! testeables las caducidades de caché y el backoff de reintentos, sin que la
//! suite tarde minutos ni dependa del reloj real.

use chrono::{DateTime, Utc};

pub trait Clock: Send + Sync + 'static {
    fn now(&self) -> DateTime<Utc>;

    /// Instante monótono en milisegundos desde el arranque del proceso.
    ///
    /// Para medir intervalos hay que usar esto y no `now()`: el reloj de pared
    /// puede saltar hacia atrás (NTP, cambio de horario) y convertir una
    /// duración en negativa.
    fn monotonic_ms(&self) -> u64;
}

/// Reloj del sistema.
#[derive(Debug, Clone, Copy)]
pub struct SystemClock {
    inicio: std::time::Instant,
}

impl SystemClock {
    #[must_use]
    pub fn new() -> Self {
        Self {
            inicio: std::time::Instant::now(),
        }
    }
}

impl Default for SystemClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }

    #[allow(clippy::cast_possible_truncation)]
    fn monotonic_ms(&self) -> u64 {
        self.inicio.elapsed().as_millis() as u64
    }
}

//! Scrobbles: escuchas que se envían a un servicio externo.
//!
//! ## Por qué esto no es el historial
//!
//! El historial es de Localify y sirve para recomendar; un scrobble es de
//! Last.fm y sirve para su perfil público. Comparten el hecho —alguien escuchó
//! algo— y **no comparten la regla**: Localify da una escucha por completa al
//! 90 %, Last.fm al 50 % o a los cuatro minutos, lo que ocurra antes, y encima
//! descarta lo que dure menos de treinta segundos.
//!
//! Usar el umbral del historial para scrobblear haría lo que parece un detalle
//! y no lo es: una canción de nueve minutos escuchada seis —Last.fm la cuenta
//! desde el minuto cuatro— no llegaría al 90 % y no se scrobblearía nunca.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::audio::DurationMs;
use super::ids::TrackId;

/// Duración mínima para que Last.fm acepte una pista. Por debajo la ignora.
pub const MINIMO_SCROBBLEABLE: DurationMs = DurationMs::new(30_000);

/// Mitad de la pista: uno de los dos umbrales de la regla oficial.
const FRACCION: f32 = 0.5;

/// El otro: cuatro minutos, para las pistas largas.
pub const TOPE_ABSOLUTO_MS: u32 = 4 * 60 * 1000;

/// Escucha aún no entregada.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingScrobble {
    /// Clave de la fila, para poder borrarla o marcarla sin ambigüedad. Es de
    /// la cola, no del dominio: dos escuchas de la misma pista en el mismo
    /// segundo son dos scrobbles distintos y hay que poder distinguirlos.
    pub id: i64,
    pub track_id: TrackId,
    /// **Cuándo empezó a sonar**, que es lo que pide la API, no cuándo acabó.
    pub started_at: DateTime<Utc>,
    pub attempts: u32,
}

/// Decide si una escucha se scrobblea, según la regla oficial de Last.fm.
///
/// <https://www.last.fm/api/scrobbling#when-is-a-scrobble-a-scrobble>
#[must_use]
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
pub fn merece_scrobble(ms_played: u32, duration: DurationMs) -> bool {
    // Sin duración conocida no hay forma de aplicar la regla, y Last.fm rechaza
    // el scrobble igualmente: no se envía.
    if duration.as_ms() < MINIMO_SCROBBLEABLE.as_ms() {
        return false;
    }
    let mitad = (duration.as_ms() as f32 * FRACCION) as u32;
    ms_played >= mitad.min(TOPE_ABSOLUTO_MS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn una_pista_corriente_se_scrobblea_a_la_mitad() {
        let dur = DurationMs::new(200_000);
        assert!(!merece_scrobble(99_000, dur));
        assert!(merece_scrobble(100_000, dur));
    }

    #[test]
    fn una_pista_larga_se_scrobblea_a_los_cuatro_minutos() {
        // Nueve minutos: la mitad serían cuatro y medio, pero la regla corta
        // antes. Con el umbral del historial —el 90 %— esto no se scrobblearía
        // hasta el minuto ocho.
        let dur = DurationMs::new(9 * 60 * 1000);
        assert!(merece_scrobble(TOPE_ABSOLUTO_MS, dur));
        assert!(!merece_scrobble(TOPE_ABSOLUTO_MS - 1, dur));
    }

    #[test]
    fn lo_que_dura_menos_de_treinta_segundos_no_se_envia() {
        // Da igual que se haya oído entera: Last.fm la rechazaría.
        let dur = DurationMs::new(20_000);
        assert!(!merece_scrobble(20_000, dur));
    }

    #[test]
    fn una_duracion_desconocida_no_se_envia() {
        assert!(!merece_scrobble(120_000, DurationMs::ZERO));
    }
}

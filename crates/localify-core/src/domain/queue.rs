//! Cola de reproducción y estado del reproductor.
//!
//! El modelo de **dos colas** es lo que hace que la reproducción se sienta como
//! Spotify:
//!
//! - La **cola de usuario** (`add_next` / `add_last`) tiene prioridad absoluta,
//!   se consume al reproducirse y sobrevive a un cambio de contexto.
//! - La **cola de contexto** se deriva del álbum, playlist o búsqueda que
//!   originó la reproducción, y se regenera al cambiar de contexto.

use serde::{Deserialize, Serialize};

use super::audio::{DurationMs, Volume};
use super::ids::{AlbumId, ArtistId, PlaylistId, QueueEntryId, TrackId};
use super::track::TrackRow;

/// De dónde salió la reproducción actual. Determina qué suena después y qué
/// texto muestra el panel de cola ("Siguiente desde: …").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum PlaybackContext {
    #[serde(rename_all = "camelCase")]
    Album { id: AlbumId },
    #[serde(rename_all = "camelCase")]
    Playlist { id: PlaylistId },
    #[serde(rename_all = "camelCase")]
    Artist { id: ArtistId },
    /// "Tus me gusta".
    Liked,
    /// Toda la biblioteca.
    Library,
    /// Resultados de búsqueda: el conjunto es efímero, así que se lleva
    /// consigo en lugar de poder reconstruirse desde un ID.
    #[serde(rename_all = "camelCase")]
    Search {
        query: String,
        track_ids: Vec<TrackId>,
    },
    #[serde(rename_all = "camelCase")]
    Recommendation {
        seed_track_id: TrackId,
        track_ids: Vec<TrackId>,
    },
    /// Una sola pista, sin contexto: al acabar, no hay siguiente.
    Single,
}

impl PlaybackContext {
    /// Clave i18n para "Siguiente desde: …".
    #[must_use]
    pub const fn label_key(&self) -> &'static str {
        match self {
            Self::Album { .. } => "context.album",
            Self::Playlist { .. } => "context.playlist",
            Self::Artist { .. } => "context.artist",
            Self::Liked => "context.liked",
            Self::Library => "context.library",
            Self::Search { .. } => "context.search",
            Self::Recommendation { .. } => "context.recommendation",
            Self::Single => "context.single",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RepeatMode {
    #[default]
    Off,
    /// Al acabar la cola, vuelve al principio del contexto.
    Queue,
    /// Repite la pista actual indefinidamente. `next` manual **sí** avanza,
    /// igual que en Spotify.
    Track,
}

impl RepeatMode {
    /// Rotación del botón: Off → Queue → Track → Off.
    #[must_use]
    pub const fn siguiente(self) -> Self {
        match self {
            Self::Off => Self::Queue,
            Self::Queue => Self::Track,
            Self::Track => Self::Off,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PlayStatus {
    Playing,
    Paused,
    /// Esperando datos: descarga en curso o seek más allá de lo descargado.
    Buffering,
    Stopped,
}

impl PlayStatus {
    #[must_use]
    pub const fn esta_activo(self) -> bool {
        matches!(self, Self::Playing | Self::Buffering)
    }
}

/// Por qué se avanzó de pista. Distinguirlo importa: solo un final natural
/// cuenta como reproducción completa para el historial y el scrobbling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AdvanceReason {
    NaturalEnd,
    UserSkip,
    UserPrevious,
    /// Fallo de reproducción: se salta la pista sin detener la sesión.
    Error,
}

/// Quién originó un cambio de pista.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ChangeSource {
    User,
    Queue,
    /// Restauración de la sesión anterior al arrancar.
    Restore,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueEntry {
    pub entry_id: QueueEntryId,
    pub track: TrackRow,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueSnapshot {
    /// Revisión monótona: permite descartar respuestas obsoletas sin comparar
    /// el contenido entero.
    pub revision: u64,
    pub current: Option<QueueEntry>,
    pub user_queue: Vec<QueueEntry>,
    /// Ventana de las siguientes del contexto, no el contexto completo: una
    /// biblioteca de 50 000 pistas no cabe en un evento IPC.
    pub context_queue: Vec<QueueEntry>,
    pub context: Option<PlaybackContext>,
}

impl QueueSnapshot {
    #[must_use]
    pub fn vacia() -> Self {
        Self {
            revision: 0,
            current: None,
            user_queue: Vec::new(),
            context_queue: Vec::new(),
            context: None,
        }
    }
}

impl Default for QueueSnapshot {
    fn default() -> Self {
        Self::vacia()
    }
}

/// Estado completo del reproductor. Es la respuesta de `player_get_state`, el
/// comando de resincronización cuando el frontend pierde eventos.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayerState {
    pub track: Option<TrackRow>,
    pub status: PlayStatus,
    pub position: DurationMs,
    pub duration: DurationMs,
    /// Cuánto hay decodificable. Solo difiere de `duration` durante una
    /// descarga progresiva.
    pub buffered: DurationMs,
    pub volume: Volume,
    pub repeat: RepeatMode,
    pub shuffle: bool,
    pub context: Option<PlaybackContext>,
}

impl PlayerState {
    #[must_use]
    pub fn detenido() -> Self {
        Self {
            track: None,
            status: PlayStatus::Stopped,
            position: DurationMs::ZERO,
            duration: DurationMs::ZERO,
            buffered: DurationMs::ZERO,
            volume: Volume::default(),
            repeat: RepeatMode::default(),
            shuffle: false,
            context: None,
        }
    }
}

impl Default for PlayerState {
    fn default() -> Self {
        Self::detenido()
    }
}

/// Umbral de la regla de "anterior": por debajo, `previous` va a la pista
/// anterior; por encima, reinicia la actual. Es el comportamiento de Spotify.
pub const UMBRAL_ANTERIOR: DurationMs = DurationMs::new(3000);

/// Genera una permutación estable para el modo aleatorio.
///
/// No se sortea en cada avance: se baraja una vez con una semilla que se
/// persiste. Así "anterior" funciona, desactivar shuffle recupera el orden
/// original y la permutación sobrevive a un reinicio de la aplicación.
///
/// El generador es un xorshift64* propio: no necesitamos calidad
/// criptográfica, sí reproducibilidad exacta a partir de la semilla y cero
/// dependencias en `core`.
#[must_use]
#[allow(
    clippy::cast_possible_truncation,
    reason = "el módulo acota el valor a `longitud`, que ya es un usize válido"
)]
pub fn permutacion_estable(longitud: usize, semilla: u64) -> Vec<usize> {
    let mut indices: Vec<usize> = (0..longitud).collect();
    if longitud < 2 {
        return indices;
    }

    let mut estado = if semilla == 0 {
        0x9E37_79B9_7F4A_7C15
    } else {
        semilla
    };
    let mut siguiente = || {
        estado ^= estado << 13;
        estado ^= estado >> 7;
        estado ^= estado << 17;
        estado.wrapping_mul(0x2545_F491_4F6C_DD1D)
    };

    // Fisher-Yates.
    for i in (1..longitud).rev() {
        let j = (siguiente() % (i as u64 + 1)) as usize;
        indices.swap(i, j);
    }
    indices
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn el_boton_de_repeticion_cicla() {
        assert_eq!(RepeatMode::Off.siguiente(), RepeatMode::Queue);
        assert_eq!(RepeatMode::Queue.siguiente(), RepeatMode::Track);
        assert_eq!(RepeatMode::Track.siguiente(), RepeatMode::Off);
    }

    #[test]
    fn la_permutacion_es_reproducible_a_partir_de_la_semilla() {
        let a = permutacion_estable(100, 42);
        let b = permutacion_estable(100, 42);
        assert_eq!(
            a, b,
            "la misma semilla debe dar la misma permutación tras reiniciar"
        );
    }

    #[test]
    fn semillas_distintas_dan_permutaciones_distintas() {
        assert_ne!(permutacion_estable(100, 1), permutacion_estable(100, 2));
    }

    #[test]
    fn la_permutacion_contiene_todos_los_indices_una_vez() {
        let p = permutacion_estable(500, 7);
        let mut ordenada = p.clone();
        ordenada.sort_unstable();
        assert_eq!(
            ordenada,
            (0..500).collect::<Vec<_>>(),
            "se perdió o duplicó algún índice"
        );
    }

    #[test]
    fn los_casos_degenerados_no_revientan() {
        assert!(permutacion_estable(0, 5).is_empty());
        assert_eq!(permutacion_estable(1, 5), vec![0]);
        // Semilla cero: xorshift se quedaría clavado en 0, por eso se sustituye.
        assert_eq!(permutacion_estable(50, 0).len(), 50);
    }

    #[test]
    fn la_permutacion_baraja_de_verdad() {
        let p = permutacion_estable(1000, 12345);
        let en_su_sitio = p.iter().enumerate().filter(|(i, v)| i == *v).count();
        // En una permutación aleatoria de n elementos, el número esperado de
        // puntos fijos es 1, independientemente de n.
        assert!(
            en_su_sitio < 20,
            "demasiados elementos sin mover: {en_su_sitio}"
        );
    }
}

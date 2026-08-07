//! Playlists locales.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::audio::DurationMs;
use super::ids::{AlbumId, PlaylistEntryId, PlaylistId};
use super::track::TrackRow;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PlaylistSource {
    #[default]
    Local,
    /// Importada desde una playlist pública de Spotify. Tras importarla es una
    /// playlist local normal: no se sincroniza ni se mantiene vinculada.
    SpotifyImport,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Playlist {
    pub id: PlaylistId,
    pub name: String,
    pub description: Option<String>,
    /// Portada elegida por el usuario. Si es `None`, la UI compone un mosaico
    /// con las portadas de las primeras pistas.
    pub cover_path: Option<String>,
    pub source: PlaylistSource,
    pub source_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaylistSummary {
    pub id: PlaylistId,
    pub name: String,
    pub track_count: u32,
    /// Álbumes cuyas portadas componen la imagen de la playlist, hasta cuatro.
    ///
    /// ## Identificadores, no rutas
    ///
    /// El campo llevaba rutas de fichero y era una mezcla de dos cosas: la
    /// portada propia venía como ruta relativa y el mosaico como
    /// identificadores de álbum, en la misma lista de `String`. Quien la
    /// recibía no podía saber cuál le había tocado.
    ///
    /// Ahora son siempre identificadores. El frontend pide la imagen por el
    /// esquema `cover://`, como hace con los álbumes, y ninguna ruta de disco
    /// cruza el puente (ADR-018).
    pub cover_albums: Vec<AlbumId>,
    /// `true` si el usuario eligió una imagen propia.
    ///
    /// Va como booleano y no como ruta: la imagen se sirve por el mismo esquema
    /// que las portadas de álbum, direccionada por el identificador de la
    /// playlist. Quien la pinta solo necesita saber si tiene que pedirla o
    /// componer el mosaico.
    pub has_own_cover: bool,
    pub updated_at: DateTime<Utc>,
    pub source: PlaylistSource,
}

/// Entrada de playlist. Tiene identidad propia porque la misma pista puede
/// aparecer varias veces y "elimina esta fila" debe ser inequívoco.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaylistEntry {
    pub entry_id: PlaylistEntryId,
    pub track: TrackRow,
    pub added_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaylistDetail {
    pub summary: PlaylistSummary,
    pub description: Option<String>,
    pub entries: Vec<PlaylistEntry>,
    pub total_duration: DurationMs,
}

/// Progreso de una importación desde Spotify.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportProgress {
    pub import_id: uuid::Uuid,
    pub done: u32,
    pub total: u32,
    pub playlist_id: Option<PlaylistId>,
}

/// Claves de ordenación fraccionarias (ADR-009).
///
/// Reordenar por índice entero obliga a reescribir hasta N filas por arrastre.
/// Con una clave `f64` basta actualizar **una**: la nueva posición es el punto
/// medio entre los vecinos.
pub mod position {
    /// Separación mínima antes de rebalancear. Por debajo, la precisión de
    /// `f64` empieza a comprometerse tras muchas inserciones en el mismo hueco.
    pub const EPSILON: f64 = 1e-6;

    /// Separación entre elementos al numerar desde cero.
    pub const PASO: f64 = 1024.0;

    /// Clave para insertar entre dos vecinos.
    ///
    /// - Al principio (`antes = None`): por debajo del primero.
    /// - Al final (`despues = None`): por encima del último.
    /// - En medio: el punto medio.
    #[must_use]
    pub fn entre(antes: Option<f64>, despues: Option<f64>) -> f64 {
        match (antes, despues) {
            (None, None) => 0.0,
            (None, Some(d)) => d - PASO,
            (Some(a), None) => a + PASO,
            (Some(a), Some(d)) => f64::midpoint(a, d),
        }
    }

    /// `true` si el hueco es tan estrecho que conviene rebalancear la playlist
    /// en segundo plano.
    #[must_use]
    pub fn necesita_rebalanceo(antes: Option<f64>, despues: Option<f64>) -> bool {
        match (antes, despues) {
            (Some(a), Some(d)) => (d - a).abs() < EPSILON,
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::position;

    #[test]
    fn insertar_en_una_lista_vacia_da_cero() {
        assert!((position::entre(None, None) - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn insertar_en_los_extremos_deja_hueco() {
        assert!(position::entre(None, Some(0.0)) < 0.0);
        assert!(position::entre(Some(0.0), None) > 0.0);
    }

    #[test]
    fn insertar_en_medio_da_el_punto_medio() {
        assert!((position::entre(Some(1.0), Some(2.0)) - 1.5).abs() < f64::EPSILON);
    }

    #[test]
    fn insertar_repetidamente_en_el_mismo_hueco_acaba_pidiendo_rebalanceo() {
        // Simula el peor caso real: arrastrar siempre al mismo punto.
        let (mut a, mut d) = (0.0_f64, 1.0_f64);
        let mut iteraciones = 0;
        while !position::necesita_rebalanceo(Some(a), Some(d)) {
            d = position::entre(Some(a), Some(d));
            iteraciones += 1;
            assert!(
                iteraciones < 100,
                "el umbral de rebalanceo no se alcanza nunca"
            );
        }
        // ~20 inserciones sucesivas en el mismo hueco. Se detecta mucho antes
        // de que f64 pierda precisión, que es lo que se quería garantizar.
        assert!(
            iteraciones > 15,
            "el rebalanceo se dispararía demasiado pronto"
        );
        assert!(a < d);
        a = 0.0;
        assert!(a < d);
    }
}

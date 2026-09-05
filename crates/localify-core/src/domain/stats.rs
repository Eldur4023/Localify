//! Estadísticas de escucha: cuánto tiempo real se ha pasado oyendo música.
//!
//! Distinto de [`super::library::LibraryStats`], que cuenta disco (pistas,
//! bytes, álbumes): esto cuenta **tiempo**, sobre los mismos datos que ya
//! alimenta el motor de recomendaciones (`play_history`). `ms_played` es
//! tiempo real de reproducción —sin pausas, sin saltos cortos— porque así lo
//! garantiza `cerrar_escucha` en el actor de reproducción; aquí solo se agrega.

use super::artist::ArtistRow;
use super::track::TrackRow;

/// Resumen de toda la escucha registrada, sin ventana temporal.
#[derive(Debug, Clone, Default)]
pub struct ListeningStats {
    pub total_ms_played: u64,
    /// Escuchas registradas en total. Cada una ya superó el mínimo que evita
    /// contar un salto de un par de segundos como una escucha.
    pub total_plays: u64,
    pub distinct_tracks: u64,
    pub top_tracks: Vec<TrackListeningStat>,
    pub top_artists: Vec<ArtistListeningStat>,
}

/// Una canción y cuánto tiempo real se ha pasado escuchándola.
///
/// No es lo mismo que "más escuchada" en el sentido de Inicio
/// (`HistoryRepository::top_tracks`), que pondera por si la escucha se
/// completó para decidir qué recomendar. Aquí el orden es tiempo puro: una
/// canción larga escuchada pocas veces puede superar a una corta repetida.
#[derive(Debug, Clone)]
pub struct TrackListeningStat {
    pub track: TrackRow,
    pub ms_played: u64,
    pub plays: u32,
}

/// Un artista y cuánto tiempo real se ha pasado escuchándolo.
#[derive(Debug, Clone)]
pub struct ArtistListeningStat {
    pub artist: ArtistRow,
    pub ms_played: u64,
}

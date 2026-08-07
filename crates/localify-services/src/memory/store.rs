//! Almacén en memoria compartido por los servicios provisionales.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use localify_core::domain::album::{Album, AlbumType, CoverSet};
use localify_core::domain::artist::Artist;
use localify_core::domain::audio::{DurationMs, Volume};
use localify_core::domain::availability::Availability;
use localify_core::domain::ids::{AlbumId, ArtistId, PlaylistEntryId, PlaylistId, TrackId};
use localify_core::domain::playlist::{Playlist, PlaylistSource};
use localify_core::domain::queue::{PlayStatus, PlayerState, QueueSnapshot, RepeatMode};
use localify_core::domain::settings::Settings;
use localify_core::domain::track::{AlbumRef, ArtistRef, Track, TrackRow};

/// Catálogo de ejemplo: título, artista, álbum y duración en milisegundos.
///
/// Va como constante y no incrustado en el código de siembra para que ampliarlo
/// sea añadir una línea. La variedad importa: nombres con diacríticos, varios
/// álbumes por artista y duraciones distintas ejercitan la normalización, las
/// agrupaciones y el formateo de tiempos.
const CATALOGO_EJEMPLO: &[(&str, &str, &str, u32)] = &[
    (
        "Bohemian Rhapsody",
        "Queen",
        "A Night at the Opera",
        354_000,
    ),
    ("Under Pressure", "Queen", "Hot Space", 248_000),
    ("Radio Ga Ga", "Queen", "The Works", 348_000),
    ("Paranoid Android", "Radiohead", "OK Computer", 383_000),
    ("Karma Police", "Radiohead", "OK Computer", 264_000),
    ("No Surprises", "Radiohead", "OK Computer", 229_000),
    ("Jóga", "Björk", "Homogenic", 305_000),
    ("Hyperballad", "Björk", "Post", 315_000),
    ("Around the World", "Daft Punk", "Homework", 428_000),
    ("Digital Love", "Daft Punk", "Discovery", 301_000),
    ("One More Time", "Daft Punk", "Discovery", 320_000),
    ("Glory Box", "Portishead", "Dummy", 305_000),
    ("Roads", "Portishead", "Dummy", 302_000),
    ("Teardrop", "Massive Attack", "Mezzanine", 330_000),
    ("Angel", "Massive Attack", "Mezzanine", 380_000),
];

/// Datos de la aplicación mientras la persistencia real no está cableada.
#[derive(Debug, Default)]
pub struct Datos {
    pub tracks: Vec<Track>,
    pub albums: HashMap<AlbumId, Album>,
    pub artists: HashMap<ArtistId, Artist>,
    pub availability: HashMap<TrackId, Availability>,
    pub favorites: Vec<TrackId>,
    pub playlists: Vec<Playlist>,
    pub playlist_items: HashMap<PlaylistId, Vec<(PlaylistEntryId, TrackId)>>,
    pub player: PlayerState,
    pub queue: QueueSnapshot,
    pub settings: Option<Settings>,
    /// Contador de búsquedas, para el `queryId` monótono.
    pub query_id: u64,
}

/// Almacén compartido. Clonarlo comparte los datos, no los copia.
#[derive(Debug, Clone)]
pub struct MemoryStore {
    datos: Arc<RwLock<Datos>>,
}

impl MemoryStore {
    /// Almacén vacío.
    #[must_use]
    pub fn vacio() -> Self {
        Self {
            datos: Arc::new(RwLock::new(Datos::default())),
        }
    }

    /// Almacén con un catálogo de ejemplo.
    ///
    /// Existe para que el frontend tenga algo que pintar antes de que haya
    /// proveedores reales: sin datos, no se puede comprobar que una lista
    /// virtualizada funcione ni que la barra de reproducción se actualice.
    #[must_use]
    pub fn con_ejemplo() -> Self {
        let store = Self::vacio();
        store.sembrar();
        store
    }

    /// Ejecuta una lectura sobre los datos.
    ///
    /// Si el lock estuviera envenenado por un `panic` previo, se recupera en
    /// lugar de propagar: perder el acceso a datos provisionales por un fallo
    /// ajeno no aporta nada.
    pub fn leer<T>(&self, f: impl FnOnce(&Datos) -> T) -> T {
        let guard = self
            .datos
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        f(&guard)
    }

    /// Ejecuta una escritura sobre los datos.
    pub fn escribir<T>(&self, f: impl FnOnce(&mut Datos) -> T) -> T {
        let mut guard = self
            .datos
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        f(&mut guard)
    }

    /// Construye la fila de lista de una pista.
    #[must_use]
    pub fn fila(datos: &Datos, track: &Track) -> TrackRow {
        TrackRow {
            id: track.id.clone(),
            title: track.title.clone(),
            artist_display: track.artist_display(),
            album_id: track.album.as_ref().map(|a| a.id.clone()),
            album_title: track.album.as_ref().map(|a| a.title.clone()),
            duration: track.duration,
            availability: datos
                .availability
                .get(&track.id)
                .cloned()
                .unwrap_or(Availability::Absent),
            is_favorite: datos.favorites.contains(&track.id),
            explicit: track.explicit,
            popularity: track.popularity,
            // El almacén en memoria no guarda cuándo entró nada en cada lista:
            // existe para probar caminos, no para fechar filas.
            added_at: None,
        }
    }

    /// Busca una pista por identificador.
    #[must_use]
    pub fn buscar_pista(datos: &Datos, id: &TrackId) -> Option<Track> {
        datos.tracks.iter().find(|t| &t.id == id).cloned()
    }

    fn sembrar(&self) {
        self.escribir(|d| {
            Self::sembrar_catalogo(d);
            Self::sembrar_playlist(d);

            d.player = PlayerState {
                track: None,
                status: PlayStatus::Stopped,
                position: DurationMs::ZERO,
                duration: DurationMs::ZERO,
                buffered: DurationMs::ZERO,
                volume: Volume::new(0.7),
                repeat: RepeatMode::Off,
                shuffle: false,
                context: None,
            };
            d.queue = QueueSnapshot::vacia();
        });
    }

    fn sembrar_catalogo(d: &mut Datos) {
        {
            let mut albumes: HashMap<String, AlbumId> = HashMap::new();
            let mut artistas: HashMap<String, ArtistId> = HashMap::new();

            for (i, &(titulo, artista, album, duracion)) in CATALOGO_EJEMPLO.iter().enumerate() {
                let artist_id = artistas
                    .entry(artista.to_owned())
                    .or_insert_with(ArtistId::nuevo_local)
                    .clone();
                let album_id = albumes
                    .entry(album.to_owned())
                    .or_insert_with(AlbumId::nuevo_local)
                    .clone();

                d.artists
                    .entry(artist_id.clone())
                    .or_insert_with(|| Artist {
                        id: artist_id.clone(),
                        name: artista.to_owned(),
                        image_url: None,
                        genres: vec!["rock".to_owned()],
                        popularity: Some(80),
                        followers: Some(1_000_000),
                    });

                d.albums.entry(album_id.clone()).or_insert_with(|| Album {
                    id: album_id.clone(),
                    title: album.to_owned(),
                    artists: vec![ArtistRef {
                        id: artist_id.clone(),
                        name: artista.to_owned(),
                    }],
                    album_type: AlbumType::Album,
                    release_date: None,
                    total_tracks: None,
                    cover_url: None,
                    covers: CoverSet::default(),
                    label: None,
                });

                let track = Track {
                    id: TrackId::nuevo_local(),
                    title: titulo.to_owned(),
                    album: Some(AlbumRef {
                        id: album_id,
                        title: album.to_owned(),
                    }),
                    artists: vec![ArtistRef {
                        id: artist_id,
                        name: artista.to_owned(),
                    }],
                    duration: DurationMs::new(duracion),
                    track_number: Some(u16::try_from(i % 12 + 1).unwrap_or(1)),
                    disc_number: Some(1),
                    explicit: false,
                    isrc: None,
                    release_date: None,
                    popularity: Some(u8::try_from(50 + i % 50).unwrap_or(50)),
                    added_at: chrono::Utc::now(),
                };

                // Dos de cada tres aparecen como descargadas, para que la
                // interfaz muestre los dos estados sin tener que inventarlos.
                if i % 3 != 2 {
                    d.availability.insert(
                        track.id.clone(),
                        Availability::Local {
                            rel_path: std::path::PathBuf::from(format!("audio/xx/{i}.opus")),
                            format: localify_core::domain::audio::AudioFormat::Opus,
                            bytes: 4_000_000,
                        },
                    );
                }
                if i % 5 == 0 {
                    d.favorites.push(track.id.clone());
                }

                d.tracks.push(track);
            }
        }
    }

    fn sembrar_playlist(d: &mut Datos) {
        let playlist = Playlist {
            id: PlaylistId::nuevo(),
            name: "Para trabajar".to_owned(),
            description: Some("Ejemplo mientras no hay datos reales".to_owned()),
            cover_path: None,
            source: PlaylistSource::Local,
            source_id: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };
        let entradas: Vec<(PlaylistEntryId, TrackId)> = d
            .tracks
            .iter()
            .take(6)
            .map(|t| (PlaylistEntryId::nuevo(), t.id.clone()))
            .collect();
        d.playlist_items.insert(playlist.id, entradas);
        d.playlists.push(playlist);
    }
}

impl Default for MemoryStore {
    fn default() -> Self {
        Self::vacio()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn el_ejemplo_trae_catalogo_playlist_y_estados_variados() {
        let store = MemoryStore::con_ejemplo();
        store.leer(|d| {
            assert_eq!(d.tracks.len(), 15);
            assert!(d.albums.len() >= 5);
            assert!(d.artists.len() >= 5);
            assert_eq!(d.playlists.len(), 1);

            let locales = d
                .tracks
                .iter()
                .filter(|t| {
                    d.availability
                        .get(&t.id)
                        .is_some_and(Availability::es_local)
                })
                .count();
            assert!(
                locales > 0 && locales < d.tracks.len(),
                "deben verse ambos estados"
            );
            assert!(!d.favorites.is_empty());
        });
    }

    #[test]
    fn clonar_el_almacen_comparte_los_datos() {
        let uno = MemoryStore::vacio();
        let otro = uno.clone();

        otro.escribir(|d| d.query_id = 42);
        assert_eq!(uno.leer(|d| d.query_id), 42);
    }

    #[test]
    fn la_fila_refleja_disponibilidad_y_favorito() {
        let store = MemoryStore::con_ejemplo();
        store.leer(|d| {
            let track = &d.tracks[0];
            let fila = MemoryStore::fila(d, track);
            assert_eq!(fila.title, track.title);
            assert_eq!(fila.artist_display, "Queen");
            assert!(fila.is_favorite, "la primera del ejemplo es favorita");
            assert!(fila.availability.es_local());
        });
    }
}

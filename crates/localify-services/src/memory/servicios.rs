//! Implementaciones provisionales de los servicios sobre [`MemoryStore`].
//!
//! Cumplen el contrato completo de los puertos para que la capa de comandos y
//! el frontend puedan construirse **antes** de que existan los proveedores
//! reales. Cada una se sustituye en su fase correspondiente sin tocar una línea
//! de `localify-app`: es la prueba de que la inversión de dependencias funciona.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use localify_core::domain::album::{AlbumDetail, AlbumFilter, AlbumRow};
use localify_core::domain::artist::{ArtistDetail, ArtistRow};
use localify_core::domain::audio::{AudioDevice, DurationMs, EqProfile, Volume};
use localify_core::domain::availability::Availability;
use localify_core::domain::download::Priority;
use localify_core::domain::ids::{
    AlbumId, ArtistId, PlaylistEntryId, PlaylistId, QueueEntryId, TrackId,
};
use localify_core::domain::library::{LibraryStats, ScanReport};
use localify_core::domain::lyrics::Lyrics;
use localify_core::domain::playlist::{
    Playlist, PlaylistDetail, PlaylistEntry, PlaylistSource, PlaylistSummary,
};
use localify_core::domain::queue::{
    AdvanceReason, ChangeSource, PlayStatus, PlaybackContext, PlayerState, QueueEntry,
    QueueSnapshot, RepeatMode,
};
use localify_core::domain::settings::{Settings, SettingsPatch};
use localify_core::domain::track::{TrackFilter, TrackRow, TrackSort};
use localify_core::error::{CoreError, CoreResult};
use localify_core::events::{DomainEvent, EventPublisher, LibraryScope, ProviderStatus};
use localify_core::page::{Page, PageRequest};
use localify_core::ports::services::{
    DownloadHandle, DownloadService, GrupoDeVersiones, HomeItems, HomeSection, LibraryService,
    LyricsService, MetadataService, NotificationService, PlaybackService, PlaylistService,
    PrimeraCoincidencia, QueueService, RecommendationService, RemoteResults, SearchResults,
    SearchScope, SearchService, SettingsService, ToastLevel,
};
use localify_core::text;
use uuid::Uuid;

use super::store::{Datos, MemoryStore};

/// Dependencias comunes a todos los servicios provisionales.
#[derive(Clone)]
pub struct Contexto {
    pub store: MemoryStore,
    pub bus: Arc<dyn EventPublisher>,
}

impl std::fmt::Debug for Contexto {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Contexto").finish_non_exhaustive()
    }
}

/// Aplica paginación a una colección ya materializada.
fn paginar<T: Clone>(items: &[T], page: &PageRequest) -> Page<T> {
    let inicio = page.offset() as usize;
    let limite = page.limit() as usize;
    let trozo: Vec<T> = items.iter().skip(inicio).take(limite).cloned().collect();
    let consumidos = inicio + trozo.len();
    let next = (consumidos < items.len())
        .then(|| localify_core::page::Cursor::new(consumidos.to_string()));
    Page::new(trozo, Some(items.len() as u64), next)
}

// ─────────────────────────────────────────────────────────────────────────────
// Biblioteca
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct LibraryEnMemoria(pub Contexto);

#[async_trait]
impl LibraryService for LibraryEnMemoria {
    async fn tracks(
        &self,
        filter: &TrackFilter,
        sort: TrackSort,
        page: &PageRequest,
    ) -> CoreResult<Page<TrackRow>> {
        let filas = self.0.store.leer(|d| {
            let mut filas: Vec<TrackRow> = d
                .tracks
                .iter()
                .map(|t| MemoryStore::fila(d, t))
                .filter(|f| !filter.local_only || f.availability.es_local())
                .filter(|f| !filter.favorites_only || f.is_favorite)
                .filter(|f| {
                    filter
                        .album_id
                        .as_ref()
                        .is_none_or(|a| f.album_id.as_ref() == Some(a))
                })
                .filter(|f| {
                    filter.text.as_ref().is_none_or(|t| {
                        let q = text::normalize(t);
                        text::normalize(&f.title).contains(&q)
                            || text::normalize(&f.artist_display).contains(&q)
                    })
                })
                .collect();

            match sort {
                TrackSort::TitleAsc => filas.sort_by(|a, b| a.title.cmp(&b.title)),
                TrackSort::ArtistAsc => {
                    filas.sort_by(|a, b| a.artist_display.cmp(&b.artist_display));
                }
                TrackSort::DurationAsc => filas.sort_by_key(|f| f.duration.as_ms()),
                _ => {}
            }
            filas
        });

        Ok(paginar(&filas, page))
    }

    async fn albums(
        &self,
        _filter: &AlbumFilter,
        page: &PageRequest,
    ) -> CoreResult<Page<AlbumRow>> {
        let filas = self.0.store.leer(|d| {
            let mut filas: Vec<AlbumRow> = d
                .albums
                .values()
                .map(|al| AlbumRow {
                    id: al.id.clone(),
                    title: al.title.clone(),
                    artist_display: al
                        .artists
                        .iter()
                        .map(|a| a.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", "),
                    year: None,
                    cover: None,
                    track_count: u16::try_from(
                        d.tracks
                            .iter()
                            .filter(|t| t.album.as_ref().is_some_and(|x| x.id == al.id))
                            .count(),
                    )
                    .unwrap_or(0),
                    local_count: 0,
                })
                .collect();
            filas.sort_by(|a, b| a.title.cmp(&b.title));
            filas
        });
        Ok(paginar(&filas, page))
    }

    async fn artists(&self, page: &PageRequest) -> CoreResult<Page<ArtistRow>> {
        let filas = self.0.store.leer(|d| {
            let mut filas: Vec<ArtistRow> = d
                .artists
                .values()
                .map(|ar| ArtistRow {
                    id: ar.id.clone(),
                    name: ar.name.clone(),
                    image_url: ar.image_url.clone(),
                    track_count: u32::try_from(
                        d.tracks
                            .iter()
                            .filter(|t| t.artists.iter().any(|a| a.id == ar.id))
                            .count(),
                    )
                    .unwrap_or(0),
                    local_track_count: 0,
                })
                .collect();
            filas.sort_by(|a, b| a.name.cmp(&b.name));
            filas
        });
        Ok(paginar(&filas, page))
    }

    async fn album_detail(&self, id: &AlbumId) -> CoreResult<AlbumDetail> {
        self.0.store.leer(|d| {
            let album = d
                .albums
                .get(id)
                .cloned()
                .ok_or_else(|| CoreError::not_found("album", id.as_str()))?;
            let tracks: Vec<TrackRow> = d
                .tracks
                .iter()
                .filter(|t| t.album.as_ref().is_some_and(|a| &a.id == id))
                .map(|t| MemoryStore::fila(d, t))
                .collect();
            let total: u32 = tracks.iter().map(|t| t.duration.as_ms()).sum();
            let local_count =
                u16::try_from(tracks.iter().filter(|t| t.availability.es_local()).count())
                    .unwrap_or(0);

            Ok(AlbumDetail {
                album,
                tracks,
                total_duration: DurationMs::new(total),
                local_count,
            })
        })
    }

    async fn artist_detail(&self, id: &ArtistId) -> CoreResult<ArtistDetail> {
        self.0.store.leer(|d| {
            let artist = d
                .artists
                .get(id)
                .cloned()
                .ok_or_else(|| CoreError::not_found("artist", id.as_str()))?;
            let top_tracks: Vec<TrackRow> = d
                .tracks
                .iter()
                .filter(|t| t.artists.iter().any(|a| &a.id == id))
                .map(|t| MemoryStore::fila(d, t))
                .collect();

            Ok(ArtistDetail {
                local_track_count: u32::try_from(top_tracks.len()).unwrap_or(0),
                artist,
                top_tracks,
                albums: Vec::new(),
            })
        })
    }

    async fn set_favorite(&self, id: &TrackId, enabled: bool) -> CoreResult<()> {
        self.0.store.escribir(|d| {
            d.favorites.retain(|f| f != id);
            if enabled {
                d.favorites.push(id.clone());
            }
        });
        self.0.bus.publish(DomainEvent::LibraryChanged {
            scope: LibraryScope::Favorites,
        });
        Ok(())
    }

    async fn favorites(&self, page: &PageRequest) -> CoreResult<Page<TrackRow>> {
        let filas = self.0.store.leer(|d| {
            d.tracks
                .iter()
                .filter(|t| d.favorites.contains(&t.id))
                .map(|t| MemoryStore::fila(d, t))
                .collect::<Vec<_>>()
        });
        Ok(paginar(&filas, page))
    }

    async fn record_play(&self, id: &TrackId, ms_played: u32, completed: bool) -> CoreResult<()> {
        self.0.bus.publish(DomainEvent::TrackFinished {
            track_id: id.clone(),
            completed,
            ms_played,
        });
        Ok(())
    }

    async fn recent(&self, limit: u16) -> CoreResult<Vec<TrackRow>> {
        Ok(self.0.store.leer(|d| {
            d.tracks
                .iter()
                .take(limit as usize)
                .map(|t| MemoryStore::fila(d, t))
                .collect()
        }))
    }

    async fn stats(&self) -> CoreResult<LibraryStats> {
        Ok(self.0.store.leer(|d| LibraryStats {
            track_count: d.tracks.len() as u64,
            local_count: d.availability.values().filter(|a| a.es_local()).count() as u64,
            album_count: d.albums.len() as u64,
            artist_count: d.artists.len() as u64,
            total_duration_ms: d.tracks.iter().map(|t| u64::from(t.duration.as_ms())).sum(),
            total_bytes: 0,
        }))
    }

    async fn delete_download(&self, id: &TrackId) -> CoreResult<()> {
        self.0.store.escribir(|d| {
            d.availability.remove(id);
        });
        self.0.bus.publish(DomainEvent::AvailabilityChanged {
            track_id: id.clone(),
            availability: localify_core::domain::availability::Availability::Absent,
        });
        Ok(())
    }

    async fn wipe_downloads(&self) -> CoreResult<u32> {
        let cuantas = self.0.store.escribir(|d| {
            let n = u32::try_from(d.availability.len()).unwrap_or(u32::MAX);
            d.availability.clear();
            n
        });
        self.0.bus.publish(DomainEvent::LibraryChanged {
            scope: localify_core::events::LibraryScope::Tracks,
        });
        Ok(cuantas)
    }

    async fn rescan(&self) -> CoreResult<Uuid> {
        Ok(Uuid::now_v7())
    }

    async fn last_scan_report(&self) -> CoreResult<Option<ScanReport>> {
        Ok(None)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Búsqueda
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct SearchEnMemoria(pub Contexto);

#[async_trait]
impl SearchService for SearchEnMemoria {
    async fn search(
        &self,
        query: &str,
        _scope: SearchScope,
        _page: &PageRequest,
    ) -> CoreResult<SearchResults> {
        let q = text::normalize(query);

        Ok(self.0.store.escribir(|d| {
            d.query_id += 1;
            let query_id = d.query_id;

            if q.is_empty() {
                return SearchResults {
                    query_id,
                    top: None,
                    tracks: Vec::new(),
                    albums: Vec::new(),
                    artists: Vec::new(),
                    playlists: Vec::new(),
                    remote: RemoteResults::NotAttempted,
                };
            }

            let tracks: Vec<TrackRow> = d
                .tracks
                .iter()
                .filter(|t| {
                    text::normalize(&t.title).contains(&q)
                        || text::normalize(&t.artist_display()).contains(&q)
                })
                .map(|t| MemoryStore::fila(d, t))
                .collect();

            let artists: Vec<ArtistRow> = d
                .artists
                .values()
                .filter(|a| text::normalize(&a.name).contains(&q))
                .map(|a| ArtistRow {
                    id: a.id.clone(),
                    name: a.name.clone(),
                    image_url: a.image_url.clone(),
                    track_count: 0,
                    local_track_count: 0,
                })
                .collect();

            SearchResults {
                query_id,
                top: tracks.first().cloned().map(PrimeraCoincidencia::Track),
                // Sin agrupar: el almacén de ejemplo no tiene versiones de una
                // misma canción, y agruparlas aquí solo probaría el doble.
                tracks: tracks
                    .into_iter()
                    .map(|principal| GrupoDeVersiones {
                        principal,
                        versiones: Vec::new(),
                    })
                    .collect(),
                albums: Vec::new(),
                artists,
                playlists: Vec::new(),
                // Sin proveedor configurado no se pregunta a nadie. Es el mismo
                // estado que verá quien no ponga credenciales.
                remote: RemoteResults::NotAttempted,
            }
        }))
    }

    async fn suggest(&self, prefix: &str, limit: u8) -> CoreResult<Vec<String>> {
        let p = text::normalize(prefix);
        if p.is_empty() {
            return Ok(Vec::new());
        }
        Ok(self.0.store.leer(|d| {
            d.tracks
                .iter()
                .filter(|t| text::normalize(&t.title).starts_with(&p))
                .map(|t| t.title.clone())
                .take(limit as usize)
                .collect()
        }))
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Reproducción y cola
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct PlaybackEnMemoria(pub Contexto);

impl PlaybackEnMemoria {
    fn estado(&self) -> PlayerState {
        self.0.store.leer(|d| d.player.clone())
    }

    fn cambiar_estado(&self, status: PlayStatus) -> PlayerState {
        let estado = self.0.store.escribir(|d| {
            d.player.status = status;
            d.player.clone()
        });
        self.0
            .bus
            .publish(DomainEvent::PlayStatusChanged { status });
        estado
    }
}

#[async_trait]
impl PlaybackService for PlaybackEnMemoria {
    async fn play_track(&self, id: &TrackId, ctx: PlaybackContext) -> CoreResult<PlayerState> {
        let estado = self.0.store.escribir(|d| {
            let track = MemoryStore::buscar_pista(d, id)
                .ok_or_else(|| CoreError::not_found("track", id.as_str()))?;
            let fila = MemoryStore::fila(d, &track);

            d.player.duration = track.duration;
            d.player.position = DurationMs::ZERO;
            d.player.buffered = track.duration;
            d.player.status = PlayStatus::Playing;
            d.player.context = Some(ctx.clone());
            d.player.track = Some(fila);
            d.queue.context = Some(ctx);
            Ok::<_, CoreError>(d.player.clone())
        })?;

        self.0.bus.publish(DomainEvent::TrackChanged {
            track_id: id.clone(),
            source: ChangeSource::User,
        });
        self.0.bus.publish(DomainEvent::PlayStatusChanged {
            status: PlayStatus::Playing,
        });
        Ok(estado)
    }

    async fn toggle(&self) -> CoreResult<PlayerState> {
        let actual = self.estado().status;
        Ok(self.cambiar_estado(if actual == PlayStatus::Playing {
            PlayStatus::Paused
        } else {
            PlayStatus::Playing
        }))
    }

    async fn pause(&self) -> CoreResult<PlayerState> {
        Ok(self.cambiar_estado(PlayStatus::Paused))
    }

    async fn resume(&self) -> CoreResult<PlayerState> {
        Ok(self.cambiar_estado(PlayStatus::Playing))
    }

    async fn next(&self) -> CoreResult<PlayerState> {
        Ok(self.estado())
    }

    async fn previous(&self) -> CoreResult<PlayerState> {
        // Regla de Spotify: por debajo de tres segundos va a la anterior; por
        // encima, reinicia la actual. Aquí solo se implementa el reinicio.
        Ok(self.0.store.escribir(|d| {
            d.player.position = DurationMs::ZERO;
            d.player.clone()
        }))
    }

    async fn seek(&self, position: DurationMs) -> CoreResult<PlayerState> {
        Ok(self.0.store.escribir(|d| {
            d.player.position = DurationMs::new(position.as_ms().min(d.player.duration.as_ms()));
            d.player.clone()
        }))
    }

    async fn set_volume(&self, volume: Volume) -> CoreResult<PlayerState> {
        let estado = self.0.store.escribir(|d| {
            d.player.volume = volume;
            d.player.clone()
        });
        self.0.bus.publish(DomainEvent::VolumeChanged {
            volume: volume.as_f32(),
        });
        Ok(estado)
    }

    async fn set_repeat(&self, mode: RepeatMode) -> CoreResult<PlayerState> {
        let estado = self.0.store.escribir(|d| {
            d.player.repeat = mode;
            d.player.clone()
        });
        self.0.bus.publish(DomainEvent::RepeatModeChanged { mode });
        Ok(estado)
    }

    async fn set_shuffle(&self, enabled: bool) -> CoreResult<PlayerState> {
        let estado = self.0.store.escribir(|d| {
            d.player.shuffle = enabled;
            d.player.clone()
        });
        self.0.bus.publish(DomainEvent::ShuffleChanged { enabled });
        Ok(estado)
    }

    async fn jump_to(&self, _entry: QueueEntryId) -> CoreResult<PlayerState> {
        Ok(self.estado())
    }

    async fn state(&self) -> PlayerState {
        self.estado()
    }

    fn position(&self) -> (DurationMs, DurationMs) {
        self.0
            .store
            .leer(|d| (d.player.position, d.player.buffered))
    }

    async fn persist_now(&self) -> CoreResult<()> {
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct QueueEnMemoria(pub Contexto);

impl QueueEnMemoria {
    fn notificar(&self) -> QueueSnapshot {
        let snapshot = self.0.store.escribir(|d| {
            d.queue.revision += 1;
            d.queue.clone()
        });
        self.0.bus.publish(DomainEvent::QueueChanged {
            revision: snapshot.revision,
        });
        snapshot
    }

    fn entradas(&self, tracks: &[TrackId]) -> Vec<QueueEntry> {
        self.0.store.leer(|d| {
            tracks
                .iter()
                .filter_map(|id| MemoryStore::buscar_pista(d, id))
                .map(|t| QueueEntry {
                    entry_id: QueueEntryId::nuevo(),
                    track: MemoryStore::fila(d, &t),
                })
                .collect()
        })
    }
}

#[async_trait]
impl QueueService for QueueEnMemoria {
    async fn snapshot(&self) -> QueueSnapshot {
        self.0.store.leer(|d| d.queue.clone())
    }

    async fn set_context(&self, ctx: PlaybackContext, _start_index: usize) -> CoreResult<()> {
        self.0.store.escribir(|d| d.queue.context = Some(ctx));
        self.notificar();
        Ok(())
    }

    async fn add_next(&self, tracks: &[TrackId]) -> CoreResult<()> {
        let nuevas = self.entradas(tracks);
        self.0.store.escribir(|d| {
            for (i, e) in nuevas.into_iter().enumerate() {
                d.queue.user_queue.insert(i, e);
            }
        });
        self.notificar();
        Ok(())
    }

    async fn add_last(&self, tracks: &[TrackId]) -> CoreResult<()> {
        let nuevas = self.entradas(tracks);
        self.0.store.escribir(|d| d.queue.user_queue.extend(nuevas));
        self.notificar();
        Ok(())
    }

    async fn remove(&self, entry: QueueEntryId) -> CoreResult<()> {
        self.0
            .store
            .escribir(|d| d.queue.user_queue.retain(|e| e.entry_id != entry));
        self.notificar();
        Ok(())
    }

    async fn move_entry(&self, entry: QueueEntryId, to_index: usize) -> CoreResult<()> {
        self.0.store.escribir(|d| {
            if let Some(pos) = d.queue.user_queue.iter().position(|e| e.entry_id == entry) {
                let e = d.queue.user_queue.remove(pos);
                let destino = to_index.min(d.queue.user_queue.len());
                d.queue.user_queue.insert(destino, e);
            }
        });
        self.notificar();
        Ok(())
    }

    async fn clear_user_queue(&self) -> CoreResult<()> {
        self.0.store.escribir(|d| d.queue.user_queue.clear());
        self.notificar();
        Ok(())
    }

    async fn advance(&self, _reason: AdvanceReason) -> CoreResult<Option<TrackId>> {
        let siguiente = self.0.store.escribir(|d| {
            if d.queue.user_queue.is_empty() {
                None
            } else {
                Some(d.queue.user_queue.remove(0).track.id)
            }
        });
        self.notificar();
        Ok(siguiente)
    }

    async fn go_back(&self) -> CoreResult<Option<TrackId>> {
        Ok(None)
    }

    async fn peek_next(&self) -> CoreResult<Option<TrackId>> {
        Ok(self
            .0
            .store
            .leer(|d| d.queue.user_queue.first().map(|e| e.track.id.clone())))
    }

    async fn set_shuffle(&self, enabled: bool) -> CoreResult<()> {
        self.0.store.escribir(|d| d.player.shuffle = enabled);
        Ok(())
    }

    async fn set_repeat(&self, mode: RepeatMode) -> CoreResult<()> {
        self.0.store.escribir(|d| d.player.repeat = mode);
        Ok(())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Descargas
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct DownloadEnMemoria(pub Contexto);

#[async_trait]
impl DownloadService for DownloadEnMemoria {
    async fn ensure(&self, track: &TrackId, _priority: Priority) -> CoreResult<DownloadHandle> {
        Ok(DownloadHandle {
            playable_path: PathBuf::from("memoria"),
            complete: self.0.store.leer(|d| {
                d.availability
                    .get(track)
                    .is_some_and(Availability::es_local)
            }),
        })
    }

    async fn status(&self, track: &TrackId) -> CoreResult<Availability> {
        Ok(self.0.store.leer(|d| {
            d.availability
                .get(track)
                .cloned()
                .unwrap_or(Availability::Absent)
        }))
    }

    async fn statuses(&self, tracks: &[TrackId]) -> CoreResult<Vec<(TrackId, Availability)>> {
        Ok(self.0.store.leer(|d| {
            tracks
                .iter()
                .map(|id| {
                    (
                        id.clone(),
                        d.availability
                            .get(id)
                            .cloned()
                            .unwrap_or(Availability::Absent),
                    )
                })
                .collect()
        }))
    }

    async fn retry_failed(&self) -> CoreResult<u32> {
        Ok(0)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Playlists
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct PlaylistEnMemoria(pub Contexto);

impl PlaylistEnMemoria {
    fn resumen(datos: &Datos, p: &Playlist) -> PlaylistSummary {
        PlaylistSummary {
            id: p.id,
            name: p.name.clone(),
            track_count: u32::try_from(datos.playlist_items.get(&p.id).map_or(0, Vec::len))
                .unwrap_or(0),
            cover_albums: Vec::new(),

            has_own_cover: false,
            updated_at: p.updated_at,
            source: p.source,
        }
    }
}

#[async_trait]
impl PlaylistService for PlaylistEnMemoria {
    async fn list(&self) -> CoreResult<Vec<PlaylistSummary>> {
        Ok(self
            .0
            .store
            .leer(|d| d.playlists.iter().map(|p| Self::resumen(d, p)).collect()))
    }

    async fn create(&self, name: &str) -> CoreResult<PlaylistSummary> {
        if name.trim().is_empty() {
            return Err(CoreError::invalid(
                "el nombre de la playlist no puede estar vacío",
            ));
        }

        let playlist = Playlist {
            id: PlaylistId::nuevo(),
            name: name.to_owned(),
            description: None,
            cover_path: None,
            source: PlaylistSource::Local,
            source_id: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };

        let resumen = self.0.store.escribir(|d| {
            d.playlist_items.insert(playlist.id, Vec::new());
            d.playlists.push(playlist.clone());
            Self::resumen(d, &playlist)
        });

        self.0.bus.publish(DomainEvent::PlaylistChanged {
            playlist_id: resumen.id,
            kind: localify_core::events::PlaylistChangeKind::Created,
        });
        Ok(resumen)
    }

    async fn set_description(&self, id: &PlaylistId, description: Option<&str>) -> CoreResult<()> {
        let texto = description
            .map(str::trim)
            .filter(|d| !d.is_empty())
            .map(ToOwned::to_owned);

        let encontrada = self.0.store.escribir(|d| {
            d.playlists
                .iter_mut()
                .find(|p| &p.id == id)
                .is_some_and(|p| {
                    p.description.clone_from(&texto);
                    p.updated_at = chrono::Utc::now();
                    true
                })
        });
        if !encontrada {
            return Err(CoreError::not_found("playlist", id.to_string()));
        }
        self.0.bus.publish(DomainEvent::PlaylistChanged {
            playlist_id: *id,
            kind: localify_core::events::PlaylistChangeKind::Renamed,
        });
        Ok(())
    }

    async fn rename(&self, id: &PlaylistId, name: &str) -> CoreResult<()> {
        let encontrada = self.0.store.escribir(|d| {
            d.playlists
                .iter_mut()
                .find(|p| &p.id == id)
                .is_some_and(|p| {
                    name.clone_into(&mut p.name);
                    p.updated_at = chrono::Utc::now();
                    true
                })
        });
        if !encontrada {
            return Err(CoreError::not_found("playlist", id.to_string()));
        }
        self.0.bus.publish(DomainEvent::PlaylistChanged {
            playlist_id: *id,
            kind: localify_core::events::PlaylistChangeKind::Renamed,
        });
        Ok(())
    }

    async fn delete(&self, id: &PlaylistId) -> CoreResult<()> {
        self.0.store.escribir(|d| {
            d.playlists.retain(|p| &p.id != id);
            d.playlist_items.remove(id);
        });
        self.0.bus.publish(DomainEvent::PlaylistChanged {
            playlist_id: *id,
            kind: localify_core::events::PlaylistChangeKind::Deleted,
        });
        Ok(())
    }

    async fn detail(&self, id: &PlaylistId, page: &PageRequest) -> CoreResult<PlaylistDetail> {
        self.0.store.leer(|d| {
            let playlist = d
                .playlists
                .iter()
                .find(|p| &p.id == id)
                .ok_or_else(|| CoreError::not_found("playlist", id.to_string()))?;

            let entradas: Vec<PlaylistEntry> = d
                .playlist_items
                .get(id)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|(entry_id, track_id)| {
                            MemoryStore::buscar_pista(d, track_id).map(|t| PlaylistEntry {
                                entry_id: *entry_id,
                                track: MemoryStore::fila(d, &t),
                                added_at: playlist.created_at,
                            })
                        })
                        .skip(page.offset() as usize)
                        .take(page.limit() as usize)
                        .collect()
                })
                .unwrap_or_default();

            let total: u32 = entradas.iter().map(|e| e.track.duration.as_ms()).sum();

            Ok(PlaylistDetail {
                summary: Self::resumen(d, playlist),
                description: playlist.description.clone(),
                entries: entradas,
                total_duration: DurationMs::new(total),
            })
        })
    }

    async fn add_tracks(
        &self,
        id: &PlaylistId,
        tracks: &[TrackId],
        at_index: Option<usize>,
    ) -> CoreResult<()> {
        self.0.store.escribir(|d| {
            let items = d.playlist_items.entry(*id).or_default();
            let nuevas: Vec<_> = tracks
                .iter()
                .map(|t| (PlaylistEntryId::nuevo(), t.clone()))
                .collect();
            let destino = at_index.unwrap_or(items.len()).min(items.len());
            for (i, e) in nuevas.into_iter().enumerate() {
                items.insert(destino + i, e);
            }
        });
        self.0.bus.publish(DomainEvent::PlaylistChanged {
            playlist_id: *id,
            kind: localify_core::events::PlaylistChangeKind::Items,
        });
        Ok(())
    }

    async fn remove_entries(&self, id: &PlaylistId, entries: &[PlaylistEntryId]) -> CoreResult<()> {
        self.0.store.escribir(|d| {
            if let Some(items) = d.playlist_items.get_mut(id) {
                items.retain(|(e, _)| !entries.contains(e));
            }
        });
        self.0.bus.publish(DomainEvent::PlaylistChanged {
            playlist_id: *id,
            kind: localify_core::events::PlaylistChangeKind::Items,
        });
        Ok(())
    }

    async fn reorder(
        &self,
        id: &PlaylistId,
        entry: PlaylistEntryId,
        to_index: usize,
    ) -> CoreResult<()> {
        self.0.store.escribir(|d| {
            if let Some(items) = d.playlist_items.get_mut(id)
                && let Some(pos) = items.iter().position(|(e, _)| *e == entry)
            {
                let item = items.remove(pos);
                let destino = to_index.min(items.len());
                items.insert(destino, item);
            }
        });
        self.0.bus.publish(DomainEvent::PlaylistChanged {
            playlist_id: *id,
            kind: localify_core::events::PlaylistChangeKind::Items,
        });
        Ok(())
    }

    async fn set_cover(&self, _id: &PlaylistId, _image: &Path) -> CoreResult<()> {
        Ok(())
    }

    async fn clear_cover(&self, _id: &PlaylistId) -> CoreResult<()> {
        Ok(())
    }

    async fn cover_file(&self, _id: &PlaylistId) -> CoreResult<Option<PathBuf>> {
        // Sin disco no hay portadas propias que servir.
        Ok(None)
    }

    async fn import_from_provider(&self, _url_or_id: &str) -> CoreResult<Uuid> {
        Err(CoreError::NotConfigured("spotify.client_id"))
    }

    async fn suggestions(&self, _id: &PlaylistId, limit: u8) -> CoreResult<Vec<TrackRow>> {
        Ok(self.0.store.leer(|d| {
            d.tracks
                .iter()
                .rev()
                .take(limit as usize)
                .map(|t| MemoryStore::fila(d, t))
                .collect()
        }))
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Recomendaciones, letras, ajustes y avisos
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct RecommendationEnMemoria(pub Contexto);

#[async_trait]
impl RecommendationService for RecommendationEnMemoria {
    async fn home(&self) -> CoreResult<Vec<HomeSection>> {
        Ok(self.0.store.leer(|d| {
            let recientes: Vec<TrackRow> = d
                .tracks
                .iter()
                .take(6)
                .map(|t| MemoryStore::fila(d, t))
                .collect();
            let favoritas: Vec<TrackRow> = d
                .tracks
                .iter()
                .filter(|t| d.favorites.contains(&t.id))
                .map(|t| MemoryStore::fila(d, t))
                .collect();

            let mut secciones = vec![HomeSection {
                key: "home.recently_played".to_owned(),
                params: Vec::new(),
                items: HomeItems::Tracks(recientes),
            }];
            if !favoritas.is_empty() {
                secciones.push(HomeSection {
                    key: "home.rediscover".to_owned(),
                    params: Vec::new(),
                    items: HomeItems::Tracks(favoritas),
                });
            }
            secciones
        }))
    }

    async fn similar_to_track(&self, id: &TrackId, limit: u8) -> CoreResult<Vec<TrackRow>> {
        Ok(self.0.store.leer(|d| {
            let semilla = MemoryStore::buscar_pista(d, id);
            let artista = semilla.and_then(|t| t.artists.first().map(|a| a.id.clone()));
            d.tracks
                .iter()
                .filter(|t| &t.id != id)
                .filter(|t| {
                    artista
                        .as_ref()
                        .is_none_or(|a| t.artists.iter().any(|x| &x.id == a))
                })
                .take(limit as usize)
                .map(|t| MemoryStore::fila(d, t))
                .collect()
        }))
    }

    async fn for_playlist(&self, _id: &PlaylistId, limit: u8) -> CoreResult<Vec<TrackRow>> {
        Ok(self.0.store.leer(|d| {
            d.tracks
                .iter()
                .take(limit as usize)
                .map(|t| MemoryStore::fila(d, t))
                .collect()
        }))
    }
}

#[derive(Debug, Clone)]
pub struct LyricsEnMemoria;

#[async_trait]
impl LyricsService for LyricsEnMemoria {
    async fn get(&self, _track: &TrackId) -> CoreResult<Option<Lyrics>> {
        // Sin letra no hay error: la interfaz simplemente no muestra el panel.
        Ok(None)
    }
}

#[derive(Debug, Clone)]
pub struct SettingsEnMemoria(pub Contexto);

#[async_trait]
impl SettingsService for SettingsEnMemoria {
    async fn get(&self) -> Settings {
        self.0.store.leer(|d| {
            d.settings
                .clone()
                .unwrap_or_else(|| Settings::por_defecto_en(PathBuf::from("")))
        })
    }

    async fn patch(&self, patch: SettingsPatch) -> CoreResult<Settings> {
        // Se valida **todo** antes de escribir **nada**: un patch inválido no
        // debe dejar la configuración a medias.
        patch.validar()?;
        let secciones = patch.secciones();

        let settings = self.0.store.escribir(|d| {
            let mut s = d
                .settings
                .clone()
                .unwrap_or_else(|| Settings::por_defecto_en(PathBuf::from("")));
            if let Some(l) = patch.language {
                s.language = l;
            }
            if let Some(a) = patch.audio {
                s.audio = a;
            }
            if let Some(dl) = patch.download {
                s.download = dl;
            }
            if let Some(i) = patch.integrations {
                s.integrations = i;
            }
            if let Some(u) = patch.ui {
                s.ui = u;
            }
            d.settings = Some(s.clone());
            s
        });

        if !secciones.is_empty() {
            self.0.bus.publish(DomainEvent::SettingsChanged {
                sections: secciones,
            });
        }
        Ok(settings)
    }

    async fn set_spotify_credentials(
        &self,
        _client_id: &str,
        _client_secret: &str,
    ) -> CoreResult<ProviderStatus> {
        Ok(ProviderStatus::NotConfigured)
    }

    async fn test_spotify(&self) -> CoreResult<ProviderStatus> {
        Ok(ProviderStatus::NotConfigured)
    }

    async fn set_lastfm_session(&self, user: Option<String>) -> CoreResult<Settings> {
        // Sin almacén de secretos, "conectado" es exactamente "hay usuario".
        let settings = self.0.store.escribir(|d| {
            let mut s = d
                .settings
                .clone()
                .unwrap_or_else(|| Settings::por_defecto_en(PathBuf::from("")));
            s.integrations.lastfm_connected = user.is_some();
            s.integrations.lastfm_user = user;
            d.settings = Some(s.clone());
            s
        });
        Ok(settings)
    }

    async fn change_library_path(&self, _path: &Path, _move_existing: bool) -> CoreResult<Uuid> {
        Ok(Uuid::now_v7())
    }

    async fn audio_devices(&self) -> CoreResult<Vec<AudioDevice>> {
        Ok(vec![AudioDevice {
            id: "default".to_owned(),
            name: "Predeterminado del sistema".to_owned(),
            is_default: true,
        }])
    }

    async fn eq_profiles(&self) -> CoreResult<Vec<EqProfile>> {
        Ok(EqProfile::predefinidos())
    }

    async fn preview_eq(&self, _profile: &EqProfile) -> CoreResult<()> {
        // Sin motor no hay nada que aplicar, pero tampoco nada que falle.
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct MetadataEnMemoria;

#[async_trait]
impl MetadataService for MetadataEnMemoria {
    async fn ensure_track(&self, _id: &TrackId) -> CoreResult<()> {
        Ok(())
    }
    async fn ensure_album(&self, _id: &AlbumId) -> CoreResult<()> {
        Ok(())
    }
    async fn ensure_artist(&self, _id: &ArtistId) -> CoreResult<()> {
        Ok(())
    }
    async fn ensure_cover(&self, _album: &AlbumId) -> CoreResult<Option<PathBuf>> {
        Ok(None)
    }
    async fn ensure_artist_image(&self, _artist: &ArtistId) -> CoreResult<Option<PathBuf>> {
        Ok(None)
    }
    async fn ensure_track_thumbnail(&self, _track: &TrackId) -> CoreResult<Option<PathBuf>> {
        Ok(None)
    }
    async fn refresh_stale(&self, _limit: u32) -> CoreResult<u32> {
        Ok(0)
    }
}

#[derive(Debug, Clone)]
pub struct NotificationEnMemoria(pub Contexto);

#[async_trait]
impl NotificationService for NotificationEnMemoria {
    async fn now_playing(&self, _track: &TrackId) -> CoreResult<()> {
        Ok(())
    }

    async fn playback_status(&self, _playing: bool) -> CoreResult<()> {
        Ok(())
    }

    async fn toast(&self, level: ToastLevel, key: &str, params: &[(String, String)]) {
        self.0.bus.publish(DomainEvent::Toast {
            level: match level {
                ToastLevel::Info => localify_core::events::ToastLevel::Info,
                ToastLevel::Warn => localify_core::events::ToastLevel::Warn,
                ToastLevel::Error => localify_core::events::ToastLevel::Error,
            },
            message_key: key.to_owned(),
            params: params.to_vec(),
        });
    }
}

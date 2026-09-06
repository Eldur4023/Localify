//! Reasignación manual de metadatos: buscar candidatos, elegir uno, resetear.
//!
//! Base de datos temporal real; el proveedor es un doble mínimo que solo sabe
//! devolver los candidatos que el test programe. La búsqueda real ya se prueba
//! en `busqueda.rs`; lo que importa aquí es qué hace `MetadataServiceImpl` con
//! lo que el proveedor devuelve.

#![allow(clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use async_trait::async_trait;
use localify_core::domain::album::Album;
use localify_core::domain::artist::Artist;
use localify_core::domain::audio::{AudioFormat, DurationMs};
use localify_core::domain::ids::{AlbumId, ArtistId, TrackId};
use localify_core::domain::library::{AudioFileRecord, AudioSource};
use localify_core::domain::track::{AlbumRef, ArtistRef, Track};
use localify_core::error::{CoreError, CoreResult};
use localify_core::events::{DomainEvent, EventPublisher, ProviderStatus};
use localify_core::page::Page;
use localify_core::ports::database::{AudioFileRepository, TrackRepository, YoutubeMatchRepository};
use localify_core::ports::metadata_provider::{MetadataProvider, PlaylistImport};
use localify_core::ports::platform::AppPaths;
use localify_core::ports::services::MetadataService;
use localify_db::Pool;
use localify_db::pool::TempDbGuard;
use localify_platform::LocalifyPaths;
use localify_services::metadata::MetadataServiceImpl;

#[derive(Debug, Default)]
struct BusMudo;
impl EventPublisher for BusMudo {
    fn publish(&self, _event: DomainEvent) {}
}

/// Proveedor mínimo: solo `search_tracks` hace algo, y lo que se le programe.
#[derive(Debug, Default)]
struct ProveedorDePrueba {
    candidatos: std::sync::Mutex<Vec<Track>>,
}

impl ProveedorDePrueba {
    fn con_candidatos(candidatos: Vec<Track>) -> Self {
        Self {
            candidatos: std::sync::Mutex::new(candidatos),
        }
    }
}

#[async_trait]
impl MetadataProvider for ProveedorDePrueba {
    fn name(&self) -> &'static str {
        "prueba"
    }
    async fn status(&self) -> ProviderStatus {
        ProviderStatus::Ready
    }
    async fn search_tracks(&self, _query: &str, _limit: u8, _offset: u16) -> CoreResult<Page<Track>> {
        let items = self.candidatos.lock().expect("lock").clone();
        let total = items.len() as u64;
        Ok(Page::new(items, Some(total), None))
    }
    async fn track(&self, _id: &TrackId) -> CoreResult<Track> {
        Err(CoreError::not_found("track", "no_usado"))
    }
    async fn tracks(&self, _ids: &[TrackId]) -> CoreResult<Vec<Track>> {
        Ok(Vec::new())
    }
    async fn album(&self, _id: &AlbumId) -> CoreResult<Album> {
        Err(CoreError::not_found("album", "no_usado"))
    }
    async fn album_tracks(&self, _id: &AlbumId) -> CoreResult<Vec<Track>> {
        Ok(Vec::new())
    }
    async fn artist(&self, _id: &ArtistId) -> CoreResult<Artist> {
        Err(CoreError::not_found("artist", "no_usado"))
    }
    async fn artist_top_tracks(&self, _id: &ArtistId) -> CoreResult<Vec<Track>> {
        Ok(Vec::new())
    }
    async fn artist_albums(&self, _id: &ArtistId) -> CoreResult<Vec<Album>> {
        Ok(Vec::new())
    }
    async fn public_playlist(
        &self,
        _url_or_id: &str,
        _page_callback: &(dyn Fn(u32, u32) + Send + Sync),
    ) -> CoreResult<PlaylistImport> {
        Err(CoreError::not_found("playlist", "no_usado"))
    }
}

struct Ctx {
    servicio: MetadataServiceImpl,
    tracks: Arc<dyn TrackRepository>,
    audio: Arc<dyn AudioFileRepository>,
    matches: Arc<dyn YoutubeMatchRepository>,
    paths: Arc<LocalifyPaths>,
    biblioteca: std::path::PathBuf,
    _guard: TempDbGuard,
}

impl Drop for Ctx {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.biblioteca);
    }
}

async fn ctx(candidatos: Vec<Track>) -> Ctx {
    let (pool, guard) = Pool::temporal().expect("abre");
    localify_db::ejecutar_migraciones(&pool)
        .await
        .expect("migra");

    let biblioteca = std::env::temp_dir().join(format!("localify-meta-{}", uuid::Uuid::now_v7()));
    let paths = Arc::new(LocalifyPaths::con_biblioteca(
        biblioteca.join("config"),
        biblioteca.clone(),
    ));
    paths.crear_estructura().expect("crea carpetas");

    let tracks: Arc<dyn TrackRepository> =
        Arc::new(localify_db::repositories::SqliteTrackRepository::new(
            pool.clone(),
        ));
    let audio: Arc<dyn AudioFileRepository> = Arc::new(
        localify_db::repositories::SqliteAudioFileRepository::new(pool.clone()),
    );
    let matches: Arc<dyn YoutubeMatchRepository> = Arc::new(
        localify_db::repositories::SqliteYoutubeMatchRepository::new(pool.clone()),
    );
    let albums = Arc::new(localify_db::repositories::SqliteAlbumRepository::new(
        pool.clone(),
    ));
    let artists = Arc::new(localify_db::repositories::SqliteArtistRepository::new(pool));

    let provider: Arc<dyn MetadataProvider> = Arc::new(ProveedorDePrueba::con_candidatos(candidatos));

    let servicio = MetadataServiceImpl::nuevo(
        provider,
        Arc::clone(&tracks),
        albums,
        artists,
        Arc::new(BusMudo) as Arc<dyn EventPublisher>,
        None,
        Arc::clone(&paths) as Arc<dyn localify_core::ports::platform::AppPaths>,
    )
    .con_emparejamientos(Arc::clone(&matches))
    .con_audio(Arc::clone(&audio));

    Ctx {
        servicio,
        tracks,
        audio,
        matches,
        paths,
        biblioteca,
        _guard: guard,
    }
}

fn artista(nombre: &str) -> ArtistRef {
    ArtistRef {
        id: ArtistId::nuevo_local(),
        name: nombre.into(),
    }
}

async fn catalogar(c: &Ctx, id: TrackId, titulo: &str) -> Track {
    let t = Track {
        id,
        title: titulo.into(),
        album: None,
        artists: vec![artista("Original")],
        duration: DurationMs::from_secs(180),
        track_number: None,
        disc_number: None,
        explicit: false,
        isrc: None,
        release_date: None,
        popularity: None,
        added_at: chrono::Utc::now(),
    };
    c.tracks
        .upsert(std::slice::from_ref(&t))
        .await
        .expect("guarda");
    t
}

fn id(n: usize) -> TrackId {
    let c: Vec<char> = "0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ"
        .chars()
        .collect();
    let mut v = n;
    let s: String = (0..22)
        .map(|_| {
            let d = c[v % c.len()];
            v /= c.len();
            d
        })
        .collect();
    TrackId::from_trusted(s)
}

#[tokio::test]
async fn buscar_candidatos_no_persiste_nada() {
    let candidato = Track {
        id: id(1),
        title: "Candidata".into(),
        album: None,
        artists: vec![artista("Alguien")],
        duration: DurationMs::from_secs(200),
        track_number: None,
        disc_number: None,
        explicit: false,
        isrc: None,
        release_date: None,
        popularity: None,
        added_at: chrono::Utc::now(),
    };
    let c = ctx(vec![candidato.clone()]).await;

    let encontrados = c
        .servicio
        .search_candidates("lo que sea", 10)
        .await
        .expect("busca");
    assert_eq!(encontrados.len(), 1);
    assert_eq!(encontrados[0].title, "Candidata");

    assert!(
        c.tracks.get(&candidato.id).await.expect("consulta").is_none(),
        "buscar candidatos no debe escribir nada en el catálogo"
    );
}

#[tokio::test]
async fn reasignar_metadatos_conserva_id_y_fecha_pero_sobreescribe_el_resto() {
    let c = ctx(Vec::new()).await;
    catalogar(&c, id(2), "Título viejo").await;
    // Se relee de la base de datos: `added_at` se persiste truncado al segundo
    // (`de_fecha`/`a_fecha`), así que comparar contra el valor en memoria antes
    // de guardar compararía nanosegundos que nunca sobrevivieron al viaje.
    let original = c
        .tracks
        .get(&id(2))
        .await
        .expect("consulta")
        .expect("existe");

    c.matches
        .reject(&original.id, "video_viejo")
        .await
        .expect("rechaza");

    let candidato = Track {
        id: id(99), // el id del candidato NO debe usarse
        title: "Título correcto".into(),
        album: Some(AlbumRef {
            id: AlbumId::nuevo_local(),
            title: "Álbum correcto".into(),
        }),
        artists: vec![artista("Artista correcto")],
        duration: DurationMs::from_secs(210),
        track_number: Some(3),
        disc_number: Some(1),
        explicit: false,
        isrc: Some("ISRC123".into()),
        release_date: None,
        popularity: Some(50),
        added_at: chrono::Utc::now(), // distinta a la de `original` a propósito
    };

    c.servicio
        .assign_metadata(&original.id, &candidato)
        .await
        .expect("reasigna");

    let actualizada = c
        .tracks
        .get(&original.id)
        .await
        .expect("consulta")
        .expect("existe");
    assert_eq!(actualizada.id, original.id, "el id no cambia");
    assert_eq!(actualizada.title, "Título correcto");
    assert_eq!(actualizada.artist_display(), "Artista correcto");
    assert_eq!(
        actualizada.album.map(|a| a.title),
        Some("Álbum correcto".to_owned())
    );
    assert_eq!(actualizada.isrc.as_deref(), Some("ISRC123"));
    assert_eq!(
        actualizada.added_at, original.added_at,
        "la fecha de alta es de la pista, no del candidato"
    );

    assert!(
        c.matches
            .rejected_ids(&original.id)
            .await
            .expect("consulta")
            .is_empty(),
        "el emparejamiento viejo no debe sobrevivir a una reasignación"
    );
}

#[tokio::test]
async fn resetear_metadatos_usa_el_nombre_de_fichero_si_hay_audio() {
    let c = ctx(Vec::new()).await;
    let original = catalogar(&c, id(3), "Se va a resetear").await;

    let rel = std::path::PathBuf::from("audio").join("xx").join(format!(
        "{}.opus",
        original.id.as_str()
    ));
    let absoluta = c.paths.resolve(&rel);
    std::fs::create_dir_all(absoluta.parent().expect("tiene carpeta")).expect("crea");
    std::fs::write(&absoluta, b"contenido").expect("escribe");
    c.audio
        .insert(&AudioFileRecord {
            track_id: original.id.clone(),
            rel_path: rel,
            format: AudioFormat::Opus,
            codec: "opus".into(),
            bitrate_kbps: None,
            sample_rate: None,
            channels: None,
            size_bytes: 9,
            duration: original.duration,
            source: AudioSource::Youtube,
            youtube_id: None,
            verified_at: chrono::Utc::now(),
        })
        .await
        .expect("registra");

    c.servicio
        .reset_metadata(&original.id)
        .await
        .expect("resetea");

    let reseteada = c
        .tracks
        .get(&original.id)
        .await
        .expect("consulta")
        .expect("existe");
    assert_eq!(reseteada.title, original.id.as_str());
    assert!(reseteada.artists.is_empty());
    assert!(reseteada.album.is_none());
}

#[tokio::test]
async fn resetear_metadatos_sin_audio_conserva_el_titulo() {
    let c = ctx(Vec::new()).await;
    let original = catalogar(&c, id(4), "Sin fichero todavía").await;

    c.servicio
        .reset_metadata(&original.id)
        .await
        .expect("resetea");

    let reseteada = c
        .tracks
        .get(&original.id)
        .await
        .expect("consulta")
        .expect("existe");
    assert_eq!(
        reseteada.title, "Sin fichero todavía",
        "sin audio que nombrar, el título se conserva"
    );
    assert!(reseteada.artists.is_empty());
}

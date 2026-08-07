//! Pipeline de descarga de extremo a extremo.
//!
//! Base de datos temporal real, sistema de ficheros real, y dobles para lo que
//! sale de la máquina (yt-dlp y el etiquetado). Comprueba las invariantes que
//! más importan: que en `audio/` no aparezca nada incompleto, que una pista ya
//! descargada no se vuelva a descargar, y que sin coincidencia fiable no entre
//! nada en la biblioteca.

#![allow(clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use localify_core::domain::audio::DurationMs;
use localify_core::domain::download::{
    Confidence, MatchResult, Priority, ScoreBreakdown, YoutubeCandidate,
};
use localify_core::domain::ids::{ArtistId, TrackId};
use localify_core::domain::settings::FormatPreference;
use localify_core::domain::track::{ArtistRef, Track};
use localify_core::error::{CoreError, CoreResult};
use localify_core::events::{DomainEvent, EventPublisher};
use localify_core::ports::database::TrackRepository;
use localify_core::ports::platform::AppPaths;
use localify_core::ports::services::DownloadService;
use localify_core::ports::youtube::{
    DownloadObserver, DownloadedFile, MediaInfo, TagWriter, YoutubeDownloader, YoutubeMatcher,
};
use localify_db::Pool;
use localify_db::pool::TempDbGuard;
use localify_platform::{LocalifyPaths, RealFileSystem};
use localify_services::actors::{DependenciasDescarga, DownloadActor};

// ─────────────────────────────────────────────────────────────────────────────
// Dobles
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Default)]
struct BusDePrueba {
    eventos: Mutex<Vec<DomainEvent>>,
}

impl BusDePrueba {
    fn nombres(&self) -> Vec<String> {
        self.eventos
            .lock()
            .map(|e| e.iter().map(|x| x.nombre().to_owned()).collect())
            .unwrap_or_default()
    }
}

impl EventPublisher for BusDePrueba {
    fn publish(&self, event: DomainEvent) {
        if let Ok(mut e) = self.eventos.lock() {
            e.push(event);
        }
    }
}

/// Catálogo que no sabe nada.
///
/// El actor de descargas le pregunta si conoce el vídeo oficial de la pista.
/// Casi ningún catálogo lo sabe —solo MusicBrainz—, así que este doble
/// representa el caso normal: la respuesta es que no y el emparejamiento sigue
/// su camino de siempre. Todos los métodos usan el valor por defecto del puerto.
#[derive(Debug)]
struct CatalogoMudo;

#[async_trait]
impl localify_core::ports::metadata_provider::MetadataProvider for CatalogoMudo {
    fn name(&self) -> &'static str {
        "mudo"
    }
    async fn status(&self) -> localify_core::events::ProviderStatus {
        localify_core::events::ProviderStatus::NotConfigured
    }
    async fn search_tracks(
        &self,
        _q: &str,
        _limit: u8,
        _offset: u16,
    ) -> CoreResult<localify_core::page::Page<Track>> {
        Ok(localify_core::page::Page::empty())
    }
    async fn track(&self, id: &TrackId) -> CoreResult<Track> {
        Err(CoreError::not_found("mudo", id.as_str()))
    }
    async fn tracks(&self, _ids: &[TrackId]) -> CoreResult<Vec<Track>> {
        Ok(Vec::new())
    }
    async fn album(
        &self,
        id: &localify_core::domain::ids::AlbumId,
    ) -> CoreResult<localify_core::domain::album::Album> {
        Err(CoreError::not_found("mudo", id.as_str()))
    }
    async fn album_tracks(
        &self,
        _id: &localify_core::domain::ids::AlbumId,
    ) -> CoreResult<Vec<Track>> {
        Ok(Vec::new())
    }
    async fn artist(
        &self,
        id: &localify_core::domain::ids::ArtistId,
    ) -> CoreResult<localify_core::domain::artist::Artist> {
        Err(CoreError::not_found("mudo", id.as_str()))
    }
    async fn artist_top_tracks(
        &self,
        _id: &localify_core::domain::ids::ArtistId,
    ) -> CoreResult<Vec<Track>> {
        Ok(Vec::new())
    }
    async fn artist_albums(
        &self,
        _id: &localify_core::domain::ids::ArtistId,
    ) -> CoreResult<Vec<localify_core::domain::album::Album>> {
        Ok(Vec::new())
    }
    async fn public_playlist(
        &self,
        _url: &str,
        _cb: &(dyn Fn(u32, u32) + Send + Sync),
    ) -> CoreResult<localify_core::ports::metadata_provider::PlaylistImport> {
        Err(CoreError::not_found("mudo", "lista"))
    }
}

/// Emparejador programable.
struct MatcherFalso {
    confianza: Confidence,
    llamadas: AtomicUsize,
}

#[async_trait]
impl YoutubeMatcher for MatcherFalso {
    async fn find_best(
        &self,
        track: &Track,
        _exclude: &[String],
        _conocido: Option<&str>,
    ) -> CoreResult<MatchResult> {
        self.llamadas.fetch_add(1, Ordering::Relaxed);
        Ok(MatchResult {
            track_id: track.id.clone(),
            best: YoutubeCandidate {
                video_id: "video123".to_owned(),
                title: track.title.clone(),
                channel: Some("Canal - Topic".to_owned()),
                duration: track.duration,
                view_count: Some(1_000_000),
                from_youtube_music: true,
                score: match self.confianza {
                    Confidence::High => 92.0,
                    Confidence::Medium => 62.0,
                    Confidence::Low => 30.0,
                },
                breakdown: ScoreBreakdown::default(),
            },
            confidence: self.confianza,
            candidates_considered: 5,
        })
    }
}

/// Descargador que escribe un fichero de mentira y avisa del progreso.
struct DescargadorFalso {
    bytes: usize,
    fallar: bool,
    llamadas: AtomicUsize,
    /// Cuánto tarda en terminar **después** de dejar los bytes en el temporal.
    ///
    /// Sin esta espera, el doble escribe el fichero entero y devuelve en el
    /// mismo instante: la descarga termina antes de que nadie llegue a ver un
    /// `.part`, y el arranque progresivo —que es el caso real y el que más
    /// importa— quedaría sin probar.
    retraso: Duration,
}

#[async_trait]
impl YoutubeDownloader for DescargadorFalso {
    async fn download(
        &self,
        _video_id: &str,
        _preference: FormatPreference,
        dest: &Path,
        expected: DurationMs,
        observer: &dyn DownloadObserver,
    ) -> CoreResult<DownloadedFile> {
        self.llamadas.fetch_add(1, Ordering::Relaxed);

        if self.fallar {
            return Err(CoreError::ProviderUnavailable {
                provider: "youtube",
                source: None,
            });
        }

        if let Some(padre) = dest.parent() {
            tokio::fs::create_dir_all(padre).await.ok();
        }
        tokio::fs::write(dest, vec![0_u8; self.bytes])
            .await
            .map_err(|e| CoreError::storage(e.to_string()))?;

        observer.on_progress(&localify_core::domain::download::DownloadProgress {
            bytes_done: self.bytes as u64,
            bytes_total: Some(self.bytes as u64),
            playable: true,
            state: localify_core::domain::download::DownloadState::Downloading,
        });
        observer.on_playable(dest);

        if !self.retraso.is_zero() {
            tokio::time::sleep(self.retraso).await;
        }

        Ok(DownloadedFile {
            path: dest.to_path_buf(),
            info: MediaInfo {
                duration: expected,
                codec: "opus".to_owned(),
                bitrate_kbps: Some(160),
                sample_rate: Some(48_000),
                channels: Some(2),
                seekable_from_start: true,
            },
            extension: "opus".to_owned(),
        })
    }

    async fn probe(&self, _path: &Path) -> CoreResult<MediaInfo> {
        Err(CoreError::internal("no se usa"))
    }
}

#[derive(Debug, Default)]
struct EtiquetadorFalso {
    llamadas: AtomicUsize,
}

#[async_trait]
impl TagWriter for EtiquetadorFalso {
    async fn write(&self, _path: &Path, _track: &Track, _cover: Option<&[u8]>) -> CoreResult<()> {
        self.llamadas.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    async fn read_track_id(&self, _path: &Path) -> CoreResult<Option<String>> {
        Ok(None)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Montaje
// ─────────────────────────────────────────────────────────────────────────────

struct Ctx {
    actor: DownloadActor,
    tracks: Arc<dyn TrackRepository>,
    jobs: Arc<dyn localify_core::ports::database::DownloadJobRepository>,
    paths: Arc<LocalifyPaths>,
    bus: Arc<BusDePrueba>,
    descargador: Arc<DescargadorFalso>,
    etiquetador: Arc<EtiquetadorFalso>,
    biblioteca: PathBuf,
    _guard: TempDbGuard,
}

impl Drop for Ctx {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.biblioteca);
    }
}

async fn ctx(confianza: Confidence, fallar_descarga: bool) -> Ctx {
    ctx_con_retraso(confianza, fallar_descarga, Duration::ZERO).await
}

/// Igual, pero con una descarga que tarda en terminar.
async fn ctx_con_retraso(
    confianza: Confidence,
    fallar_descarga: bool,
    retraso_descarga: Duration,
) -> Ctx {
    let (pool, guard) = Pool::temporal().expect("abre");
    localify_db::ejecutar_migraciones(&pool)
        .await
        .expect("migra");

    let biblioteca = std::env::temp_dir().join(format!("localify-dl-{}", uuid::Uuid::now_v7()));
    let paths = Arc::new(LocalifyPaths::con_biblioteca(
        biblioteca.join("config"),
        biblioteca.clone(),
    ));
    paths.crear_estructura().expect("crea carpetas");

    let tracks: Arc<dyn TrackRepository> = Arc::new(
        localify_db::repositories::SqliteTrackRepository::new(pool.clone()),
    );
    let jobs: Arc<dyn localify_core::ports::database::DownloadJobRepository> = Arc::new(
        localify_db::repositories::SqliteDownloadJobRepository::new(pool.clone()),
    );
    let bus = Arc::new(BusDePrueba::default());
    let descargador = Arc::new(DescargadorFalso {
        bytes: 100_000,
        retraso: retraso_descarga,
        fallar: fallar_descarga,
        llamadas: AtomicUsize::new(0),
    });
    let etiquetador = Arc::new(EtiquetadorFalso::default());

    let deps = DependenciasDescarga {
        matcher: Arc::new(MatcherFalso {
            confianza,
            llamadas: AtomicUsize::new(0),
        }),
        // Un catálogo que no sabe de vídeos oficiales, que es el caso normal:
        // la pista conocida es un extra, no un requisito.
        provider: Arc::new(CatalogoMudo),
        downloader: Arc::clone(&descargador) as Arc<dyn YoutubeDownloader>,
        tagger: Arc::clone(&etiquetador) as Arc<dyn TagWriter>,
        tracks: Arc::clone(&tracks),
        audio: Arc::new(localify_db::repositories::SqliteAudioFileRepository::new(
            pool.clone(),
        )),
        jobs: Arc::clone(&jobs),
        matches: Arc::new(localify_db::repositories::SqliteYoutubeMatchRepository::new(pool)),
        fs: Arc::new(RealFileSystem::new()),
        paths: Arc::clone(&paths) as Arc<dyn AppPaths>,
        bus: Arc::clone(&bus) as Arc<dyn EventPublisher>,
        formato: FormatPreference::Opus,
        // Mismo numero de intentos que en produccion, sin las esperas: lo que
        // se prueba es la politica de reintento, no el reloj.
        backoff: vec![Duration::from_millis(5), Duration::from_millis(5)],
    };

    Ctx {
        actor: DownloadActor::arrancar(deps),
        tracks,
        jobs,
        paths,
        bus,
        descargador,
        etiquetador,
        biblioteca,
        _guard: guard,
    }
}

fn pista() -> Track {
    Track {
        id: TrackId::from_trusted("3z8h0TU7ReDPLIbEnYhWZb"),
        title: "Under Pressure".into(),
        album: None,
        artists: vec![ArtistRef {
            id: ArtistId::nuevo_local(),
            name: "Queen".into(),
        }],
        duration: DurationMs::new(248_000),
        track_number: None,
        disc_number: None,
        explicit: false,
        isrc: None,
        release_date: None,
        popularity: None,
        added_at: chrono::Utc::now(),
    }
}

/// Espera a que la pista quede disponible en disco.
async fn esperar_final(c: &Ctx, track: &TrackId) -> bool {
    esperar_estado(c, track, localify_core::domain::Availability::es_local).await
}

/// Espera a que la descarga se dé por fallida tras agotar los reintentos.
async fn esperar_fallo(c: &Ctx, track: &TrackId) -> bool {
    esperar_estado(c, track, |a| {
        matches!(
            a,
            localify_core::domain::availability::Availability::Failed { .. }
        )
    })
    .await
}

/// Espera a que la disponibilidad cumpla una condición.
///
/// Sondear el estado y no un simple "ya no está ausente" importa: entre el
/// primer intento y el último, una descarga pasa por `Downloading` varias
/// veces, y quedarse con el primer cambio daría por terminado algo que sigue
/// en marcha.
async fn esperar_estado(
    c: &Ctx,
    track: &TrackId,
    condicion: impl Fn(&localify_core::domain::availability::Availability) -> bool,
) -> bool {
    for _ in 0..400 {
        if c.actor.status(track).await.is_ok_and(|a| condicion(&a)) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    false
}

// ─────────────────────────────────────────────────────────────────────────────
// Casos
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn una_descarga_completa_deja_el_fichero_en_la_biblioteca() {
    // Con una descarga que tarda, `ensure` devuelve el temporal: es el caso
    // real y el que prueba el arranque progresivo. Con la descarga instantánea
    // del doble por defecto, terminaría antes de que nadie viera un `.part` y
    // esta parte del test no comprobaría nada.
    let c = ctx_con_retraso(Confidence::High, false, Duration::from_millis(600)).await;
    let t = pista();
    c.tracks
        .upsert(std::slice::from_ref(&t))
        .await
        .expect("guarda");

    let handle = c
        .actor
        .ensure(&t.id, Priority::Immediate)
        .await
        .expect("arranca");
    assert!(!handle.complete, "aun no esta descargada");
    assert!(
        handle.playable_path.to_string_lossy().ends_with(".part"),
        "la reproduccion arranca sobre el temporal: {:?}",
        handle.playable_path
    );
    // Y la ruta que se devuelve **existe ya**: es la promesa de `ensure`, y
    // durante un tiempo no se cumplía. El motor abría un fichero inexistente y
    // la reproducción moría en silencio mientras la descarga iba bien.
    assert!(
        handle.playable_path.exists(),
        "ensure prometió una ruta reproducible y no existe: {:?}",
        handle.playable_path
    );

    assert!(esperar_final(&c, &t.id).await, "la descarga no termino");

    let estado = c.actor.status(&t.id).await.expect("consulta");
    assert!(estado.es_local(), "estado: {estado:?}");

    let definitivo = c
        .paths
        .audio_dir()
        .join("3z")
        .join("3z8h0TU7ReDPLIbEnYhWZb.opus");
    assert!(definitivo.exists(), "el fichero debe estar en audio/");
    assert_eq!(c.etiquetador.llamadas.load(Ordering::Relaxed), 1);
}

#[tokio::test]
async fn el_temporal_no_sobrevive_a_una_descarga_correcta() {
    // Si quedara, la proxima limpieza lo borraria, pero mientras tanto ocuparia
    // el doble de espacio por cada cancion.
    let c = ctx(Confidence::High, false).await;
    let t = pista();
    c.tracks
        .upsert(std::slice::from_ref(&t))
        .await
        .expect("guarda");

    c.actor
        .ensure(&t.id, Priority::Immediate)
        .await
        .expect("arranca");
    assert!(esperar_final(&c, &t.id).await);

    let restos: Vec<_> = std::fs::read_dir(c.paths.temp_dir())
        .expect("lee .tmp")
        .filter_map(Result::ok)
        .collect();
    assert!(restos.is_empty(), "quedaron temporales: {restos:?}");
}

#[tokio::test]
async fn una_pista_ya_descargada_no_se_vuelve_a_descargar() {
    // Es la invariante central del proyecto.
    let c = ctx(Confidence::High, false).await;
    let t = pista();
    c.tracks
        .upsert(std::slice::from_ref(&t))
        .await
        .expect("guarda");

    c.actor
        .ensure(&t.id, Priority::Immediate)
        .await
        .expect("primera");
    assert!(esperar_final(&c, &t.id).await);
    let tras_la_primera = c.descargador.llamadas.load(Ordering::Relaxed);

    let handle = c
        .actor
        .ensure(&t.id, Priority::Immediate)
        .await
        .expect("segunda");

    assert!(handle.complete, "debe reportarse como ya completa");
    assert_eq!(
        c.descargador.llamadas.load(Ordering::Relaxed),
        tras_la_primera,
        "no debe salir una segunda descarga"
    );
}

#[tokio::test]
async fn dos_peticiones_a_la_vez_comparten_una_sola_descarga() {
    let c = ctx(Confidence::High, false).await;
    let t = pista();
    c.tracks
        .upsert(std::slice::from_ref(&t))
        .await
        .expect("guarda");

    let uno = c
        .actor
        .ensure(&t.id, Priority::Immediate)
        .await
        .expect("uno");
    let otro = c
        .actor
        .ensure(&t.id, Priority::Prefetch)
        .await
        .expect("otro");

    assert_eq!(
        uno.playable_path, otro.playable_path,
        "ambas deben apuntar al mismo temporal"
    );
    assert!(esperar_final(&c, &t.id).await);
    assert_eq!(
        c.descargador.llamadas.load(Ordering::Relaxed),
        1,
        "una peticion duplicada no debe duplicar el trabajo"
    );
}

#[tokio::test]
async fn sin_coincidencia_fiable_no_entra_nada_en_la_biblioteca() {
    // ADR-017: lo descargado no se vuelve a descargar, asi que un karaoke se
    // quedaria para siempre.
    let c = ctx(Confidence::Low, false).await;
    let t = pista();
    c.tracks
        .upsert(std::slice::from_ref(&t))
        .await
        .expect("guarda");

    // `ensure` promete devolver algo reproducible, asi que cuando no lo hay el
    // fallo viaja por el camino de errores. Antes devolvia Ok con la ruta de un
    // temporal que nadie iba a escribir, y quien llamaba se quedaba con una
    // cancion cargada, en pausa y sin explicacion.
    let error = c
        .actor
        .ensure(&t.id, Priority::Immediate)
        .await
        .expect_err("sin coincidencia no hay nada que reproducir");
    assert_eq!(error.code(), "NOT_FOUND");

    assert!(
        esperar_fallo(&c, &t.id).await,
        "deberia acabar como fallida"
    );

    assert_eq!(
        c.descargador.llamadas.load(Ordering::Relaxed),
        0,
        "no debe descargarse nada"
    );

    let entradas: Vec<_> = std::fs::read_dir(c.paths.audio_dir())
        .expect("lee audio/")
        .filter_map(Result::ok)
        .collect();
    assert!(entradas.is_empty(), "audio/ debe seguir vacia");
}

#[tokio::test]
async fn una_confianza_media_si_descarga() {
    let c = ctx(Confidence::Medium, false).await;
    let t = pista();
    c.tracks
        .upsert(std::slice::from_ref(&t))
        .await
        .expect("guarda");

    c.actor
        .ensure(&t.id, Priority::Immediate)
        .await
        .expect("arranca");
    assert!(esperar_final(&c, &t.id).await);

    assert!(c.actor.status(&t.id).await.expect("consulta").es_local());
}

#[tokio::test]
async fn una_descarga_fallida_no_deja_restos_en_la_biblioteca() {
    let c = ctx(Confidence::High, true).await;
    let t = pista();
    c.tracks
        .upsert(std::slice::from_ref(&t))
        .await
        .expect("guarda");

    let error = c
        .actor
        .ensure(&t.id, Priority::Immediate)
        .await
        .expect_err("una descarga que falla no puede devolver una ruta");
    assert_eq!(error.code(), "NOT_FOUND");

    assert!(
        esperar_fallo(&c, &t.id).await,
        "deberia acabar como fallida"
    );

    let entradas: Vec<_> = std::fs::read_dir(c.paths.audio_dir())
        .expect("lee audio/")
        .filter_map(Result::ok)
        .collect();
    assert!(
        entradas.is_empty(),
        "audio/ debe seguir vacia: {entradas:?}"
    );

    assert!(
        c.descargador.llamadas.load(Ordering::Relaxed) > 1,
        "un fallo de proveedor debe reintentarse"
    );
}

#[tokio::test]
async fn se_emiten_los_eventos_del_ciclo_completo() {
    let c = ctx(Confidence::High, false).await;
    let t = pista();
    c.tracks
        .upsert(std::slice::from_ref(&t))
        .await
        .expect("guarda");

    c.actor
        .ensure(&t.id, Priority::Immediate)
        .await
        .expect("arranca");
    assert!(esperar_final(&c, &t.id).await);

    let nombres = c.bus.nombres();
    for esperado in [
        "downloadStarted",
        "downloadPlayable",
        "downloadCompleted",
        "availabilityChanged",
    ] {
        assert!(
            nombres.iter().any(|n| n == esperado),
            "falta '{esperado}' en {nombres:?}"
        );
    }
}

#[tokio::test]
async fn una_pista_desconocida_falla_sin_descargar() {
    let c = ctx(Confidence::High, false).await;
    let desconocida = TrackId::nuevo_local();

    let error = c
        .actor
        .ensure(&desconocida, Priority::Immediate)
        .await
        .expect_err("sin metadatos no se puede reproducir nada");
    assert_eq!(error.code(), "NOT_FOUND");

    assert_eq!(
        c.descargador.llamadas.load(Ordering::Relaxed),
        0,
        "sin metadatos no hay nada que emparejar"
    );
}

#[tokio::test]
async fn los_trabajos_interrumpidos_se_descartan_al_arrancar() {
    // Reanudar una descarga parcial arriesgaria un fichero mal concatenado.
    let c = ctx(Confidence::High, false).await;
    let t = pista();
    c.tracks
        .upsert(std::slice::from_ref(&t))
        .await
        .expect("guarda");

    // Una descarga a medias, tal y como la habria dejado un corte de luz.
    let parcial = c.paths.temp_dir().join("interrumpida.part");
    std::fs::write(&parcial, b"medio fichero").expect("escribe parcial");
    c.jobs
        .upsert(&localify_core::domain::download::DownloadJob {
            track_id: t.id.clone(),
            state: localify_core::domain::download::DownloadState::Downloading,
            priority: Priority::Immediate,
            video_id: Some("video123".to_owned()),
            tmp_path: Some(parcial.clone()),
            bytes_done: 13,
            bytes_total: Some(100_000),
            attempts: 1,
            last_error_key: None,
        })
        .await
        .expect("guarda trabajo");

    // Y un `.part` sin fila: el proceso murio antes de persistir nada.
    let huerfano = c.paths.temp_dir().join("sin-dueno.part");
    std::fs::write(&huerfano, b"nadie me reclama").expect("escribe huerfano");

    let descartados = c.actor.limpiar_interrumpidos().await.expect("limpia");

    assert_eq!(descartados, 1, "el trabajo a medias debe descartarse");
    assert!(!parcial.exists(), "el .part con fila debe borrarse");
    assert!(
        !huerfano.exists(),
        "el .part sin fila tambien: si no, ocuparia disco para siempre"
    );
    assert!(
        c.jobs.get(&t.id).await.expect("consulta").is_none(),
        "la fila no debe sobrevivir al descarte"
    );
}

#[tokio::test]
async fn cinco_canciones_seguidas_dejan_cinco_descargas_vivas() {
    // ADR-016: cambiar de cancion no cancela nada. Si el usuario pulsa play
    // cinco veces en cinco segundos, acaba con cinco ficheros, no con uno.
    let c = ctx(Confidence::High, false).await;

    let ids = [
        "3z8h0TU7ReDPLIbEnYhWZb",
        "4u7EnebtmKWzUH433cf5Qv",
        "5CQ30WqJwcep0pYcV4AMNc",
        "1AhDOtG9vPSOmsWgNW0BEY",
        "7ouMYWpwJ422jRcDASZB7P",
    ];
    let pistas: Vec<Track> = ids
        .iter()
        .map(|id| Track {
            id: TrackId::from_trusted(*id),
            ..pista()
        })
        .collect();
    c.tracks.upsert(&pistas).await.expect("guarda");

    for p in &pistas {
        c.actor
            .ensure(&p.id, Priority::Immediate)
            .await
            .expect("arranca");
    }

    for p in &pistas {
        assert!(
            esperar_final(&c, &p.id).await,
            "la descarga de {} se quedo por el camino",
            p.id.as_str()
        );
    }

    assert_eq!(
        c.descargador.llamadas.load(Ordering::Relaxed),
        5,
        "ninguna de las cinco debe cancelarse ni repetirse"
    );
}

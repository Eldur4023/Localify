//! Playlists, con base de datos real.
//!
//! Lo que más importa aquí es el reordenamiento: es la operación que el usuario
//! hace arrastrando, y la que con índices enteros costaría `n` escrituras. Los
//! tests comprueban tanto el resultado —el orden que ve— como el coste —una
//! sola fila tocada— porque lo segundo es lo que se rompe en silencio.

#![allow(clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use async_trait::async_trait;
use localify_core::domain::album::Album;
use localify_core::domain::artist::Artist;
use localify_core::domain::audio::DurationMs;
use localify_core::domain::ids::{AlbumId, ArtistId, PlaylistId, TrackId};
use localify_core::domain::track::{ArtistRef, Track};
use localify_core::error::{CoreError, CoreResult};
use localify_core::events::{DomainEvent, EventPublisher};
use localify_core::page::PageRequest;
use localify_core::ports::database::TrackRepository;
use localify_core::ports::metadata_provider::{MetadataProvider, PlaylistImport};
use localify_core::ports::platform::AppPaths;
use localify_core::ports::services::PlaylistService;
use localify_db::Pool;
use localify_db::pool::TempDbGuard;
use localify_platform::{LocalifyPaths, RealFileSystem};
use localify_services::{DependenciasPlaylists, PlaylistServiceImpl};

// ─────────────────────────────────────────────────────────────────────────────
// Dobles
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Default)]
struct BusDePrueba(std::sync::Mutex<Vec<DomainEvent>>);

impl EventPublisher for BusDePrueba {
    fn publish(&self, event: DomainEvent) {
        if let Ok(mut v) = self.0.lock() {
            v.push(event);
        }
    }
}

impl BusDePrueba {
    fn nombres(&self) -> Vec<String> {
        self.0
            .lock()
            .map(|v| v.iter().map(|e| e.nombre().to_owned()).collect())
            .unwrap_or_default()
    }
}

/// Proveedor que devuelve una playlist preparada.
struct ProveedorFalso {
    pistas: Vec<Track>,
    paginas: std::sync::atomic::AtomicU32,
}

#[async_trait]
impl MetadataProvider for ProveedorFalso {
    fn name(&self) -> &'static str {
        "falso"
    }
    async fn status(&self) -> localify_core::events::ProviderStatus {
        localify_core::events::ProviderStatus::Ready
    }
    async fn search_tracks(
        &self,
        _q: &str,
        _limit: u8,
        _offset: u16,
    ) -> CoreResult<localify_core::page::Page<Track>> {
        Ok(localify_core::page::Page::new(Vec::new(), Some(0), None))
    }
    async fn track(&self, _id: &TrackId) -> CoreResult<Track> {
        Err(CoreError::internal("no se usa"))
    }
    async fn tracks(&self, _ids: &[TrackId]) -> CoreResult<Vec<Track>> {
        Ok(Vec::new())
    }
    async fn album(&self, _id: &AlbumId) -> CoreResult<Album> {
        Err(CoreError::internal("no se usa"))
    }
    async fn album_tracks(&self, _id: &AlbumId) -> CoreResult<Vec<Track>> {
        Ok(Vec::new())
    }
    async fn artist(&self, _id: &ArtistId) -> CoreResult<Artist> {
        Err(CoreError::internal("no se usa"))
    }
    async fn artist_top_tracks(&self, _id: &ArtistId) -> CoreResult<Vec<Track>> {
        Ok(Vec::new())
    }
    async fn artist_albums(&self, _id: &ArtistId) -> CoreResult<Vec<Album>> {
        Ok(Vec::new())
    }
    async fn public_playlist(
        &self,
        _url: &str,
        page_callback: &(dyn Fn(u32, u32) + Send + Sync),
    ) -> CoreResult<PlaylistImport> {
        let total = u32::try_from(self.pistas.len()).unwrap_or(0);
        // Dos páginas, para comprobar que el progreso se emite conforme llegan.
        page_callback(total / 2, total);
        self.paginas
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        page_callback(total, total);
        self.paginas
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        Ok(PlaylistImport {
            source_id: "37i9dQZF1DXcBWIGoYBM5M".to_owned(),
            name: "Today's Top Hits".to_owned(),
            description: Some("Los éxitos del momento".to_owned()),
            cover_url: None,
            total,
            tracks: self.pistas.clone(),
        })
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Montaje
// ─────────────────────────────────────────────────────────────────────────────

struct Ctx {
    svc: PlaylistServiceImpl,
    pool: Pool,
    bus: Arc<BusDePrueba>,
    paths: Arc<LocalifyPaths>,
    pistas: Vec<TrackId>,
    biblioteca: std::path::PathBuf,
    _guard: TempDbGuard,
}

impl Drop for Ctx {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.biblioteca);
    }
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

fn pista(n: usize) -> Track {
    Track {
        id: id(n),
        title: format!("Pista {n:03}"),
        album: None,
        artists: vec![ArtistRef {
            id: ArtistId::nuevo_local(),
            name: "Artista".into(),
        }],
        duration: DurationMs::from_secs(180),
        track_number: None,
        disc_number: None,
        explicit: false,
        isrc: None,
        release_date: None,
        popularity: None,
        added_at: chrono::Utc::now(),
    }
}

async fn ctx(cuantas: usize) -> Ctx {
    let (pool, guard) = Pool::temporal().expect("abre");
    localify_db::ejecutar_migraciones(&pool)
        .await
        .expect("migra");

    let biblioteca = std::env::temp_dir().join(format!("localify-pl-{}", uuid::Uuid::now_v7()));
    let paths = Arc::new(LocalifyPaths::con_biblioteca(
        biblioteca.join("config"),
        biblioteca.clone(),
    ));
    paths.crear_estructura().expect("crea carpetas");

    let tracks: Arc<dyn TrackRepository> = Arc::new(
        localify_db::repositories::SqliteTrackRepository::new(pool.clone()),
    );
    let filas: Vec<Track> = (0..cuantas).map(pista).collect();
    if !filas.is_empty() {
        tracks.upsert(&filas).await.expect("guarda");
    }

    let bus = Arc::new(BusDePrueba::default());
    let svc = PlaylistServiceImpl::nuevo(DependenciasPlaylists {
        playlists: Arc::new(localify_db::repositories::SqlitePlaylistRepository::new(
            pool.clone(),
        )),
        tracks: Arc::clone(&tracks),
        similitud: Arc::new(localify_db::repositories::SqliteSimilarityRepository::new(
            pool.clone(),
        )),
        provider: Arc::new(ProveedorFalso {
            pistas: (100..110).map(pista).collect(),
            paginas: std::sync::atomic::AtomicU32::new(0),
        }),
        // Sin descargador de imágenes: la lista se importa igual y se queda con
        // el mosaico, que es el caso de las creadas a mano.
        imagenes: None,
        // Sin descargas: estos tests comprueban orden y persistencia, no que se
        // prepare el audio. Montar el actor solo para eso metería yt-dlp en una
        // suite que hoy no toca la red.
        descargas: None,
        fs: Arc::new(RealFileSystem::new()),
        paths: Arc::clone(&paths) as Arc<dyn AppPaths>,
        bus: Arc::clone(&bus) as Arc<dyn EventPublisher>,
    });

    Ctx {
        svc,
        pool,
        bus,
        paths,
        pistas: filas.into_iter().map(|t| t.id).collect(),
        biblioteca,
        _guard: guard,
    }
}

/// Títulos en el orden en que están en la playlist.
async fn orden(c: &Ctx, id: &PlaylistId) -> Vec<String> {
    c.svc
        .detail(id, &PageRequest::new(0, 500))
        .await
        .expect("detalle")
        .entries
        .into_iter()
        .map(|e| e.track.title)
        .collect()
}

/// Cuántas filas de `playlist_items` tienen la posición dada.
async fn posiciones(c: &Ctx, id: &PlaylistId) -> Vec<f64> {
    let texto = id.to_string();
    c.pool
        .leer(move |conn| {
            let mut st = conn.prepare(
                "SELECT position FROM playlist_items WHERE playlist_id = ?1 ORDER BY position",
            )?;
            let filas = st
                .query_map([&texto], |r| r.get::<_, f64>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(filas)
        })
        .await
        .expect("consulta")
}

// ─────────────────────────────────────────────────────────────────────────────
// CRUD
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn crear_una_playlist_la_deja_vacia_y_avisa() {
    let c = ctx(0).await;
    let resumen = c.svc.create("Mis favoritas").await.expect("crea");

    assert_eq!(resumen.name, "Mis favoritas");
    assert_eq!(resumen.track_count, 0);
    assert!(c.bus.nombres().iter().any(|n| n == "playlistChanged"));
    assert_eq!(c.svc.list().await.expect("lista").len(), 1);
}

#[tokio::test]
async fn un_nombre_vacio_no_crea_nada() {
    let c = ctx(0).await;
    assert!(c.svc.create("   ").await.is_err());
    assert!(
        c.svc.list().await.expect("lista").is_empty(),
        "una validacion fallida no debe dejar rastro"
    );
}

#[tokio::test]
async fn renombrar_y_borrar_funcionan() {
    let c = ctx(0).await;
    let p = c.svc.create("Antes").await.expect("crea");

    c.svc.rename(&p.id, "Después").await.expect("renombra");
    assert_eq!(c.svc.list().await.expect("lista")[0].name, "Después");

    c.svc.delete(&p.id).await.expect("borra");
    assert!(c.svc.list().await.expect("lista").is_empty());
}

#[tokio::test]
async fn el_detalle_de_una_playlist_inexistente_da_no_encontrado() {
    let c = ctx(0).await;
    let error = c
        .svc
        .detail(&PlaylistId::nuevo(), &PageRequest::new(0, 50))
        .await
        .expect_err("deberia fallar");
    assert_eq!(error.code(), "NOT_FOUND");
}

// ─────────────────────────────────────────────────────────────────────────────
// Añadir y quitar
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn las_pistas_se_anaden_en_orden() {
    let c = ctx(5).await;
    let p = c.svc.create("Lista").await.expect("crea");

    c.svc
        .add_tracks(&p.id, &c.pistas, None)
        .await
        .expect("anade");

    assert_eq!(
        orden(&c, &p.id).await,
        vec![
            "Pista 000",
            "Pista 001",
            "Pista 002",
            "Pista 003",
            "Pista 004"
        ]
    );
}

#[tokio::test]
async fn anadir_al_final_respeta_lo_que_ya_habia() {
    let c = ctx(6).await;
    let p = c.svc.create("Lista").await.expect("crea");

    c.svc
        .add_tracks(&p.id, &c.pistas[..3], None)
        .await
        .expect("primeras");
    c.svc
        .add_tracks(&p.id, &c.pistas[3..], None)
        .await
        .expect("resto");

    let titulos = orden(&c, &p.id).await;
    assert_eq!(titulos.len(), 6);
    assert_eq!(titulos[0], "Pista 000");
    assert_eq!(titulos[5], "Pista 005");
}

#[tokio::test]
async fn anadir_en_una_posicion_intermedia_inserta_ahi() {
    let c = ctx(5).await;
    let p = c.svc.create("Lista").await.expect("crea");
    c.svc
        .add_tracks(&p.id, &c.pistas[..3], None)
        .await
        .expect("base");

    // Entre la primera y la segunda.
    c.svc
        .add_tracks(&p.id, &c.pistas[4..5], Some(1))
        .await
        .expect("inserta");

    assert_eq!(
        orden(&c, &p.id).await,
        vec!["Pista 000", "Pista 004", "Pista 001", "Pista 002"]
    );
}

#[tokio::test]
async fn la_misma_pista_puede_estar_dos_veces() {
    // El identificador es de la entrada, no de la pista: quitar una copia no
    // debe llevarse la otra.
    let c = ctx(2).await;
    let p = c.svc.create("Lista").await.expect("crea");
    c.svc
        .add_tracks(&p.id, &[c.pistas[0].clone(), c.pistas[0].clone()], None)
        .await
        .expect("anade");

    let detalle = c
        .svc
        .detail(&p.id, &PageRequest::new(0, 50))
        .await
        .expect("detalle");
    assert_eq!(detalle.entries.len(), 2);

    c.svc
        .remove_entries(&p.id, &[detalle.entries[0].entry_id])
        .await
        .expect("quita");
    assert_eq!(
        c.svc
            .detail(&p.id, &PageRequest::new(0, 50))
            .await
            .expect("detalle")
            .entries
            .len(),
        1
    );
}

#[tokio::test]
async fn anadir_una_lista_vacia_no_hace_nada() {
    let c = ctx(3).await;
    let p = c.svc.create("Lista").await.expect("crea");
    c.svc.add_tracks(&p.id, &[], None).await.expect("nada");
    assert!(orden(&c, &p.id).await.is_empty());
}

// ─────────────────────────────────────────────────────────────────────────────
// Reordenar: el corazón de ADR-009
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn mover_una_pista_al_principio_solo_toca_una_fila() {
    // Es la razon de ser de las claves fraccionarias: con indices enteros,
    // esto renumeraria las cinco filas.
    let c = ctx(5).await;
    let p = c.svc.create("Lista").await.expect("crea");
    c.svc
        .add_tracks(&p.id, &c.pistas, None)
        .await
        .expect("anade");

    let antes = posiciones(&c, &p.id).await;
    let detalle = c
        .svc
        .detail(&p.id, &PageRequest::new(0, 50))
        .await
        .expect("detalle");
    let ultima = detalle.entries[4].entry_id;

    c.svc.reorder(&p.id, ultima, 0).await.expect("mueve");

    let despues = posiciones(&c, &p.id).await;
    let cambiadas = antes
        .iter()
        .filter(|a| !despues.iter().any(|d| (d - *a).abs() < f64::EPSILON))
        .count();
    assert_eq!(
        cambiadas, 1,
        "solo la fila movida deberia cambiar de posicion"
    );

    assert_eq!(orden(&c, &p.id).await[0], "Pista 004");
}

#[tokio::test]
async fn mover_al_final_deja_la_pista_la_ultima() {
    let c = ctx(4).await;
    let p = c.svc.create("Lista").await.expect("crea");
    c.svc
        .add_tracks(&p.id, &c.pistas, None)
        .await
        .expect("anade");

    let detalle = c
        .svc
        .detail(&p.id, &PageRequest::new(0, 50))
        .await
        .expect("detalle");
    let primera = detalle.entries[0].entry_id;

    c.svc.reorder(&p.id, primera, 4).await.expect("mueve");
    assert_eq!(*orden(&c, &p.id).await.last().expect("hay"), "Pista 000");
}

#[tokio::test]
async fn reordenar_muchas_veces_mantiene_el_orden_coherente() {
    // Insertar siempre en el mismo hueco estrecha las posiciones. Con
    // suficientes movimientos, el rebalanceo tiene que entrar sin que el orden
    // visible cambie.
    let c = ctx(8).await;
    let p = c.svc.create("Lista").await.expect("crea");
    c.svc
        .add_tracks(&p.id, &c.pistas, None)
        .await
        .expect("anade");

    for _ in 0..40 {
        let detalle = c
            .svc
            .detail(&p.id, &PageRequest::new(0, 50))
            .await
            .expect("detalle");
        let ultima = detalle.entries[7].entry_id;
        c.svc.reorder(&p.id, ultima, 1).await.expect("mueve");
    }

    let titulos = orden(&c, &p.id).await;
    assert_eq!(titulos.len(), 8, "no debe perderse ni duplicarse nada");

    let unicos: std::collections::HashSet<_> = titulos.iter().collect();
    assert_eq!(unicos.len(), 8, "hay entradas duplicadas: {titulos:?}");

    let pos = posiciones(&c, &p.id).await;
    assert!(
        pos.windows(2).all(|w| w[1] > w[0]),
        "las posiciones deben ser estrictamente crecientes: {pos:?}"
    );
}

#[tokio::test]
async fn el_rebalanceo_devuelve_hueco_a_las_posiciones() {
    // El test anterior pasa con o sin rebalanceo: mientras `f64` aguante, el
    // orden sigue siendo correcto. Este comprueba lo que de verdad hace el
    // rebalanceo, que es recuperar separacion antes de que la precision se
    // agote.
    let c = ctx(4).await;
    let p = c.svc.create("Lista").await.expect("crea");
    c.svc
        .add_tracks(&p.id, &c.pistas, None)
        .await
        .expect("anade");

    // Insertar siempre en el mismo hueco lo va partiendo por la mitad. Con un
    // hueco inicial de 1024 hacen falta 31 partes para bajar del epsilon
    // (1024 / 2^30 ≈ 9.5e-7), y el aviso se comprueba con el hueco de **antes**
    // de partirlo: con 30 vueltas justas el rebalanceo no llegaria a dispararse.
    for _ in 0..40 {
        let detalle = c
            .svc
            .detail(&p.id, &PageRequest::new(0, 50))
            .await
            .expect("detalle");
        let ultima = detalle.entries[3].entry_id;
        c.svc.reorder(&p.id, ultima, 1).await.expect("mueve");
    }

    // El rebalanceo va en segundo plano: se le da tiempo a asentarse.
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let hueco = separacion_minima(&posiciones(&c, &p.id).await);

    // Sin rebalanceo, 40 particiones de un hueco de 1024 lo dejarian en
    // 1024 / 2^40 ≈ 9e-10: tres ordenes de magnitud por debajo del epsilon y
    // camino de agotar la precision de `f64`. Que siga por encima es la prueba
    // de que el rebalanceo entro.
    assert!(
        hueco > localify_core::domain::playlist::position::EPSILON,
        "las posiciones se agotaron: hueco minimo {hueco:e}"
    );
    assert_eq!(
        orden(&c, &p.id).await.len(),
        4,
        "y sin perder ni duplicar entradas"
    );
}

/// Menor distancia entre dos posiciones consecutivas.
fn separacion_minima(pos: &[f64]) -> f64 {
    pos.windows(2)
        .map(|w| w[1] - w[0])
        .fold(f64::INFINITY, f64::min)
}

// ─────────────────────────────────────────────────────────────────────────────
// Importación
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn importar_trae_las_pistas_y_avisa_del_progreso() {
    let c = ctx(0).await;
    c.svc
        .import_from_provider("https://open.spotify.com/playlist/37i9dQZF1DXcBWIGoYBM5M")
        .await
        .expect("importa");

    // La importación va en una tarea de fondo.
    for _ in 0..100 {
        if c.svc.list().await.expect("lista").len() == 1 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    let listas = c.svc.list().await.expect("lista");
    assert_eq!(listas.len(), 1, "deberia haber creado la playlist");
    assert_eq!(listas[0].name, "Today's Top Hits");

    let titulos = orden(&c, &listas[0].id).await;
    assert_eq!(titulos.len(), 10, "deberian estar las diez pistas");
    assert_eq!(titulos[0], "Pista 100", "y en su orden original");

    let nombres = c.bus.nombres();
    for esperado in ["playlistImportProgress", "playlistImportFinished"] {
        assert!(
            nombres.iter().any(|n| n == esperado),
            "falta '{esperado}' en {nombres:?}"
        );
    }
}

#[tokio::test]
async fn importar_no_descarga_audio() {
    // Descargar 500 canciones que quiza no se escuchen nunca contradice que la
    // descarga sea consecuencia de darle a play.
    let c = ctx(0).await;
    c.svc
        .import_from_provider("cualquier-cosa")
        .await
        .expect("importa");

    for _ in 0..100 {
        if !c.svc.list().await.expect("lista").is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    let nombres = c.bus.nombres();
    assert!(
        !nombres.iter().any(|n| n.starts_with("download")),
        "la importacion no debe disparar descargas: {nombres:?}"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Portada
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn la_portada_se_copia_a_la_biblioteca() {
    // Guardar la ruta original dejaria la portada rota en cuanto el usuario
    // moviera el fichero, que puede estar en Descargas o en un USB.
    let c = ctx(0).await;
    let p = c.svc.create("Con portada").await.expect("crea");

    let origen = std::env::temp_dir().join("localify-portada-prueba.png");
    std::fs::write(&origen, b"no es un png de verdad, pero da igual").expect("escribe");

    c.svc.set_cover(&p.id, &origen).await.expect("portada");

    let copiada = c
        .paths
        .library_dir()
        .join("covers")
        .join(format!("playlist-{}.png", p.id.as_uuid()));
    assert!(copiada.exists(), "la imagen deberia estar en la biblioteca");

    // Y sigue estando aunque se borre el original.
    std::fs::remove_file(&origen).expect("borra el original");
    assert!(copiada.exists());
}

#[tokio::test]
async fn una_portada_que_no_es_imagen_se_rechaza() {
    let c = ctx(0).await;
    let p = c.svc.create("Lista").await.expect("crea");

    let origen = std::env::temp_dir().join("localify-portada.txt");
    std::fs::write(&origen, b"texto").expect("escribe");

    assert!(c.svc.set_cover(&p.id, &origen).await.is_err());
    let _ = std::fs::remove_file(&origen);
}

// ─────────────────────────────────────────────────────────────────────────────
// Sugerencias
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn una_playlist_vacia_no_sugiere_nada() {
    // Sin semilla no hay afinidad que calcular; devolver pistas al azar seria
    // peor que no devolver nada.
    let c = ctx(5).await;
    let p = c.svc.create("Vacia").await.expect("crea");
    assert!(
        c.svc
            .suggestions(&p.id, 10)
            .await
            .expect("sugiere")
            .is_empty()
    );
}

#[tokio::test]
async fn las_sugerencias_no_repiten_lo_que_ya_esta_dentro() {
    let c = ctx(5).await;
    let p = c.svc.create("Lista").await.expect("crea");
    c.svc
        .add_tracks(&p.id, &c.pistas, None)
        .await
        .expect("anade");

    let sugeridas = c.svc.suggestions(&p.id, 10).await.expect("sugiere");
    for s in &sugeridas {
        assert!(
            !c.pistas.contains(&s.id),
            "sugirio una pista que ya esta en la playlist"
        );
    }
}

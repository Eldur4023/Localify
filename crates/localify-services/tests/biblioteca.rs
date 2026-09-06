//! El reconciliador de biblioteca, con disco y base de datos reales.
//!
//! Los dos escenarios que reconcilia ocurren de verdad y tienen consecuencias
//! muy distintas:
//!
//! - Borrar una canción desde el explorador debe dejarla en el catálogo, no
//!   borrarla: sus favoritos y su historial son del usuario, no del fichero.
//! - Restaurar una copia vieja de la base de datos debe **recuperar** los
//!   ficheros que ya están en disco, no volver a descargarlos. Es el caso que
//!   justifica toda la identidad dual de ADR-021.

#![allow(clippy::expect_used, clippy::panic)]

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use localify_core::domain::audio::{AudioFormat, DurationMs};
use localify_core::domain::ids::{ArtistId, TrackId};
use localify_core::domain::library::{AudioFileRecord, AudioSource};
use localify_core::domain::track::{ArtistRef, Track};
use localify_core::error::CoreResult;
use localify_core::events::{DomainEvent, EventPublisher};
use localify_core::ports::database::{AudioFileRepository, TrackRepository};
use localify_core::ports::platform::AppPaths;
use localify_core::ports::services::LibraryService;
use localify_core::ports::youtube::{GenericTags, TagWriter};
use localify_db::Pool;
use localify_db::pool::TempDbGuard;
use localify_platform::{LocalifyPaths, RealFileSystem};
use localify_services::{DependenciasBiblioteca, LibraryServiceImpl};

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

/// Etiquetador que devuelve el identificador que se le programe.
///
/// Sirve para probar las dos vías de ADR-021 por separado: con etiqueta legible
/// y sin ella. También sirve a la importación de ficheros propios: cada ruta
/// puede llevar sus propios metadatos genéricos programados, para simular un
/// fichero etiquetado o uno que no lo está.
#[derive(Debug, Default)]
struct EtiquetadorFalso {
    /// `None` simula un fichero cuyo etiquetado falló o se perdió.
    id: std::sync::Mutex<Option<String>>,
    /// Metadatos genéricos por ruta. Una ruta ausente del mapa se comporta
    /// como un fichero sin ninguna etiqueta legible.
    genericos: std::sync::Mutex<std::collections::HashMap<std::path::PathBuf, GenericTags>>,
}

impl EtiquetadorFalso {
    fn con_tags(&self, ruta: &Path, tags: GenericTags) {
        if let Ok(mut g) = self.genericos.lock() {
            g.insert(ruta.to_path_buf(), tags);
        }
    }
}

#[async_trait]
impl TagWriter for EtiquetadorFalso {
    async fn write(&self, _path: &Path, _track: &Track, _cover: Option<&[u8]>) -> CoreResult<()> {
        Ok(())
    }
    async fn read_track_id(&self, _path: &Path) -> CoreResult<Option<String>> {
        Ok(self.id.lock().ok().and_then(|g| g.clone()))
    }
    async fn read_generic_tags(&self, path: &Path) -> CoreResult<GenericTags> {
        Ok(self
            .genericos
            .lock()
            .ok()
            .and_then(|g| g.get(path).cloned())
            .unwrap_or_else(|| GenericTags {
                // `lofty` mide la duración a partir de las propiedades del
                // propio audio, no de las etiquetas: un fichero sin ninguna
                // etiqueta legible sigue teniendo una duración real. Sin este
                // valor por defecto, el doble simularía un fichero que ni
                // siquiera `lofty` podría abrir, que es un caso distinto.
                duration: Some(DurationMs::from_secs(180)),
                ..GenericTags::default()
            }))
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Montaje
// ─────────────────────────────────────────────────────────────────────────────

struct Ctx {
    lib: LibraryServiceImpl,
    pool: Pool,
    tracks: Arc<dyn TrackRepository>,
    audio: Arc<dyn AudioFileRepository>,
    paths: Arc<LocalifyPaths>,
    bus: Arc<BusDePrueba>,
    etiquetador: Arc<EtiquetadorFalso>,
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

async fn ctx() -> Ctx {
    let (pool, guard) = Pool::temporal().expect("abre");
    localify_db::ejecutar_migraciones(&pool)
        .await
        .expect("migra");

    let biblioteca = std::env::temp_dir().join(format!("localify-lib-{}", uuid::Uuid::now_v7()));
    let paths = Arc::new(LocalifyPaths::con_biblioteca(
        biblioteca.join("config"),
        biblioteca.clone(),
    ));
    paths.crear_estructura().expect("crea carpetas");

    let tracks: Arc<dyn TrackRepository> = Arc::new(
        localify_db::repositories::SqliteTrackRepository::new(pool.clone()),
    );
    let audio: Arc<dyn AudioFileRepository> = Arc::new(
        localify_db::repositories::SqliteAudioFileRepository::new(pool.clone()),
    );
    let bus = Arc::new(BusDePrueba::default());
    let etiquetador = Arc::new(EtiquetadorFalso::default());

    let lib = LibraryServiceImpl::nuevo(DependenciasBiblioteca {
        tracks: Arc::clone(&tracks),
        albums: Arc::new(localify_db::repositories::SqliteAlbumRepository::new(
            pool.clone(),
        )),
        artists: Arc::new(localify_db::repositories::SqliteArtistRepository::new(
            pool.clone(),
        )),
        audio: Arc::clone(&audio),
        favoritos: Arc::new(localify_db::repositories::SqliteFavoriteRepository::new(
            pool.clone(),
        )),
        historial: Arc::new(localify_db::repositories::SqliteHistoryRepository::new(
            pool.clone(),
        )),
        estado_repo: Arc::new(localify_db::repositories::SqlitePlayerStateRepository::new(
            pool.clone(),
        )),
        informes: Arc::new(localify_db::repositories::SqliteScanReportRepository::new(
            pool.clone(),
        )),
        tagger: Arc::clone(&etiquetador) as Arc<dyn TagWriter>,
        fs: Arc::new(RealFileSystem::new()),
        paths: Arc::clone(&paths) as Arc<dyn AppPaths>,
        bus: Arc::clone(&bus) as Arc<dyn EventPublisher>,
    });

    Ctx {
        lib,
        pool,
        tracks,
        audio,
        paths,
        bus,
        etiquetador,
        biblioteca,
        _guard: guard,
    }
}

/// Da de alta una pista en el catálogo.
async fn catalogar(c: &Ctx, n: usize, titulo: &str) -> TrackId {
    let t = Track {
        id: id(n),
        title: titulo.into(),
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
    };
    c.tracks
        .upsert(std::slice::from_ref(&t))
        .await
        .expect("guarda");
    t.id
}

/// Escribe el fichero de una pista y devuelve su ruta relativa.
fn escribir_fichero(c: &Ctx, track: &TrackId) -> std::path::PathBuf {
    let prefijo = &track.as_str()[..2];
    let dir = c.paths.audio_dir().join(prefijo);
    std::fs::create_dir_all(&dir).expect("crea carpeta");
    let absoluta = dir.join(format!("{}.opus", track.as_str()));
    std::fs::write(&absoluta, vec![0_u8; 4096]).expect("escribe");

    absoluta
        .strip_prefix(c.paths.library_dir())
        .expect("dentro de la biblioteca")
        .to_path_buf()
}

/// Registra el fichero en la base de datos, como haría una descarga.
async fn registrar(c: &Ctx, track: &TrackId, rel: std::path::PathBuf) {
    c.audio
        .insert(&AudioFileRecord {
            track_id: track.clone(),
            rel_path: rel,
            format: AudioFormat::Opus,
            codec: "opus".into(),
            bitrate_kbps: Some(160),
            sample_rate: Some(48_000),
            channels: Some(2),
            size_bytes: 4096,
            duration: DurationMs::from_secs(180),
            source: AudioSource::Youtube,
            youtube_id: None,
            verified_at: chrono::Utc::now(),
        })
        .await
        .expect("registra");
}

// ─────────────────────────────────────────────────────────────────────────────
// Fila sin fichero
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn borrar_el_fichero_a_mano_deja_la_pista_en_el_catalogo() {
    // Es la parte importante: la pista sigue existiendo, con sus favoritos y su
    // historial. Solo deja de estar en local.
    let c = ctx().await;
    let t = catalogar(&c, 1, "Se borrara").await;
    let rel = escribir_fichero(&c, &t);
    registrar(&c, &t, rel.clone()).await;

    std::fs::remove_file(c.paths.resolve(&rel)).expect("borra el fichero");

    let informe = c.lib.escanear().await.expect("escanea");
    assert_eq!(informe.missing, 1, "deberia detectar el fichero perdido");

    assert!(
        c.audio.get(&t).await.expect("consulta").is_none(),
        "el registro de audio debe irse"
    );
    assert!(
        c.tracks.get(&t).await.expect("consulta").is_some(),
        "la pista NO debe borrarse del catalogo"
    );
}

#[tokio::test]
async fn una_biblioteca_intacta_no_reporta_nada() {
    let c = ctx().await;
    for n in 1..=5 {
        let t = catalogar(&c, n, &format!("Pista {n}")).await;
        let rel = escribir_fichero(&c, &t);
        registrar(&c, &t, rel).await;
    }

    let informe = c.lib.escanear().await.expect("escanea");
    assert_eq!(informe.missing, 0);
    assert_eq!(informe.recovered, 0);
    assert_eq!(informe.unreadable, 0);
}

// ─────────────────────────────────────────────────────────────────────────────
// Fichero sin fila: la recuperación
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn un_fichero_huerfano_se_recupera_por_el_nombre() {
    // El caso de ADR-021: la base de datos perdio el registro pero el fichero
    // sigue en disco con su identificador por nombre. Redescargarlo seria
    // gastar red por algo que ya se tiene.
    let c = ctx().await;
    let t = catalogar(&c, 2, "Huerfana").await;
    escribir_fichero(&c, &t); // fichero en disco, sin registrar en `audio_files`

    let informe = c.lib.escanear().await.expect("escanea");
    assert_eq!(informe.recovered, 1, "deberia recuperarse sin descargar");

    let registro = c.audio.get(&t).await.expect("consulta").expect("existe");
    assert_eq!(registro.source, AudioSource::Imported);
    assert_eq!(
        registro.duration,
        DurationMs::from_secs(180),
        "la duracion se toma del catalogo, no se mide"
    );
}

#[tokio::test]
async fn un_fichero_huerfano_se_recupera_por_la_etiqueta() {
    // La otra via de ADR-021: el fichero se renombro fuera de la app, pero
    // conserva su etiqueta.
    let c = ctx().await;
    let t = catalogar(&c, 3, "Renombrada").await;

    let dir = c.paths.audio_dir().join("xx");
    std::fs::create_dir_all(&dir).expect("crea carpeta");
    std::fs::write(dir.join("nombre cambiado.opus"), vec![0_u8; 2048]).expect("escribe");

    // El etiquetador sabe de quien es.
    *c.etiquetador.id.lock().expect("lock") = Some(t.as_str().to_owned());

    let informe = c.lib.escanear().await.expect("escanea");
    assert_eq!(informe.recovered, 1, "la etiqueta deberia identificarlo");
    assert!(c.audio.get(&t).await.expect("consulta").is_some());
}

#[tokio::test]
async fn un_fichero_sin_identidad_no_ensucia_la_biblioteca() {
    // Alguien copio su musica a mano en `audio/`. Sin identidad no se puede
    // saber que cancion es, e inventarla con el nombre del fichero llenaria el
    // catalogo de basura.
    let c = ctx().await;
    let dir = c.paths.audio_dir().join("zz");
    std::fs::create_dir_all(&dir).expect("crea carpeta");
    std::fs::write(dir.join("mi cancion favorita.mp3"), vec![0_u8; 1024]).expect("escribe");

    let informe = c.lib.escanear().await.expect("escanea");
    assert_eq!(informe.recovered, 0);
    assert_eq!(informe.unreadable, 1, "deberia contarse como ilegible");
    assert_eq!(
        c.lib.stats().await.expect("stats").track_count,
        0,
        "no debe aparecer ninguna pista inventada"
    );
}

#[tokio::test]
async fn un_fichero_de_una_pista_desconocida_no_se_da_de_alta() {
    // El nombre es un identificador valido, pero el catalogo no lo conoce: sin
    // titulo ni artista, darlo de alta dejaria una fila inutil.
    let c = ctx().await;
    let desconocida = id(999);
    let dir = c.paths.audio_dir().join(&desconocida.as_str()[..2]);
    std::fs::create_dir_all(&dir).expect("crea carpeta");
    std::fs::write(
        dir.join(format!("{}.opus", desconocida.as_str())),
        vec![0_u8; 1024],
    )
    .expect("escribe");

    let informe = c.lib.escanear().await.expect("escanea");
    assert_eq!(informe.recovered, 0);
    assert!(c.audio.get(&desconocida).await.expect("consulta").is_none());
}

#[tokio::test]
async fn los_ficheros_que_no_son_audio_se_ignoran() {
    // La carpeta tiene portadas y temporales. Contarlos como ilegibles
    // inflaria el informe y asustaria sin motivo.
    let c = ctx().await;
    let dir = c.paths.audio_dir();
    std::fs::write(dir.join("portada.jpg"), b"x").expect("escribe");
    std::fs::write(dir.join("a-medias.part"), b"x").expect("escribe");

    let informe = c.lib.escanear().await.expect("escanea");
    assert_eq!(informe.files_scanned, 0);
    assert_eq!(informe.unreadable, 0);
}

// ─────────────────────────────────────────────────────────────────────────────
// Concurrencia y avisos
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn dos_escaneos_a_la_vez_no_se_pisan() {
    let c = ctx().await;
    for n in 1..=3 {
        let t = catalogar(&c, n, &format!("P{n}")).await;
        let rel = escribir_fichero(&c, &t);
        registrar(&c, &t, rel).await;
    }

    let (a, b) = tokio::join!(c.lib.escanear(), c.lib.escanear());
    assert!(
        a.is_ok() != b.is_ok(),
        "solo uno de los dos debe prosperar: {a:?} / {b:?}"
    );
}

#[tokio::test]
async fn el_informe_queda_guardado_para_ajustes() {
    let c = ctx().await;
    let t = catalogar(&c, 4, "X").await;
    let rel = escribir_fichero(&c, &t);
    registrar(&c, &t, rel.clone()).await;
    std::fs::remove_file(c.paths.resolve(&rel)).expect("borra");

    c.lib.escanear().await.expect("escanea");

    let guardado = c
        .lib
        .last_scan_report()
        .await
        .expect("consulta")
        .expect("existe");
    assert_eq!(guardado.missing, 1);
}

#[tokio::test]
async fn el_escaneo_avisa_de_que_la_biblioteca_cambio() {
    let c = ctx().await;
    c.lib.escanear().await.expect("escanea");
    assert!(
        c.bus.nombres().iter().any(|n| n == "libraryChanged"),
        "la interfaz necesita saber que refrescar: {:?}",
        c.bus.nombres()
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Consultas
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn marcar_favorito_avisa_y_aparece_en_la_lista() {
    let c = ctx().await;
    let t = catalogar(&c, 5, "Favorita").await;

    c.lib.set_favorite(&t, true).await.expect("marca");
    let pagina = c
        .lib
        .favorites(&localify_core::page::PageRequest::new(0, 50))
        .await
        .expect("lista");

    assert_eq!(pagina.items.len(), 1);
    assert_eq!(pagina.items[0].id, t);
    assert!(c.bus.nombres().iter().any(|n| n == "libraryChanged"));
}

#[tokio::test]
async fn desmarcar_un_favorito_lo_quita() {
    let c = ctx().await;
    let t = catalogar(&c, 6, "Ya no").await;
    c.lib.set_favorite(&t, true).await.expect("marca");
    c.lib.set_favorite(&t, false).await.expect("desmarca");

    let pagina = c
        .lib
        .favorites(&localify_core::page::PageRequest::new(0, 50))
        .await
        .expect("lista");
    assert!(pagina.items.is_empty());
}

#[tokio::test]
async fn una_escucha_se_registra_y_aparece_en_lo_reciente() {
    let c = ctx().await;
    let t = catalogar(&c, 7, "Escuchada").await;

    c.lib
        .record_play(&t, 175_000, true)
        .await
        .expect("registra");

    let recientes = c.lib.recent(10).await.expect("consulta");
    assert_eq!(recientes.len(), 1);
    assert_eq!(recientes[0].id, t);
}

#[tokio::test]
async fn pasar_de_largo_por_una_pista_no_cuenta_como_escucharla() {
    // Saltar entre canciones generaria una escucha por pista y envenenaria el
    // historial y las recomendaciones.
    let c = ctx().await;
    let t = catalogar(&c, 8, "Saltada").await;

    c.lib.record_play(&t, 300, false).await.expect("registra");

    assert!(
        c.lib.recent(10).await.expect("consulta").is_empty(),
        "menos de un segundo no es una escucha"
    );
}

#[tokio::test]
async fn las_estadisticas_cuentan_lo_que_hay() {
    let c = ctx().await;
    for n in 10..15 {
        let t = catalogar(&c, n, &format!("E{n}")).await;
        let rel = escribir_fichero(&c, &t);
        registrar(&c, &t, rel).await;
    }
    // Una mas sin fichero: esta en el catalogo pero no en local.
    catalogar(&c, 20, "Sin fichero").await;

    let stats = c.lib.stats().await.expect("stats");
    assert_eq!(stats.track_count, 6);
    assert_eq!(stats.local_count, 5, "solo cinco tienen fichero");
}

#[tokio::test]
async fn un_album_que_no_existe_da_no_encontrado() {
    let c = ctx().await;
    let error = c
        .lib
        .album_detail(&localify_core::domain::ids::AlbumId::nuevo_local())
        .await
        .expect_err("deberia fallar");
    assert_eq!(error.code(), "NOT_FOUND");
}

#[tokio::test]
async fn vaciar_las_descargas_tambien_vacia_el_historial() {
    // Inicio se construye entero a partir del historial. Sin este borrado, la
    // pantalla seguía enseñando "sigue escuchando" con canciones cuyos ficheros
    // acababan de irse, en secciones que ya no llevaban a ninguna parte.
    let c = ctx().await;
    let t = catalogar(&c, 1, "Una").await;
    let rel = escribir_fichero(&c, &t);
    registrar(&c, &t, rel).await;

    let historial: Arc<dyn localify_core::ports::database::HistoryRepository> = Arc::new(
        localify_db::repositories::SqliteHistoryRepository::new(c.pool.clone()),
    );
    historial
        .record(&localify_core::domain::library::PlayHistoryEntry {
            track_id: t.clone(),
            played_at: chrono::Utc::now(),
            ms_played: 180_000,
            completed: true,
            context: None,
        })
        .await
        .expect("registra escucha");

    c.lib.wipe_downloads().await.expect("vacía");

    assert!(
        historial
            .recent_tracks(10)
            .await
            .expect("consulta")
            .is_empty(),
        "el historial se va con las descargas"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Importar ficheros propios
// ─────────────────────────────────────────────────────────────────────────────

/// Escribe un fichero **fuera** de la biblioteca, como si el usuario lo
/// hubiera elegido en el selector nativo.
fn escribir_fichero_externo(nombre: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("localify-import-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(&dir).expect("crea carpeta");
    let ruta = dir.join(nombre);
    std::fs::write(&ruta, vec![0_u8; 256]).expect("escribe");
    ruta
}

#[tokio::test]
async fn importar_un_fichero_con_etiquetas_crea_track_artista_y_album() {
    let c = ctx().await;
    let origen = escribir_fichero_externo("cancion.mp3");
    c.etiquetador.con_tags(
        &origen,
        GenericTags {
            title: Some("Bohemian Rhapsody".into()),
            artist: Some("Queen".into()),
            album: Some("A Night at the Opera".into()),
            track_number: Some(11),
            duration: Some(DurationMs::from_secs(355)),
        },
    );

    let informe = c
        .lib
        .import_files(vec![origen.clone()])
        .await
        .expect("importa");
    assert_eq!(informe.files_selected, 1);
    assert_eq!(informe.imported, 1);
    assert_eq!(informe.skipped_unreadable, 0);

    let pagina = c
        .audio
        .list_all(&localify_core::page::PageRequest::new(0, 10))
        .await
        .expect("lista");
    assert_eq!(pagina.items.len(), 1);
    let registro = &pagina.items[0];
    assert_eq!(registro.source, AudioSource::Imported);

    let pista = c
        .tracks
        .get(&registro.track_id)
        .await
        .expect("consulta")
        .expect("existe");
    assert_eq!(pista.title, "Bohemian Rhapsody");
    assert_eq!(pista.artist_display(), "Queen");
    assert_eq!(
        pista.album.as_ref().map(|a| a.title.clone()),
        Some("A Night at the Opera".to_owned())
    );
    assert_eq!(pista.track_number, Some(11));

    let _ = std::fs::remove_dir_all(origen.parent().expect("tiene carpeta"));
}

#[tokio::test]
async fn importar_dos_pistas_del_mismo_album_reutiliza_un_solo_album() {
    let c = ctx().await;
    let una = escribir_fichero_externo("una.mp3");
    let otra = escribir_fichero_externo("otra.mp3");
    for (ruta, titulo, numero) in [(&una, "Una", 1_u16), (&otra, "Otra", 2_u16)] {
        c.etiquetador.con_tags(
            ruta,
            GenericTags {
                title: Some(titulo.into()),
                artist: Some("Radiohead".into()),
                album: Some("OK Computer".into()),
                track_number: Some(numero),
                duration: Some(DurationMs::from_secs(200)),
            },
        );
    }

    let informe = c
        .lib
        .import_files(vec![una.clone(), otra.clone()])
        .await
        .expect("importa");
    assert_eq!(informe.imported, 2);

    let pagina = c
        .audio
        .list_all(&localify_core::page::PageRequest::new(0, 10))
        .await
        .expect("lista");
    assert_eq!(pagina.items.len(), 2);

    let mut album_ids = std::collections::HashSet::new();
    for registro in &pagina.items {
        let pista = c
            .tracks
            .get(&registro.track_id)
            .await
            .expect("consulta")
            .expect("existe");
        album_ids.insert(pista.album.expect("tiene álbum").id);
    }
    assert_eq!(
        album_ids.len(),
        1,
        "las dos pistas del mismo álbum no deben mintar dos álbumes distintos"
    );

    let _ = std::fs::remove_dir_all(una.parent().expect("tiene carpeta"));
    let _ = std::fs::remove_dir_all(otra.parent().expect("tiene carpeta"));
}

#[tokio::test]
async fn importar_un_fichero_sin_etiquetas_usa_el_nombre_de_fichero() {
    let c = ctx().await;
    // Sin `con_tags`: simula un fichero sin ninguna etiqueta legible.
    let origen = escribir_fichero_externo("Mi Cancion Favorita.mp3");

    let informe = c
        .lib
        .import_files(vec![origen.clone()])
        .await
        .expect("importa");
    assert_eq!(informe.imported, 1);

    let pagina = c
        .audio
        .list_all(&localify_core::page::PageRequest::new(0, 10))
        .await
        .expect("lista");
    let pista = c
        .tracks
        .get(&pagina.items[0].track_id)
        .await
        .expect("consulta")
        .expect("existe");
    assert_eq!(pista.title, "Mi Cancion Favorita");
    assert!(pista.artists.is_empty());
    assert!(pista.album.is_none());

    let _ = std::fs::remove_dir_all(origen.parent().expect("tiene carpeta"));
}

#[tokio::test]
async fn una_extension_no_reconocida_se_cuenta_y_no_aborta_el_lote() {
    let c = ctx().await;
    let buena = escribir_fichero_externo("buena.mp3");
    let mala = escribir_fichero_externo("notas.txt");

    let informe = c
        .lib
        .import_files(vec![buena.clone(), mala.clone()])
        .await
        .expect("importa");
    assert_eq!(informe.files_selected, 2);
    assert_eq!(informe.imported, 1);
    assert_eq!(informe.skipped_unreadable, 1);

    let _ = std::fs::remove_dir_all(buena.parent().expect("tiene carpeta"));
    let _ = std::fs::remove_dir_all(mala.parent().expect("tiene carpeta"));
}

#[tokio::test]
async fn el_fichero_original_no_se_toca_al_importar() {
    let c = ctx().await;
    let origen = escribir_fichero_externo("original.mp3");

    c.lib
        .import_files(vec![origen.clone()])
        .await
        .expect("importa");

    assert!(origen.exists(), "el fichero del usuario no debe borrarse");
    assert_eq!(
        std::fs::metadata(&origen).expect("metadatos").len(),
        256,
        "el fichero del usuario no debe modificarse"
    );

    let _ = std::fs::remove_dir_all(origen.parent().expect("tiene carpeta"));
}

#[tokio::test]
async fn un_rescan_tras_importar_no_marca_la_pista_como_huerfana() {
    let c = ctx().await;
    let origen = escribir_fichero_externo("importada.mp3");
    c.etiquetador.con_tags(
        &origen,
        GenericTags {
            title: Some("Importada".into()),
            duration: Some(DurationMs::from_secs(180)),
            ..GenericTags::default()
        },
    );

    c.lib
        .import_files(vec![origen.clone()])
        .await
        .expect("importa");

    let informe = c.lib.escanear().await.expect("escanea");
    assert_eq!(
        informe.missing, 0,
        "el fichero recién importado no debe darse por perdido"
    );
    assert_eq!(
        informe.recovered, 0,
        "ya estaba registrado: no es un huérfano que recuperar"
    );

    let pagina = c
        .audio
        .list_all(&localify_core::page::PageRequest::new(0, 10))
        .await
        .expect("lista");
    assert_eq!(pagina.items.len(), 1, "sigue habiendo exactamente un fichero");

    let _ = std::fs::remove_dir_all(origen.parent().expect("tiene carpeta"));
}

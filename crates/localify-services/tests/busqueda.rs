//! Flujo de búsqueda de extremo a extremo.
//!
//! Usa repositorios reales sobre una base de datos temporal y un proveedor de
//! Spotify con respuestas preparadas. **Sin red**: es lo que permite que la
//! suite corra en milisegundos y de forma determinista.

#![allow(clippy::expect_used, clippy::panic)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use localify_core::domain::audio::DurationMs;
use localify_core::domain::ids::TrackId;
use localify_core::domain::track::{ArtistRef, Track};
use localify_core::events::{DomainEvent, EventPublisher};
use localify_core::page::PageRequest;
use localify_core::ports::database::TrackRepository;
use localify_core::ports::metadata_provider::MetadataProvider;
use localify_core::ports::services::{RemoteResults, SearchScope, SearchService};
use localify_db::Pool;
use localify_db::pool::TempDbGuard;
use localify_db::repositories::{SqliteSearchRepository, SqliteTrackRepository};
use localify_services::metadata::MetadataServiceImpl;
use localify_services::search::SearchServiceImpl;
use localify_spotify::Credenciales;
use localify_spotify::provider::SpotifyProvider;
use localify_spotify::transporte::falso::TransporteFalso;

/// Publicador que acumula lo emitido, para poder afirmar sobre los eventos.
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

    fn query_ids(&self) -> Vec<u64> {
        self.eventos
            .lock()
            .map(|e| {
                e.iter()
                    .filter_map(|x| match x {
                        DomainEvent::SearchRemoteReady { query_id } => Some(*query_id),
                        _ => None,
                    })
                    .collect()
            })
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

struct Ctx {
    servicio: SearchServiceImpl,
    tracks: Arc<dyn TrackRepository>,
    bus: Arc<BusDePrueba>,
    _guard: TempDbGuard,
}

fn token() -> String {
    r#"{"access_token":"tok","token_type":"Bearer","expires_in":3600}"#.to_owned()
}

/// Respuesta de búsqueda con `n` pistas.
fn respuesta_busqueda(n: usize) -> String {
    let items: Vec<String> = (0..n)
        .map(|i| {
            format!(
                r#"{{"id":"remota{i:016}","name":"Remota {i}","duration_ms":200000,
                     "artists":[{{"id":"art{i:019}","name":"Artista Remoto"}}]}}"#
            )
        })
        .collect();
    format!(
        r#"{{"tracks":{{"items":[{}],"total":{n}}}}}"#,
        items.join(",")
    )
}

async fn ctx(transporte: Arc<TransporteFalso>, con_credenciales: bool) -> Ctx {
    let (pool, guard) = Pool::temporal().expect("abre");
    localify_db::ejecutar_migraciones(&pool)
        .await
        .expect("migra");

    let tracks: Arc<dyn TrackRepository> = Arc::new(SqliteTrackRepository::new(pool.clone()));
    let search = Arc::new(SqliteSearchRepository::new(pool.clone()));
    let albums = Arc::new(localify_db::repositories::SqliteAlbumRepository::new(
        pool.clone(),
    ));
    let artists = Arc::new(localify_db::repositories::SqliteArtistRepository::new(pool));

    let provider = Arc::new(SpotifyProvider::nuevo(transporte));
    if con_credenciales {
        provider
            .set_credenciales(Some(Credenciales {
                client_id: "id".into(),
                client_secret: "secreto".into(),
            }))
            .await;
    }

    let bus = Arc::new(BusDePrueba::default());
    let provider_dyn: Arc<dyn MetadataProvider> = provider;

    // Carpetas temporales: el servicio las pide para saber dónde cachear
    // portadas, aunque este test no descargue ninguna.
    let raiz = std::env::temp_dir().join(format!("localify-bus-{}", uuid::Uuid::now_v7()));
    let paths: Arc<dyn localify_core::ports::platform::AppPaths> = Arc::new(
        localify_platform::LocalifyPaths::con_biblioteca(raiz.join("config"), raiz),
    );

    let metadata = Arc::new(MetadataServiceImpl::nuevo(
        Arc::clone(&provider_dyn),
        Arc::clone(&tracks),
        albums,
        artists,
        Arc::clone(&bus) as Arc<dyn EventPublisher>,
        // Sin descargador de imágenes: este test no mira portadas.
        None,
        paths,
    ));

    let servicio = SearchServiceImpl::nuevo(
        search,
        Arc::clone(&tracks),
        provider_dyn,
        metadata,
        Arc::clone(&bus) as Arc<dyn EventPublisher>,
    );

    Ctx {
        servicio,
        tracks,
        bus,
        _guard: guard,
    }
}

fn pista_local(titulo: &str, artista: &str) -> Track {
    Track {
        id: TrackId::nuevo_local(),
        title: titulo.to_owned(),
        album: None,
        artists: vec![ArtistRef {
            id: localify_core::domain::ids::ArtistId::nuevo_local(),
            name: artista.to_owned(),
        }],
        duration: DurationMs::new(200_000),
        track_number: None,
        disc_number: None,
        explicit: false,
        isrc: None,
        release_date: None,
        popularity: None,
        added_at: chrono::Utc::now(),
    }
}

/// Espera a que el bus reciba un `searchRemoteReady`, con tope.
async fn esperar_aviso(bus: &BusDePrueba) -> bool {
    for _ in 0..100 {
        if !bus.query_ids().is_empty() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    false
}

#[tokio::test]
async fn lo_local_se_devuelve_de_inmediato_sin_esperar_al_proveedor() {
    let transporte = Arc::new(
        TransporteFalso::nuevo()
            .con_json(&token())
            .con_json(&respuesta_busqueda(20)),
    );
    let c = ctx(transporte, true).await;

    c.tracks
        .upsert(&[pista_local("Bohemian Rhapsody", "Queen")])
        .await
        .expect("guarda");

    let r = c
        .servicio
        .search("bohemian", SearchScope::All, &PageRequest::new(0, 50))
        .await
        .expect("busca");

    assert_eq!(r.tracks.len(), 1, "lo local llega en la misma respuesta");
    assert_eq!(r.tracks[0].principal.title, "Bohemian Rhapsody");
    assert!(
        matches!(r.remote, RemoteResults::Loading),
        "con pocos resultados locales debe consultarse al proveedor"
    );
}

#[tokio::test]
async fn tener_muchos_resultados_guardados_no_evita_preguntar_al_proveedor() {
    // Hubo un atajo: con ocho coincidencias locales se daba por buena la
    // respuesta y no se salia a la red. El razonamiento era que quien busca
    // algo que ya tiene esta buscando lo suyo.
    //
    // Dejo de ser cierto en cuanto cada busqueda empezo a persistir sus
    // resultados: esas coincidencias son lo que contesto el proveedor la vez
    // anterior, no una biblioteca del usuario. Con el atajo, buscar dos veces
    // lo mismo enseñaba para siempre la respuesta vieja y no se volvia a
    // preguntar nunca.
    let transporte = Arc::new(TransporteFalso::nuevo());
    let c = ctx(Arc::clone(&transporte), true).await;

    let pistas: Vec<Track> = (0..10)
        .map(|i| pista_local(&format!("Cancion {i}"), "Artista"))
        .collect();
    c.tracks.upsert(&pistas).await.expect("guarda");

    let r = c
        .servicio
        .search("cancion", SearchScope::All, &PageRequest::new(0, 50))
        .await
        .expect("busca");

    assert_eq!(r.tracks.len(), 10, "lo guardado se devuelve al instante");
    assert!(
        matches!(r.remote, RemoteResults::Loading),
        "y aun asi hay que volver a preguntar"
    );
}

#[tokio::test]
async fn los_resultados_remotos_se_persisten_antes_de_avisar() {
    let transporte = Arc::new(
        TransporteFalso::nuevo()
            .con_json(&token())
            .con_json(&respuesta_busqueda(5)),
    );
    let c = ctx(transporte, true).await;

    let r = c
        .servicio
        .search("remota", SearchScope::All, &PageRequest::new(0, 50))
        .await
        .expect("busca");

    assert!(r.tracks.is_empty());
    assert!(matches!(r.remote, RemoteResults::Loading));

    assert!(esperar_aviso(&c.bus).await, "debe llegar searchRemoteReady");

    // Al recibir el aviso, el cliente repite la consulta local: los resultados
    // remotos ya tienen que estar en la base de datos.
    let segunda = c
        .servicio
        .search("remota", SearchScope::All, &PageRequest::new(0, 50))
        .await
        .expect("busca");

    assert_eq!(
        segunda.tracks.len(),
        5,
        "persistir antes de avisar es lo que hace que baste con repetir la consulta local"
    );
}

#[tokio::test]
async fn el_aviso_lleva_el_mismo_identificador_que_recibio_el_cliente() {
    let transporte = Arc::new(
        TransporteFalso::nuevo()
            .con_json(&token())
            .con_json(&respuesta_busqueda(3)),
    );
    let c = ctx(transporte, true).await;

    let r = c
        .servicio
        .search("remota", SearchScope::All, &PageRequest::new(0, 50))
        .await
        .expect("busca");

    assert!(esperar_aviso(&c.bus).await);
    assert_eq!(
        c.bus.query_ids(),
        vec![r.query_id],
        "sin esto el cliente no podría descartar respuestas de pulsaciones viejas"
    );
}

#[tokio::test]
async fn el_identificador_de_consulta_es_monotono() {
    let transporte = Arc::new(TransporteFalso::nuevo());
    let c = ctx(transporte, false).await;

    let mut anterior = 0;
    for _ in 0..5 {
        let r = c
            .servicio
            .search("algo", SearchScope::All, &PageRequest::new(0, 10))
            .await
            .expect("busca");
        assert!(r.query_id > anterior);
        anterior = r.query_id;
    }
}

#[tokio::test]
async fn sin_credenciales_la_busqueda_local_sigue_funcionando() {
    let transporte = Arc::new(TransporteFalso::nuevo());
    let c = ctx(Arc::clone(&transporte), false).await;

    c.tracks
        .upsert(&[pista_local("Bohemian Rhapsody", "Queen")])
        .await
        .expect("guarda");

    let r = c
        .servicio
        .search("bohemian", SearchScope::All, &PageRequest::new(0, 50))
        .await
        .expect("busca");

    assert_eq!(
        r.tracks.len(),
        1,
        "la biblioteca local no depende de Spotify"
    );
    match r.remote {
        RemoteResults::Unavailable { reason_key } => {
            assert_eq!(reason_key, "provider.not_configured");
        }
        otro => panic!("se esperaba Unavailable, llegó {otro:?}"),
    }
    assert_eq!(transporte.cuantas(), 0);
}

#[tokio::test]
async fn un_fallo_del_proveedor_no_rompe_la_busqueda() {
    let transporte = Arc::new(
        TransporteFalso::nuevo()
            .con_json(&token())
            .con_estado(500, None)
            .con_estado(500, None)
            .con_estado(500, None),
    );
    let c = ctx(transporte, true).await;

    c.tracks
        .upsert(&[pista_local("Algo Local", "Artista")])
        .await
        .expect("guarda");

    let r = c
        .servicio
        .search("algo", SearchScope::All, &PageRequest::new(0, 50))
        .await
        .expect("la búsqueda no debe fallar por el proveedor");

    assert_eq!(r.tracks.len(), 1);
    assert!(matches!(r.remote, RemoteResults::Loading));

    // La tarea de fondo falla en silencio: el cliente se queda con lo local.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        !c.bus.nombres().contains(&"toast".to_owned()),
        "un fallo por pulsación no debe generar avisos al usuario"
    );
}

#[tokio::test]
async fn una_consulta_en_blanco_no_consulta_nada() {
    let transporte = Arc::new(TransporteFalso::nuevo());
    let c = ctx(Arc::clone(&transporte), true).await;

    for entrada in ["", "   ", "!!!", "---"] {
        let r = c
            .servicio
            .search(entrada, SearchScope::All, &PageRequest::new(0, 50))
            .await
            .expect("no falla");
        assert!(r.tracks.is_empty(), "'{entrada}'");
        assert!(
            matches!(r.remote, RemoteResults::NotAttempted),
            "'{entrada}'"
        );
    }
    assert_eq!(transporte.cuantas(), 0);
}

#[tokio::test]
async fn las_sugerencias_son_locales_y_no_repiten() {
    let transporte = Arc::new(TransporteFalso::nuevo());
    let c = ctx(Arc::clone(&transporte), true).await;

    c.tracks
        .upsert(&[
            pista_local("Bohemian Rhapsody", "Queen"),
            pista_local("Bohemian Rhapsody", "Otro"),
            pista_local("Bohemian Like You", "Dandy Warhols"),
        ])
        .await
        .expect("guarda");

    let sugerencias = c.servicio.suggest("bohem", 10).await.expect("sugiere");

    assert_eq!(
        sugerencias.len(),
        2,
        "dos versiones del mismo título son una sola sugerencia"
    );
    assert_eq!(
        transporte.cuantas(),
        0,
        "una petición de red por pulsación sería insostenible"
    );
}

#[tokio::test]
async fn el_ambito_de_pistas_no_consulta_albumes_ni_artistas() {
    let transporte = Arc::new(TransporteFalso::nuevo());
    let c = ctx(transporte, false).await;

    c.tracks
        .upsert(&[pista_local("Bohemian Rhapsody", "Queen")])
        .await
        .expect("guarda");

    let r = c
        .servicio
        .search("queen", SearchScope::Tracks, &PageRequest::new(0, 50))
        .await
        .expect("busca");

    assert_eq!(r.tracks.len(), 1);
    assert!(r.artists.is_empty(), "el ámbito acotado ahorra trabajo");
    assert!(r.albums.is_empty());
}

//! La semántica de la cola, que es la que el usuario nota.
//!
//! Cada test de aquí corresponde a un comportamiento concreto de Spotify que
//! Localify tiene que reproducir. Son las reglas que, si se rompen, hacen que
//! el reproductor "se sienta raro" sin que nadie sepa decir por qué.

#![allow(clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use localify_core::domain::ids::{AlbumId, ArtistId, TrackId};
use localify_core::domain::queue::{AdvanceReason, PlaybackContext, RepeatMode};
use localify_core::domain::track::{ArtistRef, Track};
use localify_core::events::{DomainEvent, EventPublisher};
use localify_core::ports::database::TrackRepository;
use localify_core::ports::services::QueueService;
use localify_db::Pool;
use localify_db::pool::TempDbGuard;
use localify_services::actors::{DependenciasCola, QueueActor};

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
    fn revisiones(&self) -> Vec<u64> {
        self.0
            .lock()
            .map(|v| {
                v.iter()
                    .filter_map(|e| match e {
                        DomainEvent::QueueChanged { revision } => Some(*revision),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}

struct Ctx {
    cola: QueueActor,
    bus: Arc<BusDePrueba>,
    pistas: Vec<TrackId>,
    _guard: TempDbGuard,
}

/// Identificadores válidos de Spotify: 22 caracteres en base62.
///
/// Escribe `n` en base 62 y rellena. Tiene que ser **inyectivo**: con una
/// función que colisione, un test que comprueba "esta pista no está en el
/// contexto" acaba comprobando lo contrario sin que nadie se entere.
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

async fn ctx(cuantas: usize) -> Ctx {
    let (pool, guard) = Pool::temporal().expect("abre");
    localify_db::ejecutar_migraciones(&pool)
        .await
        .expect("migra");

    let tracks: Arc<dyn TrackRepository> = Arc::new(
        localify_db::repositories::SqliteTrackRepository::new(pool.clone()),
    );

    let pistas: Vec<TrackId> = (0..cuantas).map(id).collect();
    let filas: Vec<Track> = pistas
        .iter()
        .enumerate()
        .map(|(i, t)| Track {
            id: t.clone(),
            title: format!("Pista {i}"),
            album: None,
            artists: vec![ArtistRef {
                id: ArtistId::nuevo_local(),
                name: "Artista".into(),
            }],
            duration: localify_core::domain::audio::DurationMs::from_secs(180),
            track_number: None,
            disc_number: None,
            explicit: false,
            isrc: None,
            release_date: None,
            popularity: None,
            added_at: chrono::Utc::now(),
        })
        .collect();
    tracks.upsert(&filas).await.expect("guarda");

    let bus = Arc::new(BusDePrueba::default());
    let cola = QueueActor::nuevo(DependenciasCola {
        tracks,
        bus: Arc::clone(&bus) as Arc<dyn EventPublisher>,
    });

    Ctx {
        cola,
        bus,
        pistas,
        _guard: guard,
    }
}

/// Instala un contexto de biblioteca con las pistas dadas.
async fn con_contexto(c: &Ctx, empezar_en: usize) {
    c.cola
        .set_context(PlaybackContext::Library, empezar_en)
        .await
        .expect("contexto");
    c.cola.poner_pistas(c.pistas.clone(), empezar_en);
}

// ─────────────────────────────────────────────────────────────────────────────
// Las dos colas
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn la_cola_de_usuario_tiene_prioridad_sobre_el_contexto() {
    let c = ctx(10).await;
    con_contexto(&c, 0).await;

    c.cola
        .add_next(std::slice::from_ref(&c.pistas[7]))
        .await
        .expect("encola");

    let siguiente = c
        .cola
        .advance(AdvanceReason::UserSkip)
        .await
        .expect("avanza");
    assert_eq!(
        siguiente,
        Some(c.pistas[7].clone()),
        "lo puesto a continuacion debe sonar antes que la siguiente del album"
    );
}

#[tokio::test]
async fn la_cola_de_usuario_sobrevive_a_un_cambio_de_contexto() {
    // Es la diferencia entre las dos colas. Vaciarla al abrir otro album seria,
    // para el usuario, perder algo que habia puesto a mano.
    let c = ctx(10).await;
    con_contexto(&c, 0).await;

    c.cola
        .add_last(std::slice::from_ref(&c.pistas[9]))
        .await
        .expect("encola");

    c.cola
        .set_context(
            PlaybackContext::Album {
                id: AlbumId::nuevo_local(),
            },
            0,
        )
        .await
        .expect("otro contexto");

    let snapshot = c.cola.snapshot().await;
    assert_eq!(
        snapshot.user_queue.len(),
        1,
        "la cola de usuario se perdio al cambiar de contexto"
    );
}

#[tokio::test]
async fn anadir_varias_a_continuacion_conserva_su_orden() {
    // `add_next` inserta al frente: hacerlo ingenuamente invertiria el orden.
    let c = ctx(10).await;
    con_contexto(&c, 0).await;

    c.cola
        .add_next(&[c.pistas[3].clone(), c.pistas[4].clone()])
        .await
        .expect("encola");

    assert_eq!(
        c.cola.advance(AdvanceReason::UserSkip).await.expect("a"),
        Some(c.pistas[3].clone())
    );
    assert_eq!(
        c.cola.advance(AdvanceReason::UserSkip).await.expect("b"),
        Some(c.pistas[4].clone())
    );
}

#[tokio::test]
async fn una_entrada_se_quita_sin_afectar_a_su_gemela() {
    // La misma cancion puede estar dos veces; el identificador es de la
    // entrada, no de la pista.
    let c = ctx(10).await;
    con_contexto(&c, 0).await;

    c.cola
        .add_last(&[c.pistas[5].clone(), c.pistas[5].clone()])
        .await
        .expect("encola");

    let snapshot = c.cola.snapshot().await;
    assert_eq!(snapshot.user_queue.len(), 2);
    let primera = snapshot.user_queue[0].entry_id;

    c.cola.remove(primera).await.expect("quita");
    assert_eq!(
        c.cola.snapshot().await.user_queue.len(),
        1,
        "quitar una entrada no debe llevarse a su gemela"
    );
}

#[tokio::test]
async fn una_entrada_se_puede_reordenar() {
    let c = ctx(10).await;
    con_contexto(&c, 0).await;
    c.cola
        .add_last(&[
            c.pistas[1].clone(),
            c.pistas[2].clone(),
            c.pistas[3].clone(),
        ])
        .await
        .expect("encola");

    let snapshot = c.cola.snapshot().await;
    let ultima = snapshot.user_queue[2].entry_id;
    c.cola.move_entry(ultima, 0).await.expect("mueve");

    let despues = c.cola.snapshot().await;
    assert_eq!(despues.user_queue[0].track.id, c.pistas[3]);
}

#[tokio::test]
async fn mover_a_un_indice_imposible_no_entra_en_panico() {
    let c = ctx(10).await;
    c.cola
        .add_last(std::slice::from_ref(&c.pistas[1]))
        .await
        .expect("encola");
    let e = c.cola.snapshot().await.user_queue[0].entry_id;
    c.cola.move_entry(e, 9999).await.expect("mueve");
    assert_eq!(c.cola.snapshot().await.user_queue.len(), 1);
}

// ─────────────────────────────────────────────────────────────────────────────
// Aleatorio
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn activar_el_aleatorio_no_cambia_de_cancion() {
    // Es lo que nadie espera al pulsar ese boton, y lo mas facil de romper.
    let c = ctx(50).await;
    con_contexto(&c, 20).await;
    let sonando = c.cola.actual().expect("hay pista");

    c.cola.set_shuffle(true).await.expect("aleatorio");

    assert_eq!(
        c.cola.actual(),
        Some(sonando),
        "activar el aleatorio salto de cancion"
    );
}

#[tokio::test]
async fn desactivar_el_aleatorio_recupera_el_orden_sin_cambiar_de_cancion() {
    let c = ctx(50).await;
    con_contexto(&c, 0).await;
    c.cola.set_shuffle(true).await.expect("activa");

    // Se avanza un poco para no quedarse en la primera.
    for _ in 0..5 {
        c.cola
            .advance(AdvanceReason::UserSkip)
            .await
            .expect("avanza");
    }
    let sonando = c.cola.actual().expect("hay pista");

    c.cola.set_shuffle(false).await.expect("desactiva");
    assert_eq!(c.cola.actual(), Some(sonando.clone()));

    // Y lo siguiente ya es el orden natural.
    let posicion = c.pistas.iter().position(|t| *t == sonando).expect("esta");
    let siguiente = c.cola.peek_next().await.expect("peek");
    assert_eq!(
        siguiente,
        c.pistas.get(posicion + 1).cloned(),
        "tras desactivar el aleatorio deberia seguir el orden del album"
    );
}

#[tokio::test]
async fn el_aleatorio_recorre_todas_las_canciones_sin_repetir() {
    // Sortear en cada avance repetiria canciones antes de haber sonado todas.
    let c = ctx(30).await;
    con_contexto(&c, 0).await;
    c.cola.set_shuffle(true).await.expect("aleatorio");

    let mut vistas = vec![c.cola.actual().expect("hay pista")];
    for _ in 1..30 {
        let Some(t) = c
            .cola
            .advance(AdvanceReason::NaturalEnd)
            .await
            .expect("avanza")
        else {
            break;
        };
        vistas.push(t);
    }

    let unicas: std::collections::HashSet<_> = vistas.iter().collect();
    assert_eq!(
        unicas.len(),
        30,
        "se repitio alguna cancion antes de sonar todas: {} unicas de {}",
        unicas.len(),
        vistas.len()
    );
}

#[tokio::test]
async fn el_aleatorio_deja_ir_hacia_atras() {
    // Sortear en cada avance haria imposible el boton de "anterior".
    let c = ctx(30).await;
    con_contexto(&c, 0).await;
    c.cola.set_shuffle(true).await.expect("aleatorio");

    let primera = c.cola.actual().expect("hay pista");
    let segunda = c
        .cola
        .advance(AdvanceReason::UserSkip)
        .await
        .expect("avanza")
        .expect("hay siguiente");
    assert_ne!(primera, segunda);

    let vuelta = c.cola.go_back().await.expect("atras");
    assert_eq!(vuelta, Some(primera), "'anterior' no volvio a la que sono");
}

// ─────────────────────────────────────────────────────────────────────────────
// Repetición
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn repetir_pista_no_avanza_al_terminar_sola() {
    let c = ctx(10).await;
    con_contexto(&c, 3).await;
    c.cola.set_repeat(RepeatMode::Track).await.expect("modo");

    let actual = c.cola.actual();
    let siguiente = c
        .cola
        .advance(AdvanceReason::NaturalEnd)
        .await
        .expect("avanza");
    assert_eq!(siguiente, actual, "deberia repetirse la misma");
}

#[tokio::test]
async fn repetir_pista_si_avanza_si_el_usuario_pulsa_siguiente() {
    // Es el comportamiento de Spotify: repetir no bloquea el boton.
    let c = ctx(10).await;
    con_contexto(&c, 3).await;
    c.cola.set_repeat(RepeatMode::Track).await.expect("modo");

    let actual = c.cola.actual();
    let siguiente = c
        .cola
        .advance(AdvanceReason::UserSkip)
        .await
        .expect("avanza");
    assert_ne!(siguiente, actual, "un salto manual debe avanzar de verdad");
}

#[tokio::test]
async fn repetir_cola_vuelve_al_principio_al_acabar() {
    let c = ctx(5).await;
    con_contexto(&c, 4).await;
    c.cola.set_repeat(RepeatMode::Queue).await.expect("modo");

    let siguiente = c
        .cola
        .advance(AdvanceReason::NaturalEnd)
        .await
        .expect("avanza");
    assert_eq!(siguiente, Some(c.pistas[0].clone()));
}

#[tokio::test]
async fn sin_repeticion_la_cola_se_acaba() {
    let c = ctx(5).await;
    con_contexto(&c, 4).await;
    let siguiente = c
        .cola
        .advance(AdvanceReason::NaturalEnd)
        .await
        .expect("avanza");
    assert_eq!(siguiente, None, "sin repeticion, el final es el final");
}

// ─────────────────────────────────────────────────────────────────────────────
// Precarga y restauración
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn peek_no_consume_la_cola_de_usuario() {
    // `peek_next` sirve para precargar y preparar el fundido: si consumiera,
    // la cancion se saltaria sin sonar.
    let c = ctx(10).await;
    con_contexto(&c, 0).await;
    c.cola
        .add_next(std::slice::from_ref(&c.pistas[8]))
        .await
        .expect("encola");

    assert_eq!(
        c.cola.peek_next().await.expect("peek"),
        Some(c.pistas[8].clone())
    );
    assert_eq!(
        c.cola.peek_next().await.expect("peek"),
        Some(c.pistas[8].clone()),
        "mirar dos veces no debe consumir nada"
    );
    assert_eq!(c.cola.snapshot().await.user_queue.len(), 1);
}

#[tokio::test]
async fn restaurar_reproduce_la_sucesion_entera_no_solo_la_siguiente() {
    // El test de al lado comprobaba una sola posicion, y por eso el fallo tardo
    // en salir: la permutacion restaurada solo se desviaba en dos sitios, asi
    // que mirar un punto acertaba cinco de cada seis veces y parecia ruido.
    //
    // Comparando el recorrido completo, cualquier desvio cae a la primera.
    let c = ctx(40).await;
    con_contexto(&c, 0).await;
    c.cola.set_shuffle(true).await.expect("aleatorio");
    for _ in 0..6 {
        c.cola
            .advance(AdvanceReason::NaturalEnd)
            .await
            .expect("avanza");
    }

    let sonando = c.cola.actual().expect("hay pista");
    let (pistas, semilla) = c.cola.para_persistir();

    // Lo que sonaria a partir de aqui en la sesion viva.
    let mut esperado = Vec::new();
    for _ in 0..10 {
        esperado.push(
            c.cola
                .advance(AdvanceReason::NaturalEnd)
                .await
                .expect("avanza"),
        );
    }

    let otra = ctx(0).await;
    otra.cola.restaurar(
        Some(PlaybackContext::Library),
        pistas,
        Some(sonando),
        true,
        semilla,
        RepeatMode::Off,
    );

    let mut obtenido = Vec::new();
    for _ in 0..10 {
        obtenido.push(
            otra.cola
                .advance(AdvanceReason::NaturalEnd)
                .await
                .expect("avanza"),
        );
    }

    assert_eq!(
        obtenido, esperado,
        "la sesion restaurada debe seguir sonando igual, no solo la siguiente"
    );
}

#[tokio::test]
async fn restaurar_una_sesion_deja_la_misma_permutacion() {
    // Sin la semilla, reabrir con el aleatorio activo daria otro orden y
    // "anterior" llevaria a una cancion distinta de la que sono.
    let c = ctx(40).await;
    con_contexto(&c, 0).await;
    c.cola.set_shuffle(true).await.expect("aleatorio");
    for _ in 0..6 {
        c.cola
            .advance(AdvanceReason::NaturalEnd)
            .await
            .expect("avanza");
    }

    let sonando = c.cola.actual().expect("hay pista");
    let esperada = c.cola.peek_next().await.expect("peek");
    let (pistas, semilla) = c.cola.para_persistir();

    // Otra sesion: misma cola reconstruida desde lo guardado.
    let otra = ctx(0).await;
    otra.cola.restaurar(
        Some(PlaybackContext::Library),
        pistas,
        Some(sonando),
        true,
        semilla,
        RepeatMode::Off,
    );

    assert_eq!(
        otra.cola.peek_next().await.expect("peek"),
        esperada,
        "la permutacion no sobrevivio al reinicio"
    );
}

#[tokio::test]
async fn cada_cambio_avisa_con_una_revision_mayor() {
    // El frontend descarta respuestas viejas comparando este numero.
    let c = ctx(10).await;
    con_contexto(&c, 0).await;
    c.cola
        .add_last(std::slice::from_ref(&c.pistas[1]))
        .await
        .expect("encola");
    c.cola.set_repeat(RepeatMode::Queue).await.expect("modo");

    let revisiones = c.bus.revisiones();
    assert!(revisiones.len() >= 3, "faltan avisos: {revisiones:?}");
    assert!(
        revisiones.windows(2).all(|w| w[1] > w[0]),
        "las revisiones no crecen: {revisiones:?}"
    );
}

#[tokio::test]
async fn ir_a_una_pista_concreta_la_pone_a_sonar() {
    let c = ctx(20).await;
    con_contexto(&c, 0).await;

    assert!(c.cola.ir_a(&c.pistas[12]));
    assert_eq!(c.cola.actual(), Some(c.pistas[12].clone()));
    assert_eq!(
        c.cola.peek_next().await.expect("peek"),
        Some(c.pistas[13].clone())
    );
}

#[tokio::test]
async fn ir_a_una_pista_que_no_esta_en_el_contexto_no_hace_nada() {
    let c = ctx(20).await;
    con_contexto(&c, 5).await;
    let antes = c.cola.actual();

    assert!(!c.cola.ir_a(&id(999)));
    assert_eq!(c.cola.actual(), antes);
}

#[tokio::test]
async fn una_cola_vacia_no_entra_en_panico() {
    let c = ctx(0).await;
    assert_eq!(c.cola.actual(), None);
    assert_eq!(c.cola.peek_next().await.expect("peek"), None);
    assert_eq!(
        c.cola.advance(AdvanceReason::NaturalEnd).await.expect("a"),
        None
    );
    assert_eq!(c.cola.go_back().await.expect("atras"), None);
    assert!(c.cola.snapshot().await.current.is_none());
}

#[tokio::test]
async fn la_ventana_del_contexto_no_manda_la_biblioteca_entera() {
    // Una biblioteca de 50 000 pistas no cabe en un evento IPC.
    let c = ctx(200).await;
    con_contexto(&c, 0).await;
    let snapshot = c.cola.snapshot().await;
    assert!(
        snapshot.context_queue.len() <= 50,
        "se mandaron {} pistas",
        snapshot.context_queue.len()
    );
}

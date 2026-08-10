//! Las decisiones del reproductor, con un motor de audio falso.
//!
//! Aquí no suena nada. Es a propósito: lo que se prueba es la **política** —qué
//! va después, cuándo se precarga, si "anterior" reinicia o retrocede— y esa
//! lógica no debería necesitar una tarjeta de sonido para verificarse. Es
//! justamente lo que compra separar mecanismo de política (ADR-015).
//!
//! El motor falso anota lo que le piden. Los tests comprueban esas órdenes, que
//! es lo que de verdad determina lo que el usuario oye.

#![allow(clippy::expect_used, clippy::panic)]

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use localify_core::domain::audio::{AudioDevice, DurationMs, EqProfile, Volume};
use localify_core::domain::availability::Availability;
use localify_core::domain::download::Priority;
use localify_core::domain::ids::{ArtistId, TrackId};
use localify_core::domain::queue::{PlayStatus, PlaybackContext, RepeatMode};
use localify_core::domain::track::{ArtistRef, Track};
use localify_core::error::CoreResult;
use localify_core::events::{DomainEvent, EventPublisher};
use localify_core::ports::audio_engine::{
    AudioEngine, AudioError, AudioSource, EngineEvent, VoiceId,
};
use localify_core::ports::database::TrackRepository;
use localify_core::ports::services::{
    DownloadHandle, DownloadService, PlaybackService, QueueService,
};
use localify_db::Pool;
use localify_db::pool::TempDbGuard;
use localify_services::actors::{
    DependenciasCola, DependenciasReproduccion, PlaybackActor, QueueActor,
};

// ─────────────────────────────────────────────────────────────────────────────
// Dobles
// ─────────────────────────────────────────────────────────────────────────────

/// Lo que el reproductor le pidió al motor, en orden.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Orden {
    Cargar(VoiceId),
    Tocar(VoiceId),
    Pausar,
    Parar(VoiceId),
    Saltar(VoiceId, u32),
    Fundir(VoiceId, u32),
}

#[derive(Debug, Default)]
struct MotorFalso {
    ordenes: Mutex<Vec<Orden>>,
    siguiente_voz: AtomicU32,
    /// Posición que finge el motor, para probar la regla de los tres segundos.
    posicion_ms: AtomicU32,
}

impl MotorFalso {
    fn ordenes(&self) -> Vec<Orden> {
        self.ordenes.lock().map(|v| v.clone()).unwrap_or_default()
    }

    fn anotar(&self, o: Orden) {
        if let Ok(mut v) = self.ordenes.lock() {
            v.push(o);
        }
    }
}

impl AudioEngine for MotorFalso {
    fn load(&self, _source: AudioSource, _start_at: DurationMs) -> Result<VoiceId, AudioError> {
        let id = VoiceId(self.siguiente_voz.fetch_add(1, Ordering::Relaxed));
        self.anotar(Orden::Cargar(id));
        Ok(id)
    }
    fn play(&self, voice: VoiceId) {
        self.anotar(Orden::Tocar(voice));
    }
    fn pause(&self) {
        self.anotar(Orden::Pausar);
    }
    fn stop(&self, voice: VoiceId) {
        self.anotar(Orden::Parar(voice));
    }
    fn seek(&self, voice: VoiceId, position: DurationMs) {
        self.anotar(Orden::Saltar(voice, position.as_ms()));
        self.posicion_ms.store(position.as_ms(), Ordering::Relaxed);
    }
    fn crossfade_to(&self, next: VoiceId, duration: DurationMs) {
        self.anotar(Orden::Fundir(next, duration.as_ms()));
    }
    fn set_volume(&self, _volume: Volume) {}
    fn set_equalizer(&self, _profile: &EqProfile) {}
    fn position(&self) -> DurationMs {
        DurationMs::new(self.posicion_ms.load(Ordering::Relaxed))
    }
    fn buffered(&self) -> DurationMs {
        DurationMs::ZERO
    }
    fn devices(&self) -> Vec<AudioDevice> {
        Vec::new()
    }
    fn set_device(&self, _device_id: Option<&str>) -> Result<(), AudioError> {
        Ok(())
    }
}

/// Descargador que siempre tiene el fichero listo, y anota a quién se le pidió.
#[derive(Debug, Default)]
struct DescargasFalsas {
    pedidas: Mutex<Vec<(TrackId, Priority)>>,
}

#[async_trait::async_trait]
impl DownloadService for DescargasFalsas {
    async fn ensure(&self, track: &TrackId, priority: Priority) -> CoreResult<DownloadHandle> {
        if let Ok(mut v) = self.pedidas.lock() {
            v.push((track.clone(), priority));
        }
        Ok(DownloadHandle {
            playable_path: std::path::PathBuf::from("C:/no/importa.opus"),
            complete: true,
        })
    }
    async fn status(&self, _track: &TrackId) -> CoreResult<Availability> {
        Ok(Availability::Absent)
    }
    async fn statuses(&self, _tracks: &[TrackId]) -> CoreResult<Vec<(TrackId, Availability)>> {
        Ok(Vec::new())
    }
    async fn retry_failed(&self) -> CoreResult<u32> {
        Ok(0)
    }
}

impl DescargasFalsas {
    fn con_prioridad(&self, p: Priority) -> Vec<TrackId> {
        self.pedidas
            .lock()
            .map(|v| {
                v.iter()
                    .filter(|(_, pr)| *pr == p)
                    .map(|(t, _)| t.clone())
                    .collect()
            })
            .unwrap_or_default()
    }
}

#[derive(Debug, Default)]
struct BusDePrueba(Mutex<Vec<DomainEvent>>);

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

// ─────────────────────────────────────────────────────────────────────────────
// Montaje
// ─────────────────────────────────────────────────────────────────────────────

struct Ctx {
    player: PlaybackActor,
    cola: QueueActor,
    motor: Arc<MotorFalso>,
    descargas: Arc<DescargasFalsas>,
    bus: Arc<BusDePrueba>,
    pistas: Vec<TrackId>,
    /// Se conserva para poder abrir una segunda sesión sobre la misma base de
    /// datos, que es como se prueba la restauración de verdad.
    pool: Pool,
    _guard: TempDbGuard,
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

async fn ctx(cuantas: usize) -> Ctx {
    ctx_con_crossfade(cuantas, 3000).await
}

/// Igual, eligiendo el crossfade. Con cero, el reproductor debe encadenar sin
/// fundir: es la configuración por defecto y la que destapó que "preparar el
/// fundido" adelantaba el cambio quince segundos.
async fn ctx_con_crossfade(cuantas: usize, crossfade_ms: u32) -> Ctx {
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
            title: format!("Pista {i:03}"),
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
        })
        .collect();
    if !filas.is_empty() {
        tracks.upsert(&filas).await.expect("guarda");
    }

    let bus = Arc::new(BusDePrueba::default());
    let motor = Arc::new(MotorFalso::default());
    let descargas = Arc::new(DescargasFalsas::default());

    let cola = QueueActor::nuevo(DependenciasCola {
        tracks: Arc::clone(&tracks),
        bus: Arc::clone(&bus) as Arc<dyn EventPublisher>,
    });

    let player = actor(
        &pool,
        &tracks,
        &cola,
        &motor,
        &descargas,
        &bus,
        crossfade_ms,
    );

    Ctx {
        player,
        cola,
        motor,
        descargas,
        bus,
        pistas,
        pool,
        _guard: guard,
    }
}

/// Construye un reproductor sobre una base de datos ya abierta.
fn actor(
    pool: &Pool,
    tracks: &Arc<dyn TrackRepository>,
    cola: &QueueActor,
    motor: &Arc<MotorFalso>,
    descargas: &Arc<DescargasFalsas>,
    bus: &Arc<BusDePrueba>,
    crossfade_ms: u32,
) -> PlaybackActor {
    PlaybackActor::arrancar(DependenciasReproduccion {
        motor: Arc::clone(motor) as Arc<dyn AudioEngine>,
        cola: cola.clone(),
        descargas: Arc::clone(descargas) as Arc<dyn DownloadService>,
        tracks: Arc::clone(tracks),
        albums: Arc::new(localify_db::repositories::SqliteAlbumRepository::new(
            pool.clone(),
        )),
        playlists: Arc::new(localify_db::repositories::SqlitePlaylistRepository::new(
            pool.clone(),
        )),
        favoritos: Arc::new(localify_db::repositories::SqliteFavoriteRepository::new(
            pool.clone(),
        )),
        historial: Arc::new(localify_db::repositories::SqliteHistoryRepository::new(
            pool.clone(),
        )),
        estado_repo: Arc::new(localify_db::repositories::SqlitePlayerStateRepository::new(
            pool.clone(),
        )),
        bus: Arc::clone(bus) as Arc<dyn EventPublisher>,
        crossfade: Arc::new(AtomicU32::new(crossfade_ms)),
    })
}

/// Abre una sesión nueva sobre la misma base de datos, como haría reabrir la
/// aplicación.
fn segunda_sesion(c: &Ctx) -> (PlaybackActor, QueueActor) {
    let tracks: Arc<dyn TrackRepository> = Arc::new(
        localify_db::repositories::SqliteTrackRepository::new(c.pool.clone()),
    );
    let cola = QueueActor::nuevo(DependenciasCola {
        tracks: Arc::clone(&tracks),
        bus: Arc::clone(&c.bus) as Arc<dyn EventPublisher>,
    });
    let motor = Arc::new(MotorFalso::default());
    let descargas = Arc::new(DescargasFalsas::default());
    let player = actor(&c.pool, &tracks, &cola, &motor, &descargas, &c.bus, 3000);
    (player, cola)
}

/// Fuente de eventos del motor manejada a mano.
///
/// El motor real los emite desde su vigilante, midiendo la posición. Aquí se
/// empujan a mano para poder llegar al final de una canción sin esperar tres
/// minutos.
struct EventosAMano(std::sync::mpsc::Receiver<localify_core::ports::audio_engine::EngineEvent>);

impl localify_core::ports::audio_engine::AudioEventSource for EventosAMano {
    fn recv(&mut self) -> Option<localify_core::ports::audio_engine::EngineEvent> {
        self.0.recv().ok()
    }
    fn try_recv(&mut self) -> Option<localify_core::ports::audio_engine::EngineEvent> {
        self.0.try_recv().ok()
    }
}

/// Espera a que el motor reciba una orden que cumpla `condicion`.
///
/// La preparación de una pista se delega a una tarea hija —es lo que evita que
/// el bucle se bloquee—, así que las órdenes llegan de forma asíncrona.
async fn esperar_orden(c: &Ctx, condicion: impl Fn(&Orden) -> bool) -> bool {
    for _ in 0..100 {
        if c.motor.ordenes().iter().any(&condicion) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    false
}

/// Instala la biblioteca como contexto y arranca una pista.
async fn reproducir(c: &Ctx, indice: usize) {
    c.cola.poner_pistas(c.pistas.clone(), indice);
    c.player
        .play_track(&c.pistas[indice], PlaybackContext::Library)
        .await
        .expect("reproduce");
    assert!(
        esperar_orden(c, |o| matches!(o, Orden::Tocar(_))).await,
        "el motor nunca recibio la orden de tocar"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Reproducir
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn reproducir_una_pista_la_carga_y_la_toca() {
    let c = ctx(10).await;
    reproducir(&c, 0).await;

    let ordenes = c.motor.ordenes();
    assert!(
        matches!(ordenes.first(), Some(Orden::Cargar(_))),
        "lo primero debe ser cargar: {ordenes:?}"
    );
    assert_eq!(c.player.state().await.status, PlayStatus::Playing);
}

#[tokio::test]
async fn la_descarga_se_pide_en_el_carril_inmediato() {
    // Lo que el usuario esta esperando no puede ir en el mismo carril que una
    // precarga: la precarga le robaria ancho de banda.
    let c = ctx(10).await;
    reproducir(&c, 3).await;

    assert!(
        c.descargas
            .con_prioridad(Priority::Immediate)
            .contains(&c.pistas[3]),
        "la pista que suena debe pedirse como inmediata"
    );
}

#[tokio::test]
async fn reproducir_precarga_las_dos_siguientes() {
    let c = ctx(10).await;
    reproducir(&c, 0).await;

    // La precarga va en una tarea hija.
    for _ in 0..50 {
        if c.descargas.con_prioridad(Priority::Prefetch).len() >= 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    let precargadas = c.descargas.con_prioridad(Priority::Prefetch);
    assert!(
        precargadas.contains(&c.pistas[1]) && precargadas.contains(&c.pistas[2]),
        "deberian precargarse las dos siguientes: {precargadas:?}"
    );
}

#[tokio::test]
async fn reproducir_algo_que_no_existe_falla_sin_tocar_el_motor() {
    let c = ctx(5).await;
    let error = c
        .player
        .play_track(&id(999), PlaybackContext::Single)
        .await
        .expect_err("deberia fallar");
    assert_eq!(error.code(), "NOT_FOUND");
    assert!(
        c.motor.ordenes().is_empty(),
        "no deberia haberse pedido nada al motor"
    );
}

#[tokio::test]
async fn reproducir_avisa_del_cambio_de_pista_y_de_estado() {
    let c = ctx(10).await;
    reproducir(&c, 0).await;

    let nombres = c.bus.nombres();
    for esperado in ["trackChanged", "playStatusChanged"] {
        assert!(
            nombres.iter().any(|n| n == esperado),
            "falta '{esperado}' en {nombres:?}"
        );
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Pausa y reanudación
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn alternar_pausa_y_reanuda() {
    let c = ctx(10).await;
    reproducir(&c, 0).await;

    let estado = c.player.toggle().await.expect("alterna");
    assert_eq!(estado.status, PlayStatus::Paused);
    assert!(c.motor.ordenes().contains(&Orden::Pausar));

    let estado = c.player.toggle().await.expect("alterna");
    assert_eq!(estado.status, PlayStatus::Playing);
}

// ─────────────────────────────────────────────────────────────────────────────
// La regla de los tres segundos
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn anterior_por_debajo_de_tres_segundos_va_a_la_pista_previa() {
    let c = ctx(10).await;
    reproducir(&c, 5).await;
    c.motor.posicion_ms.store(1500, Ordering::Relaxed);

    c.player.previous().await.expect("anterior");

    assert_eq!(
        c.cola.actual(),
        Some(c.pistas[4].clone()),
        "con menos de 3 s deberia retroceder de pista"
    );
}

#[tokio::test]
async fn anterior_por_encima_de_tres_segundos_reinicia_la_actual() {
    // Es lo que evita que un doble toque se lleve por delante la cancion que
    // acababa de empezar.
    let c = ctx(10).await;
    reproducir(&c, 5).await;
    c.motor.posicion_ms.store(30_000, Ordering::Relaxed);

    c.player.previous().await.expect("anterior");

    assert_eq!(
        c.cola.actual(),
        Some(c.pistas[5].clone()),
        "con mas de 3 s no deberia cambiar de pista"
    );
    assert!(
        c.motor
            .ordenes()
            .iter()
            .any(|o| matches!(o, Orden::Saltar(_, 0))),
        "deberia haber reiniciado al segundo cero"
    );
}

#[tokio::test]
async fn anterior_en_la_primera_pista_la_reinicia() {
    let c = ctx(10).await;
    reproducir(&c, 0).await;
    c.motor.posicion_ms.store(1000, Ordering::Relaxed);

    c.player.previous().await.expect("anterior");
    assert!(
        c.motor
            .ordenes()
            .iter()
            .any(|o| matches!(o, Orden::Saltar(_, 0))),
        "sin pista anterior, reiniciar es mejor que no hacer nada"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Avance
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn siguiente_avanza_en_el_contexto() {
    let c = ctx(10).await;
    reproducir(&c, 2).await;

    c.player.next().await.expect("siguiente");
    assert_eq!(c.cola.actual(), Some(c.pistas[3].clone()));
}

#[tokio::test]
async fn al_final_de_la_cola_la_reproduccion_se_para() {
    let c = ctx(3).await;
    reproducir(&c, 2).await;

    let estado = c.player.next().await.expect("siguiente");
    assert_eq!(
        estado.status,
        PlayStatus::Stopped,
        "sin repeticion, el final es el final"
    );
}

#[tokio::test]
async fn la_cola_de_usuario_manda_al_avanzar() {
    let c = ctx(10).await;
    reproducir(&c, 0).await;
    c.cola
        .add_next(std::slice::from_ref(&c.pistas[7]))
        .await
        .expect("encola");

    c.player.next().await.expect("siguiente");
    assert_eq!(c.cola.actual(), Some(c.pistas[7].clone()));
}

// ─────────────────────────────────────────────────────────────────────────────
// Modos
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn los_modos_se_reflejan_en_el_estado() {
    let c = ctx(10).await;
    reproducir(&c, 0).await;

    let estado = c.player.set_repeat(RepeatMode::Queue).await.expect("modo");
    assert_eq!(estado.repeat, RepeatMode::Queue);

    let estado = c.player.set_shuffle(true).await.expect("aleatorio");
    assert!(estado.shuffle);
}

#[tokio::test]
async fn el_volumen_llega_al_motor_y_al_estado() {
    let c = ctx(10).await;
    reproducir(&c, 0).await;

    let estado = c
        .player
        .set_volume(Volume::new(0.25))
        .await
        .expect("volumen");
    assert!((estado.volume.as_f32() - 0.25).abs() < f32::EPSILON);
}

// ─────────────────────────────────────────────────────────────────────────────
// Persistencia y restauración
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn el_estado_se_puede_volcar_a_disco() {
    let c = ctx(10).await;
    reproducir(&c, 4).await;
    c.motor.posicion_ms.store(42_000, Ordering::Relaxed);

    c.player.persist_now().await.expect("persiste");
}

#[tokio::test]
async fn cerrar_y_reabrir_restaura_la_pista_y_el_segundo_exacto() {
    // Es el criterio de la fase: reabrir la aplicacion debe dejar la sesion
    // donde estaba, no al principio de la cancion ni en otra distinta.
    let c = ctx(20).await;
    reproducir(&c, 7).await;
    c.motor.posicion_ms.store(42_000, Ordering::Relaxed);
    c.player.persist_now().await.expect("persiste");

    // Otra sesion sobre la misma base de datos, como al reabrir.
    let (otro, _cola) = segunda_sesion(&c);
    assert!(otro.restaurar().await.expect("restaura"), "no habia sesion");

    let estado = otro.state().await;
    assert_eq!(
        estado.track.map(|t| t.id),
        Some(c.pistas[7].clone()),
        "se restauro otra pista"
    );
    assert_eq!(
        estado.status,
        PlayStatus::Paused,
        "reabrir no debe ponerse a sonar solo"
    );
}

#[tokio::test]
async fn restaurar_una_sesion_conserva_los_modos() {
    let c = ctx(20).await;
    reproducir(&c, 3).await;
    c.player.set_repeat(RepeatMode::Queue).await.expect("modo");
    c.player.set_shuffle(true).await.expect("aleatorio");
    c.player.persist_now().await.expect("persiste");

    let (otro, _cola) = segunda_sesion(&c);
    assert!(otro.restaurar().await.expect("restaura"));

    let estado = otro.state().await;
    assert_eq!(estado.repeat, RepeatMode::Queue);
    assert!(estado.shuffle, "el aleatorio no sobrevivio al reinicio");
}

#[tokio::test]
async fn sin_sesion_guardada_no_se_inventa_una() {
    let c = ctx(10).await;
    let (otro, _cola) = segunda_sesion(&c);
    assert!(
        !otro.restaurar().await.expect("consulta"),
        "una base de datos sin sesion no debe restaurar nada"
    );
    assert_eq!(otro.state().await.status, PlayStatus::Stopped);
}

#[tokio::test]
async fn la_posicion_se_publica_sin_pasar_por_el_actor() {
    // La interfaz la sondea varias veces por segundo: hacerla pasar por el
    // canal lo saturaria de mensajes que no cambian nada.
    let c = ctx(10).await;
    reproducir(&c, 0).await;
    c.motor.posicion_ms.store(12_345, Ordering::Relaxed);

    for _ in 0..50 {
        if c.player.position().0.as_ms() == 12_345 {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!(
        "la posicion nunca llego al handle: {:?}",
        c.player.position()
    );
}

#[tokio::test]
async fn el_estado_completo_sirve_para_resincronizar() {
    let c = ctx(10).await;
    reproducir(&c, 6).await;

    let estado = c.player.state().await;
    assert_eq!(estado.track.map(|t| t.id), Some(c.pistas[6].clone()));
    assert_eq!(estado.duration, DurationMs::from_secs(180));
    assert!(estado.context.is_some(), "falta el contexto para la UI");
}

// ─────────────────────────────────────────────────────────────────────────────
// Historial de escuchas
// ─────────────────────────────────────────────────────────────────────────────

/// Filas de `play_history`, con su contexto.
async fn escuchas(c: &Ctx) -> Vec<(String, u32, bool, Option<String>)> {
    c.pool
        .leer(|conn| {
            let mut stmt = conn.prepare(
                "SELECT track_id, ms_played, completed, context
                 FROM play_history ORDER BY played_at",
            )?;
            let filas = stmt
                .query_map([], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        u32::try_from(r.get::<_, i64>(1)?).unwrap_or(0),
                        r.get::<_, i64>(2)? != 0,
                        r.get::<_, Option<String>>(3)?,
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(filas)
        })
        .await
        .expect("lee el historial")
}

/// Espera a que aparezcan `cuantas` escuchas: se anotan en segundo plano.
async fn esperar_escuchas(c: &Ctx, cuantas: usize) -> Vec<(String, u32, bool, Option<String>)> {
    for _ in 0..100 {
        let filas = escuchas(c).await;
        if filas.len() >= cuantas {
            return filas;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    escuchas(c).await
}

/// Lo que hay que esperar para superar el mínimo de escucha.
///
/// El actor mide con el reloj real —es lo que significa "he escuchado esto"—,
/// así que un test que compruebe el umbral tiene que esperar de verdad. Se paga
/// una vez: los dos asertos que lo necesitan van en el mismo test en lugar de
/// cobrar seis segundos cada uno.
const SOBRE_EL_MINIMO: Duration = Duration::from_millis(5_400);

#[tokio::test]
async fn cambiar_de_cancion_anota_lo_que_sono_y_desde_donde() {
    // Es la base de todas las recomendaciones, y estuvo sin implementar: el
    // metodo existia en `LibraryService`, no lo llamaba nadie, y el historial
    // se quedaba vacio para siempre. Inicio no tenia de donde sacar nada.
    //
    // El contexto importa tanto como la cancion: sin el, "tus playlists mas
    // escuchadas" solo podria calcularse como "playlists que contienen
    // canciones que has oido", y una cancion suelta contaria para las diez
    // listas que la incluyen.
    let c = ctx(10).await;
    c.cola.poner_pistas(c.pistas.clone(), 0);
    c.player
        .play_track(&c.pistas[0], PlaybackContext::Liked)
        .await
        .expect("reproduce");
    assert!(esperar_orden(&c, |o| matches!(o, Orden::Tocar(_))).await);

    tokio::time::sleep(SOBRE_EL_MINIMO).await;
    reproducir(&c, 1).await;

    let filas = esperar_escuchas(&c, 1).await;
    assert_eq!(
        filas.len(),
        1,
        "deberia haberse anotado la primera: {filas:?}"
    );
    assert_eq!(filas[0].0, c.pistas[0].as_str());
    assert!(filas[0].1 >= 5_000, "los ms oidos: {}", filas[0].1);
    assert!(
        !filas[0].2,
        "180 segundos de cancion y 5 oidos no es completa"
    );
    assert_eq!(filas[0].3.as_deref(), Some("liked"));
}

#[tokio::test]
async fn pasar_de_largo_no_deja_rastro() {
    // Saltar por encima de una cancion para llegar a otra no es escucharla, y
    // contarlo la recomendaria mas cuanto mas se salta.
    let c = ctx(10).await;
    reproducir(&c, 0).await;
    reproducir(&c, 1).await;

    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(
        escuchas(&c).await.is_empty(),
        "un salto inmediato no deberia anotarse"
    );
}

#[tokio::test]
async fn el_tiempo_en_pausa_no_cuenta_como_escucha() {
    // Sin esto, dejar la aplicacion pausada toda la noche anotaria ocho horas
    // de escucha de la cancion que quedo cargada, y esa cancion se comeria
    // Inicio entero.
    let c = ctx(10).await;
    reproducir(&c, 0).await;

    c.player.pause().await.expect("pausa");
    tokio::time::sleep(SOBRE_EL_MINIMO).await;
    reproducir(&c, 1).await;

    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(
        escuchas(&c).await.is_empty(),
        "el rato en pausa no deberia contar"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Final de pista
// ─────────────────────────────────────────────────────────────────────────────

/// Conecta una fuente de eventos manual al reproductor y devuelve el emisor.
fn eventos_de(player: &PlaybackActor) -> std::sync::mpsc::Sender<EngineEvent> {
    let (tx, rx) = std::sync::mpsc::channel();
    localify_services::actors::conectar_eventos(player, Box::new(EventosAMano(rx)));
    tx
}

#[tokio::test]
async fn sin_crossfade_el_aviso_de_final_no_cambia_de_cancion() {
    // El aviso llega **quince segundos** antes del final, que es el margen que
    // necesita el crossfade más largo. Con el ajuste a cero, pedir el fundido
    // ahí no encadena: sustituye la voz en el acto, y la canción saltaba a la
    // siguiente con quince segundos todavía por sonar.
    let c = ctx_con_crossfade(10, 0).await;
    let eventos = eventos_de(&c.player);
    reproducir(&c, 0).await;

    let antes = c.motor.ordenes().len();
    eventos
        .send(EngineEvent::ApproachingEnd {
            voice: VoiceId(0),
            remaining: DurationMs::from_secs(15),
        })
        .expect("avisa");

    // Se deja tiempo de sobra para que el actor lo procese y precargue.
    tokio::time::sleep(Duration::from_millis(200)).await;

    let nuevas = &c.motor.ordenes()[antes..];
    assert!(
        !nuevas.iter().any(|o| matches!(o, Orden::Fundir(..))),
        "sin crossfade no debe fundirse nada al avisar del final: {nuevas:?}"
    );
    assert!(
        nuevas.iter().any(|o| matches!(o, Orden::Cargar(_))),
        "la siguiente sí debe quedar cargada y esperando: {nuevas:?}"
    );
    assert_eq!(
        c.player.state().await.track.map(|t| t.id),
        Some(c.pistas[0].clone()),
        "la canción que suena sigue siendo la primera"
    );
}

#[tokio::test]
async fn al_terminar_de_verdad_suena_la_que_estaba_preparada() {
    // La otra mitad de lo anterior: si al avisar no se instala nada, alguien
    // tiene que instalarlo al final. Sin esto el arreglo del salto adelantado
    // dejaría la reproducción parada al acabar cada canción.
    let c = ctx_con_crossfade(10, 0).await;
    let eventos = eventos_de(&c.player);
    reproducir(&c, 0).await;

    eventos
        .send(EngineEvent::ApproachingEnd {
            voice: VoiceId(0),
            remaining: DurationMs::from_secs(15),
        })
        .expect("avisa");
    tokio::time::sleep(Duration::from_millis(200)).await;

    eventos
        .send(EngineEvent::Ended { voice: VoiceId(0) })
        .expect("termina");
    tokio::time::sleep(Duration::from_millis(200)).await;

    assert!(
        c.motor
            .ordenes()
            .iter()
            .any(|o| matches!(o, Orden::Fundir(_, 0))),
        "al terminar hay que instalar la voz preparada: {:?}",
        c.motor.ordenes()
    );
    assert_eq!(
        c.player.state().await.track.map(|t| t.id),
        Some(c.pistas[1].clone()),
        "ahora sí toca la segunda"
    );
}

#[tokio::test]
async fn saltar_dentro_de_la_cancion_avisa_de_donde_quedo() {
    // Saltar no cambia ni de pista ni de estado, así que no emitía nada. Quien
    // publica la posición hacia fuera —el perfil de Discord lleva la hora a la
    // que empezó y a la que acaba— se quedaba anunciando el minuto de antes
    // hasta la canción siguiente.
    let c = ctx(10).await;
    reproducir(&c, 0).await;

    c.player
        .seek(DurationMs::from_secs(90))
        .await
        .expect("salta");

    assert!(
        c.bus.nombres().contains(&"seeked".to_owned()),
        "un salto tiene que anunciarse: {:?}",
        c.bus.nombres()
    );
    assert!(
        c.motor
            .ordenes()
            .iter()
            .any(|o| matches!(o, Orden::Saltar(_, 90_000))),
        "y el motor tiene que recibirlo: {:?}",
        c.motor.ordenes()
    );
}

#[tokio::test]
async fn cerrar_y_reabrir_conserva_el_volumen() {
    // El volumen se guardaba en disco y **no se aplicaba nunca** al restaurar:
    // se leía y se descartaba, así que cada arranque empezaba al máximo.
    let c = ctx(20).await;
    reproducir(&c, 0).await;
    let bajo = Volume::new(0.25);
    c.player.set_volume(bajo).await.expect("volumen");
    c.player.persist_now().await.expect("persiste");

    let (otro, _cola) = segunda_sesion(&c);
    otro.restaurar().await.expect("restaura");

    assert!(
        (otro.state().await.volume.as_f32() - bajo.as_f32()).abs() < 1e-6,
        "el volumen debe sobrevivir al cierre"
    );
}

#[tokio::test]
async fn los_modos_sobreviven_aunque_no_quedara_cancion_puesta() {
    // Los modos y el volumen colgaban del camino que restaura la pista, así que
    // cerrar sin nada sonando los perdía todos. No son parte de una sesión: son
    // ajustes del reproductor.
    let c = ctx(20).await;
    reproducir(&c, 0).await;
    c.player.set_repeat(RepeatMode::Queue).await.expect("modo");
    c.player.set_shuffle(true).await.expect("aleatorio");
    c.player
        .set_volume(Volume::new(0.4))
        .await
        .expect("volumen");
    c.player.persist_now().await.expect("persiste");

    // Se borra la pista guardada, como tras vaciar la biblioteca.
    let estado_repo: Arc<dyn localify_core::ports::database::PlayerStateRepository> = Arc::new(
        localify_db::repositories::SqlitePlayerStateRepository::new(c.pool.clone()),
    );
    estado_repo.clear().await.expect("olvida la sesión");

    let (otro, _cola) = segunda_sesion(&c);
    // Devuelve `false` —no hay sesión que continuar— pero deja los ajustes.
    assert!(!otro.restaurar().await.expect("restaura"));

    let estado = otro.state().await;
    assert!(
        (estado.volume.as_f32() - 0.4).abs() < 1e-6,
        "el volumen no depende de que hubiera canción: {}",
        estado.volume.as_f32()
    );
    assert_eq!(estado.repeat, RepeatMode::Queue);
    assert!(estado.shuffle);
}

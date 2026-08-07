//! Estado del reproductor entre sesiones.
//!
//! Restaura la sesión **exactamente** donde se dejó: pista, segundo, cola
//! completa, modos y permutación de aleatorio. Es uno de los requisitos
//! explícitos del proyecto.
//!
//! La cola se guarda como JSON en una fila única. Se lee y se escribe siempre
//! entera, nunca se consulta por partes y se actualiza cada pocos segundos:
//! normalizarla en una tabla solo añadiría escrituras. La normalización sirve
//! para consultar, y esto no se consulta.

use async_trait::async_trait;
use localify_core::domain::audio::{DurationMs, Volume};
use localify_core::domain::ids::TrackId;
use localify_core::error::CoreResult;
use localify_core::ports::database::{PersistedPlayerState, PlayerStateRepository};
use rusqlite::params;
use serde::{Deserialize, Serialize};

use crate::error::ToCore;
use crate::mappers::{a_modo_repeticion, de_modo_repeticion};
use crate::pool::Pool;

pub struct SqlitePlayerStateRepository {
    pool: Pool,
}

impl std::fmt::Debug for SqlitePlayerStateRepository {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqlitePlayerStateRepository")
            .finish_non_exhaustive()
    }
}

impl SqlitePlayerStateRepository {
    #[must_use]
    pub const fn new(pool: Pool) -> Self {
        Self { pool }
    }
}

/// Representación serializada de una entrada de cola.
///
/// Solo se guarda el `TrackId`, no la fila completa: los metadatos pueden haber
/// cambiado entre sesiones, y rehidratarlos desde el catálogo al arrancar es más
/// correcto que restaurar una copia obsoleta. Además mantiene el JSON pequeño.
#[derive(Serialize, Deserialize)]
struct EntradaSerializada {
    entry_id: uuid::Uuid,
    track_id: String,
}

fn serializar_ids(ids: &[TrackId]) -> String {
    let ligeras: Vec<EntradaSerializada> = ids
        .iter()
        .map(|id| EntradaSerializada {
            // El identificador de entrada es efímero: se regenera al restaurar.
            // Guardarlo no aportaría nada, porque nadie lo referencia entre
            // sesiones.
            entry_id: uuid::Uuid::now_v7(),
            track_id: id.as_str().to_owned(),
        })
        .collect();
    serde_json::to_string(&ligeras).unwrap_or_else(|_| "[]".to_owned())
}

/// Devuelve los IDs de una cola serializada, en orden.
fn deserializar_ids(json: Option<&str>) -> Vec<TrackId> {
    let Some(texto) = json else { return Vec::new() };
    serde_json::from_str::<Vec<EntradaSerializada>>(texto)
        .unwrap_or_default()
        .into_iter()
        .map(|e| TrackId::from_trusted(e.track_id))
        .collect()
}

#[async_trait]
impl PlayerStateRepository for SqlitePlayerStateRepository {
    async fn load(&self) -> CoreResult<Option<PersistedPlayerState>> {
        self.pool
            .leer(|conn| {
                let fila = conn.query_row(
                    "SELECT track_id, position_ms, volume, repeat_mode, shuffle,
                            shuffle_seed, context, context_queue, user_queue, queue_index
                     FROM player_state WHERE id = 1",
                    [],
                    |r| {
                        Ok((
                            r.get::<_, Option<String>>(0)?,
                            r.get::<_, i64>(1)?,
                            r.get::<_, f64>(2)?,
                            r.get::<_, String>(3)?,
                            r.get::<_, i64>(4)?,
                            r.get::<_, Option<i64>>(5)?,
                            r.get::<_, Option<String>>(6)?,
                            r.get::<_, Option<String>>(7)?,
                            r.get::<_, Option<String>>(8)?,
                            r.get::<_, i64>(9)?,
                        ))
                    },
                );

                let Ok(f) = fila else { return Ok(None) };

                Ok(Some(PersistedPlayerState {
                    track_id: f.0.map(TrackId::from_trusted),
                    position: DurationMs::new(u32::try_from(f.1.max(0)).unwrap_or(0)),
                    #[allow(
                        clippy::cast_possible_truncation,
                        reason = "el CHECK del esquema acota el volumen a [0,1]"
                    )]
                    volume: Volume::new(f.2 as f32),
                    repeat: a_modo_repeticion(&f.3),
                    shuffle: f.4 != 0,
                    shuffle_seed: f.5.map(i64::unsigned_abs),
                    context: f.6.as_deref().and_then(|s| serde_json::from_str(s).ok()),
                    context_queue: deserializar_ids(f.7.as_deref()),
                    user_queue: deserializar_ids(f.8.as_deref()),
                    queue_index: usize::try_from(f.9.max(0)).unwrap_or(0),
                }))
            })
            .await
            .to_core()
            // Sin pista guardada no hay sesión que restaurar. La fila existe
            // siempre —la crea la migración—, así que el filtro tiene que ser
            // por contenido y no por presencia.
            .map(|estado| estado.filter(|e| e.track_id.is_some()))
    }

    async fn save(&self, state: &PersistedPlayerState) -> CoreResult<()> {
        let track_id = state.track_id.as_ref().map(|t| t.as_str().to_owned());
        let position = i64::from(state.position.as_ms());
        let volume = f64::from(state.volume.as_f32());
        let repeat = de_modo_repeticion(state.repeat);
        let shuffle = i64::from(state.shuffle);
        let seed = state
            .shuffle_seed
            .map(|s| i64::try_from(s).unwrap_or(i64::MAX));
        let contexto = state
            .context
            .as_ref()
            .and_then(|c| serde_json::to_string(c).ok());
        let contexto_cola = serializar_ids(&state.context_queue);
        let cola_usuario = serializar_ids(&state.user_queue);
        let indice = i64::try_from(state.queue_index).unwrap_or(0);

        self.pool
            .escribir(move |tx| {
                tx.execute(
                    "UPDATE player_state SET
                         track_id      = ?1,
                         position_ms   = ?2,
                         volume        = ?3,
                         repeat_mode   = ?4,
                         shuffle       = ?5,
                         shuffle_seed  = ?6,
                         context       = ?7,
                         context_queue = ?8,
                         user_queue    = ?9,
                         queue_index   = ?10,
                         updated_at    = unixepoch()
                     WHERE id = 1",
                    params![
                        track_id,
                        position,
                        volume,
                        repeat,
                        shuffle,
                        seed,
                        contexto,
                        contexto_cola,
                        cola_usuario,
                        indice,
                    ],
                )?;
                Ok(())
            })
            .await
            .to_core()
    }

    async fn clear(&self) -> CoreResult<()> {
        self.pool
            .escribir(move |tx| {
                // La fila se vacía, no se borra: es un singleton con `id = 1` y
                // `save` la actualiza en vez de insertarla. Borrarla dejaría el
                // guardado siguiente sin nada que actualizar y la sesión no se
                // volvería a persistir jamás.
                tx.execute(
                    "UPDATE player_state SET
                         track_id      = NULL,
                         position_ms   = 0,
                         context       = NULL,
                         context_queue = NULL,
                         user_queue    = NULL,
                         queue_index   = 0,
                         updated_at    = unixepoch()
                     WHERE id = 1",
                    [],
                )?;
                Ok(())
            })
            .await
            .to_core()
    }
}

#[cfg(test)]
mod tests {
    use localify_core::domain::queue::{PlaybackContext, RepeatMode};
    use localify_core::domain::track::Track;
    use localify_core::ports::database::TrackRepository;

    use super::*;
    use crate::pool::TempDbGuard;
    use crate::repositories::tracks::SqliteTrackRepository;

    async fn ctx() -> (SqlitePlayerStateRepository, Pool, TempDbGuard) {
        let (pool, guard) = Pool::temporal().expect("abre");
        crate::migrations::ejecutar(&pool).await.expect("migra");
        (SqlitePlayerStateRepository::new(pool.clone()), pool, guard)
    }

    async fn nueva_pista(pool: &Pool, titulo: &str) -> TrackId {
        let repo = SqliteTrackRepository::new(pool.clone());
        let t = Track {
            id: TrackId::nuevo_local(),
            title: titulo.into(),
            album: None,
            artists: vec![],
            duration: DurationMs::new(200_000),
            track_number: None,
            disc_number: None,
            explicit: false,
            isrc: None,
            release_date: None,
            popularity: None,
            added_at: chrono::Utc::now(),
        };
        repo.upsert(std::slice::from_ref(&t)).await.expect("guarda");
        t.id
    }

    /// Estado mínimo, con la pista dada.
    fn estado(id: &TrackId) -> PersistedPlayerState {
        PersistedPlayerState {
            track_id: Some(id.clone()),
            position: DurationMs::ZERO,
            volume: Volume::MAX,
            repeat: RepeatMode::Off,
            shuffle: false,
            shuffle_seed: None,
            context: None,
            context_queue: Vec::new(),
            user_queue: Vec::new(),
            queue_index: 0,
        }
    }

    #[tokio::test]
    async fn una_sesion_guardada_se_restaura_al_segundo_exacto() {
        let (repo, pool, _g) = ctx().await;
        let id = nueva_pista(&pool, "Actual").await;

        repo.save(&PersistedPlayerState {
            position: DurationMs::new(87_450),
            volume: Volume::new(0.62),
            repeat: RepeatMode::Queue,
            shuffle: true,
            shuffle_seed: Some(123_456_789),
            context: Some(PlaybackContext::Liked),
            queue_index: 7,
            ..estado(&id)
        })
        .await
        .expect("guarda");

        let leido = repo.load().await.expect("lee").expect("existe");

        // La pista es lo primero que se comprueba: sin ella no hay sesión que
        // restaurar, por muy bien que se hayan guardado los demás campos.
        assert_eq!(
            leido.track_id,
            Some(id),
            "la pista guardada no volvió en `load`"
        );
        assert_eq!(leido.position, DurationMs::new(87_450));
        assert_eq!(leido.repeat, RepeatMode::Queue);
        assert!(leido.shuffle);
        assert_eq!(leido.shuffle_seed, Some(123_456_789));
        assert_eq!(leido.queue_index, 7);
        assert_eq!(leido.context, Some(PlaybackContext::Liked));
        assert!((leido.volume.as_f32() - 0.62).abs() < 0.001);
    }

    #[tokio::test]
    async fn las_colas_conservan_su_orden_al_restaurar() {
        let (repo, pool, _g) = ctx().await;
        let a = nueva_pista(&pool, "A").await;
        let b = nueva_pista(&pool, "B").await;
        let c = nueva_pista(&pool, "C").await;

        repo.save(&PersistedPlayerState {
            context: Some(PlaybackContext::Library),
            context_queue: vec![a.clone(), b.clone(), c.clone()],
            user_queue: vec![c.clone()],
            ..estado(&a)
        })
        .await
        .expect("guarda");

        let leido = repo.load().await.expect("lee").expect("existe");
        assert_eq!(leido.track_id, Some(a.clone()));
        assert_eq!(leido.context_queue, vec![a, b, c.clone()]);
        assert_eq!(
            leido.user_queue,
            vec![c],
            "la cola de usuario sobrevive al reinicio"
        );
    }

    #[tokio::test]
    async fn una_base_de_datos_recien_creada_no_tiene_sesion() {
        // La migración crea la fila siempre, así que `load` tiene que
        // distinguir "hay fila vacía" de "hay sesión".
        let (repo, _pool, _g) = ctx().await;
        assert!(
            repo.load().await.expect("lee").is_none(),
            "sin pista guardada no hay nada que restaurar"
        );
    }

    #[tokio::test]
    async fn borrar_la_pista_actual_no_impide_arrancar() {
        // Escenario real: el usuario borra el fichero y la pista fuera de la
        // app. `ON DELETE SET NULL` debe dejar el estado utilizable.
        let (repo, pool, _g) = ctx().await;
        let id = nueva_pista(&pool, "Se borrará").await;

        repo.save(&PersistedPlayerState {
            position: DurationMs::new(5000),
            ..estado(&id)
        })
        .await
        .expect("guarda");

        let txt = id.as_str().to_owned();
        pool.escribir(move |tx| {
            tx.execute("DELETE FROM tracks WHERE id = ?1", [&txt])?;
            Ok(())
        })
        .await
        .expect("borra");

        assert!(
            repo.load().await.expect("lee").is_none(),
            "la referencia queda a NULL: no hay sesión, pero tampoco error"
        );
    }

    #[tokio::test]
    async fn guardar_repetidamente_no_crea_filas_nuevas() {
        let (repo, pool, _g) = ctx().await;
        let id = nueva_pista(&pool, "X").await;

        for ms in [1000_u32, 6000, 11_000] {
            repo.save(&PersistedPlayerState {
                position: DurationMs::new(ms),
                ..estado(&id)
            })
            .await
            .expect("guarda");
        }

        let filas: i64 = pool
            .leer(|c| Ok(c.query_row("SELECT COUNT(*) FROM player_state", [], |r| r.get(0))?))
            .await
            .expect("cuenta");
        assert_eq!(
            filas, 1,
            "se guarda cada 5 s: debe ser siempre la misma fila"
        );

        assert_eq!(
            repo.load().await.expect("lee").expect("existe").position,
            DurationMs::new(11_000)
        );
    }

    #[tokio::test]
    async fn una_cola_vacia_no_rompe_la_serializacion() {
        let (repo, pool, _g) = ctx().await;
        let id = nueva_pista(&pool, "Sola").await;
        repo.save(&estado(&id)).await.expect("guarda");

        let leido = repo.load().await.expect("lee").expect("existe");
        assert!(leido.context_queue.is_empty());
        assert!(leido.user_queue.is_empty());
    }
}

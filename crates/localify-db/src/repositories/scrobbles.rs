//! Cola persistente de scrobbles.
//!
//! Es la tabla que sostiene la promesa de "no se pierde un scrobble por estar
//! sin conexión". Todo lo demás del scrobbling —cuándo cuenta una escucha, cómo
//! se firma la petición— vive en otro sitio: aquí solo se guardan y se sacan
//! filas.
//!
//! ## `track_id` y no el nombre de la canción
//!
//! Guardar "Faint — Linkin Park" haría la cola independiente del catálogo, lo
//! cual suena bien hasta que se piensa en qué pasa al borrar la pista: la fila
//! sobreviviría a una canción que ya no existe y se enviaría a Last.fm sin que
//! nadie pueda comprobar de dónde salió. Con la clave foránea, borrar la pista
//! se lleva sus scrobbles pendientes, que es lo que el usuario espera cuando
//! borra algo.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use localify_core::domain::ids::TrackId;
use localify_core::domain::scrobble::PendingScrobble;
use localify_core::error::CoreResult;
use localify_core::ports::database::ScrobbleRepository;
use rusqlite::params;

use crate::error::ToCore;
use crate::mappers::{a_fecha, de_fecha};
use crate::pool::Pool;

/// Segundos en un día.
const DIA_SEGUNDOS: i64 = 86_400;

pub struct SqliteScrobbleRepository {
    pool: Pool,
}

impl std::fmt::Debug for SqliteScrobbleRepository {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteScrobbleRepository")
            .finish_non_exhaustive()
    }
}

impl SqliteScrobbleRepository {
    #[must_use]
    pub const fn new(pool: Pool) -> Self {
        Self { pool }
    }
}

/// Marcadores `?1, ?2, …` para una lista de identificadores.
///
/// Se construyen a mano porque rusqlite no expande un `Vec` dentro de un `IN`.
/// Los valores siguen viajando como parámetros: lo único que se interpola es el
/// número de huecos.
fn huecos(cuantos: usize) -> String {
    (1..=cuantos)
        .map(|i| format!("?{i}"))
        .collect::<Vec<_>>()
        .join(", ")
}

#[async_trait]
impl ScrobbleRepository for SqliteScrobbleRepository {
    async fn enqueue(&self, track: &TrackId, started_at: DateTime<Utc>) -> CoreResult<()> {
        let id = track.as_str().to_owned();
        let cuando = de_fecha(started_at);
        self.pool
            .escribir(move |tx| {
                tx.execute(
                    "INSERT INTO scrobble_queue (track_id, timestamp) VALUES (?1, ?2)",
                    params![id, cuando],
                )?;
                Ok(())
            })
            .await
            .to_core()
    }

    async fn pending(&self, limit: u16) -> CoreResult<Vec<PendingScrobble>> {
        self.pool
            .leer(move |conn| {
                // Por `timestamp` y no por `id`: son casi lo mismo, pero el
                // orden que le importa a Last.fm es el de escucha.
                let mut stmt = conn.prepare_cached(
                    "SELECT id, track_id, timestamp, attempts
                     FROM scrobble_queue
                     ORDER BY timestamp ASC, id ASC
                     LIMIT ?1",
                )?;
                let filas = stmt
                    .query_map([i64::from(limit)], |r| {
                        Ok(PendingScrobble {
                            id: r.get(0)?,
                            track_id: TrackId::from_trusted(r.get::<_, String>(1)?),
                            started_at: a_fecha(r.get(2)?),
                            attempts: u32::try_from(r.get::<_, i64>(3)?).unwrap_or(0),
                        })
                    })?
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(filas)
            })
            .await
            .to_core()
    }

    async fn remove(&self, ids: &[i64]) -> CoreResult<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let ids = ids.to_vec();
        self.pool
            .escribir(move |tx| {
                let sql = format!(
                    "DELETE FROM scrobble_queue WHERE id IN ({})",
                    huecos(ids.len())
                );
                tx.execute(&sql, rusqlite::params_from_iter(ids.iter()))?;
                Ok(())
            })
            .await
            .to_core()
    }

    async fn mark_failed(&self, ids: &[i64], error: &str) -> CoreResult<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let ids = ids.to_vec();
        // El mensaje se recorta: un error de red puede traer una cadena larguísima
        // y esta columna es para diagnosticar, no para archivar.
        let motivo: String = error.chars().take(200).collect();
        self.pool
            .escribir(move |tx| {
                let sql = format!(
                    "UPDATE scrobble_queue SET attempts = attempts + 1, last_error = ?1
                     WHERE id IN ({})",
                    // El primer hueco lo ocupa el mensaje, así que los
                    // identificadores empiezan en el segundo.
                    (2..=ids.len() + 1)
                        .map(|i| format!("?{i}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                );
                let mut valores: Vec<rusqlite::types::Value> = vec![motivo.clone().into()];
                valores.extend(ids.iter().map(|i| rusqlite::types::Value::from(*i)));
                tx.execute(&sql, rusqlite::params_from_iter(valores.iter()))?;
                Ok(())
            })
            .await
            .to_core()
    }

    async fn purge_older_than(&self, days: u16) -> CoreResult<u32> {
        let corte = de_fecha(Utc::now()) - i64::from(days) * DIA_SEGUNDOS;
        self.pool
            .escribir(move |tx| {
                let n = tx.execute("DELETE FROM scrobble_queue WHERE timestamp < ?1", [corte])?;
                Ok(u32::try_from(n).unwrap_or(u32::MAX))
            })
            .await
            .to_core()
    }

    async fn count(&self) -> CoreResult<u64> {
        self.pool
            .leer(|conn| {
                let n: i64 =
                    conn.query_row("SELECT COUNT(*) FROM scrobble_queue", [], |r| r.get(0))?;
                Ok(u64::try_from(n.max(0)).unwrap_or(0))
            })
            .await
            .to_core()
    }
}

#[cfg(test)]
mod tests {
    use localify_core::domain::audio::DurationMs;
    use localify_core::domain::track::Track;
    use localify_core::ports::database::TrackRepository;

    use super::*;
    use crate::pool::TempDbGuard;
    use crate::repositories::tracks::SqliteTrackRepository;

    async fn ctx() -> (SqliteScrobbleRepository, Vec<TrackId>, Pool, TempDbGuard) {
        let (pool, guard) = Pool::temporal().expect("abre");
        crate::migrations::ejecutar(&pool).await.expect("migra");

        let tracks = SqliteTrackRepository::new(pool.clone());
        let pistas: Vec<Track> = (0..3)
            .map(|i| Track {
                id: TrackId::nuevo_local(),
                title: format!("Pista {i}"),
                album: None,
                artists: vec![],
                duration: DurationMs::new(200_000),
                track_number: None,
                disc_number: None,
                explicit: false,
                isrc: None,
                release_date: None,
                popularity: None,
                added_at: Utc::now(),
            })
            .collect();
        tracks.upsert(&pistas).await.expect("guarda");

        (
            SqliteScrobbleRepository::new(pool.clone()),
            pistas.into_iter().map(|p| p.id).collect(),
            pool,
            guard,
        )
    }

    #[tokio::test]
    async fn las_pendientes_salen_en_orden_de_escucha() {
        // Last.fm construye una linea temporal con los timestamps. Entregarlas
        // desordenadas no rompe nada visiblemente, pero deja el perfil del
        // usuario contando una historia que no paso asi.
        let (repo, pistas, _pool, _g) = ctx().await;
        let ahora = Utc::now();

        repo.enqueue(&pistas[0], ahora - chrono::Duration::seconds(10))
            .await
            .expect("encola");
        repo.enqueue(&pistas[1], ahora - chrono::Duration::seconds(600))
            .await
            .expect("encola");
        repo.enqueue(&pistas[2], ahora - chrono::Duration::seconds(300))
            .await
            .expect("encola");

        let cola = repo.pending(10).await.expect("consulta");
        assert_eq!(
            cola.iter().map(|s| s.track_id.clone()).collect::<Vec<_>>(),
            vec![pistas[1].clone(), pistas[2].clone(), pistas[0].clone()]
        );
    }

    #[tokio::test]
    async fn un_fallo_deja_la_fila_y_cuenta_el_intento() {
        // Es la garantia entera del modulo: si un intento fallido borrara la
        // fila, quedarse sin red perderia el scrobble.
        let (repo, pistas, _pool, _g) = ctx().await;
        repo.enqueue(&pistas[0], Utc::now()).await.expect("encola");

        let cola = repo.pending(10).await.expect("consulta");
        repo.mark_failed(&[cola[0].id], "sin conexion")
            .await
            .expect("marca");

        let cola = repo.pending(10).await.expect("consulta");
        assert_eq!(cola.len(), 1, "la fila sigue en la cola");
        assert_eq!(cola[0].attempts, 1);
    }

    #[tokio::test]
    async fn entregar_saca_solo_las_entregadas() {
        let (repo, pistas, _pool, _g) = ctx().await;
        for p in &pistas {
            repo.enqueue(p, Utc::now()).await.expect("encola");
        }

        let cola = repo.pending(10).await.expect("consulta");
        repo.remove(&[cola[0].id, cola[2].id]).await.expect("borra");

        let quedan = repo.pending(10).await.expect("consulta");
        assert_eq!(quedan.len(), 1);
        assert_eq!(quedan[0].id, cola[1].id);
    }

    #[tokio::test]
    async fn lo_que_lastfm_ya_no_acepta_se_descarta() {
        // Catorce dias es el limite del servicio: guardar mas es acumular filas
        // que no van a salir jamas.
        let (repo, pistas, _pool, _g) = ctx().await;
        repo.enqueue(&pistas[0], Utc::now() - chrono::Duration::days(20))
            .await
            .expect("encola");
        repo.enqueue(&pistas[1], Utc::now()).await.expect("encola");

        assert_eq!(repo.purge_older_than(14).await.expect("purga"), 1);
        let quedan = repo.pending(10).await.expect("consulta");
        assert_eq!(quedan.len(), 1);
        assert_eq!(quedan[0].track_id, pistas[1]);
    }

    #[tokio::test]
    async fn borrar_la_pista_arrastra_sus_scrobbles() {
        let (repo, pistas, pool, _g) = ctx().await;
        repo.enqueue(&pistas[0], Utc::now()).await.expect("encola");

        let id = pistas[0].as_str().to_owned();
        pool.escribir(move |tx| {
            tx.execute("DELETE FROM tracks WHERE id = ?1", [&id])?;
            Ok(())
        })
        .await
        .expect("borra");

        assert_eq!(repo.count().await.expect("cuenta"), 0);
    }
}

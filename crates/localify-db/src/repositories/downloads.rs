//! Repositorios de la capa de descarga: trabajos y emparejamientos con YouTube.
//!
//! Ambas tablas son **caché operativa**, no dominio. Borrarlas cuesta tiempo
//! (habría que volver a emparejar) pero nunca pierde biblioteca: los ficheros
//! de audio y sus etiquetas siguen en disco.

use std::path::PathBuf;

use async_trait::async_trait;
use localify_core::domain::audio::DurationMs;
use localify_core::domain::download::{DownloadJob, MatchResult, ScoreBreakdown, YoutubeCandidate};
use localify_core::domain::ids::TrackId;
use localify_core::error::CoreResult;
use localify_core::ports::database::{DownloadJobRepository, YoutubeMatchRepository};
use rusqlite::params;

use crate::error::{DbResult, ToCore};
use crate::mappers::{
    a_estado_descarga, a_prioridad, de_confianza, de_estado_descarga, de_prioridad,
};
use crate::pool::Pool;

// ─────────────────────────────────────────────────────────────────────────────
// Trabajos de descarga
// ─────────────────────────────────────────────────────────────────────────────

pub struct SqliteDownloadJobRepository {
    pool: Pool,
}

impl std::fmt::Debug for SqliteDownloadJobRepository {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteDownloadJobRepository")
            .finish_non_exhaustive()
    }
}

impl SqliteDownloadJobRepository {
    #[must_use]
    pub const fn new(pool: Pool) -> Self {
        Self { pool }
    }
}

const COLUMNAS_JOB: &str = "track_id, state, priority, video_id, tmp_path,
                            bytes_done, bytes_total, attempts, last_error";

fn a_job(row: &rusqlite::Row<'_>) -> DbResult<DownloadJob> {
    let estado: String = row.get("state")?;
    let prioridad: String = row.get("priority")?;
    let tmp: Option<String> = row.get("tmp_path")?;

    Ok(DownloadJob {
        track_id: TrackId::from_trusted(row.get::<_, String>("track_id")?),
        state: a_estado_descarga(&estado)?,
        priority: a_prioridad(&prioridad),
        video_id: row.get("video_id")?,
        tmp_path: tmp.map(PathBuf::from),
        bytes_done: row.get::<_, i64>("bytes_done")?.max(0).unsigned_abs(),
        bytes_total: row
            .get::<_, Option<i64>>("bytes_total")?
            .map(|b| b.max(0).unsigned_abs()),
        attempts: u8::try_from(row.get::<_, i64>("attempts")?.clamp(0, 255)).unwrap_or(0),
        last_error_key: row.get("last_error")?,
    })
}

#[async_trait]
impl DownloadJobRepository for SqliteDownloadJobRepository {
    async fn upsert(&self, job: &DownloadJob) -> CoreResult<()> {
        let j = job.clone();
        self.pool
            .escribir(move |tx| {
                tx.execute(
                    "INSERT INTO download_jobs (
                         track_id, state, priority, video_id, tmp_path,
                         bytes_done, bytes_total, attempts, last_error, started_at, updated_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, unixepoch(), unixepoch())
                     ON CONFLICT (track_id) DO UPDATE SET
                         state       = ?2,
                         priority    = ?3,
                         video_id    = ?4,
                         tmp_path    = ?5,
                         bytes_done  = ?6,
                         bytes_total = ?7,
                         attempts    = ?8,
                         last_error  = ?9,
                         updated_at  = unixepoch()",
                    params![
                        j.track_id.as_str(),
                        de_estado_descarga(j.state),
                        de_prioridad(j.priority),
                        j.video_id,
                        j.tmp_path.as_ref().map(|p| p.to_string_lossy().to_string()),
                        i64::try_from(j.bytes_done).unwrap_or(i64::MAX),
                        j.bytes_total.map(|b| i64::try_from(b).unwrap_or(i64::MAX)),
                        i64::from(j.attempts),
                        j.last_error_key,
                    ],
                )?;
                Ok(())
            })
            .await
            .to_core()
    }

    async fn get(&self, track: &TrackId) -> CoreResult<Option<DownloadJob>> {
        let id = track.as_str().to_owned();
        let sql = format!("SELECT {COLUMNAS_JOB} FROM download_jobs WHERE track_id = ?1");

        self.pool
            .leer(move |conn| {
                let mut stmt = conn.prepare_cached(&sql)?;
                let mut filas = stmt.query([&id])?;
                match filas.next()? {
                    Some(row) => Ok(Some(a_job(row)?)),
                    None => Ok(None),
                }
            })
            .await
            .to_core()
    }

    async fn delete(&self, track: &TrackId) -> CoreResult<()> {
        let id = track.as_str().to_owned();
        self.pool
            .escribir(move |tx| {
                tx.execute("DELETE FROM download_jobs WHERE track_id = ?1", [&id])?;
                Ok(())
            })
            .await
            .to_core()
    }

    async fn interrupted(&self) -> CoreResult<Vec<DownloadJob>> {
        // Trabajos que quedaron a medias al cerrarse la aplicación. Se
        // reencolan desde cero: reanudar una descarga parcial arriesgaría un
        // fichero mal concatenado, y la regla es no dejar nunca nada corrupto.
        let sql = format!(
            "SELECT {COLUMNAS_JOB} FROM download_jobs
             WHERE state IN ('matching', 'downloading', 'finalizing')
             ORDER BY priority ASC, updated_at ASC"
        );

        self.pool
            .leer(move |conn| {
                let mut stmt = conn.prepare_cached(&sql)?;
                let mut filas = stmt.query([])?;
                let mut jobs = Vec::new();
                while let Some(row) = filas.next()? {
                    jobs.push(a_job(row)?);
                }
                Ok(jobs)
            })
            .await
            .to_core()
    }

    async fn failed(&self) -> CoreResult<Vec<DownloadJob>> {
        let sql = format!(
            "SELECT {COLUMNAS_JOB} FROM download_jobs
             WHERE state = 'failed'
             ORDER BY updated_at DESC"
        );

        self.pool
            .leer(move |conn| {
                let mut stmt = conn.prepare_cached(&sql)?;
                let mut filas = stmt.query([])?;
                let mut jobs = Vec::new();
                while let Some(row) = filas.next()? {
                    jobs.push(a_job(row)?);
                }
                Ok(jobs)
            })
            .await
            .to_core()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Emparejamientos con YouTube
// ─────────────────────────────────────────────────────────────────────────────

pub struct SqliteYoutubeMatchRepository {
    pool: Pool,
}

impl std::fmt::Debug for SqliteYoutubeMatchRepository {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteYoutubeMatchRepository")
            .finish_non_exhaustive()
    }
}

impl SqliteYoutubeMatchRepository {
    #[must_use]
    pub const fn new(pool: Pool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl YoutubeMatchRepository for SqliteYoutubeMatchRepository {
    async fn best_for(&self, track: &TrackId) -> CoreResult<Option<YoutubeCandidate>> {
        let id = track.as_str().to_owned();
        self.pool
            .leer(move |conn| {
                // Los rechazados quedan excluidos: si el usuario dijo que ese
                // vídeo no era, volver a elegirlo sería ignorarle.
                let mut stmt = conn.prepare_cached(
                    "SELECT video_id, title, channel, duration_s, view_count,
                            from_music, score, breakdown
                     FROM youtube_matches
                     WHERE track_id = ?1 AND rejected = 0
                     ORDER BY score DESC
                     LIMIT 1",
                )?;

                let mut filas = stmt.query([&id])?;
                let Some(row) = filas.next()? else {
                    return Ok(None);
                };

                let breakdown: String = row.get("breakdown")?;
                let duracion: Option<i64> = row.get("duration_s")?;

                Ok(Some(YoutubeCandidate {
                    video_id: row.get("video_id")?,
                    title: row.get("title")?,
                    channel: row.get("channel")?,
                    duration: DurationMs::from_secs(
                        u32::try_from(duracion.unwrap_or(0).max(0)).unwrap_or(0),
                    ),
                    view_count: row
                        .get::<_, Option<i64>>("view_count")?
                        .map(|v| v.max(0).unsigned_abs()),
                    from_youtube_music: row.get::<_, i64>("from_music")? != 0,
                    #[allow(
                        clippy::cast_possible_truncation,
                        reason = "la puntuación vive en [0, 100]"
                    )]
                    score: row.get::<_, f64>("score")? as f32,
                    // Un desglose ilegible no debe impedir usar el
                    // emparejamiento: solo sirve para explicarlo.
                    breakdown: serde_json::from_str(&breakdown)
                        .unwrap_or_else(|_| ScoreBreakdown::default()),
                }))
            })
            .await
            .to_core()
    }

    async fn save(&self, result: &MatchResult) -> CoreResult<()> {
        let r = result.clone();
        let breakdown =
            serde_json::to_string(&r.best.breakdown).unwrap_or_else(|_| "{}".to_owned());

        self.pool
            .escribir(move |tx| {
                tx.execute(
                    "INSERT INTO youtube_matches (
                         track_id, video_id, title, channel, duration_s, view_count,
                         from_music, score, confidence, breakdown
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                     ON CONFLICT (track_id, video_id) DO UPDATE SET
                         title      = ?3,
                         channel    = ?4,
                         duration_s = ?5,
                         view_count = ?6,
                         from_music = ?7,
                         score      = ?8,
                         confidence = ?9,
                         breakdown  = ?10",
                    params![
                        r.track_id.as_str(),
                        r.best.video_id,
                        r.best.title,
                        r.best.channel,
                        i64::from(r.best.duration.as_secs()),
                        r.best
                            .view_count
                            .map(|v| i64::try_from(v).unwrap_or(i64::MAX)),
                        i64::from(r.best.from_youtube_music),
                        f64::from(r.best.score),
                        de_confianza(r.confidence),
                        breakdown,
                    ],
                )?;
                Ok(())
            })
            .await
            .to_core()
    }

    async fn reject(&self, track: &TrackId, video_id: &str) -> CoreResult<()> {
        let (id, video) = (track.as_str().to_owned(), video_id.to_owned());
        self.pool
            .escribir(move |tx| {
                tx.execute(
                    "INSERT INTO youtube_matches
                         (track_id, video_id, title, score, confidence, breakdown, rejected)
                     VALUES (?1, ?2, '', 0.0, 'low', '{}', 1)
                     ON CONFLICT (track_id, video_id) DO UPDATE SET rejected = 1",
                    params![id, video],
                )?;
                Ok(())
            })
            .await
            .to_core()
    }

    async fn rejected_ids(&self, track: &TrackId) -> CoreResult<Vec<String>> {
        let id = track.as_str().to_owned();
        self.pool
            .leer(move |conn| {
                let mut stmt = conn.prepare_cached(
                    "SELECT video_id FROM youtube_matches
                     WHERE track_id = ?1 AND rejected = 1",
                )?;
                let ids = stmt
                    .query_map([&id], |r| r.get::<_, String>(0))?
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(ids)
            })
            .await
            .to_core()
    }

    async fn clear(&self, track: &TrackId) -> CoreResult<()> {
        let id = track.as_str().to_owned();
        self.pool
            .escribir(move |tx| {
                tx.execute("DELETE FROM youtube_matches WHERE track_id = ?1", [&id])?;
                Ok(())
            })
            .await
            .to_core()
    }
}

#[cfg(test)]
mod tests {
    use localify_core::domain::download::{Confidence, DownloadState, Priority};
    use localify_core::domain::track::Track;
    use localify_core::ports::database::TrackRepository;

    use super::*;
    use crate::pool::TempDbGuard;
    use crate::repositories::tracks::SqliteTrackRepository;

    struct Ctx {
        jobs: SqliteDownloadJobRepository,
        matches: SqliteYoutubeMatchRepository,
        pool: Pool,
        _guard: TempDbGuard,
    }

    async fn ctx() -> Ctx {
        let (pool, guard) = Pool::temporal().expect("abre");
        crate::migrations::ejecutar(&pool).await.expect("migra");
        Ctx {
            jobs: SqliteDownloadJobRepository::new(pool.clone()),
            matches: SqliteYoutubeMatchRepository::new(pool.clone()),
            pool,
            _guard: guard,
        }
    }

    async fn nueva_pista(pool: &Pool) -> TrackId {
        let repo = SqliteTrackRepository::new(pool.clone());
        let t = Track {
            id: TrackId::nuevo_local(),
            title: "X".into(),
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

    fn job(track: &TrackId, estado: DownloadState) -> DownloadJob {
        DownloadJob {
            track_id: track.clone(),
            state: estado,
            priority: Priority::Immediate,
            video_id: Some("dQw4w9WgXcQ".into()),
            tmp_path: Some(PathBuf::from(".tmp/x.webm.part")),
            bytes_done: 1024,
            bytes_total: Some(4096),
            attempts: 1,
            last_error_key: None,
        }
    }

    fn candidato(score: f32) -> YoutubeCandidate {
        YoutubeCandidate {
            video_id: format!("vid{score}"),
            title: "Bohemian Rhapsody".into(),
            channel: Some("Queen Official".into()),
            duration: DurationMs::from_secs(355),
            view_count: Some(1_000_000),
            from_youtube_music: true,
            score,
            breakdown: ScoreBreakdown {
                duration_factor: 1.0,
                duration_diff_ms: 500,
                source_bonus: 30.0,
                title_bonus: 20.0,
                artist_bonus: 15.0,
                album_bonus: 10.0,
                penalties: 0.0,
                penalty_reasons: vec![],
                total: score,
            },
        }
    }

    #[tokio::test]
    async fn un_trabajo_hace_ida_y_vuelta() {
        let c = ctx().await;
        let id = nueva_pista(&c.pool).await;

        c.jobs
            .upsert(&job(&id, DownloadState::Downloading))
            .await
            .expect("guarda");

        let leido = c.jobs.get(&id).await.expect("lee").expect("existe");
        assert_eq!(leido.state, DownloadState::Downloading);
        assert_eq!(leido.priority, Priority::Immediate);
        assert_eq!(leido.bytes_done, 1024);
        assert_eq!(leido.bytes_total, Some(4096));
        assert_eq!(leido.video_id.as_deref(), Some("dQw4w9WgXcQ"));
    }

    #[tokio::test]
    async fn los_trabajos_a_medias_se_recuperan_al_arrancar() {
        let c = ctx().await;
        let a = nueva_pista(&c.pool).await;
        let b = nueva_pista(&c.pool).await;
        let hecha = nueva_pista(&c.pool).await;
        let fallida = nueva_pista(&c.pool).await;

        c.jobs
            .upsert(&job(&a, DownloadState::Downloading))
            .await
            .expect("a");
        c.jobs
            .upsert(&job(&b, DownloadState::Finalizing))
            .await
            .expect("b");
        c.jobs
            .upsert(&job(&hecha, DownloadState::Done))
            .await
            .expect("hecha");
        c.jobs
            .upsert(&job(&fallida, DownloadState::Failed))
            .await
            .expect("fallida");

        let interrumpidos = c.jobs.interrupted().await.expect("consulta");
        let ids: Vec<_> = interrumpidos.iter().map(|j| j.track_id.clone()).collect();

        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&a) && ids.contains(&b));
        assert!(!ids.contains(&hecha), "un trabajo terminado no se reencola");
        assert!(
            !ids.contains(&fallida),
            "un fallo se reintenta de forma explícita"
        );
    }

    #[tokio::test]
    async fn los_fallidos_se_consultan_aparte() {
        let c = ctx().await;
        let id = nueva_pista(&c.pool).await;
        let mut j = job(&id, DownloadState::Failed);
        j.last_error_key = Some("download.no_match".into());
        j.attempts = 3;
        c.jobs.upsert(&j).await.expect("guarda");

        let fallidos = c.jobs.failed().await.expect("consulta");
        assert_eq!(fallidos.len(), 1);
        assert_eq!(fallidos[0].attempts, 3);
        assert_eq!(
            fallidos[0].last_error_key.as_deref(),
            Some("download.no_match")
        );
    }

    #[tokio::test]
    async fn el_mejor_emparejamiento_es_el_de_mayor_puntuacion() {
        let c = ctx().await;
        let id = nueva_pista(&c.pool).await;

        for score in [55.0_f32, 91.0, 70.0] {
            c.matches
                .save(&MatchResult {
                    track_id: id.clone(),
                    best: candidato(score),
                    confidence: Confidence::desde_puntuacion(score),
                    candidates_considered: 10,
                })
                .await
                .expect("guarda");
        }

        let mejor = c.matches.best_for(&id).await.expect("lee").expect("existe");
        assert!((mejor.score - 91.0).abs() < 0.01);
        assert!(mejor.from_youtube_music);
        assert_eq!(mejor.duration, DurationMs::from_secs(355));
    }

    #[tokio::test]
    async fn el_desglose_de_puntuacion_se_conserva() {
        let c = ctx().await;
        let id = nueva_pista(&c.pool).await;
        c.matches
            .save(&MatchResult {
                track_id: id.clone(),
                best: candidato(91.0),
                confidence: Confidence::High,
                candidates_considered: 10,
            })
            .await
            .expect("guarda");

        let mejor = c.matches.best_for(&id).await.expect("lee").expect("existe");
        assert!((mejor.breakdown.source_bonus - 30.0).abs() < 0.01);
        assert_eq!(mejor.breakdown.duration_diff_ms, 500);
    }

    #[tokio::test]
    async fn un_video_rechazado_deja_de_elegirse() {
        let c = ctx().await;
        let id = nueva_pista(&c.pool).await;

        let bueno = candidato(91.0);
        let alternativo = candidato(70.0);
        for cand in [bueno.clone(), alternativo.clone()] {
            let score = cand.score;
            c.matches
                .save(&MatchResult {
                    track_id: id.clone(),
                    best: cand,
                    confidence: Confidence::desde_puntuacion(score),
                    candidates_considered: 10,
                })
                .await
                .expect("guarda");
        }

        c.matches
            .reject(&id, &bueno.video_id)
            .await
            .expect("rechaza");

        let mejor = c.matches.best_for(&id).await.expect("lee").expect("existe");
        assert_eq!(
            mejor.video_id, alternativo.video_id,
            "si el usuario dijo que no era, volver a elegirlo sería ignorarle"
        );
        assert_eq!(
            c.matches.rejected_ids(&id).await.expect("consulta"),
            vec![bueno.video_id]
        );
    }

    #[tokio::test]
    async fn rechazar_un_video_que_no_estaba_registrado_funciona_igual() {
        // Ocurre cuando el usuario rechaza tras un rematch que no llegó a
        // persistirse.
        let c = ctx().await;
        let id = nueva_pista(&c.pool).await;

        c.matches.reject(&id, "desconocido").await.expect("rechaza");
        assert_eq!(
            c.matches.rejected_ids(&id).await.expect("consulta"),
            vec!["desconocido".to_owned()]
        );
        assert!(c.matches.best_for(&id).await.expect("consulta").is_none());
    }

    #[tokio::test]
    async fn borrar_la_pista_arrastra_trabajos_y_emparejamientos() {
        let c = ctx().await;
        let id = nueva_pista(&c.pool).await;
        c.jobs
            .upsert(&job(&id, DownloadState::Queued))
            .await
            .expect("job");
        c.matches
            .save(&MatchResult {
                track_id: id.clone(),
                best: candidato(91.0),
                confidence: Confidence::High,
                candidates_considered: 1,
            })
            .await
            .expect("match");

        let txt = id.as_str().to_owned();
        c.pool
            .escribir(move |tx| {
                tx.execute("DELETE FROM tracks WHERE id = ?1", [&txt])?;
                Ok(())
            })
            .await
            .expect("borra");

        assert!(c.jobs.get(&id).await.expect("consulta").is_none());
        assert!(c.matches.best_for(&id).await.expect("consulta").is_none());
    }

    #[tokio::test]
    async fn borrar_los_emparejamientos_de_una_pista_olvida_aceptados_y_rechazados() {
        // Se usa al resetear o reasignar metadatos: con una identidad nueva, ni
        // el vídeo elegido ni los rechazados dicen nada de la pista.
        let c = ctx().await;
        let id = nueva_pista(&c.pool).await;
        c.matches
            .save(&MatchResult {
                track_id: id.clone(),
                best: candidato(91.0),
                confidence: Confidence::High,
                candidates_considered: 1,
            })
            .await
            .expect("match");
        c.matches.reject(&id, "otro_video").await.expect("rechaza");

        c.matches.clear(&id).await.expect("olvida");

        assert!(c.matches.best_for(&id).await.expect("consulta").is_none());
        assert!(c.matches.rejected_ids(&id).await.expect("consulta").is_empty());
    }
}

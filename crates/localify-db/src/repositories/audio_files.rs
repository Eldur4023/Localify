//! Repositorio de ficheros de audio.
//!
//! La existencia de una fila aquí **es** la definición de que una pista está en
//! la biblioteca. No hay estado intermedio persistido: los ficheros a medias
//! viven en `.tmp/` y jamás se registran.

use std::path::PathBuf;

use async_trait::async_trait;
use localify_core::domain::audio::DurationMs;
use localify_core::domain::availability::Availability;
use localify_core::domain::ids::TrackId;
use localify_core::domain::library::AudioFileRecord;
use localify_core::error::CoreResult;
use localify_core::page::{Cursor, Page, PageRequest};
use localify_core::ports::database::AudioFileRepository;
use rusqlite::params;

use crate::error::{DbResult, ToCore};
use crate::mappers::{a_fecha, a_formato, a_origen_audio, de_fecha, de_origen_audio};
use crate::pool::Pool;

pub struct SqliteAudioFileRepository {
    pool: Pool,
}

impl std::fmt::Debug for SqliteAudioFileRepository {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteAudioFileRepository")
            .finish_non_exhaustive()
    }
}

impl SqliteAudioFileRepository {
    #[must_use]
    pub const fn new(pool: Pool) -> Self {
        Self { pool }
    }
}

const COLUMNAS: &str = "track_id, rel_path, format, codec, bitrate_kbps, sample_rate,
                        channels, size_bytes, duration_ms, source, youtube_id, verified_at";

fn a_registro(row: &rusqlite::Row<'_>) -> DbResult<AudioFileRecord> {
    let formato: String = row.get("format")?;
    let origen: String = row.get("source")?;

    Ok(AudioFileRecord {
        track_id: TrackId::from_trusted(row.get::<_, String>("track_id")?),
        rel_path: PathBuf::from(row.get::<_, String>("rel_path")?),
        format: a_formato(&formato)?,
        codec: row.get("codec")?,
        bitrate_kbps: row.get("bitrate_kbps")?,
        sample_rate: row.get("sample_rate")?,
        channels: row.get("channels")?,
        size_bytes: row.get::<_, i64>("size_bytes")?.max(0).unsigned_abs(),
        duration: DurationMs::new(u32::try_from(row.get::<_, i64>("duration_ms")?).unwrap_or(0)),
        source: a_origen_audio(&origen),
        youtube_id: row.get("youtube_id")?,
        verified_at: a_fecha(row.get::<_, i64>("verified_at")?),
    })
}

#[async_trait]
impl AudioFileRepository for SqliteAudioFileRepository {
    async fn get(&self, track: &TrackId) -> CoreResult<Option<AudioFileRecord>> {
        let id = track.as_str().to_owned();
        let sql = format!("SELECT {COLUMNAS} FROM audio_files WHERE track_id = ?1");

        self.pool
            .leer(move |conn| {
                let mut stmt = conn.prepare_cached(&sql)?;
                let mut filas = stmt.query([&id])?;
                match filas.next()? {
                    Some(row) => Ok(Some(a_registro(row)?)),
                    None => Ok(None),
                }
            })
            .await
            .to_core()
    }

    async fn availability(&self, tracks: &[TrackId]) -> CoreResult<Vec<(TrackId, Availability)>> {
        if tracks.is_empty() {
            return Ok(Vec::new());
        }
        let ids: Vec<String> = tracks.iter().map(|t| t.as_str().to_owned()).collect();

        // Una sola consulta para la ventana visible de la lista. Sin esto,
        // habría una llamada por fila al hacer scroll.
        self.pool
            .leer(move |conn| {
                let marcadores = vec!["?"; ids.len()].join(",");
                let sql = format!(
                    "SELECT t.id,
                            af.rel_path, af.format, af.size_bytes,
                            dj.state AS dl_state, dj.bytes_done, dj.bytes_total,
                            dj.attempts, dj.last_error
                     FROM tracks t
                     LEFT JOIN audio_files   af ON af.track_id = t.id
                     LEFT JOIN download_jobs dj ON dj.track_id = t.id
                     WHERE t.id IN ({marcadores})"
                );

                let refs: Vec<&dyn rusqlite::ToSql> =
                    ids.iter().map(|s| s as &dyn rusqlite::ToSql).collect();

                let mut stmt = conn.prepare(&sql)?;
                let filas = stmt
                    .query_map(refs.as_slice(), |row| {
                        Ok((
                            row.get::<_, String>("id")?,
                            crate::mappers::disponibilidad_de_fila(row),
                        ))
                    })?
                    .collect::<Result<Vec<_>, _>>()?;

                let mut resultado = Vec::with_capacity(filas.len());
                for (id, disp) in filas {
                    resultado.push((TrackId::from_trusted(id), disp?));
                }
                Ok(resultado)
            })
            .await
            .to_core()
    }

    async fn insert(&self, record: &AudioFileRecord) -> CoreResult<()> {
        let r = record.clone();
        self.pool
            .escribir(move |tx| {
                tx.execute(
                    "INSERT INTO audio_files (
                         track_id, rel_path, format, codec, bitrate_kbps, sample_rate,
                         channels, size_bytes, duration_ms, source, youtube_id, verified_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
                     ON CONFLICT (track_id) DO UPDATE SET
                         rel_path     = ?2,
                         format       = ?3,
                         codec        = ?4,
                         bitrate_kbps = ?5,
                         sample_rate  = ?6,
                         channels     = ?7,
                         size_bytes   = ?8,
                         duration_ms  = ?9,
                         source       = ?10,
                         youtube_id   = ?11,
                         verified_at  = ?12",
                    params![
                        r.track_id.as_str(),
                        r.rel_path.to_string_lossy().replace('\\', "/"),
                        r.format.extension(),
                        r.codec,
                        r.bitrate_kbps,
                        r.sample_rate,
                        r.channels,
                        i64::try_from(r.size_bytes).unwrap_or(i64::MAX),
                        i64::from(r.duration.as_ms()),
                        de_origen_audio(r.source),
                        r.youtube_id,
                        de_fecha(r.verified_at),
                    ],
                )?;

                // El trabajo de descarga ya cumplió su función. Dejarlo haría
                // que un reinicio lo reencolara para una pista que ya está.
                tx.execute(
                    "DELETE FROM download_jobs WHERE track_id = ?1",
                    [r.track_id.as_str()],
                )?;
                Ok(())
            })
            .await
            .to_core()
    }

    async fn delete(&self, track: &TrackId) -> CoreResult<()> {
        let id = track.as_str().to_owned();
        self.pool
            .escribir(move |tx| {
                tx.execute("DELETE FROM audio_files WHERE track_id = ?1", [&id])?;
                Ok(())
            })
            .await
            .to_core()
    }

    async fn list_all(&self, page: &PageRequest) -> CoreResult<Page<AudioFileRecord>> {
        let limite = i64::from(page.limit());
        let offset = i64::from(page.offset());
        // Orden estable por clave primaria: `rescan` recorre todo en páginas y
        // una inserción a mitad no debe hacerle saltarse un fichero.
        let sql =
            format!("SELECT {COLUMNAS} FROM audio_files ORDER BY track_id ASC LIMIT ?1 OFFSET ?2");

        self.pool
            .leer(move |conn| {
                let total: i64 =
                    conn.query_row("SELECT COUNT(*) FROM audio_files", [], |r| r.get(0))?;
                let total = total.max(0).unsigned_abs();

                let mut stmt = conn.prepare_cached(&sql)?;
                let mut filas = stmt.query(params![limite, offset])?;
                let mut items = Vec::new();
                while let Some(row) = filas.next()? {
                    items.push(a_registro(row)?);
                }

                let consumidos = offset.max(0).unsigned_abs() + items.len() as u64;
                let next = (consumidos < total).then(|| Cursor::new(consumidos.to_string()));

                Ok(Page::new(items, Some(total), next))
            })
            .await
            .to_core()
    }
}

#[cfg(test)]
mod tests {
    use localify_core::domain::audio::AudioFormat;
    use localify_core::domain::library::AudioSource;
    use localify_core::domain::track::Track;
    use localify_core::ports::database::TrackRepository;

    use super::*;
    use crate::pool::TempDbGuard;
    use crate::repositories::tracks::SqliteTrackRepository;

    async fn ctx() -> (
        SqliteAudioFileRepository,
        SqliteTrackRepository,
        Pool,
        TempDbGuard,
    ) {
        let (pool, guard) = Pool::temporal().expect("abre");
        crate::migrations::ejecutar(&pool).await.expect("migra");
        (
            SqliteAudioFileRepository::new(pool.clone()),
            SqliteTrackRepository::new(pool.clone()),
            pool,
            guard,
        )
    }

    fn pista() -> Track {
        Track {
            id: TrackId::nuevo_local(),
            title: "X".into(),
            album: None,
            artists: vec![],
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

    fn registro(track: &TrackId) -> AudioFileRecord {
        AudioFileRecord {
            track_id: track.clone(),
            rel_path: PathBuf::from("audio").join("3z").join("x.opus"),
            format: AudioFormat::Opus,
            codec: "opus".into(),
            bitrate_kbps: Some(160),
            sample_rate: Some(48_000),
            channels: Some(2),
            size_bytes: 5_000_000,
            duration: DurationMs::new(248_100),
            source: AudioSource::Youtube,
            youtube_id: Some("dQw4w9WgXcQ".into()),
            verified_at: chrono::Utc::now(),
        }
    }

    #[tokio::test]
    async fn un_registro_guardado_se_recupera_igual() {
        let (repo, tracks, _pool, _g) = ctx().await;
        let t = pista();
        tracks
            .upsert(std::slice::from_ref(&t))
            .await
            .expect("guarda pista");

        let r = registro(&t.id);
        repo.insert(&r).await.expect("guarda fichero");

        let leido = repo.get(&t.id).await.expect("lee").expect("existe");
        assert_eq!(leido.format, AudioFormat::Opus);
        assert_eq!(leido.bitrate_kbps, Some(160));
        assert_eq!(leido.size_bytes, 5_000_000);
        assert_eq!(leido.duration, DurationMs::new(248_100));
        assert_eq!(leido.youtube_id.as_deref(), Some("dQw4w9WgXcQ"));
    }

    #[tokio::test]
    async fn las_rutas_se_guardan_con_barras_normales() {
        // La base de datos debe ser portable entre sistemas: una ruta con
        // barras invertidas no se resolvería en Linux.
        let (repo, tracks, pool, _g) = ctx().await;
        let t = pista();
        tracks
            .upsert(std::slice::from_ref(&t))
            .await
            .expect("guarda");
        repo.insert(&registro(&t.id)).await.expect("guarda fichero");

        let ruta: String = pool
            .leer(|c| Ok(c.query_row("SELECT rel_path FROM audio_files", [], |r| r.get(0))?))
            .await
            .expect("lee");
        assert_eq!(ruta, "audio/3z/x.opus");
        assert!(!ruta.contains('\\'));
    }

    #[tokio::test]
    async fn registrar_el_fichero_cierra_el_trabajo_de_descarga() {
        let (repo, tracks, pool, _g) = ctx().await;
        let t = pista();
        tracks
            .upsert(std::slice::from_ref(&t))
            .await
            .expect("guarda");

        let id = t.id.as_str().to_owned();
        pool.escribir(move |tx| {
            tx.execute(
                "INSERT INTO download_jobs (track_id, state) VALUES (?1, 'downloading')",
                [&id],
            )?;
            Ok(())
        })
        .await
        .expect("crea job");

        repo.insert(&registro(&t.id)).await.expect("guarda fichero");

        let jobs: i64 = pool
            .leer(|c| Ok(c.query_row("SELECT COUNT(*) FROM download_jobs", [], |r| r.get(0))?))
            .await
            .expect("cuenta");
        assert_eq!(
            jobs, 0,
            "un job vivo reencolaría una pista ya descargada al reiniciar"
        );
    }

    #[tokio::test]
    async fn availability_resuelve_varias_pistas_de_una_vez() {
        let (repo, tracks, pool, _g) = ctx().await;
        let local = pista();
        let ausente = pista();
        let fallida = pista();
        tracks
            .upsert(&[local.clone(), ausente.clone(), fallida.clone()])
            .await
            .expect("guarda");

        repo.insert(&registro(&local.id))
            .await
            .expect("guarda fichero");

        let id = fallida.id.as_str().to_owned();
        pool.escribir(move |tx| {
            tx.execute(
                "INSERT INTO download_jobs (track_id, state, attempts, last_error)
                 VALUES (?1, 'failed', 3, 'download.no_match')",
                [&id],
            )?;
            Ok(())
        })
        .await
        .expect("crea job fallido");

        let estados = repo
            .availability(&[local.id.clone(), ausente.id.clone(), fallida.id.clone()])
            .await
            .expect("consulta");

        let por_id: std::collections::HashMap<_, _> = estados.into_iter().collect();
        assert!(por_id[&local.id].es_local());
        assert_eq!(por_id[&ausente.id], Availability::Absent);
        assert_eq!(
            por_id[&fallida.id],
            Availability::Failed {
                reason_key: "download.no_match".into(),
                attempts: 3
            }
        );
    }

    #[tokio::test]
    async fn borrar_la_pista_arrastra_su_fichero() {
        let (repo, tracks, pool, _g) = ctx().await;
        let t = pista();
        tracks
            .upsert(std::slice::from_ref(&t))
            .await
            .expect("guarda");
        repo.insert(&registro(&t.id)).await.expect("guarda fichero");

        let id = t.id.as_str().to_owned();
        pool.escribir(move |tx| {
            tx.execute("DELETE FROM tracks WHERE id = ?1", [&id])?;
            Ok(())
        })
        .await
        .expect("borra");

        assert!(repo.get(&t.id).await.expect("consulta").is_none());
    }

    #[tokio::test]
    async fn list_all_pagina_sin_perder_registros() {
        let (repo, tracks, _pool, _g) = ctx().await;
        let pistas: Vec<Track> = (0..25).map(|_| pista()).collect();
        tracks.upsert(&pistas).await.expect("guarda");

        for (i, t) in pistas.iter().enumerate() {
            let mut r = registro(&t.id);
            r.rel_path = PathBuf::from(format!("audio/aa/{i}.opus"));
            repo.insert(&r).await.expect("guarda fichero");
        }

        let mut vistos = std::collections::HashSet::new();
        let mut offset = 0_u32;
        loop {
            let pagina = repo
                .list_all(&PageRequest::new(offset, 10))
                .await
                .expect("lista");
            for r in &pagina.items {
                assert!(
                    vistos.insert(r.track_id.as_str().to_owned()),
                    "registro repetido"
                );
            }
            if pagina.next_cursor.is_none() {
                break;
            }
            offset += 10;
        }
        assert_eq!(vistos.len(), 25);
    }
}

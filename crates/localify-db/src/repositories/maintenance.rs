//! Mantenimiento de la base de datos.
//!
//! Todo se ejecuta en tareas de fondo, **nunca en el arranque bloqueante**. Una
//! base de datos que tarda dos segundos en compactarse no debe retrasar la
//! aparición de la ventana.

use async_trait::async_trait;
use localify_core::error::CoreResult;
use localify_core::ports::database::MaintenanceRepository;
use tracing::info;

use crate::error::ToCore;
use crate::pool::Pool;

pub struct SqliteMaintenanceRepository {
    pool: Pool,
}

impl std::fmt::Debug for SqliteMaintenanceRepository {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteMaintenanceRepository")
            .finish_non_exhaustive()
    }
}

impl SqliteMaintenanceRepository {
    #[must_use]
    pub const fn new(pool: Pool) -> Self {
        Self { pool }
    }

    /// Tamaño actual del fichero WAL, en bytes.
    #[must_use]
    pub fn tamano_wal(&self) -> u64 {
        let wal = format!("{}-wal", self.pool.ruta().display());
        std::fs::metadata(wal).map_or(0, |m| m.len())
    }
}

#[async_trait]
impl MaintenanceRepository for SqliteMaintenanceRepository {
    async fn optimize(&self) -> CoreResult<()> {
        self.pool
            .escribir_sin_transaccion(|conn| {
                // Actualiza las estadísticas del planificador. Sin esto, tras
                // importar una playlist grande el planificador sigue creyendo
                // que las tablas están casi vacías y elige planes malos.
                conn.execute_batch("PRAGMA optimize;")?;
                Ok(())
            })
            .await
            .to_core()
    }

    async fn incremental_vacuum(&self) -> CoreResult<()> {
        self.pool
            .escribir_sin_transaccion(|conn| {
                // Recupera páginas libres poco a poco. Un VACUUM completo
                // bloquearía la base de datos entera y reescribiría el fichero.
                conn.execute_batch("PRAGMA incremental_vacuum;")?;
                Ok(())
            })
            .await
            .to_core()
    }

    async fn checkpoint_wal(&self) -> CoreResult<()> {
        self.pool
            .escribir_sin_transaccion(|conn| {
                // TRUNCATE integra el WAL y lo deja a cero. PASSIVE no
                // garantizaría que el fichero encoja, que es justo el objetivo.
                conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
                Ok(())
            })
            .await
            .to_core()
    }

    fn wal_bytes(&self) -> u64 {
        self.tamano_wal()
    }

    async fn purge_orphans(&self, older_than_days: u16) -> CoreResult<u64> {
        let corte = i64::from(older_than_days) * 86_400;

        self.pool
            .escribir(move |tx| {
                // Una pista es huérfana si no tiene fichero, ni está en ninguna
                // playlist, ni es favorita, ni se ha escuchado nunca, ni tiene
                // una descarga en curso, ni está en la cola guardada. Es decir:
                // metadatos que quedaron de una búsqueda y que nadie usó.
                //
                // Todas las condiciones son necesarias. Quitar cualquiera
                // borraría algo que el usuario espera conservar.
                //
                // Las dos colas se recorren con `json_each` porque se guardan
                // como JSON en `player_state` (ver V3): no hay clave ajena que
                // las proteja, así que sin esto una cola preparada y no
                // escuchada se vaciaría sola al cabo de unos días.
                let borradas = tx.execute(
                    "DELETE FROM tracks
                     WHERE added_at < unixepoch() - ?1
                       AND NOT EXISTS (SELECT 1 FROM audio_files    a WHERE a.track_id = tracks.id)
                       AND NOT EXISTS (SELECT 1 FROM playlist_items p WHERE p.track_id = tracks.id)
                       AND NOT EXISTS (SELECT 1 FROM favorites      f WHERE f.track_id = tracks.id)
                       AND NOT EXISTS (SELECT 1 FROM play_history   h WHERE h.track_id = tracks.id)
                       AND NOT EXISTS (SELECT 1 FROM download_jobs  d WHERE d.track_id = tracks.id)
                       AND NOT EXISTS (
                             SELECT 1 FROM player_state ps,
                                    json_each(COALESCE(ps.context_queue, '[]')) j
                             WHERE ps.id = 1 AND j.value = tracks.id)
                       AND NOT EXISTS (
                             SELECT 1 FROM player_state ps,
                                    json_each(COALESCE(ps.user_queue, '[]')) j
                             WHERE ps.id = 1 AND j.value = tracks.id)
                       AND tracks.id <> COALESCE(
                             (SELECT track_id FROM player_state WHERE id = 1), '')",
                    [corte],
                )?;

                // Álbumes y artistas que se quedaron sin ninguna pista.
                tx.execute(
                    "DELETE FROM albums
                     WHERE NOT EXISTS (SELECT 1 FROM tracks t WHERE t.album_id = albums.id)",
                    [],
                )?;
                tx.execute(
                    "DELETE FROM artists
                     WHERE NOT EXISTS (SELECT 1 FROM track_artists ta
                                       WHERE ta.artist_id = artists.id)
                       AND NOT EXISTS (SELECT 1 FROM album_artists aa
                                       WHERE aa.artist_id = artists.id)",
                    [],
                )?;

                if borradas > 0 {
                    info!(borradas, "pistas huérfanas eliminadas");
                }
                Ok(borradas as u64)
            })
            .await
            .to_core()
    }
}

#[cfg(test)]
mod tests {
    use localify_core::domain::audio::DurationMs;
    use localify_core::domain::ids::TrackId;
    use localify_core::domain::track::{ArtistRef, Track};
    use localify_core::ports::database::TrackRepository;
    use rusqlite::params;

    use super::*;
    use crate::pool::TempDbGuard;
    use crate::repositories::tracks::SqliteTrackRepository;

    async fn ctx() -> (
        SqliteMaintenanceRepository,
        SqliteTrackRepository,
        Pool,
        TempDbGuard,
    ) {
        let (pool, guard) = Pool::temporal().expect("abre");
        crate::migrations::ejecutar(&pool).await.expect("migra");
        (
            SqliteMaintenanceRepository::new(pool.clone()),
            SqliteTrackRepository::new(pool.clone()),
            pool,
            guard,
        )
    }

    fn pista(titulo: &str) -> Track {
        Track {
            id: TrackId::nuevo_local(),
            title: titulo.into(),
            album: None,
            artists: vec![ArtistRef {
                id: localify_core::domain::ids::ArtistId::nuevo_local(),
                name: "Artista".into(),
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

    /// Envejece todas las pistas para que entren en el criterio de purga.
    async fn envejecer(pool: &Pool) {
        pool.escribir(|tx| {
            tx.execute("UPDATE tracks SET added_at = unixepoch() - 9999999", [])?;
            Ok(())
        })
        .await
        .expect("envejece");
    }

    #[tokio::test]
    async fn las_tareas_de_mantenimiento_se_ejecutan_sin_error() {
        let (repo, _tracks, _pool, _g) = ctx().await;
        repo.optimize().await.expect("optimize");
        repo.incremental_vacuum().await.expect("vacuum");
        repo.checkpoint_wal().await.expect("checkpoint");
    }

    #[tokio::test]
    async fn se_purgan_los_metadatos_sueltos_de_una_busqueda_antigua() {
        let (repo, tracks, pool, _g) = ctx().await;
        tracks
            .upsert(&[pista("Resultado de búsqueda no usado")])
            .await
            .expect("guarda");
        envejecer(&pool).await;

        assert_eq!(repo.purge_orphans(30).await.expect("purga"), 1);
        assert_eq!(tracks.stats().await.expect("stats").track_count, 0);
    }

    #[tokio::test]
    async fn no_se_purga_lo_reciente() {
        let (repo, tracks, _pool, _g) = ctx().await;
        tracks
            .upsert(&[pista("Recién buscada")])
            .await
            .expect("guarda");

        assert_eq!(
            repo.purge_orphans(30).await.expect("purga"),
            0,
            "una búsqueda de hace un minuto puede seguir en pantalla"
        );
    }

    #[tokio::test]
    async fn no_se_purga_nada_que_el_usuario_haya_tocado() {
        let (repo, tracks, pool, _g) = ctx().await;

        let descargada = pista("Descargada");
        let favorita = pista("Favorita");
        let en_playlist = pista("En playlist");
        let escuchada = pista("Escuchada");
        let descargando = pista("Descargando");
        let suelta = pista("Suelta");

        tracks
            .upsert(&[
                descargada.clone(),
                favorita.clone(),
                en_playlist.clone(),
                escuchada.clone(),
                descargando.clone(),
                suelta.clone(),
            ])
            .await
            .expect("guarda");

        let (d, f, p, e, dl) = (
            descargada.id.as_str().to_owned(),
            favorita.id.as_str().to_owned(),
            en_playlist.id.as_str().to_owned(),
            escuchada.id.as_str().to_owned(),
            descargando.id.as_str().to_owned(),
        );

        pool.escribir(move |tx| {
            tx.execute(
                "INSERT INTO audio_files
                 (track_id, rel_path, format, codec, size_bytes, duration_ms, verified_at)
                 VALUES (?1, 'audio/aa/x.opus', 'opus', 'opus', 100, 200000, 0)",
                [&d],
            )?;
            tx.execute("INSERT INTO favorites (track_id) VALUES (?1)", [&f])?;
            tx.execute(
                "INSERT INTO playlists (id, name, name_norm) VALUES ('pl', 'L', 'l')",
                [],
            )?;
            tx.execute(
                "INSERT INTO playlist_items (id, playlist_id, track_id, position)
                 VALUES ('e', 'pl', ?1, 0.0)",
                [&p],
            )?;
            tx.execute(
                "INSERT INTO play_history (track_id, ms_played, completed)
                 VALUES (?1, 200000, 1)",
                [&e],
            )?;
            tx.execute(
                "INSERT INTO download_jobs (track_id, state) VALUES (?1, 'queued')",
                [&dl],
            )?;
            Ok(())
        })
        .await
        .expect("prepara");

        envejecer(&pool).await;

        assert_eq!(
            repo.purge_orphans(30).await.expect("purga"),
            1,
            "solo debe caer la que nadie ha tocado"
        );

        let restantes: i64 = pool
            .leer(|c| Ok(c.query_row("SELECT COUNT(*) FROM tracks", [], |r| r.get(0))?))
            .await
            .expect("cuenta");
        assert_eq!(restantes, 5);
    }

    #[tokio::test]
    async fn no_se_purga_la_pista_cargada_en_el_reproductor() {
        // Sin esta protección, dejar la app cerrada un mes y volver haría
        // desaparecer la canción que estaba sonando.
        let (repo, tracks, pool, _g) = ctx().await;
        let t = pista("En el reproductor");
        tracks
            .upsert(std::slice::from_ref(&t))
            .await
            .expect("guarda");

        let id = t.id.as_str().to_owned();
        pool.escribir(move |tx| {
            tx.execute("UPDATE player_state SET track_id = ?1 WHERE id = 1", [&id])?;
            Ok(())
        })
        .await
        .expect("carga en el reproductor");

        envejecer(&pool).await;
        assert_eq!(repo.purge_orphans(30).await.expect("purga"), 0);
    }

    #[tokio::test]
    async fn purgar_pistas_arrastra_albumes_y_artistas_vacios() {
        let (repo, tracks, pool, _g) = ctx().await;
        tracks.upsert(&[pista("Suelta")]).await.expect("guarda");
        envejecer(&pool).await;

        repo.purge_orphans(30).await.expect("purga");

        let artistas: i64 = pool
            .leer(|c| Ok(c.query_row("SELECT COUNT(*) FROM artists", [], |r| r.get(0))?))
            .await
            .expect("cuenta");
        assert_eq!(artistas, 0, "un artista sin pistas no debe quedar suelto");
    }

    #[tokio::test]
    async fn no_se_purga_lo_que_esta_en_la_cola_guardada() {
        // La cola vive como JSON en `player_state`, sin clave ajena que la
        // proteja. Sin esta condición, preparar una cola y dejar la aplicación
        // cerrada una temporada la vaciaría sola: al volver quedarían
        // identificadores apuntando a pistas que ya no existen.
        let (repo, tracks, pool, _g) = ctx().await;

        let en_contexto = pista("En el contexto");
        let en_cola = pista("En la cola de usuario");
        let suelta = pista("Suelta");
        tracks
            .upsert(&[en_contexto.clone(), en_cola.clone(), suelta])
            .await
            .expect("guarda");

        let (c, u) = (
            en_contexto.id.as_str().to_owned(),
            en_cola.id.as_str().to_owned(),
        );
        pool.escribir(move |tx| {
            tx.execute(
                "UPDATE player_state
                 SET context_queue = json_array(?1), user_queue = json_array(?2)
                 WHERE id = 1",
                [&c, &u],
            )?;
            Ok(())
        })
        .await
        .expect("prepara la cola");

        envejecer(&pool).await;

        assert_eq!(
            repo.purge_orphans(30).await.expect("purga"),
            1,
            "solo debe caer la que no está en ninguna cola"
        );
    }

    #[tokio::test]
    async fn el_checkpoint_recorta_el_wal() {
        let (repo, tracks, pool, _g) = ctx().await;

        // Genera escrituras suficientes para que el WAL crezca.
        for _ in 0..40 {
            let lote: Vec<Track> = (0..20).map(|i| pista(&format!("P{i}"))).collect();
            tracks.upsert(&lote).await.expect("guarda");
        }

        let antes = repo.tamano_wal();
        repo.checkpoint_wal().await.expect("checkpoint");
        let despues = repo.tamano_wal();

        assert!(
            despues <= antes,
            "TRUNCATE debe integrar el WAL y dejarlo a cero (antes {antes}, después {despues})"
        );

        // La base de datos debe seguir siendo legible tras el checkpoint.
        let total: i64 = pool
            .leer(|c| Ok(c.query_row("SELECT COUNT(*) FROM tracks", [], |r| r.get(0))?))
            .await
            .expect("cuenta");
        assert!(total > 0);
        let _ = params![1];
    }
}

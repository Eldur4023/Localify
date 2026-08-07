//! Repositorio del historial de reproducción.
//!
//! Es la materia prima del motor de recomendaciones locales. Por eso registra
//! `completed`: una pista saltada a los diez segundos es una señal negativa tan
//! informativa como una escuchada entera.

use async_trait::async_trait;
use localify_core::domain::album::AlbumRow;
use localify_core::domain::artist::ArtistRow;
use localify_core::domain::ids::{ArtistId, TrackId};
use localify_core::domain::library::PlayHistoryEntry;
use localify_core::domain::track::TrackRow;
use localify_core::error::CoreResult;
use localify_core::ports::database::HistoryRepository;
use rusqlite::params;

use crate::error::{DbResult, ToCore};
use crate::mappers::{COLUMNAS_TRACK_ROW, JOINS_TRACK_ROW, a_track_row, de_fecha};
use crate::pool::Pool;

pub struct SqliteHistoryRepository {
    pool: Pool,
}

impl std::fmt::Debug for SqliteHistoryRepository {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteHistoryRepository")
            .finish_non_exhaustive()
    }
}

impl SqliteHistoryRepository {
    #[must_use]
    pub const fn new(pool: Pool) -> Self {
        Self { pool }
    }
}

/// Segundos en un día, para convertir los parámetros expresados en días.
const DIA_SEGUNDOS: i64 = 86_400;

#[async_trait]
impl HistoryRepository for SqliteHistoryRepository {
    async fn record(&self, entry: &PlayHistoryEntry) -> CoreResult<()> {
        let e = entry.clone();
        self.pool
            .escribir(move |tx| {
                tx.execute(
                    "INSERT INTO play_history (track_id, played_at, ms_played, completed, context)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![
                        e.track_id.as_str(),
                        de_fecha(e.played_at),
                        i64::from(e.ms_played),
                        i64::from(e.completed),
                        e.context,
                    ],
                )?;
                Ok(())
            })
            .await
            .to_core()
    }

    async fn recent_tracks(&self, limit: u16) -> CoreResult<Vec<TrackRow>> {
        let columnas = COLUMNAS_TRACK_ROW;
        let joins = JOINS_TRACK_ROW;

        // Una pista escuchada cinco veces debe aparecer una sola vez, en la
        // posición de su escucha más reciente. De ahí el `MAX` con `GROUP BY`
        // en lugar de leer las últimas N filas del historial.
        let sql = format!(
            "SELECT {columnas}, MAX(h.played_at) AS ultima
             FROM play_history h
             JOIN tracks t ON t.id = h.track_id
             {joins}
             GROUP BY t.id
             ORDER BY ultima DESC
             LIMIT ?1"
        );

        self.pool
            .leer(move |conn| {
                let mut stmt = conn.prepare_cached(&sql)?;
                let filas = stmt
                    .query_map([i64::from(limit)], |row| Ok(a_track_row(row)))?
                    .collect::<Result<Vec<_>, _>>()?
                    .into_iter()
                    .collect::<DbResult<Vec<_>>>()?;
                Ok(filas)
            })
            .await
            .to_core()
    }

    async fn play_count(&self, track: &TrackId) -> CoreResult<u32> {
        let id = track.as_str().to_owned();
        self.pool
            .leer(move |conn| {
                let n: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM play_history WHERE track_id = ?1",
                    [&id],
                    |r| r.get(0),
                )?;
                Ok(u32::try_from(n.max(0)).unwrap_or(u32::MAX))
            })
            .await
            .to_core()
    }

    async fn top_artists(&self, days: u16, limit: u8) -> CoreResult<Vec<ArtistRow>> {
        let desde = de_fecha(chrono::Utc::now()) - i64::from(days) * DIA_SEGUNDOS;

        self.pool
            .leer(move |conn| {
                // Se pondera por escuchas completadas: dos minutos de una pista
                // saltada no dicen que el artista guste.
                let mut stmt = conn.prepare_cached(
                    "SELECT ar.id, ar.name, ar.image_url,
                            (SELECT COUNT(*) FROM track_artists ta2
                              WHERE ta2.artist_id = ar.id) AS track_count,
                            (SELECT COUNT(*) FROM track_artists ta2
                              JOIN audio_files af ON af.track_id = ta2.track_id
                              WHERE ta2.artist_id = ar.id) AS local_track_count,
                            SUM(CASE WHEN h.completed = 1 THEN 3 ELSE 1 END) AS peso
                     FROM play_history h
                     JOIN track_artists ta ON ta.track_id = h.track_id
                     JOIN artists ar       ON ar.id = ta.artist_id
                     WHERE h.played_at >= ?1
                     GROUP BY ar.id
                     ORDER BY peso DESC, ar.name_norm ASC
                     LIMIT ?2",
                )?;

                let filas = stmt
                    .query_map(params![desde, i64::from(limit)], |r| {
                        Ok(ArtistRow {
                            id: ArtistId::from_trusted(r.get::<_, String>(0)?),
                            name: r.get(1)?,
                            image_url: r.get(2)?,
                            track_count: u32::try_from(r.get::<_, i64>(3)?).unwrap_or(0),
                            local_track_count: u32::try_from(r.get::<_, i64>(4)?).unwrap_or(0),
                        })
                    })?
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(filas)
            })
            .await
            .to_core()
    }

    async fn top_tracks(&self, days: u16, limit: u8) -> CoreResult<Vec<TrackRow>> {
        let desde = de_fecha(chrono::Utc::now()) - i64::from(days) * DIA_SEGUNDOS;
        let columnas = COLUMNAS_TRACK_ROW;
        let joins = JOINS_TRACK_ROW;

        self.pool
            .leer(move |conn| {
                // El mismo peso que en "tus artistas": una escucha completa
                // vale por tres. Una canción saltada a los diez segundos
                // aparece en el historial, pero no es lo que más escuchas.
                let sql = format!(
                    "SELECT {columnas},
                            SUM(CASE WHEN h.completed = 1 THEN 3 ELSE 1 END) AS peso
                     FROM play_history h
                     JOIN tracks t ON t.id = h.track_id
                     {joins}
                     WHERE h.played_at >= ?1
                     GROUP BY t.id
                     ORDER BY peso DESC, t.title_norm ASC
                     LIMIT ?2"
                );
                let mut stmt = conn.prepare_cached(&sql)?;
                let filas = stmt
                    .query_map(params![desde, i64::from(limit)], |row| Ok(a_track_row(row)))?
                    .collect::<Result<Vec<_>, _>>()?
                    .into_iter()
                    .collect::<DbResult<Vec<_>>>()?;
                Ok(filas)
            })
            .await
            .to_core()
    }

    async fn top_albums(&self, days: u16, limit: u8) -> CoreResult<Vec<AlbumRow>> {
        let desde = de_fecha(chrono::Utc::now()) - i64::from(days) * DIA_SEGUNDOS;
        let columnas = crate::repositories::albums::COLUMNAS_ALBUM_ROW;

        self.pool
            .leer(move |conn| {
                // Se ordena por **canciones distintas** oídas del álbum y solo
                // después por escuchas totales. Sin lo primero, un disco del
                // que solo suena el single quedaría por delante de otro que se
                // ha escuchado entero, que es al revés de lo que dice la
                // sección.
                let sql = format!(
                    "SELECT {columnas},
                            COUNT(DISTINCT h.track_id) AS distintas,
                            COUNT(*) AS escuchas
                     FROM play_history h
                     JOIN tracks t  ON t.id = h.track_id
                     JOIN albums al ON al.id = t.album_id
                     WHERE h.played_at >= ?1
                     GROUP BY al.id
                     ORDER BY distintas DESC, escuchas DESC, al.title_norm ASC
                     LIMIT ?2"
                );
                let mut stmt = conn.prepare_cached(&sql)?;
                let filas = stmt
                    .query_map(params![desde, i64::from(limit)], |r| {
                        crate::repositories::albums::a_album_row(r)
                    })?
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(filas)
            })
            .await
            .to_core()
    }

    async fn rediscover(&self, days: u16, limit: u8) -> CoreResult<Vec<TrackRow>> {
        let corte = de_fecha(chrono::Utc::now()) - i64::from(days) * DIA_SEGUNDOS;
        let columnas = COLUMNAS_TRACK_ROW;
        let joins = JOINS_TRACK_ROW;

        // Favoritos con fichero en disco que no se escuchan desde hace tiempo.
        // Sugerir algo que no está descargado obligaría a esperar; sugerir algo
        // que no es favorito no sería "redescubrir".
        let sql = format!(
            "SELECT {columnas} FROM tracks t {joins}
             WHERE f.track_id IS NOT NULL
               AND af.track_id IS NOT NULL
               AND COALESCE((SELECT MAX(h.played_at) FROM play_history h
                             WHERE h.track_id = t.id), 0) < ?1
             ORDER BY RANDOM()
             LIMIT ?2"
        );

        self.pool
            .leer(move |conn| {
                let mut stmt = conn.prepare_cached(&sql)?;
                let filas = stmt
                    .query_map(params![corte, i64::from(limit)], |row| Ok(a_track_row(row)))?
                    .collect::<Result<Vec<_>, _>>()?
                    .into_iter()
                    .collect::<DbResult<Vec<_>>>()?;
                Ok(filas)
            })
            .await
            .to_core()
    }

    async fn clear(&self) -> CoreResult<u32> {
        self.pool
            .escribir(move |tx| {
                let borradas = tx.execute("DELETE FROM play_history", [])?;
                Ok(u32::try_from(borradas).unwrap_or(u32::MAX))
            })
            .await
            .to_core()
    }
}

#[cfg(test)]
mod tests {
    use localify_core::domain::audio::DurationMs;
    use localify_core::domain::track::{ArtistRef, Track};
    use localify_core::ports::database::TrackRepository;

    use super::*;
    use crate::pool::TempDbGuard;
    use crate::repositories::tracks::SqliteTrackRepository;

    async fn ctx() -> (
        SqliteHistoryRepository,
        SqliteTrackRepository,
        Pool,
        TempDbGuard,
    ) {
        let (pool, guard) = Pool::temporal().expect("abre");
        crate::migrations::ejecutar(&pool).await.expect("migra");
        (
            SqliteHistoryRepository::new(pool.clone()),
            SqliteTrackRepository::new(pool.clone()),
            pool,
            guard,
        )
    }

    fn pista(titulo: &str, artistas: Vec<ArtistRef>) -> Track {
        Track {
            id: TrackId::nuevo_local(),
            title: titulo.into(),
            album: None,
            artists: artistas,
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

    fn artista(nombre: &str) -> ArtistRef {
        ArtistRef {
            id: ArtistId::nuevo_local(),
            name: nombre.into(),
        }
    }

    fn escucha(track: &TrackId, hace_segundos: i64, completada: bool) -> PlayHistoryEntry {
        PlayHistoryEntry {
            track_id: track.clone(),
            played_at: chrono::Utc::now() - chrono::Duration::seconds(hace_segundos),
            ms_played: if completada { 200_000 } else { 15_000 },
            completed: completada,
            context: Some("album:xyz".into()),
        }
    }

    #[tokio::test]
    async fn el_recuento_de_reproducciones_se_acumula() {
        let (repo, tracks, _pool, _g) = ctx().await;
        let t = pista("X", vec![]);
        tracks
            .upsert(std::slice::from_ref(&t))
            .await
            .expect("guarda");

        assert_eq!(repo.play_count(&t.id).await.expect("cuenta"), 0);
        for _ in 0..3 {
            repo.record(&escucha(&t.id, 0, true))
                .await
                .expect("registra");
        }
        assert_eq!(repo.play_count(&t.id).await.expect("cuenta"), 3);
    }

    #[tokio::test]
    async fn recientes_no_repite_una_pista_escuchada_varias_veces() {
        let (repo, tracks, _pool, _g) = ctx().await;
        let a = pista("A", vec![]);
        let b = pista("B", vec![]);
        tracks
            .upsert(&[a.clone(), b.clone()])
            .await
            .expect("guarda");

        repo.record(&escucha(&a.id, 300, true))
            .await
            .expect("registra");
        repo.record(&escucha(&b.id, 200, true))
            .await
            .expect("registra");
        repo.record(&escucha(&a.id, 10, true))
            .await
            .expect("registra");

        let recientes = repo.recent_tracks(10).await.expect("consulta");
        assert_eq!(
            recientes.len(),
            2,
            "una pista repetida no debe ocupar dos huecos"
        );
        assert_eq!(
            recientes
                .iter()
                .map(|t| t.title.clone())
                .collect::<Vec<_>>(),
            vec!["A", "B"],
            "A es la más reciente por su última escucha, no por la primera"
        );
    }

    #[tokio::test]
    async fn los_artistas_top_ponderan_las_escuchas_completadas() {
        let (repo, tracks, _pool, _g) = ctx().await;
        let uno = artista("Completado");
        let otro = artista("Saltado");
        let a = pista("A", vec![uno.clone()]);
        let b = pista("B", vec![otro.clone()]);
        tracks
            .upsert(&[a.clone(), b.clone()])
            .await
            .expect("guarda");

        // Dos escuchas completas contra tres saltadas: debe ganar el completado
        // (2×3 = 6 frente a 3×1 = 3).
        for _ in 0..2 {
            repo.record(&escucha(&a.id, 100, true))
                .await
                .expect("registra");
        }
        for _ in 0..3 {
            repo.record(&escucha(&b.id, 100, false))
                .await
                .expect("registra");
        }

        let top = repo.top_artists(30, 10).await.expect("consulta");
        assert_eq!(top[0].name, "Completado");
        assert_eq!(top[1].name, "Saltado");
    }

    #[tokio::test]
    async fn los_artistas_top_ignoran_lo_anterior_a_la_ventana() {
        let (repo, tracks, _pool, _g) = ctx().await;
        let viejo = artista("Antiguo");
        let t = pista("A", vec![viejo]);
        tracks
            .upsert(std::slice::from_ref(&t))
            .await
            .expect("guarda");

        // Hace 60 días, con una ventana de 30.
        repo.record(&escucha(&t.id, 60 * 86_400, true))
            .await
            .expect("registra");

        let top = repo.top_artists(30, 10).await.expect("consulta");
        assert!(
            top.is_empty(),
            "la ventana temporal debe excluir lo antiguo"
        );
    }

    #[tokio::test]
    async fn redescubre_solo_propone_favoritos_descargados_y_olvidados() {
        let (repo, tracks, pool, _g) = ctx().await;
        let olvidada = pista("Olvidada", vec![]);
        let reciente = pista("Reciente", vec![]);
        let no_favorita = pista("No favorita", vec![]);
        let sin_fichero = pista("Sin fichero", vec![]);
        tracks
            .upsert(&[
                olvidada.clone(),
                reciente.clone(),
                no_favorita.clone(),
                sin_fichero.clone(),
            ])
            .await
            .expect("guarda");

        // Favoritas: todas menos `no_favorita`. Con fichero: todas menos
        // `sin_fichero`.
        let (i1, i2, i4) = (
            olvidada.id.as_str().to_owned(),
            reciente.id.as_str().to_owned(),
            sin_fichero.id.as_str().to_owned(),
        );
        pool.escribir(move |tx| {
            for id in [&i1, &i2, &i4] {
                tx.execute("INSERT INTO favorites (track_id) VALUES (?1)", [id])?;
            }
            for (n, id) in [&i1, &i2].into_iter().enumerate() {
                tx.execute(
                    "INSERT INTO audio_files
                     (track_id, rel_path, format, codec, size_bytes, duration_ms, verified_at)
                     VALUES (?1, ?2, 'opus', 'opus', 100, 200000, 0)",
                    params![id, format!("audio/aa/{n}.opus")],
                )?;
            }
            Ok(())
        })
        .await
        .expect("prepara");

        repo.record(&escucha(&olvidada.id, 200 * 86_400, true))
            .await
            .expect("vieja");
        repo.record(&escucha(&reciente.id, 3600, true))
            .await
            .expect("reciente");

        let sugerencias = repo.rediscover(90, 10).await.expect("consulta");
        let titulos: Vec<_> = sugerencias.iter().map(|t| t.title.clone()).collect();

        assert!(titulos.contains(&"Olvidada".to_owned()));
        assert!(
            !titulos.contains(&"Reciente".to_owned()),
            "se escuchó hace poco"
        );
        assert!(
            !titulos.contains(&"No favorita".to_owned()),
            "no es favorita"
        );
        assert!(
            !titulos.contains(&"Sin fichero".to_owned()),
            "sugerir algo no descargado obligaría a esperar"
        );
    }

    #[tokio::test]
    async fn borrar_la_pista_arrastra_su_historial() {
        let (repo, tracks, pool, _g) = ctx().await;
        let t = pista("X", vec![]);
        tracks
            .upsert(std::slice::from_ref(&t))
            .await
            .expect("guarda");
        repo.record(&escucha(&t.id, 0, true))
            .await
            .expect("registra");

        let id = t.id.as_str().to_owned();
        pool.escribir(move |tx| {
            tx.execute("DELETE FROM tracks WHERE id = ?1", [&id])?;
            Ok(())
        })
        .await
        .expect("borra");

        let filas: i64 = pool
            .leer(|c| Ok(c.query_row("SELECT COUNT(*) FROM play_history", [], |r| r.get(0))?))
            .await
            .expect("cuenta");
        assert_eq!(filas, 0);
    }

    /// Álbum con `n` pistas, ya guardado.
    async fn album_con_pistas(
        pool: &Pool,
        tracks: &SqliteTrackRepository,
        id: &str,
        titulo: &str,
        n: usize,
    ) -> Vec<TrackId> {
        let album_id = id.to_owned();
        let album_titulo = titulo.to_owned();
        pool.escribir(move |tx| {
            tx.execute(
                "INSERT INTO albums (id, title, title_norm) VALUES (?1, ?2, ?2)",
                rusqlite::params![album_id, album_titulo],
            )?;
            Ok(())
        })
        .await
        .expect("crea album");

        let pistas: Vec<Track> = (0..n)
            .map(|i| Track {
                album: Some(localify_core::domain::track::AlbumRef {
                    id: localify_core::domain::ids::AlbumId::from_trusted(id.to_owned()),
                    title: titulo.to_owned(),
                }),
                ..pista(&format!("{titulo} {i}"), vec![])
            })
            .collect();
        tracks.upsert(&pistas).await.expect("guarda");
        pistas.into_iter().map(|t| t.id).collect()
    }

    #[tokio::test]
    async fn lo_mas_escuchado_pesa_las_escuchas_completas() {
        // Una cancion saltada aparece en el historial, pero "lo que mas
        // escuchas" no puede estar encabezado por lo que mas se salta.
        let (repo, tracks, _pool, _g) = ctx().await;
        let saltada = pista("Saltada", vec![artista("A")]);
        let oida = pista("Oida", vec![artista("B")]);
        tracks
            .upsert(&[saltada.clone(), oida.clone()])
            .await
            .expect("guarda");

        // Cinco saltos frente a tres escuchas enteras: 5 puntos contra 9.
        for _ in 0..5 {
            repo.record(&escucha(&saltada.id, 60, false))
                .await
                .expect("registra");
        }
        for _ in 0..3 {
            repo.record(&escucha(&oida.id, 60, true))
                .await
                .expect("registra");
        }

        let top = repo.top_tracks(30, 10).await.expect("top");
        assert_eq!(top.first().map(|t| t.title.as_str()), Some("Oida"));
    }

    #[tokio::test]
    async fn los_albumes_se_ordenan_por_cuantas_canciones_suyas_suenan() {
        // Un disco del que solo suena el single no es "un disco que escuchas",
        // por muchas veces que se repita ese single. Sin esta regla, cualquier
        // exito suelto arrasaria la seccion.
        let (repo, tracks, pool, _g) = ctx().await;

        let del_single = album_con_pistas(&pool, &tracks, "alb-hit", "Solo el single", 10).await;
        let entero = album_con_pistas(&pool, &tracks, "alb-entero", "Escuchado entero", 6).await;

        // Veinte escuchas, todas de la misma cancion.
        for _ in 0..20 {
            repo.record(&escucha(&del_single[0], 60, true))
                .await
                .expect("registra");
        }
        // Seis escuchas, una por cancion.
        for t in &entero {
            repo.record(&escucha(t, 60, true)).await.expect("registra");
        }

        let top = repo.top_albums(30, 10).await.expect("top");
        assert_eq!(
            top.first().map(|a| a.title.as_str()),
            Some("Escuchado entero"),
            "seis canciones distintas pesan mas que una repetida veinte veces"
        );
    }

    #[tokio::test]
    async fn lo_de_hace_meses_no_cuenta_para_el_top() {
        let (repo, tracks, _pool, _g) = ctx().await;
        let t = pista("Vieja", vec![]);
        tracks
            .upsert(std::slice::from_ref(&t))
            .await
            .expect("guarda");

        // Hace cien dias: fuera de la ventana de treinta.
        repo.record(&escucha(&t.id, 100 * 86_400, true))
            .await
            .expect("registra");

        assert!(
            repo.top_tracks(30, 10).await.expect("top").is_empty(),
            "la ventana existe para que Inicio refleje lo de ahora"
        );
    }
}

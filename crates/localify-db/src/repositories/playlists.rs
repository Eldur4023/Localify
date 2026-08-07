//! Repositorio de playlists.
//!
//! El orden usa **claves fraccionarias** (ADR-009): reordenar por índice entero
//! obligaría a reescribir hasta N filas por arrastre, lo que en una playlist de
//! 5 000 pistas es inaceptable. Con una clave `REAL`, mover un elemento es un
//! único `UPDATE` sin importar el tamaño.

use async_trait::async_trait;
use localify_core::domain::ids::{AlbumId, PlaylistEntryId, PlaylistId, TrackId};
use localify_core::domain::playlist::{Playlist, PlaylistEntry, PlaylistSummary, position};
use localify_core::error::CoreResult;
use localify_core::page::{Cursor, Page, PageRequest};
use localify_core::ports::database::PlaylistRepository;
use localify_core::text;
use rusqlite::{Transaction, params};

use crate::error::{DbError, DbResult, ToCore};
use crate::mappers::{
    COLUMNAS_TRACK_ROW, JOINS_TRACK_ROW, a_fecha, a_origen_playlist, a_track_row, de_fecha,
    de_origen_playlist, fecha_track_row,
};
use crate::pool::Pool;

/// Portadas que se recogen para el mosaico cuando la playlist no tiene una
/// propia. Cuatro es lo que cabe en la rejilla 2×2 de la interfaz.
const PORTADAS_MOSAICO: usize = 4;

pub struct SqlitePlaylistRepository {
    pool: Pool,
}

impl std::fmt::Debug for SqlitePlaylistRepository {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqlitePlaylistRepository")
            .finish_non_exhaustive()
    }
}

impl SqlitePlaylistRepository {
    #[must_use]
    pub const fn new(pool: Pool) -> Self {
        Self { pool }
    }
}

/// Marca la playlist como modificada.
///
/// Se llama desde cada mutación de contenido: sin esto, la barra lateral
/// ordenada por actividad reciente no reflejaría los cambios.
fn tocar(tx: &Transaction<'_>, id: &str) -> DbResult<()> {
    tx.execute(
        "UPDATE playlists SET updated_at = unixepoch() WHERE id = ?1",
        [id],
    )?;
    Ok(())
}

fn a_resumen(row: &rusqlite::Row<'_>) -> rusqlite::Result<PlaylistSummary> {
    let id: String = row.get("id")?;
    let mosaico: Option<String> = row.get("mosaico")?;
    let origen: String = row.get("source")?;

    // Hasta cuatro álbumes de las primeras pistas. No se filtra por portada ya
    // descargada: el frontend las pide por `cover://` y se bajan al mirarlas,
    // así que exigir que estuvieran en caché dejaba sin imagen justo a las
    // playlists recién hechas, que son las que uno mira.
    let cover_albums = mosaico
        .unwrap_or_default()
        .split(',')
        .filter(|s| !s.is_empty())
        .take(PORTADAS_MOSAICO)
        .map(|s| AlbumId::from_trusted(s.to_owned()))
        .collect();

    Ok(PlaylistSummary {
        id: PlaylistId::parse(&id).unwrap_or_else(|_| PlaylistId::nuevo()),
        name: row.get("name")?,
        track_count: u32::try_from(row.get::<_, i64>("track_count")?).unwrap_or(0),
        cover_albums,
        has_own_cover: row.get::<_, Option<String>>("cover_path")?.is_some(),
        updated_at: a_fecha(row.get::<_, i64>("updated_at")?),
        source: a_origen_playlist(&origen),
    })
}

/// Columnas del resumen. El `GROUP_CONCAT` recoge los álbumes de las primeras
/// pistas para componer el mosaico, sin una segunda consulta por playlist.
const COLUMNAS_RESUMEN: &str = "
    p.id, p.name, p.cover_path, p.updated_at, p.source,
    (SELECT COUNT(*) FROM playlist_items pi WHERE pi.playlist_id = p.id) AS track_count,
    (SELECT GROUP_CONCAT(album_id)
       FROM (SELECT DISTINCT t.album_id AS album_id
             FROM playlist_items pi
             JOIN tracks t  ON t.id = pi.track_id
             JOIN albums al ON al.id = t.album_id
             WHERE pi.playlist_id = p.id
             ORDER BY pi.position
             LIMIT 4)) AS mosaico
";

#[async_trait]
impl PlaylistRepository for SqlitePlaylistRepository {
    async fn create(&self, playlist: &Playlist) -> CoreResult<()> {
        let p = playlist.clone();
        self.pool
            .escribir(move |tx| {
                tx.execute(
                    "INSERT INTO playlists (
                         id, name, name_norm, description, cover_path,
                         source, source_id, created_at, updated_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                    params![
                        p.id.to_string(),
                        p.name,
                        text::normalize(&p.name),
                        p.description,
                        p.cover_path,
                        de_origen_playlist(p.source),
                        p.source_id,
                        de_fecha(p.created_at),
                        de_fecha(p.updated_at),
                    ],
                )?;
                Ok(())
            })
            .await
            .to_core()
    }

    async fn get(&self, id: &PlaylistId) -> CoreResult<Option<Playlist>> {
        let id_txt = id.to_string();
        let id = *id;
        self.pool
            .leer(move |conn| {
                let mut stmt = conn.prepare_cached(
                    "SELECT name, description, cover_path, source, source_id,
                            created_at, updated_at
                     FROM playlists WHERE id = ?1",
                )?;

                let fila = stmt.query_row([&id_txt], |r| {
                    Ok(Playlist {
                        id,
                        name: r.get(0)?,
                        description: r.get(1)?,
                        cover_path: r.get(2)?,
                        source: a_origen_playlist(&r.get::<_, String>(3)?),
                        source_id: r.get(4)?,
                        created_at: a_fecha(r.get::<_, i64>(5)?),
                        updated_at: a_fecha(r.get::<_, i64>(6)?),
                    })
                });

                match fila {
                    Ok(p) => Ok(Some(p)),
                    Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                    Err(e) => Err(e.into()),
                }
            })
            .await
            .to_core()
    }

    async fn rename(&self, id: &PlaylistId, name: &str) -> CoreResult<()> {
        let id = id.to_string();
        let nombre = name.to_owned();
        self.pool
            .escribir(move |tx| {
                let filas = tx.execute(
                    "UPDATE playlists
                     SET name = ?2, name_norm = ?3, updated_at = unixepoch()
                     WHERE id = ?1",
                    params![id, nombre, text::normalize(&nombre)],
                )?;
                if filas == 0 {
                    return Err(DbError::error_de_mapeo("id", "la playlist no existe"));
                }
                Ok(())
            })
            .await
            .to_core()
    }

    async fn set_cover(&self, id: &PlaylistId, rel_path: Option<&str>) -> CoreResult<()> {
        let id = id.to_string();
        let ruta = rel_path.map(str::to_owned);
        self.pool
            .escribir(move |tx| {
                let filas = tx.execute(
                    "UPDATE playlists
                     SET cover_path = ?2, updated_at = unixepoch()
                     WHERE id = ?1",
                    params![id, ruta],
                )?;
                if filas == 0 {
                    return Err(DbError::error_de_mapeo("id", "la playlist no existe"));
                }
                Ok(())
            })
            .await
            .to_core()
    }

    async fn set_description(&self, id: &PlaylistId, description: Option<&str>) -> CoreResult<()> {
        let id = id.to_string();
        // Una descripción en blanco es no tener descripción. Guardar `""`
        // dejaría la ficha reservando el hueco de un párrafo vacío.
        let texto = description
            .map(str::trim)
            .filter(|d| !d.is_empty())
            .map(ToOwned::to_owned);

        self.pool
            .escribir(move |tx| {
                let filas = tx.execute(
                    "UPDATE playlists
                     SET description = ?2, updated_at = unixepoch()
                     WHERE id = ?1",
                    params![id, texto],
                )?;
                if filas == 0 {
                    return Err(DbError::error_de_mapeo("id", "la playlist no existe"));
                }
                Ok(())
            })
            .await
            .to_core()
    }

    async fn delete(&self, id: &PlaylistId) -> CoreResult<()> {
        let id = id.to_string();
        self.pool
            .escribir(move |tx| {
                tx.execute("DELETE FROM playlists WHERE id = ?1", [&id])?;
                Ok(())
            })
            .await
            .to_core()
    }

    async fn list_summaries(&self) -> CoreResult<Vec<PlaylistSummary>> {
        let columnas = COLUMNAS_RESUMEN;
        let sql = format!(
            "SELECT {columnas} FROM playlists p ORDER BY p.updated_at DESC, p.name_norm ASC"
        );
        self.pool
            .leer(move |conn| {
                let mut stmt = conn.prepare_cached(&sql)?;
                let filas = stmt
                    .query_map([], a_resumen)?
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(filas)
            })
            .await
            .to_core()
    }

    async fn most_played(&self, days: u16, limit: u8) -> CoreResult<Vec<PlaylistSummary>> {
        let desde = crate::mappers::de_fecha(chrono::Utc::now()) - i64::from(days) * 86_400;
        let columnas = COLUMNAS_RESUMEN;

        self.pool
            .leer(move |conn| {
                // El enlace es el **contexto** del historial: cada escucha
                // guarda desde dónde se lanzó, en la forma `playlist:<uuid>`.
                // Contar las playlists que contienen canciones oídas daría otra
                // cosa —una canción popular sube las diez listas que la tienen—
                // y esa otra cosa no es una recomendación.
                //
                // Se pesa la escucha completa por tres, igual que en artistas:
                // poner una playlist y saltarla entera no dice que guste.
                let sql = format!(
                    "SELECT {columnas},
                            SUM(CASE WHEN h.completed = 1 THEN 3 ELSE 1 END) AS peso
                     FROM playlists p
                     JOIN play_history h
                       ON h.context = 'playlist:' || p.id
                      AND h.played_at >= ?1
                     GROUP BY p.id
                     ORDER BY peso DESC, p.updated_at DESC
                     LIMIT ?2"
                );
                let mut stmt = conn.prepare_cached(&sql)?;
                let filas = stmt
                    .query_map(params![desde, i64::from(limit)], a_resumen)?
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(filas)
            })
            .await
            .to_core()
    }

    async fn entries(
        &self,
        id: &PlaylistId,
        page: &PageRequest,
    ) -> CoreResult<Page<PlaylistEntry>> {
        let id = id.to_string();
        let limite = i64::from(page.limit());
        let offset = i64::from(page.offset());
        let columnas = COLUMNAS_TRACK_ROW;
        let joins = JOINS_TRACK_ROW;

        let fecha = fecha_track_row("pi.added_at");
        let sql = format!(
            "SELECT {columnas}{fecha}, pi.id AS entry_id, pi.added_at AS entry_added
             FROM playlist_items pi
             JOIN tracks t ON t.id = pi.track_id
             {joins}
             WHERE pi.playlist_id = ?1
             ORDER BY pi.position ASC
             LIMIT ?2 OFFSET ?3"
        );

        self.pool
            .leer(move |conn| {
                let total: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM playlist_items WHERE playlist_id = ?1",
                    [&id],
                    |r| r.get(0),
                )?;
                let total = total.max(0).unsigned_abs();

                let mut stmt = conn.prepare_cached(&sql)?;
                let items = stmt
                    .query_map(params![id, limite, offset], |row| {
                        let entry: String = row.get("entry_id")?;
                        let added: i64 = row.get("entry_added")?;
                        Ok(a_track_row(row).map(|track| PlaylistEntry {
                            entry_id: uuid::Uuid::parse_str(&entry).map_or_else(
                                |_| PlaylistEntryId::nuevo(),
                                PlaylistEntryId::from_uuid,
                            ),
                            track,
                            added_at: a_fecha(added),
                        }))
                    })?
                    .collect::<Result<Vec<_>, _>>()?
                    .into_iter()
                    .collect::<DbResult<Vec<_>>>()?;

                let consumidos = offset.max(0).unsigned_abs() + items.len() as u64;
                let next = (consumidos < total).then(|| Cursor::new(consumidos.to_string()));

                Ok(Page::new(items, Some(total), next))
            })
            .await
            .to_core()
    }

    async fn add_entries(
        &self,
        id: &PlaylistId,
        entries: &[(PlaylistEntryId, TrackId, f64)],
    ) -> CoreResult<()> {
        if entries.is_empty() {
            return Ok(());
        }
        let id = id.to_string();
        let entries: Vec<_> = entries
            .iter()
            .map(|(e, t, p)| (e.to_string(), t.as_str().to_owned(), *p))
            .collect();

        self.pool
            .escribir(move |tx| {
                for (entry_id, track_id, posicion) in &entries {
                    tx.execute(
                        "INSERT INTO playlist_items (id, playlist_id, track_id, position)
                         VALUES (?1, ?2, ?3, ?4)",
                        params![entry_id, id, track_id, posicion],
                    )?;
                }
                tocar(tx, &id)?;
                Ok(())
            })
            .await
            .to_core()
    }

    async fn remove_entries(&self, id: &PlaylistId, entries: &[PlaylistEntryId]) -> CoreResult<()> {
        if entries.is_empty() {
            return Ok(());
        }
        let id = id.to_string();
        let entries: Vec<String> = entries.iter().map(ToString::to_string).collect();

        self.pool
            .escribir(move |tx| {
                for entry in &entries {
                    tx.execute(
                        "DELETE FROM playlist_items WHERE id = ?1 AND playlist_id = ?2",
                        params![entry, id],
                    )?;
                }
                tocar(tx, &id)?;
                Ok(())
            })
            .await
            .to_core()
    }

    async fn set_position(
        &self,
        id: &PlaylistId,
        entry: PlaylistEntryId,
        position: f64,
    ) -> CoreResult<()> {
        let id = id.to_string();
        let entry = entry.to_string();

        self.pool
            .escribir(move |tx| {
                // Un único UPDATE, sea la playlist de 10 pistas o de 5 000.
                let filas = tx.execute(
                    "UPDATE playlist_items SET position = ?3
                     WHERE id = ?1 AND playlist_id = ?2",
                    params![entry, id, position],
                )?;
                if filas == 0 {
                    return Err(DbError::error_de_mapeo("id", "la entrada no existe"));
                }
                tocar(tx, &id)?;
                Ok(())
            })
            .await
            .to_core()
    }

    async fn neighbors(
        &self,
        id: &PlaylistId,
        index: usize,
    ) -> CoreResult<(Option<f64>, Option<f64>)> {
        let id = id.to_string();
        let indice = i64::try_from(index).unwrap_or(i64::MAX);

        self.pool
            .leer(move |conn| {
                // Las claves en las posiciones `index - 1` e `index`: el hueco
                // donde caerá el elemento arrastrado.
                let mut stmt = conn.prepare_cached(
                    "SELECT position FROM playlist_items
                     WHERE playlist_id = ?1
                     ORDER BY position ASC
                     LIMIT 2 OFFSET ?2",
                )?;

                let inicio = (indice - 1).max(0);
                let posiciones: Vec<f64> = stmt
                    .query_map(params![id, inicio], |r| r.get(0))?
                    .collect::<Result<Vec<_>, _>>()?;

                Ok(if indice == 0 {
                    // Al principio: no hay anterior, el siguiente es el primero.
                    (None, posiciones.first().copied())
                } else {
                    (posiciones.first().copied(), posiciones.get(1).copied())
                })
            })
            .await
            .to_core()
    }

    async fn rebalance(&self, id: &PlaylistId) -> CoreResult<()> {
        let id = id.to_string();
        self.pool
            .escribir(move |tx| {
                // Renumera a múltiplos del paso, conservando el orden actual.
                // Ocurre rarísimamente y no es visible para el usuario.
                let ids: Vec<String> = {
                    let mut stmt = tx.prepare(
                        "SELECT id FROM playlist_items
                         WHERE playlist_id = ?1 ORDER BY position ASC",
                    )?;
                    stmt.query_map([&id], |r| r.get(0))?
                        .collect::<Result<Vec<_>, _>>()?
                };

                for (indice, entrada) in ids.iter().enumerate() {
                    // Una playlist con más de 2^52 elementos no cabe en disco.
                    #[allow(
                        clippy::cast_precision_loss,
                        reason = "índice acotado por el tamaño real"
                    )]
                    let nueva = indice as f64 * position::PASO;
                    tx.execute(
                        "UPDATE playlist_items SET position = ?2 WHERE id = ?1",
                        params![entrada, nueva],
                    )?;
                }
                Ok(())
            })
            .await
            .to_core()
    }
}

#[cfg(test)]
#[allow(
    clippy::cast_precision_loss,
    reason = "en los tests los índices son de un dígito"
)]
mod tests {
    use localify_core::domain::audio::DurationMs;
    use localify_core::domain::playlist::PlaylistSource;
    use localify_core::domain::track::Track;
    use localify_core::ports::database::TrackRepository;

    use super::*;
    use crate::pool::TempDbGuard;
    use crate::repositories::tracks::SqliteTrackRepository;

    async fn ctx() -> (
        SqlitePlaylistRepository,
        SqliteTrackRepository,
        Pool,
        TempDbGuard,
    ) {
        let (pool, guard) = Pool::temporal().expect("abre");
        crate::migrations::ejecutar(&pool).await.expect("migra");
        (
            SqlitePlaylistRepository::new(pool.clone()),
            SqliteTrackRepository::new(pool.clone()),
            pool,
            guard,
        )
    }

    fn playlist(nombre: &str) -> Playlist {
        let ahora = chrono::Utc::now();
        Playlist {
            id: PlaylistId::nuevo(),
            name: nombre.into(),
            description: None,
            cover_path: None,
            source: PlaylistSource::Local,
            source_id: None,
            created_at: ahora,
            updated_at: ahora,
        }
    }

    fn pista(titulo: &str) -> Track {
        Track {
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
        }
    }

    /// Crea una playlist con N pistas numeradas, separadas por el paso estándar.
    async fn con_pistas(
        repo: &SqlitePlaylistRepository,
        tracks: &SqliteTrackRepository,
        n: usize,
    ) -> (Playlist, Vec<PlaylistEntryId>) {
        let p = playlist("Mi lista");
        repo.create(&p).await.expect("crea");

        let pistas: Vec<Track> = (0..n).map(|i| pista(&format!("P{i}"))).collect();
        tracks.upsert(&pistas).await.expect("guarda pistas");

        let entradas: Vec<(PlaylistEntryId, TrackId, f64)> = pistas
            .iter()
            .enumerate()
            .map(|(i, t)| {
                (
                    PlaylistEntryId::nuevo(),
                    t.id.clone(),
                    i as f64 * position::PASO,
                )
            })
            .collect();
        repo.add_entries(&p.id, &entradas).await.expect("añade");

        let ids = entradas.into_iter().map(|(e, _, _)| e).collect();
        (p, ids)
    }

    async fn titulos(repo: &SqlitePlaylistRepository, id: &PlaylistId) -> Vec<String> {
        repo.entries(id, &PageRequest::new(0, 200))
            .await
            .expect("lee")
            .items
            .into_iter()
            .map(|e| e.track.title)
            .collect()
    }

    #[tokio::test]
    async fn crear_leer_renombrar_y_borrar() {
        let (repo, _tracks, _pool, _g) = ctx().await;
        let p = playlist("Para correr");
        repo.create(&p).await.expect("crea");

        let leida = repo.get(&p.id).await.expect("lee").expect("existe");
        assert_eq!(leida.name, "Para correr");
        assert_eq!(leida.source, PlaylistSource::Local);

        repo.rename(&p.id, "Para andar").await.expect("renombra");
        assert_eq!(
            repo.get(&p.id).await.expect("lee").expect("existe").name,
            "Para andar"
        );

        repo.delete(&p.id).await.expect("borra");
        assert!(repo.get(&p.id).await.expect("consulta").is_none());
    }

    #[tokio::test]
    async fn renombrar_algo_inexistente_es_un_error_y_no_un_silencio() {
        let (repo, _tracks, _pool, _g) = ctx().await;
        assert!(repo.rename(&PlaylistId::nuevo(), "X").await.is_err());
    }

    #[tokio::test]
    async fn las_entradas_salen_en_orden_de_posicion() {
        let (repo, tracks, _pool, _g) = ctx().await;
        let (p, _) = con_pistas(&repo, &tracks, 5).await;
        assert_eq!(
            titulos(&repo, &p.id).await,
            vec!["P0", "P1", "P2", "P3", "P4"]
        );
    }

    #[tokio::test]
    async fn reordenar_ejecuta_un_solo_update() {
        let (repo, tracks, pool, _g) = ctx().await;
        let (p, entradas) = con_pistas(&repo, &tracks, 5).await;

        // Mover la última al principio: la clave nueva va por debajo de la
        // primera.
        let (_, siguiente) = repo.neighbors(&p.id, 0).await.expect("vecinos");
        let nueva = position::entre(None, siguiente);

        let antes = pool
            .leer(|c| {
                Ok(
                    c.query_row::<i64, _, _>("SELECT COUNT(*) FROM playlist_items", [], |r| {
                        r.get(0)
                    })?,
                )
            })
            .await
            .expect("cuenta");

        repo.set_position(&p.id, entradas[4], nueva)
            .await
            .expect("mueve");

        assert_eq!(
            titulos(&repo, &p.id).await,
            vec!["P4", "P0", "P1", "P2", "P3"]
        );

        let despues = pool
            .leer(|c| {
                Ok(
                    c.query_row::<i64, _, _>("SELECT COUNT(*) FROM playlist_items", [], |r| {
                        r.get(0)
                    })?,
                )
            })
            .await
            .expect("cuenta");
        assert_eq!(antes, despues, "reordenar no debe crear ni borrar filas");
    }

    #[tokio::test]
    async fn mover_al_medio_usa_el_punto_medio_de_los_vecinos() {
        let (repo, tracks, _pool, _g) = ctx().await;
        let (p, entradas) = con_pistas(&repo, &tracks, 5).await;

        // Mover P0 a la posición 3 (entre P2 y P3).
        let (antes, despues) = repo.neighbors(&p.id, 3).await.expect("vecinos");
        assert!(antes.is_some() && despues.is_some());
        let nueva = position::entre(antes, despues);

        repo.set_position(&p.id, entradas[0], nueva)
            .await
            .expect("mueve");
        assert_eq!(
            titulos(&repo, &p.id).await,
            vec!["P1", "P2", "P0", "P3", "P4"]
        );
    }

    #[tokio::test]
    async fn mover_al_final_no_necesita_vecino_posterior() {
        let (repo, tracks, _pool, _g) = ctx().await;
        let (p, entradas) = con_pistas(&repo, &tracks, 3).await;

        let (antes, despues) = repo.neighbors(&p.id, 3).await.expect("vecinos");
        assert!(despues.is_none(), "no hay nada después del último");
        let nueva = position::entre(antes, despues);

        repo.set_position(&p.id, entradas[0], nueva)
            .await
            .expect("mueve");
        assert_eq!(titulos(&repo, &p.id).await, vec!["P1", "P2", "P0"]);
    }

    #[tokio::test]
    async fn el_rebalanceo_conserva_el_orden_y_separa_las_claves() {
        let (repo, tracks, pool, _g) = ctx().await;
        let (p, entradas) = con_pistas(&repo, &tracks, 4).await;

        // Amontona tres claves en un hueco minúsculo, como pasaría tras muchos
        // arrastres al mismo punto.
        for (i, e) in entradas.iter().take(3).enumerate() {
            repo.set_position(&p.id, *e, 1.0 + i as f64 * 1e-9)
                .await
                .expect("apila");
        }

        let orden_antes = titulos(&repo, &p.id).await;
        repo.rebalance(&p.id).await.expect("rebalancea");
        assert_eq!(
            titulos(&repo, &p.id).await,
            orden_antes,
            "el orden debe conservarse"
        );

        let separacion_minima: f64 = pool
            .leer(|c| {
                Ok(c.query_row(
                    "SELECT MIN(delta) FROM (
                        SELECT position - LAG(position) OVER (ORDER BY position) AS delta
                        FROM playlist_items
                     ) WHERE delta IS NOT NULL",
                    [],
                    |r| r.get(0),
                )?)
            })
            .await
            .expect("mide");
        assert!(
            separacion_minima > position::EPSILON,
            "tras rebalancear debe haber margen para nuevas inserciones"
        );
    }

    #[tokio::test]
    async fn la_misma_pista_puede_aparecer_dos_veces_y_se_borra_la_correcta() {
        let (repo, tracks, _pool, _g) = ctx().await;
        let p = playlist("Con duplicados");
        repo.create(&p).await.expect("crea");

        let t = pista("Repetida");
        tracks
            .upsert(std::slice::from_ref(&t))
            .await
            .expect("guarda");

        let primera = PlaylistEntryId::nuevo();
        let segunda = PlaylistEntryId::nuevo();
        repo.add_entries(
            &p.id,
            &[
                (primera, t.id.clone(), 0.0),
                (segunda, t.id.clone(), position::PASO),
            ],
        )
        .await
        .expect("añade");

        assert_eq!(titulos(&repo, &p.id).await.len(), 2);

        repo.remove_entries(&p.id, &[primera])
            .await
            .expect("borra una");

        let restantes = repo
            .entries(&p.id, &PageRequest::new(0, 10))
            .await
            .expect("lee")
            .items;
        assert_eq!(restantes.len(), 1);
        assert_eq!(
            restantes[0].entry_id, segunda,
            "debe quedar la entrada que no se pidió borrar"
        );
    }

    #[tokio::test]
    async fn el_resumen_cuenta_las_pistas() {
        let (repo, tracks, _pool, _g) = ctx().await;
        let (p, _) = con_pistas(&repo, &tracks, 7).await;

        let resumenes = repo.list_summaries().await.expect("lista");
        let mio = resumenes
            .iter()
            .find(|r| r.id == p.id)
            .expect("la playlist debe aparecer");
        assert_eq!(mio.track_count, 7);
        assert!(
            mio.cover_albums.is_empty(),
            "estas pistas no tienen album, asi que no hay de donde sacar mosaico"
        );
    }

    #[tokio::test]
    async fn las_mas_escuchadas_miran_desde_donde_se_puso_cada_cancion() {
        // Es la diferencia entre "playlists que pones" y "playlists que
        // contienen canciones que has oido". Con lo segundo, meter un exito en
        // una lista que nunca abres la subiria a lo mas alto.
        let (repo, tracks, pool, _g) = ctx().await;

        let puesta = playlist("La que pongo");
        let ignorada = playlist("La que nunca abro");
        repo.create(&puesta).await.expect("crea");
        repo.create(&ignorada).await.expect("crea");

        // La misma cancion esta en las dos.
        let compartida = pista("Compartida");
        tracks
            .upsert(std::slice::from_ref(&compartida))
            .await
            .expect("guarda");
        for p in [&puesta, &ignorada] {
            repo.add_entries(
                &p.id,
                &[(PlaylistEntryId::nuevo(), compartida.id.clone(), 0.0)],
            )
            .await
            .expect("añade");
        }

        // Pero solo se ha escuchado **desde** una de ellas.
        let contexto = format!("playlist:{}", puesta.id.as_uuid());
        let id_pista = compartida.id.as_str().to_owned();
        pool.escribir(move |tx| {
            for _ in 0..3 {
                tx.execute(
                    "INSERT INTO play_history (track_id, played_at, ms_played, completed, context)
                     VALUES (?1, unixepoch(), 200000, 1, ?2)",
                    rusqlite::params![id_pista, contexto],
                )?;
            }
            Ok(())
        })
        .await
        .expect("registra escuchas");

        let top = repo.most_played(30, 10).await.expect("top");
        assert_eq!(top.len(), 1, "solo una se ha puesto de verdad: {top:?}");
        assert_eq!(top[0].name, "La que pongo");
    }

    #[tokio::test]
    async fn el_mosaico_recoge_los_primeros_albumes_sin_repetir() {
        // Cuatro es lo que cabe en la rejilla, y los duplicados no aportan: una
        // playlist de un disco entero enseñaria cuatro veces la misma portada.
        let (repo, tracks, pool, _g) = ctx().await;
        let p = playlist("Con portadas");
        repo.create(&p).await.expect("crea");

        // Seis pistas repartidas en tres albumes: A, A, B, B, C, C.
        let albumes = ["alb-a", "alb-b", "alb-c"];
        for a in albumes {
            pool.escribir(move |tx| {
                tx.execute(
                    "INSERT INTO albums (id, title, title_norm) VALUES (?1, ?1, ?1)",
                    [a],
                )?;
                Ok(())
            })
            .await
            .expect("crea album");
        }

        let pistas: Vec<Track> = (0..6)
            .map(|i| Track {
                album: Some(localify_core::domain::track::AlbumRef {
                    id: localify_core::domain::ids::AlbumId::from_trusted(
                        albumes[i / 2].to_owned(),
                    ),
                    title: albumes[i / 2].to_owned(),
                }),
                ..pista(&format!("P{i}"))
            })
            .collect();
        tracks.upsert(&pistas).await.expect("guarda");

        let entradas: Vec<(PlaylistEntryId, TrackId, f64)> = pistas
            .iter()
            .enumerate()
            .map(|(i, t)| {
                #[allow(clippy::cast_precision_loss, reason = "seis elementos")]
                let pos = i as f64 * position::PASO;
                (PlaylistEntryId::nuevo(), t.id.clone(), pos)
            })
            .collect();
        repo.add_entries(&p.id, &entradas).await.expect("añade");

        let resumenes = repo.list_summaries().await.expect("lista");
        let mio = resumenes.iter().find(|r| r.id == p.id).expect("aparece");

        let ids: Vec<&str> = mio.cover_albums.iter().map(AlbumId::as_str).collect();
        assert_eq!(
            ids.len(),
            3,
            "tres albumes distintos, no seis pistas: {ids:?}"
        );
    }

    #[tokio::test]
    async fn modificar_el_contenido_actualiza_la_fecha() {
        let (repo, tracks, pool, _g) = ctx().await;
        let (p, entradas) = con_pistas(&repo, &tracks, 3).await;

        let id = p.id.to_string();
        pool.escribir(move |tx| {
            tx.execute(
                "UPDATE playlists SET updated_at = 1000 WHERE id = ?1",
                [&id],
            )?;
            Ok(())
        })
        .await
        .expect("envejece");

        repo.remove_entries(&p.id, &[entradas[0]])
            .await
            .expect("borra");

        let actualizada = repo.get(&p.id).await.expect("lee").expect("existe");
        assert!(
            actualizada.updated_at.timestamp() > 1000,
            "la barra lateral ordena por actividad reciente"
        );
    }

    #[tokio::test]
    async fn borrar_la_playlist_arrastra_sus_entradas() {
        let (repo, tracks, pool, _g) = ctx().await;
        let (p, _) = con_pistas(&repo, &tracks, 3).await;

        repo.delete(&p.id).await.expect("borra");

        let entradas: i64 = pool
            .leer(|c| Ok(c.query_row("SELECT COUNT(*) FROM playlist_items", [], |r| r.get(0))?))
            .await
            .expect("cuenta");
        assert_eq!(entradas, 0);
    }

    #[tokio::test]
    async fn borrar_una_pista_la_saca_de_las_playlists_sin_romperlas() {
        let (repo, tracks, pool, _g) = ctx().await;
        let (p, _) = con_pistas(&repo, &tracks, 3).await;

        let primera = repo
            .entries(&p.id, &PageRequest::new(0, 1))
            .await
            .expect("lee")
            .items[0]
            .track
            .id
            .clone();

        let id = primera.as_str().to_owned();
        pool.escribir(move |tx| {
            tx.execute("DELETE FROM tracks WHERE id = ?1", [&id])?;
            Ok(())
        })
        .await
        .expect("borra pista");

        let restantes = titulos(&repo, &p.id).await;
        assert_eq!(restantes.len(), 2, "la playlist debe seguir siendo legible");
    }
}

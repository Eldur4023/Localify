//! Repositorio de favoritos ("Tus me gusta").

use async_trait::async_trait;
use localify_core::domain::ids::TrackId;
use localify_core::domain::track::TrackRow;
use localify_core::error::CoreResult;
use localify_core::page::{Cursor, Page, PageRequest};
use localify_core::ports::database::FavoriteRepository;
use rusqlite::params;

use crate::error::{DbResult, ToCore};
use crate::mappers::{COLUMNAS_TRACK_ROW, JOINS_TRACK_ROW, a_track_row, fecha_track_row};
use crate::pool::Pool;

pub struct SqliteFavoriteRepository {
    pool: Pool,
}

impl std::fmt::Debug for SqliteFavoriteRepository {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteFavoriteRepository")
            .finish_non_exhaustive()
    }
}

impl SqliteFavoriteRepository {
    #[must_use]
    pub const fn new(pool: Pool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl FavoriteRepository for SqliteFavoriteRepository {
    async fn set(&self, track: &TrackId, enabled: bool) -> CoreResult<()> {
        let id = track.as_str().to_owned();
        self.pool
            .escribir(move |tx| {
                if enabled {
                    // Idempotente: marcar dos veces no debe alterar `added_at`,
                    // que es lo que ordena la lista de favoritos.
                    tx.execute(
                        "INSERT INTO favorites (track_id) VALUES (?1)
                         ON CONFLICT (track_id) DO NOTHING",
                        [&id],
                    )?;
                } else {
                    tx.execute("DELETE FROM favorites WHERE track_id = ?1", [&id])?;
                }
                Ok(())
            })
            .await
            .to_core()
    }

    async fn is_favorite(&self, track: &TrackId) -> CoreResult<bool> {
        let id = track.as_str().to_owned();
        self.pool
            .leer(move |conn| {
                let existe: i64 = conn.query_row(
                    "SELECT EXISTS (SELECT 1 FROM favorites WHERE track_id = ?1)",
                    [&id],
                    |r| r.get(0),
                )?;
                Ok(existe != 0)
            })
            .await
            .to_core()
    }

    async fn list(&self, page: &PageRequest) -> CoreResult<Page<TrackRow>> {
        let limite = i64::from(page.limit());
        let offset = i64::from(page.offset());
        let columnas = COLUMNAS_TRACK_ROW;
        let joins = JOINS_TRACK_ROW;

        // El más reciente primero, como en Spotify. Desempate por `track_id`
        // para que la paginación sea estable si se marcan varias en el mismo
        // segundo.
        // La fecha es la de la marca, no la de la pista: "Tus me gusta" ordena
        // por cuándo se dio al corazón.
        let fecha = fecha_track_row("f.added_at");
        let sql = format!(
            "SELECT {columnas}{fecha} FROM tracks t {joins}
             WHERE f.track_id IS NOT NULL
             ORDER BY f.added_at DESC, t.id DESC
             LIMIT ?1 OFFSET ?2"
        );

        self.pool
            .leer(move |conn| {
                let total: i64 =
                    conn.query_row("SELECT COUNT(*) FROM favorites", [], |r| r.get(0))?;
                let total = total.max(0).unsigned_abs();

                let mut stmt = conn.prepare_cached(&sql)?;
                let items = stmt
                    .query_map(params![limite, offset], |row| Ok(a_track_row(row)))?
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

    async fn count(&self) -> CoreResult<u64> {
        self.pool
            .leer(|conn| {
                let n: i64 = conn.query_row("SELECT COUNT(*) FROM favorites", [], |r| r.get(0))?;
                Ok(n.max(0).unsigned_abs())
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

    async fn ctx() -> (
        SqliteFavoriteRepository,
        SqliteTrackRepository,
        Pool,
        TempDbGuard,
    ) {
        let (pool, guard) = Pool::temporal().expect("abre");
        crate::migrations::ejecutar(&pool).await.expect("migra");
        (
            SqliteFavoriteRepository::new(pool.clone()),
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

    #[tokio::test]
    async fn marcar_y_desmarcar_funciona() {
        let (repo, tracks, _pool, _g) = ctx().await;
        let t = pista("X");
        tracks
            .upsert(std::slice::from_ref(&t))
            .await
            .expect("guarda");

        assert!(!repo.is_favorite(&t.id).await.expect("consulta"));

        repo.set(&t.id, true).await.expect("marca");
        assert!(repo.is_favorite(&t.id).await.expect("consulta"));
        assert_eq!(repo.count().await.expect("cuenta"), 1);

        repo.set(&t.id, false).await.expect("desmarca");
        assert!(!repo.is_favorite(&t.id).await.expect("consulta"));
        assert_eq!(repo.count().await.expect("cuenta"), 0);
    }

    #[tokio::test]
    async fn marcar_dos_veces_no_altera_la_fecha() {
        let (repo, tracks, pool, _g) = ctx().await;
        let t = pista("X");
        tracks
            .upsert(std::slice::from_ref(&t))
            .await
            .expect("guarda");

        repo.set(&t.id, true).await.expect("marca");
        let primera: i64 = pool
            .leer(|c| Ok(c.query_row("SELECT added_at FROM favorites", [], |r| r.get(0))?))
            .await
            .expect("lee");

        // Fuerza una fecha antigua y vuelve a marcar: no debe reescribirse.
        pool.escribir(|tx| {
            tx.execute("UPDATE favorites SET added_at = 1000", [])?;
            Ok(())
        })
        .await
        .expect("envejece");

        repo.set(&t.id, true).await.expect("remarca");
        let segunda: i64 = pool
            .leer(|c| Ok(c.query_row("SELECT added_at FROM favorites", [], |r| r.get(0))?))
            .await
            .expect("lee");

        assert_eq!(
            segunda, 1000,
            "remarcar no debe reordenar la lista de favoritos"
        );
        assert_ne!(primera, 0);
    }

    #[tokio::test]
    async fn desmarcar_algo_no_marcado_no_es_un_error() {
        let (repo, tracks, _pool, _g) = ctx().await;
        let t = pista("X");
        tracks
            .upsert(std::slice::from_ref(&t))
            .await
            .expect("guarda");
        assert!(repo.set(&t.id, false).await.is_ok());
    }

    #[tokio::test]
    async fn la_lista_va_del_mas_reciente_al_mas_antiguo() {
        let (repo, tracks, pool, _g) = ctx().await;
        let a = pista("A");
        let b = pista("B");
        let c = pista("C");
        tracks
            .upsert(&[a.clone(), b.clone(), c.clone()])
            .await
            .expect("guarda");

        for t in [&a, &b, &c] {
            repo.set(&t.id, true).await.expect("marca");
        }

        // Fechas explícitas: el reloj no distingue tres inserciones seguidas.
        let (ia, ib, ic) = (
            a.id.as_str().to_owned(),
            b.id.as_str().to_owned(),
            c.id.as_str().to_owned(),
        );
        pool.escribir(move |tx| {
            tx.execute(
                "UPDATE favorites SET added_at = 100 WHERE track_id = ?1",
                [&ia],
            )?;
            tx.execute(
                "UPDATE favorites SET added_at = 300 WHERE track_id = ?1",
                [&ib],
            )?;
            tx.execute(
                "UPDATE favorites SET added_at = 200 WHERE track_id = ?1",
                [&ic],
            )?;
            Ok(())
        })
        .await
        .expect("ajusta fechas");

        let pagina = repo.list(&PageRequest::new(0, 50)).await.expect("lista");
        assert_eq!(
            pagina
                .items
                .iter()
                .map(|f| f.title.clone())
                .collect::<Vec<_>>(),
            vec!["B", "C", "A"]
        );
        assert!(
            pagina.items.iter().all(|f| f.is_favorite),
            "las filas de la lista de favoritos deben venir marcadas"
        );
    }

    #[tokio::test]
    async fn la_fila_trae_la_fecha_de_la_marca() {
        // La columna de fecha no viaja en `COLUMNAS_TRACK_ROW`: la pone cada
        // consulta. Si esta se olvidara de pedirla, la fila llegaría con `None`
        // —no con un error— y la columna de la interfaz quedaría vacía sin que
        // nada lo dijera. Este test es lo único que lo nota.
        let (repo, tracks, _pool, _g) = ctx().await;
        let t = pista("X");
        tracks
            .upsert(std::slice::from_ref(&t))
            .await
            .expect("guarda");
        repo.set(&t.id, true).await.expect("marca");

        let pagina = repo.list(&PageRequest::default()).await.expect("lista");
        assert!(
            pagina.items[0].added_at.is_some(),
            "la lista de favoritos debe fechar sus filas"
        );
    }

    #[tokio::test]
    async fn borrar_la_pista_arrastra_el_favorito() {
        let (repo, tracks, pool, _g) = ctx().await;
        let t = pista("X");
        tracks
            .upsert(std::slice::from_ref(&t))
            .await
            .expect("guarda");
        repo.set(&t.id, true).await.expect("marca");

        let id = t.id.as_str().to_owned();
        pool.escribir(move |tx| {
            tx.execute("DELETE FROM tracks WHERE id = ?1", [&id])?;
            Ok(())
        })
        .await
        .expect("borra");

        assert_eq!(repo.count().await.expect("cuenta"), 0);
    }
}

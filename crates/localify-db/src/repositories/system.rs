//! Repositorios de sistema: configuración, caché y letras.
//!
//! Tres almacenes pequeños y sin relación con el catálogo. Comparten fichero
//! porque cada uno son treinta líneas y separarlos solo añadiría navegación.

use async_trait::async_trait;
use localify_core::domain::ids::TrackId;
use localify_core::domain::lyrics::Lyrics;
use localify_core::error::CoreResult;
use localify_core::ports::database::{CacheRepository, LyricsRepository, SettingsRepository};
use rusqlite::params;

use crate::error::ToCore;
use crate::pool::Pool;

// ─────────────────────────────────────────────────────────────────────────────
// Configuración
// ─────────────────────────────────────────────────────────────────────────────

pub struct SqliteSettingsRepository {
    pool: Pool,
}

impl std::fmt::Debug for SqliteSettingsRepository {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteSettingsRepository")
            .finish_non_exhaustive()
    }
}

impl SqliteSettingsRepository {
    #[must_use]
    pub const fn new(pool: Pool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl SettingsRepository for SqliteSettingsRepository {
    async fn get_raw(&self, key: &str) -> CoreResult<Option<String>> {
        let key = key.to_owned();
        self.pool
            .leer(move |conn| {
                let valor = conn
                    .query_row("SELECT value FROM settings WHERE key = ?1", [&key], |r| {
                        r.get::<_, String>(0)
                    })
                    .ok();
                Ok(valor)
            })
            .await
            .to_core()
    }

    async fn set_raw(&self, key: &str, value: &str) -> CoreResult<()> {
        let (key, value) = (key.to_owned(), value.to_owned());
        self.pool
            .escribir(move |tx| {
                tx.execute(
                    "INSERT INTO settings (key, value) VALUES (?1, ?2)
                     ON CONFLICT (key) DO UPDATE SET value = ?2, updated_at = unixepoch()",
                    params![key, value],
                )?;
                Ok(())
            })
            .await
            .to_core()
    }

    async fn get_all(&self) -> CoreResult<Vec<(String, String)>> {
        self.pool
            .leer(|conn| {
                let mut stmt = conn.prepare_cached("SELECT key, value FROM settings")?;
                let filas = stmt
                    .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(filas)
            })
            .await
            .to_core()
    }

    async fn delete(&self, key: &str) -> CoreResult<()> {
        let key = key.to_owned();
        self.pool
            .escribir(move |tx| {
                tx.execute("DELETE FROM settings WHERE key = ?1", [&key])?;
                Ok(())
            })
            .await
            .to_core()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Caché
// ─────────────────────────────────────────────────────────────────────────────

pub struct SqliteCacheRepository {
    pool: Pool,
}

impl std::fmt::Debug for SqliteCacheRepository {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteCacheRepository")
            .finish_non_exhaustive()
    }
}

impl SqliteCacheRepository {
    #[must_use]
    pub const fn new(pool: Pool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl CacheRepository for SqliteCacheRepository {
    async fn get(&self, namespace: &str, key: &str) -> CoreResult<Option<Vec<u8>>> {
        let (ns, key) = (namespace.to_owned(), key.to_owned());
        self.pool
            .leer(move |conn| {
                // La caducidad se comprueba al leer y no solo en la purga
                // periódica: entre purgas pueden pasar horas, y devolver un
                // token de Spotify vencido provocaría un fallo desconcertante.
                let valor = conn
                    .query_row(
                        "SELECT value FROM cache_entries
                         WHERE namespace = ?1 AND key = ?2 AND expires_at > unixepoch()",
                        params![ns, key],
                        |r| r.get::<_, Vec<u8>>(0),
                    )
                    .ok();
                Ok(valor)
            })
            .await
            .to_core()
    }

    async fn put(&self, namespace: &str, key: &str, value: &[u8], ttl_secs: u64) -> CoreResult<()> {
        let (ns, key, value) = (namespace.to_owned(), key.to_owned(), value.to_vec());
        let ttl = i64::try_from(ttl_secs).unwrap_or(i64::MAX);

        self.pool
            .escribir(move |tx| {
                tx.execute(
                    "INSERT INTO cache_entries (namespace, key, value, expires_at)
                     VALUES (?1, ?2, ?3, unixepoch() + ?4)
                     ON CONFLICT (namespace, key) DO UPDATE SET
                         value      = ?3,
                         expires_at = unixepoch() + ?4,
                         created_at = unixepoch()",
                    params![ns, key, value, ttl],
                )?;
                Ok(())
            })
            .await
            .to_core()
    }

    async fn invalidate(&self, namespace: &str, key: &str) -> CoreResult<()> {
        let (ns, key) = (namespace.to_owned(), key.to_owned());
        self.pool
            .escribir(move |tx| {
                tx.execute(
                    "DELETE FROM cache_entries WHERE namespace = ?1 AND key = ?2",
                    params![ns, key],
                )?;
                Ok(())
            })
            .await
            .to_core()
    }

    async fn purge_expired(&self) -> CoreResult<u64> {
        self.pool
            .escribir(|tx| {
                let borradas = tx.execute(
                    "DELETE FROM cache_entries WHERE expires_at <= unixepoch()",
                    [],
                )?;
                Ok(borradas as u64)
            })
            .await
            .to_core()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Letras
// ─────────────────────────────────────────────────────────────────────────────

pub struct SqliteLyricsRepository {
    pool: Pool,
}

impl std::fmt::Debug for SqliteLyricsRepository {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteLyricsRepository")
            .finish_non_exhaustive()
    }
}

impl SqliteLyricsRepository {
    #[must_use]
    pub const fn new(pool: Pool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl LyricsRepository for SqliteLyricsRepository {
    async fn get(&self, track: &TrackId) -> CoreResult<Option<Lyrics>> {
        let id = track.as_str().to_owned();
        self.pool
            .leer(move |conn| {
                let fila = conn
                    .query_row(
                        "SELECT synced, plain, source, not_found FROM lyrics WHERE track_id = ?1",
                        [&id],
                        |r| {
                            Ok((
                                r.get::<_, Option<String>>(0)?,
                                r.get::<_, Option<String>>(1)?,
                                r.get::<_, Option<String>>(2)?,
                                r.get::<_, i64>(3)?,
                            ))
                        },
                    )
                    .ok();

                let Some((synced, plain, source, not_found)) = fila else {
                    return Ok(None);
                };
                // Una marca de "no existe" no es una letra vacía: es la
                // ausencia de letra, y quien pregunta debe verla como `None`.
                if not_found != 0 {
                    return Ok(None);
                }

                Ok(Some(Lyrics {
                    synced: synced.as_deref().and_then(|s| serde_json::from_str(s).ok()),
                    plain,
                    source: source.unwrap_or_default(),
                }))
            })
            .await
            .to_core()
    }

    async fn save(&self, track: &TrackId, lyrics: &Lyrics) -> CoreResult<()> {
        let id = track.as_str().to_owned();
        let synced = lyrics
            .synced
            .as_ref()
            .and_then(|l| serde_json::to_string(l).ok());
        let plain = lyrics.plain.clone();
        let source = lyrics.source.clone();

        self.pool
            .escribir(move |tx| {
                tx.execute(
                    "INSERT INTO lyrics (track_id, synced, plain, source, not_found, fetched_at)
                     VALUES (?1, ?2, ?3, ?4, 0, unixepoch())
                     ON CONFLICT (track_id) DO UPDATE SET
                         synced     = ?2,
                         plain      = ?3,
                         source     = ?4,
                         not_found  = 0,
                         fetched_at = unixepoch()",
                    params![id, synced, plain, source],
                )?;
                Ok(())
            })
            .await
            .to_core()
    }

    async fn mark_not_found(&self, track: &TrackId) -> CoreResult<()> {
        let id = track.as_str().to_owned();
        self.pool
            .escribir(move |tx| {
                tx.execute(
                    "INSERT INTO lyrics (track_id, not_found, fetched_at)
                     VALUES (?1, 1, unixepoch())
                     ON CONFLICT (track_id) DO UPDATE SET
                         synced     = NULL,
                         plain      = NULL,
                         not_found  = 1,
                         fetched_at = unixepoch()",
                    [&id],
                )?;
                Ok(())
            })
            .await
            .to_core()
    }

    async fn is_marked_not_found(&self, track: &TrackId) -> CoreResult<bool> {
        let id = track.as_str().to_owned();
        self.pool
            .leer(move |conn| {
                // La caché negativa caduca: una canción sin letra hoy puede
                // tenerla dentro de un mes, y reintentar cada 30 días es
                // razonable sin llegar a molestar al proveedor.
                let marcada: i64 = conn.query_row(
                    "SELECT EXISTS (
                         SELECT 1 FROM lyrics
                         WHERE track_id = ?1
                           AND not_found = 1
                           AND fetched_at > unixepoch() - 2592000
                     )",
                    [&id],
                    |r| r.get(0),
                )?;
                Ok(marcada != 0)
            })
            .await
            .to_core()
    }
}

#[cfg(test)]
mod tests {
    use localify_core::domain::audio::DurationMs;
    use localify_core::domain::lyrics::LyricLine;
    use localify_core::domain::track::Track;
    use localify_core::ports::database::TrackRepository;

    use super::*;
    use crate::pool::TempDbGuard;
    use crate::repositories::tracks::SqliteTrackRepository;

    async fn pool_migrado() -> (Pool, TempDbGuard) {
        let (pool, guard) = Pool::temporal().expect("abre");
        crate::migrations::ejecutar(&pool).await.expect("migra");
        (pool, guard)
    }

    // ── Configuración ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn los_ajustes_hacen_ida_y_vuelta() {
        let (pool, _g) = pool_migrado().await;
        let repo = SqliteSettingsRepository::new(pool);

        assert_eq!(repo.get_raw("language").await.expect("lee"), None);

        repo.set_raw("language", r#""es""#).await.expect("guarda");
        assert_eq!(
            repo.get_raw("language").await.expect("lee"),
            Some(r#""es""#.into())
        );

        repo.set_raw("language", r#""en""#)
            .await
            .expect("sobrescribe");
        assert_eq!(
            repo.get_raw("language").await.expect("lee"),
            Some(r#""en""#.into())
        );

        repo.delete("language").await.expect("borra");
        assert_eq!(repo.get_raw("language").await.expect("lee"), None);
    }

    #[tokio::test]
    async fn get_all_devuelve_todos_los_ajustes() {
        let (pool, _g) = pool_migrado().await;
        let repo = SqliteSettingsRepository::new(pool);

        repo.set_raw("a", "1").await.expect("guarda");
        repo.set_raw("b", "2").await.expect("guarda");

        let todos = repo.get_all().await.expect("lee");
        assert_eq!(todos.len(), 2);
        assert!(todos.contains(&("a".to_owned(), "1".to_owned())));
    }

    // ── Caché ────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn la_cache_devuelve_lo_guardado() {
        let (pool, _g) = pool_migrado().await;
        let repo = SqliteCacheRepository::new(pool);

        repo.put("spotify:track", "abc", b"contenido", 3600)
            .await
            .expect("guarda");
        assert_eq!(
            repo.get("spotify:track", "abc").await.expect("lee"),
            Some(b"contenido".to_vec())
        );
    }

    #[tokio::test]
    async fn una_entrada_caducada_no_se_devuelve_aunque_siga_en_disco() {
        let (pool, _g) = pool_migrado().await;
        let repo = SqliteCacheRepository::new(pool.clone());

        repo.put("spotify:search", "q", b"viejo", 3600)
            .await
            .expect("guarda");
        pool.escribir(|tx| {
            tx.execute("UPDATE cache_entries SET expires_at = unixepoch() - 1", [])?;
            Ok(())
        })
        .await
        .expect("caduca");

        assert_eq!(
            repo.get("spotify:search", "q").await.expect("lee"),
            None,
            "entre purgas puede pasar horas: la caducidad debe mirarse al leer"
        );
    }

    #[tokio::test]
    async fn los_namespaces_no_se_pisan() {
        let (pool, _g) = pool_migrado().await;
        let repo = SqliteCacheRepository::new(pool);

        repo.put("ns1", "misma", b"uno", 3600)
            .await
            .expect("guarda");
        repo.put("ns2", "misma", b"dos", 3600)
            .await
            .expect("guarda");

        assert_eq!(
            repo.get("ns1", "misma").await.expect("lee"),
            Some(b"uno".to_vec())
        );
        assert_eq!(
            repo.get("ns2", "misma").await.expect("lee"),
            Some(b"dos".to_vec())
        );
    }

    #[tokio::test]
    async fn la_purga_solo_borra_lo_caducado() {
        let (pool, _g) = pool_migrado().await;
        let repo = SqliteCacheRepository::new(pool.clone());

        repo.put("ns", "viva", b"x", 3600).await.expect("guarda");
        repo.put("ns", "muerta", b"y", 3600).await.expect("guarda");
        pool.escribir(|tx| {
            tx.execute(
                "UPDATE cache_entries SET expires_at = unixepoch() - 1 WHERE key = 'muerta'",
                [],
            )?;
            Ok(())
        })
        .await
        .expect("caduca");

        assert_eq!(repo.purge_expired().await.expect("purga"), 1);
        assert!(repo.get("ns", "viva").await.expect("lee").is_some());
    }

    #[tokio::test]
    async fn invalidar_borra_una_entrada_concreta() {
        let (pool, _g) = pool_migrado().await;
        let repo = SqliteCacheRepository::new(pool);

        repo.put("ns", "a", b"1", 3600).await.expect("guarda");
        repo.put("ns", "b", b"2", 3600).await.expect("guarda");
        repo.invalidate("ns", "a").await.expect("invalida");

        assert!(repo.get("ns", "a").await.expect("lee").is_none());
        assert!(repo.get("ns", "b").await.expect("lee").is_some());
    }

    // ── Letras ───────────────────────────────────────────────────────────────

    async fn con_pista() -> (SqliteLyricsRepository, TrackId, Pool, TempDbGuard) {
        let (pool, guard) = pool_migrado().await;
        let tracks = SqliteTrackRepository::new(pool.clone());
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
        tracks
            .upsert(std::slice::from_ref(&t))
            .await
            .expect("guarda");
        (SqliteLyricsRepository::new(pool.clone()), t.id, pool, guard)
    }

    #[tokio::test]
    async fn una_letra_sincronizada_hace_ida_y_vuelta() {
        let (repo, id, _pool, _g) = con_pista().await;
        let letra = Lyrics {
            synced: Some(vec![
                LyricLine {
                    at: DurationMs::new(1000),
                    text: "primera".into(),
                },
                LyricLine {
                    at: DurationMs::new(5000),
                    text: "segunda".into(),
                },
            ]),
            plain: Some("primera\nsegunda".into()),
            source: "lrclib".into(),
        };

        repo.save(&id, &letra).await.expect("guarda");
        let leida = repo.get(&id).await.expect("lee").expect("existe");

        assert!(leida.tiene_sincronizacion());
        assert_eq!(leida.linea_en(DurationMs::new(6000)), Some(1));
        assert_eq!(leida.source, "lrclib");
    }

    #[tokio::test]
    async fn una_pista_sin_letra_devuelve_none_y_no_error() {
        let (repo, id, _pool, _g) = con_pista().await;
        assert!(repo.get(&id).await.expect("consulta").is_none());
    }

    #[tokio::test]
    async fn la_marca_de_no_encontrada_se_lee_como_ausencia_de_letra() {
        let (repo, id, _pool, _g) = con_pista().await;
        repo.mark_not_found(&id).await.expect("marca");

        assert!(repo.get(&id).await.expect("consulta").is_none());
        assert!(repo.is_marked_not_found(&id).await.expect("consulta"));
    }

    #[tokio::test]
    async fn la_cache_negativa_caduca_para_poder_reintentar() {
        let (repo, id, pool, _g) = con_pista().await;
        repo.mark_not_found(&id).await.expect("marca");
        assert!(repo.is_marked_not_found(&id).await.expect("consulta"));

        // Más de 30 días atrás.
        pool.escribir(|tx| {
            tx.execute("UPDATE lyrics SET fetched_at = unixepoch() - 3000000", [])?;
            Ok(())
        })
        .await
        .expect("envejece");

        assert!(
            !repo.is_marked_not_found(&id).await.expect("consulta"),
            "una canción sin letra hoy puede tenerla dentro de un mes"
        );
    }

    #[tokio::test]
    async fn guardar_una_letra_borra_la_marca_negativa_previa() {
        let (repo, id, _pool, _g) = con_pista().await;
        repo.mark_not_found(&id).await.expect("marca");

        repo.save(
            &id,
            &Lyrics {
                synced: None,
                plain: Some("texto".into()),
                source: "lrclib".into(),
            },
        )
        .await
        .expect("guarda");

        assert!(!repo.is_marked_not_found(&id).await.expect("consulta"));
        assert!(repo.get(&id).await.expect("lee").is_some());
    }
}

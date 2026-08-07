//! Migraciones del esquema.
//!
//! `refinery` con SQL embebido en el binario. Reglas del proyecto:
//!
//! 1. Una migración aplicada **nunca** se edita. Los errores se corrigen con una
//!    migración nueva.
//! 2. Antes de aplicar, se hace copia de seguridad de la base de datos.
//! 3. Si una migración falla, la aplicación **no se cierra**: arranca en modo
//!    degradado, con la interfaz operativa y el error visible. Cerrarse dejaría
//!    al usuario sin forma de recuperar su biblioteca.

use std::path::{Path, PathBuf};

use rusqlite::Connection;
use tracing::{info, warn};

use crate::error::{DbError, DbResult};
use crate::pool::Pool;

refinery::embed_migrations!("migrations");

/// Copias de seguridad que se conservan. Dos bastan para volver atrás sin
/// llenar el disco de una biblioteca grande.
const BACKUPS_A_CONSERVAR: usize = 2;

/// Resultado de arrancar las migraciones.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EstadoEsquema {
    /// El esquema está al día.
    AlDia { version: u32 },
    /// Se aplicaron migraciones.
    Migrado {
        desde: u32,
        hasta: u32,
        backup: Option<PathBuf>,
    },
    /// Falló. La aplicación debe arrancar en modo degradado.
    Fallido {
        detalle: String,
        backup: Option<PathBuf>,
    },
    /// La base de datos es de una versión posterior de Localify.
    ///
    /// Ocurre al instalar una versión anterior sobre datos nuevos. Migrar hacia
    /// atrás no existe, así que se detecta y se avisa en lugar de corromper.
    DemasiadoNueva { version_bd: u32, version_app: u32 },
}

impl EstadoEsquema {
    /// `true` si la aplicación puede funcionar con normalidad.
    #[must_use]
    pub const fn es_utilizable(&self) -> bool {
        matches!(self, Self::AlDia { .. } | Self::Migrado { .. })
    }
}

/// Aplica las migraciones pendientes.
///
/// **No devuelve `Err` por un fallo de migración**: lo reporta como
/// [`EstadoEsquema::Fallido`] para que el arranque continúe. Solo falla si ni
/// siquiera se puede consultar el estado del esquema.
///
/// # Errors
/// Si la conexión no responde.
pub async fn ejecutar(pool: &Pool) -> DbResult<EstadoEsquema> {
    let version_actual = pool.leer(version_usuario).await?;
    let version_objetivo = version_maxima();

    if version_actual > version_objetivo {
        warn!(
            version_bd = version_actual,
            version_app = version_objetivo,
            "la base de datos es más nueva que esta versión de Localify"
        );
        return Ok(EstadoEsquema::DemasiadoNueva {
            version_bd: version_actual,
            version_app: version_objetivo,
        });
    }

    if version_actual == version_objetivo {
        info!(version = version_actual, "esquema al día");
        return Ok(EstadoEsquema::AlDia {
            version: version_actual,
        });
    }

    // Copia de seguridad antes de tocar nada. En una base de datos vacía no
    // aporta y solo generaría basura.
    let backup = if version_actual > 0 {
        match crear_backup(pool.ruta(), version_actual) {
            Ok(p) => {
                info!(backup = %p.display(), "copia de seguridad creada");
                Some(p)
            }
            Err(e) => {
                // Sin copia de seguridad no se migra: el riesgo de perder una
                // biblioteca entera no compensa el de arrancar en degradado.
                warn!(error = %e, "no se pudo crear la copia de seguridad; no se migra");
                return Ok(EstadoEsquema::Fallido {
                    detalle: format!("no se pudo crear la copia de seguridad: {e}"),
                    backup: None,
                });
            }
        }
    } else {
        None
    };

    info!(
        desde = version_actual,
        hasta = version_objetivo,
        "aplicando migraciones"
    );

    let resultado = pool
        .escribir_sin_transaccion(|conn| {
            // refinery gestiona su propia transacción por migración.
            migrations::runner()
                .set_abort_divergent(true)
                .set_abort_missing(true)
                .run(conn)
                .map_err(DbError::from)?;
            Ok(())
        })
        .await;

    match resultado {
        Ok(()) => {
            pool.escribir_sin_transaccion(move |conn| {
                conn.pragma_update(None, "user_version", version_objetivo)?;
                Ok(())
            })
            .await?;

            if let Some(b) = backup.as_deref() {
                podar_backups(b);
            }

            info!(version = version_objetivo, "migraciones aplicadas");
            Ok(EstadoEsquema::Migrado {
                desde: version_actual,
                hasta: version_objetivo,
                backup,
            })
        }
        Err(e) => {
            warn!(error = %e, "las migraciones fallaron; arranque en modo degradado");
            Ok(EstadoEsquema::Fallido {
                detalle: e.to_string(),
                backup,
            })
        }
    }
}

/// Versión de esquema que espera este binario.
#[must_use]
pub fn version_maxima() -> u32 {
    migrations::runner()
        .get_migrations()
        .iter()
        .map(refinery::Migration::version)
        .max()
        .and_then(|v| u32::try_from(v).ok())
        .unwrap_or(0)
}

fn version_usuario(conn: &rusqlite::Connection) -> DbResult<u32> {
    let v: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    Ok(u32::try_from(v).unwrap_or(0))
}

/// Copia la base de datos usando la API de backup de SQLite.
///
/// No vale con copiar el fichero: el WAL puede contener transacciones aún no
/// integradas, y una copia a nivel de sistema de ficheros produciría una base de
/// datos inconsistente. `VACUUM INTO` genera una copia coherente y compacta.
fn crear_backup(ruta: &Path, version: u32) -> DbResult<PathBuf> {
    let destino = ruta.with_extension(format!("bak.v{version}.db"));
    let _ = std::fs::remove_file(&destino);

    let conn = Connection::open(ruta).map_err(|e| DbError::Apertura {
        ruta: ruta.display().to_string(),
        causa: e.to_string(),
    })?;

    conn.execute("VACUUM INTO ?1", [destino.to_string_lossy().as_ref()])?;
    Ok(destino)
}

/// Conserva solo las últimas copias de seguridad.
fn podar_backups(ejemplo: &Path) {
    let Some(dir) = ejemplo.parent() else { return };
    let Ok(entradas) = std::fs::read_dir(dir) else {
        return;
    };

    let mut backups: Vec<_> = entradas
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            p.file_name().and_then(|n| n.to_str()).is_some_and(|n| {
                n.starts_with("localify.bak.v")
                    && p.extension().is_some_and(|e| e.eq_ignore_ascii_case("db"))
            })
        })
        .collect();

    if backups.len() <= BACKUPS_A_CONSERVAR {
        return;
    }

    backups.sort();
    for viejo in &backups[..backups.len() - BACKUPS_A_CONSERVAR] {
        if std::fs::remove_file(viejo).is_ok() {
            info!(backup = %viejo.display(), "copia de seguridad antigua eliminada");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn migra_una_base_de_datos_vacia() {
        let (pool, _guard) = Pool::temporal().expect("abre");
        let estado = ejecutar(&pool).await.expect("migra");
        assert!(estado.es_utilizable());

        match &estado {
            EstadoEsquema::Migrado {
                desde,
                hasta,
                backup,
            } => {
                assert_eq!(*desde, 0);
                assert_eq!(*hasta, version_maxima());
                assert!(
                    backup.is_none(),
                    "una base de datos vacía no necesita copia"
                );
            }
            otro => panic!("se esperaba Migrado, llegó {otro:?}"),
        }
    }

    #[tokio::test]
    async fn migrar_dos_veces_es_idempotente() {
        let (pool, _guard) = Pool::temporal().expect("abre");
        ejecutar(&pool).await.expect("primera");

        let segunda = ejecutar(&pool).await.expect("segunda");
        assert_eq!(
            segunda,
            EstadoEsquema::AlDia {
                version: version_maxima()
            }
        );
    }

    #[tokio::test]
    async fn crea_todas_las_tablas_del_diseno() {
        let (pool, _guard) = Pool::temporal().expect("abre");
        ejecutar(&pool).await.expect("migra");

        let tablas: Vec<String> = pool
            .leer(|c| {
                let mut stmt = c.prepare(
                    "SELECT name FROM sqlite_master
                     WHERE type IN ('table','view') AND name NOT LIKE 'sqlite_%'
                     ORDER BY name",
                )?;
                let filas = stmt
                    .query_map([], |r| r.get::<_, String>(0))?
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(filas)
            })
            .await
            .expect("lista");

        for esperada in [
            "albums",
            "album_artists",
            "artists",
            "artist_genres",
            "audio_files",
            "cache_entries",
            "download_jobs",
            "favorites",
            "genres",
            "lyrics",
            "play_history",
            "player_state",
            "playlists",
            "playlist_items",
            "scan_reports",
            "scrobble_queue",
            "settings",
            "tracks",
            "tracks_fts",
            "track_artists",
            "youtube_matches",
        ] {
            assert!(
                tablas.contains(&esperada.to_owned()),
                "falta la tabla '{esperada}'"
            );
        }
    }

    #[tokio::test]
    async fn player_state_arranca_con_su_fila_unica() {
        let (pool, _guard) = Pool::temporal().expect("abre");
        ejecutar(&pool).await.expect("migra");

        let filas: i64 = pool
            .leer(|c| Ok(c.query_row("SELECT COUNT(*) FROM player_state", [], |r| r.get(0))?))
            .await
            .expect("cuenta");
        assert_eq!(filas, 1);

        // El CHECK debe impedir una segunda fila.
        let segunda = pool
            .escribir(|tx| Ok(tx.execute("INSERT INTO player_state (id) VALUES (2)", [])?))
            .await;
        assert!(
            segunda.is_err(),
            "player_state debe tener exactamente una fila"
        );
    }

    #[tokio::test]
    async fn una_base_de_datos_mas_nueva_se_detecta_en_vez_de_corromperse() {
        let (pool, _guard) = Pool::temporal().expect("abre");
        ejecutar(&pool).await.expect("migra");

        let futura = version_maxima() + 10;
        pool.escribir_sin_transaccion(move |c| {
            c.pragma_update(None, "user_version", futura)?;
            Ok(())
        })
        .await
        .expect("marca");

        let estado = ejecutar(&pool).await.expect("consulta");
        assert_eq!(
            estado,
            EstadoEsquema::DemasiadoNueva {
                version_bd: futura,
                version_app: version_maxima()
            }
        );
        assert!(!estado.es_utilizable());
    }
}

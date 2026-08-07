//! Pool de conexiones.
//!
//! # Por qué este diseño
//!
//! SQLite en WAL admite **N lectores concurrentes y un único escritor**. El
//! pool refleja esa realidad literalmente en lugar de disimularla:
//!
//! - **Lectores**: `min(4, núcleos)` conexiones en modo `query_only`, cada una
//!   atendida desde el pool de hilos bloqueantes de Tokio.
//! - **Escritor**: una sola conexión, tras una cola. Elimina por construcción
//!   los `SQLITE_BUSY` entre nuestras propias escrituras y hace imposible que
//!   dos transacciones se entrelacen.
//!
//! Un pool genérico de N conexiones indistintas dejaría a los escritores
//! peleándose por el lock del fichero, con reintentos y latencias erráticas.
//!
//! # Nada de SQLite en el runtime asíncrono
//!
//! rusqlite es síncrono. Toda operación se ejecuta en `spawn_blocking`, así que
//! una consulta lenta nunca ocupa un hilo del runtime ni retrasa el resto de la
//! aplicación.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rusqlite::{Connection, OpenFlags, Transaction};
use tokio::sync::{Mutex, Semaphore};
use tracing::{debug, info};

use crate::error::{DbError, DbResult};
use crate::pragmas;

/// Número máximo de conexiones de lectura.
///
/// Más allá de cuatro no se gana nada: la base de datos cabe en la caché de
/// páginas del sistema operativo y el cuello de botella deja de ser la E/S.
const MAX_LECTORES: usize = 4;

/// Handle del pool. Barato de clonar; se comparte entre todos los repositorios.
#[derive(Clone)]
pub struct Pool {
    inner: Arc<Inner>,
}

struct Inner {
    ruta: PathBuf,
    /// Limita cuántas lecturas concurrentes hay en vuelo. Cada permiso abre su
    /// propia conexión en el hilo bloqueante: es más simple que reciclar
    /// conexiones entre hilos y, al ser un fichero local, abrir cuesta poco.
    lectores: Semaphore,
    /// El escritor es un recurso único, y el `Mutex` lo hace explícito.
    escritor: Mutex<Option<Connection>>,
}

impl std::fmt::Debug for Pool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pool")
            .field("ruta", &self.inner.ruta)
            .field("max_lectores", &MAX_LECTORES)
            .finish()
    }
}

impl Pool {
    /// Abre la base de datos y prepara el pool.
    ///
    /// No aplica migraciones: eso lo hace [`crate::migrations`], que necesita
    /// poder hacer copia de seguridad antes.
    ///
    /// # Errors
    /// Si el fichero no se puede abrir o los PRAGMA no se aplican.
    pub fn abrir(ruta: &Path) -> DbResult<Self> {
        if let Some(padre) = ruta.parent() {
            std::fs::create_dir_all(padre).map_err(|e| DbError::Apertura {
                ruta: padre.display().to_string(),
                causa: e.to_string(),
            })?;
        }

        let escritor = Connection::open_with_flags(
            ruta,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_CREATE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|e| DbError::Apertura {
            ruta: ruta.display().to_string(),
            causa: e.to_string(),
        })?;

        pragmas::aplicar_escritor(&escritor)?;
        pragmas::verificar(&escritor)?;

        let lectores = std::thread::available_parallelism()
            .map_or(2, std::num::NonZeroUsize::get)
            .clamp(1, MAX_LECTORES);

        info!(ruta = %ruta.display(), lectores, "base de datos abierta");

        Ok(Self {
            inner: Arc::new(Inner {
                ruta: ruta.to_path_buf(),
                lectores: Semaphore::new(lectores),
                escritor: Mutex::new(Some(escritor)),
            }),
        })
    }

    /// Base de datos en memoria, para tests.
    ///
    /// Usa un fichero temporal y no `:memory:` porque el modo en memoria no
    /// admite WAL, y probar con una configuración distinta de la de producción
    /// deja pasar justo los fallos que importan.
    ///
    /// # Errors
    /// Si no se puede crear el fichero temporal.
    pub fn temporal() -> DbResult<(Self, TempDbGuard)> {
        let nombre = format!("localify-test-{}.db", uuid::Uuid::now_v7());
        let ruta = std::env::temp_dir().join(nombre);
        let pool = Self::abrir(&ruta)?;
        Ok((pool, TempDbGuard { ruta }))
    }

    #[must_use]
    pub fn ruta(&self) -> &Path {
        &self.inner.ruta
    }

    /// Ejecuta una lectura en el pool bloqueante.
    ///
    /// # Errors
    /// Lo que devuelva `f`, o [`DbError::PoolCaido`] si el hilo muere.
    pub async fn leer<T, F>(&self, f: F) -> DbResult<T>
    where
        F: FnOnce(&Connection) -> DbResult<T> + Send + 'static,
        T: Send + 'static,
    {
        let permiso = self
            .inner
            .lectores
            .acquire()
            .await
            .map_err(|_| DbError::PoolCaido)?;

        let ruta = self.inner.ruta.clone();

        let resultado = tokio::task::spawn_blocking(move || {
            let conn = Connection::open_with_flags(
                &ruta,
                OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )
            .map_err(|e| DbError::Apertura {
                ruta: ruta.display().to_string(),
                causa: e.to_string(),
            })?;
            pragmas::aplicar_lector(&conn)?;
            f(&conn)
        })
        .await
        .map_err(|_| DbError::PoolCaido)?;

        drop(permiso);
        resultado
    }

    /// Ejecuta una escritura dentro de una transacción.
    ///
    /// La transacción se confirma si `f` devuelve `Ok` y se revierte en caso
    /// contrario. **No hay forma de escribir fuera de una transacción**, que es
    /// justo lo que garantiza que una operación lógica no quede a medias.
    ///
    /// # Errors
    /// Lo que devuelva `f`, o [`DbError::PoolCaido`] si el hilo muere.
    pub async fn escribir<T, F>(&self, f: F) -> DbResult<T>
    where
        F: FnOnce(&Transaction<'_>) -> DbResult<T> + Send + 'static,
        T: Send + 'static,
    {
        let mut guard = self.inner.escritor.lock().await;
        let conn = guard.take().ok_or(DbError::PoolCaido)?;

        // La conexión viaja al hilo bloqueante y vuelve. Devolverla siempre
        // —incluso si `f` falla— es imprescindible: perderla dejaría la
        // aplicación sin escritor para el resto de la sesión.
        let (conn, resultado) = tokio::task::spawn_blocking(move || {
            let mut conn = conn;
            let resultado = (|| {
                let tx = conn.transaction()?;
                let valor = f(&tx)?;
                tx.commit()?;
                Ok(valor)
            })();
            (conn, resultado)
        })
        .await
        .map_err(|_| DbError::PoolCaido)?;

        *guard = Some(conn);
        resultado
    }

    /// Ejecuta trabajo sobre la conexión de escritura **sin** envolverlo en una
    /// transacción.
    ///
    /// Solo para lo que SQLite prohíbe dentro de una transacción: `VACUUM`,
    /// `PRAGMA optimize`, `wal_checkpoint` y las migraciones, que gestionan su
    /// propia transaccionalidad.
    ///
    /// # Errors
    /// Lo que devuelva `f`, o [`DbError::PoolCaido`] si el hilo muere.
    pub async fn escribir_sin_transaccion<T, F>(&self, f: F) -> DbResult<T>
    where
        F: FnOnce(&mut Connection) -> DbResult<T> + Send + 'static,
        T: Send + 'static,
    {
        let mut guard = self.inner.escritor.lock().await;
        let conn = guard.take().ok_or(DbError::PoolCaido)?;

        let (conn, resultado) = tokio::task::spawn_blocking(move || {
            let mut conn = conn;
            let resultado = f(&mut conn);
            (conn, resultado)
        })
        .await
        .map_err(|_| DbError::PoolCaido)?;

        *guard = Some(conn);
        resultado
    }

    /// Cierra el escritor. Se llama al apagar, tras el último volcado de estado.
    pub async fn cerrar(&self) {
        if let Some(conn) = self.inner.escritor.lock().await.take() {
            // `optimize` actualiza las estadísticas del planificador de
            // consultas. Hacerlo al cerrar es gratis y mejora los planes de la
            // siguiente sesión.
            let _ = conn.execute_batch("PRAGMA optimize;");
            let _ = conn.close();
            debug!("escritor cerrado");
        }
    }
}

/// Borra la base de datos temporal al soltarse.
#[derive(Debug)]
pub struct TempDbGuard {
    ruta: PathBuf,
}

impl Drop for TempDbGuard {
    fn drop(&mut self) {
        for sufijo in ["", "-wal", "-shm"] {
            let p = PathBuf::from(format!("{}{sufijo}", self.ruta.display()));
            let _ = std::fs::remove_file(p);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn abrir_aplica_wal_y_claves_foraneas() {
        let (pool, _guard) = Pool::temporal().expect("abre");

        let modo: String = pool
            .leer(|c| Ok(c.query_row("PRAGMA journal_mode", [], |r| r.get(0))?))
            .await
            .expect("lee");
        assert!(modo.eq_ignore_ascii_case("wal"));

        let fk: i32 = pool
            .leer(|c| Ok(c.query_row("PRAGMA foreign_keys", [], |r| r.get(0))?))
            .await
            .expect("lee");
        assert_eq!(
            fk, 1,
            "sin foreign_keys, las FK declaradas serían decoración"
        );
    }

    #[tokio::test]
    async fn una_lectura_no_puede_escribir() {
        let (pool, _guard) = Pool::temporal().expect("abre");
        pool.escribir(|tx| {
            tx.execute_batch("CREATE TABLE t (x INTEGER)")?;
            Ok(())
        })
        .await
        .expect("crea");

        let intento = pool
            .leer(|c| Ok(c.execute("INSERT INTO t VALUES (1)", [])?))
            .await;

        assert!(
            intento.is_err(),
            "query_only debe impedir escribir desde un lector"
        );
    }

    #[tokio::test]
    async fn una_transaccion_fallida_no_deja_rastro() {
        let (pool, _guard) = Pool::temporal().expect("abre");
        pool.escribir(|tx| {
            tx.execute_batch("CREATE TABLE t (x INTEGER)")?;
            Ok(())
        })
        .await
        .expect("crea");

        let resultado: DbResult<()> = pool
            .escribir(|tx| {
                tx.execute("INSERT INTO t VALUES (1)", [])?;
                tx.execute("INSERT INTO t VALUES (2)", [])?;
                // Abortar después de escribir: nada debe persistir.
                Err(DbError::Configuracion("fallo simulado".into()))
            })
            .await;

        assert!(resultado.is_err());

        let filas: i64 = pool
            .leer(|c| Ok(c.query_row("SELECT COUNT(*) FROM t", [], |r| r.get(0))?))
            .await
            .expect("cuenta");
        assert_eq!(filas, 0, "el rollback no revirtió las escrituras");
    }

    #[tokio::test]
    async fn el_escritor_sobrevive_a_una_transaccion_fallida() {
        let (pool, _guard) = Pool::temporal().expect("abre");
        pool.escribir(|tx| {
            tx.execute_batch("CREATE TABLE t (x INTEGER)")?;
            Ok(())
        })
        .await
        .expect("crea");

        let _: DbResult<()> = pool
            .escribir(|_| Err(DbError::Configuracion("fallo".into())))
            .await;

        // Si el fallo se hubiera llevado la conexión, la app quedaría sin
        // escritor para toda la sesión.
        pool.escribir(|tx| {
            tx.execute("INSERT INTO t VALUES (42)", [])?;
            Ok(())
        })
        .await
        .expect("el escritor debe seguir disponible");

        let x: i64 = pool
            .leer(|c| Ok(c.query_row("SELECT x FROM t", [], |r| r.get(0))?))
            .await
            .expect("lee");
        assert_eq!(x, 42);
    }

    #[tokio::test]
    async fn varias_lecturas_concurrentes_funcionan() {
        let (pool, _guard) = Pool::temporal().expect("abre");
        pool.escribir(|tx| {
            tx.execute_batch("CREATE TABLE t (x INTEGER); INSERT INTO t VALUES (7)")?;
            Ok(())
        })
        .await
        .expect("crea");

        let tareas: Vec<_> = (0..16)
            .map(|_| {
                let p = pool.clone();
                tokio::spawn(async move {
                    p.leer(|c| Ok(c.query_row::<i64, _, _>("SELECT x FROM t", [], |r| r.get(0))?))
                        .await
                })
            })
            .collect();

        for t in tareas {
            let valor = t.await.expect("join").expect("lee");
            assert_eq!(valor, 7);
        }
    }

    #[tokio::test]
    async fn las_escrituras_concurrentes_se_serializan_sin_perder_ninguna() {
        let (pool, _guard) = Pool::temporal().expect("abre");
        pool.escribir(|tx| {
            tx.execute_batch("CREATE TABLE t (x INTEGER)")?;
            Ok(())
        })
        .await
        .expect("crea");

        let tareas: Vec<_> = (0..50_i64)
            .map(|i| {
                let p = pool.clone();
                tokio::spawn(async move {
                    p.escribir(move |tx| {
                        tx.execute("INSERT INTO t VALUES (?1)", [i])?;
                        Ok(())
                    })
                    .await
                })
            })
            .collect();

        for t in tareas {
            t.await.expect("join").expect("escribe");
        }

        let total: i64 = pool
            .leer(|c| Ok(c.query_row("SELECT COUNT(*) FROM t", [], |r| r.get(0))?))
            .await
            .expect("cuenta");
        assert_eq!(
            total, 50,
            "con escritor único no debería perderse ninguna escritura"
        );
    }
}

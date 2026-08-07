//! Configuración de cada conexión SQLite.
//!
//! Los PRAGMA no se heredan: hay que aplicarlos a **cada** conexión que se
//! abre. Concentrarlos aquí evita que una conexión nueva quede con los valores
//! por defecto y se comporte distinto que las demás, que es un fallo muy
//! difícil de ver.

use rusqlite::Connection;

use crate::error::DbResult;

/// Caché de páginas por conexión, en KiB (el signo negativo lo indica).
const CACHE_KIB: i32 = -16_000; // 16 MB

/// I/O mapeado en memoria. La base de datos completa de una biblioteca grande
/// ronda los 16 MB, así que con esto cabe entera y las lecturas evitan el
/// syscall.
const MMAP_BYTES: i64 = 256 * 1024 * 1024;

/// Espera antes de devolver `SQLITE_BUSY`. Con escritor único casi nunca debería
/// entrar en juego; es una red de seguridad frente a un checkpoint largo.
const BUSY_TIMEOUT_MS: u32 = 5_000;

/// Aplica la configuración común a cualquier conexión.
pub fn aplicar_comunes(conn: &Connection) -> DbResult<()> {
    // Integridad referencial real. SQLite la trae desactivada por compatibilidad
    // histórica, lo que convierte cada FOREIGN KEY declarada en decoración.
    conn.pragma_update(None, "foreign_keys", "ON")?;

    // WAL: N lectores concurrentes sin bloquear al escritor. Es la razón por la
    // que el pool puede tener varias conexiones de lectura.
    conn.pragma_update(None, "journal_mode", "WAL")?;

    // `FULL` costaría un fsync por transacción (~10 ms) y no aporta aquí: en
    // WAL, `NORMAL` solo arriesga las últimas transacciones ante un corte de
    // corriente, y lo que se perdería es una posición de reproducción. La
    // biblioteca vive en disco con sus etiquetas escritas.
    conn.pragma_update(None, "synchronous", "NORMAL")?;

    conn.busy_timeout(std::time::Duration::from_millis(u64::from(BUSY_TIMEOUT_MS)))?;
    conn.pragma_update(None, "temp_store", "MEMORY")?;
    conn.pragma_update(None, "cache_size", CACHE_KIB)?;
    conn.pragma_update(None, "mmap_size", MMAP_BYTES)?;

    Ok(())
}

/// Configuración adicional del escritor.
pub fn aplicar_escritor(conn: &Connection) -> DbResult<()> {
    aplicar_comunes(conn)?;

    // Recupera páginas libres sin necesidad de un VACUUM completo, que
    // bloquearía la base de datos entera.
    conn.pragma_update(None, "auto_vacuum", "INCREMENTAL")?;

    // Limita el crecimiento del WAL. Sin esto, una sesión larga con muchas
    // escrituras pequeñas (posición de reproducción cada 5 s) puede dejar un
    // WAL de cientos de MB.
    conn.pragma_update(None, "journal_size_limit", 64 * 1024 * 1024)?;

    Ok(())
}

/// Marca las conexiones de lectura como solo-lectura a nivel de consulta.
///
/// Es una barrera de diseño, no una optimización: si una consulta de lectura
/// intenta escribir, debe fallar en el acto y no colarse en una conexión que no
/// pasa por la cola del escritor.
pub fn aplicar_lector(conn: &Connection) -> DbResult<()> {
    aplicar_comunes(conn)?;
    conn.pragma_update(None, "query_only", "ON")?;
    Ok(())
}

/// Comprueba que los PRAGMA críticos quedaron aplicados.
///
/// `journal_mode` puede fallar en silencio si otra conexión tiene la base de
/// datos abierta en otro modo. Verificarlo evita descubrirlo mucho más tarde,
/// en forma de bloqueos inexplicables.
pub fn verificar(conn: &Connection) -> DbResult<()> {
    let modo: String = conn.query_row("PRAGMA journal_mode", [], |r| r.get(0))?;
    if !modo.eq_ignore_ascii_case("wal") {
        return Err(crate::error::DbError::Configuracion(format!(
            "journal_mode es '{modo}' en lugar de WAL"
        )));
    }

    let fk: i32 = conn.query_row("PRAGMA foreign_keys", [], |r| r.get(0))?;
    if fk != 1 {
        return Err(crate::error::DbError::Configuracion(
            "foreign_keys no está activo".into(),
        ));
    }

    Ok(())
}

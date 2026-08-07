//! Errores de la capa de persistencia.
//!
//! Se traducen a [`CoreError`] en la frontera de los repositorios. La capa de
//! negocio nunca ve un error de rusqlite: distinguir un `SQLITE_CONSTRAINT` de
//! un `SQLITE_BUSY` es asunto de aquí.

use localify_core::error::CoreError;

pub type DbResult<T> = Result<T, DbError>;

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("error de SQLite")]
    Sqlite(#[from] rusqlite::Error),

    #[error("fallo al aplicar migraciones")]
    Migracion(#[from] refinery::Error),

    #[error("configuración de la base de datos incorrecta: {0}")]
    Configuracion(String),

    #[error("no se pudo abrir la base de datos en '{ruta}': {causa}")]
    Apertura { ruta: String, causa: String },

    #[error("el hilo del pool no respondió")]
    PoolCaido,

    #[error("valor inesperado en la columna '{columna}': {detalle}")]
    Mapeo {
        columna: &'static str,
        detalle: String,
    },

    #[error("JSON inválido en la base de datos")]
    Json(#[from] serde_json::Error),
}

impl DbError {
    /// `true` si la operación chocó con una restricción de unicidad.
    ///
    /// Se distingue porque para el negocio no es un fallo de almacenamiento
    /// sino un conflicto: "esa playlist ya existe" es accionable, "error de
    /// almacenamiento" no.
    #[must_use]
    pub fn es_conflicto(&self) -> bool {
        matches!(
            self,
            Self::Sqlite(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error {
                    code: rusqlite::ErrorCode::ConstraintViolation,
                    ..
                },
                _
            ))
        )
    }

    #[must_use]
    pub fn error_de_mapeo(columna: &'static str, detalle: impl Into<String>) -> Self {
        Self::Mapeo {
            columna,
            detalle: detalle.into(),
        }
    }
}

impl From<DbError> for CoreError {
    fn from(e: DbError) -> Self {
        if e.es_conflicto() {
            return Self::conflict(e.to_string());
        }
        match e {
            // Un mapeo fallido significa que la base de datos contiene algo que
            // el dominio no admite: es un error de programación nuestro, no un
            // problema de disco. Merece variante propia para que destaque.
            DbError::Mapeo { .. } | DbError::Json(_) => Self::internal(Box::new(e)),
            otro => Self::storage(Box::new(otro)),
        }
    }
}

/// Azúcar para convertir en la frontera de repositorio.
pub trait ToCore<T> {
    /// # Errors
    /// Propaga el error convertido a [`CoreError`].
    fn to_core(self) -> Result<T, CoreError>;
}

impl<T> ToCore<T> for DbResult<T> {
    fn to_core(self) -> Result<T, CoreError> {
        self.map_err(CoreError::from)
    }
}

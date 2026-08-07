//! Paginación.
//!
//! Ninguna operación del proyecto devuelve una colección sin acotar. Con
//! bibliotecas de decenas de miles de pistas, un `SELECT` sin `LIMIT` es un
//! fallo de diseño, no una optimización pendiente.
//!
//! Se ofrecen dos modos:
//!
//! - **Offset**: para listas cortas y saltos arbitrarios (contenido de un
//!   álbum, resultados de búsqueda).
//! - **Cursor (keyset)**: para listas largas con scroll. `OFFSET 40000` obliga
//!   a SQLite a recorrer 40 000 filas antes de devolver nada; un cursor sobre
//!   `(clave_orden, id)` cuesta lo mismo en la fila 40 000 que en la 10.

use serde::{Deserialize, Serialize};

/// Tope duro de elementos por página. Protege el puente IPC de respuestas
/// gigantes aunque el cliente pida más.
pub const LIMITE_MAXIMO: u32 = 200;
const LIMITE_POR_DEFECTO: u32 = 50;

/// Cursor opaco para el cliente: una posición codificada en el orden actual.
/// Su formato es un detalle de la capa de persistencia y puede cambiar sin
/// romper la API.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Cursor(pub String);

impl Cursor {
    #[must_use]
    pub fn new(valor: impl Into<String>) -> Self {
        Self(valor.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PageRequest {
    #[serde(default)]
    pub offset: u32,
    #[serde(default)]
    pub limit: Option<u32>,
    /// Si viene, tiene prioridad sobre `offset`.
    #[serde(default)]
    pub cursor: Option<Cursor>,
}

impl PageRequest {
    #[must_use]
    pub fn new(offset: u32, limit: u32) -> Self {
        Self {
            offset,
            limit: Some(limit),
            cursor: None,
        }
    }

    #[must_use]
    pub fn from_cursor(cursor: Cursor, limit: u32) -> Self {
        Self {
            offset: 0,
            limit: Some(limit),
            cursor: Some(cursor),
        }
    }

    /// Límite efectivo, siempre dentro de `1..=LIMITE_MAXIMO`.
    ///
    /// Acotar aquí y no en cada repositorio garantiza que ninguna consulta
    /// pueda saltarse el tope por olvido.
    #[must_use]
    pub fn limit(&self) -> u32 {
        self.limit
            .unwrap_or(LIMITE_POR_DEFECTO)
            .clamp(1, LIMITE_MAXIMO)
    }

    #[must_use]
    pub fn offset(&self) -> u32 {
        if self.cursor.is_some() {
            0
        } else {
            self.offset
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Page<T> {
    pub items: Vec<T>,
    /// `None` cuando contar sería caro y no aporta (scroll infinito).
    pub total: Option<u64>,
    /// `None` cuando no hay más resultados.
    pub next_cursor: Option<Cursor>,
}

impl<T> Page<T> {
    #[must_use]
    pub fn new(items: Vec<T>, total: Option<u64>, next_cursor: Option<Cursor>) -> Self {
        Self {
            items,
            total,
            next_cursor,
        }
    }

    #[must_use]
    pub fn empty() -> Self {
        Self {
            items: Vec::new(),
            total: Some(0),
            next_cursor: None,
        }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Transforma los elementos conservando los metadatos de paginación.
    /// Es el puente entre entidades de dominio y DTOs de la API.
    #[must_use]
    pub fn map<U, F: FnMut(T) -> U>(self, f: F) -> Page<U> {
        Page {
            items: self.items.into_iter().map(f).collect(),
            total: self.total,
            next_cursor: self.next_cursor,
        }
    }
}

impl<T> Default for Page<T> {
    fn default() -> Self {
        Self::empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn el_limite_se_acota_al_maximo() {
        let req = PageRequest::new(0, 100_000);
        assert_eq!(req.limit(), LIMITE_MAXIMO);
    }

    #[test]
    fn el_limite_nunca_es_cero() {
        let req = PageRequest::new(0, 0);
        assert_eq!(req.limit(), 1);
    }

    #[test]
    fn el_cursor_anula_el_offset() {
        let req = PageRequest {
            offset: 500,
            limit: Some(50),
            cursor: Some(Cursor::new("abc")),
        };
        assert_eq!(
            req.offset(),
            0,
            "un cursor y un offset a la vez sería ambiguo"
        );
    }

    #[test]
    fn map_conserva_los_metadatos() {
        let page = Page::new(vec![1_u32, 2, 3], Some(42), Some(Cursor::new("x")));
        let mapped = page.map(|n| n.to_string());
        assert_eq!(mapped.items, vec!["1", "2", "3"]);
        assert_eq!(mapped.total, Some(42));
        assert_eq!(mapped.next_cursor, Some(Cursor::new("x")));
    }
}

//! Repositorio de pistas.

use async_trait::async_trait;
use localify_core::domain::ids::{AlbumId, ArtistId, TrackId};
use localify_core::domain::library::LibraryStats;
use localify_core::domain::track::{AlbumRef, ArtistRef, Track, TrackFilter, TrackRow, TrackSort};
use localify_core::error::CoreResult;
use localify_core::page::{Cursor, Page, PageRequest};
use localify_core::ports::database::TrackRepository;
use localify_core::text;
use rusqlite::{OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};

use crate::error::{DbResult, ToCore};
use crate::mappers::{
    COLUMNAS_TRACK_ROW, JOINS_TRACK_ROW, a_fecha, a_fecha_lanzamiento, a_track_row, de_fecha,
    fecha_track_row,
};
use crate::pool::Pool;
use crate::repositories::artists::asegurar_artista;

pub struct SqliteTrackRepository {
    pool: Pool,
}

impl std::fmt::Debug for SqliteTrackRepository {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteTrackRepository")
            .finish_non_exhaustive()
    }
}

impl SqliteTrackRepository {
    #[must_use]
    pub const fn new(pool: Pool) -> Self {
        Self { pool }
    }
}

/// Descripción de un criterio de ordenación, con todo lo que necesita la
/// paginación por cursor.
struct Orden {
    /// Expresión SQL de la clave, sin `NULL` posibles: una clave nula rompería
    /// la comparación de tuplas del cursor (`NULL < x` es `NULL`, no `true`) y
    /// haría desaparecer filas entre páginas.
    clave: &'static str,
    /// `true` si la clave ordena de mayor a menor.
    descendente: bool,
    /// `true` si la clave es texto. Determina cómo se enlaza el cursor: SQLite
    /// ordena los enteros antes que el texto, así que enlazar un número como
    /// cadena daría comparaciones silenciosamente incorrectas.
    texto: bool,
}

/// Todas las ordenaciones desempatan por `t.id`. Sin ese desempate, dos filas
/// con la misma clave podrían intercambiarse entre páginas y el usuario vería
/// una pista repetida y otra ausente al hacer scroll.
const fn orden_de(sort: TrackSort) -> Orden {
    match sort {
        TrackSort::AddedDesc => Orden {
            clave: "t.added_at",
            descendente: true,
            texto: false,
        },
        TrackSort::TitleAsc => Orden {
            clave: "t.title_norm",
            descendente: false,
            texto: true,
        },
        TrackSort::ArtistAsc => Orden {
            clave: "t.artist_norm",
            descendente: false,
            texto: true,
        },
        TrackSort::AlbumAsc => Orden {
            clave: "COALESCE(a.title_norm, '')",
            descendente: false,
            texto: true,
        },
        TrackSort::DurationAsc => Orden {
            clave: "t.duration_ms",
            descendente: false,
            texto: false,
        },
        TrackSort::PlayCountDesc => Orden {
            clave: "(SELECT COUNT(*) FROM play_history h WHERE h.track_id = t.id)",
            descendente: true,
            texto: false,
        },
        TrackSort::LastPlayedDesc => Orden {
            clave: "COALESCE((SELECT MAX(played_at) FROM play_history h WHERE h.track_id = t.id), 0)",
            descendente: true,
            texto: false,
        },
    }
}

/// Contenido de un cursor de keyset.
///
/// Es opaco para el cliente: su formato es un detalle de esta capa y puede
/// cambiar sin romper la API.
#[derive(Serialize, Deserialize)]
struct ClaveCursor {
    /// Clave de orden como texto, o `None` si es numérica.
    #[serde(skip_serializing_if = "Option::is_none")]
    s: Option<String>,
    /// Clave de orden numérica.
    #[serde(skip_serializing_if = "Option::is_none")]
    n: Option<i64>,
    /// Desempate.
    id: String,
}

impl ClaveCursor {
    fn codificar(&self) -> Cursor {
        Cursor::new(serde_json::to_string(self).unwrap_or_else(|_| "{}".to_owned()))
    }

    fn decodificar(cursor: &Cursor) -> Option<Self> {
        serde_json::from_str(cursor.as_str()).ok()
    }
}

/// Valor de la clave de orden de una fila, tal como se lee de SQLite.
enum ValorClave {
    Texto(Option<String>),
    Numero(Option<i64>),
}

impl ValorClave {
    fn a_cursor(&self, id: &TrackId) -> Cursor {
        let (s, n) = match self {
            Self::Texto(v) => (v.clone(), None),
            Self::Numero(v) => (None, *v),
        };
        ClaveCursor {
            s,
            n,
            id: id.as_str().to_owned(),
        }
        .codificar()
    }
}

/// Consulta de listado ya resuelta: SQL, parámetros y decisiones de paginación.
///
/// Construirla es una responsabilidad distinta de ejecutarla, y separarlas deja
/// ambas partes legibles: aquí está toda la lógica de cursores y filtros, y en
/// `list_rows` solo queda el mapeo del resultado.
struct ConsultaLista {
    sql: String,
    sql_total: String,
    params: Vec<Box<dyn rusqlite::ToSql + Send>>,
    params_total: Vec<Box<dyn rusqlite::ToSql + Send>>,
    /// `true` si hay que contar el conjunto entero.
    contar: bool,
    /// `true` si la consulta lleva `OFFSET` (salto arbitrario, no scroll).
    usa_offset: bool,
    clave_es_texto: bool,
}

impl ConsultaLista {
    fn construir(filter: &TrackFilter, sort: TrackSort, page: &PageRequest) -> Self {
        let (mut where_sql, mut params) = construir_filtro(filter);
        let orden = orden_de(sort);
        let cursor = page.cursor.as_ref().and_then(ClaveCursor::decodificar);
        let offset = page.offset();

        // ── Paginación por cursor (keyset) ───────────────────────────────────
        //
        // `OFFSET 40000` obliga a SQLite a recorrer y descartar 40 000 filas
        // antes de devolver nada, y el coste crece con la profundidad. Una
        // comparación de tuplas sobre `(clave, id)` arranca directamente en el
        // punto correcto usando el índice, y cuesta lo mismo en la fila 40 000
        // que en la 10. Es lo que hace viable el scroll en bibliotecas grandes.
        if let Some(c) = &cursor {
            let comparador = if orden.descendente { "<" } else { ">" };
            let clave = orden.clave;
            let condicion = format!("({clave}, t.id) {comparador} (?, ?)");

            where_sql = if where_sql.is_empty() {
                format!("WHERE {condicion}")
            } else {
                format!("{where_sql} AND {condicion}")
            };

            if let Some(texto) = &c.s {
                params.push(Box::new(texto.clone()));
            } else {
                params.push(Box::new(c.n.unwrap_or(0)));
            }
            params.push(Box::new(c.id.clone()));
        }

        let direccion = if orden.descendente { "DESC" } else { "ASC" };
        let clave = orden.clave;
        let columnas = COLUMNAS_TRACK_ROW;
        let joins = JOINS_TRACK_ROW;

        // La clave se selecciona siempre: es lo que se necesita para construir
        // el cursor de la página siguiente.
        let fecha = fecha_track_row("t.added_at");
        let mut sql = format!(
            "SELECT {columnas}{fecha}, {clave} AS clave_orden
             FROM tracks t
             {joins}
             {where_sql}
             ORDER BY clave_orden {direccion}, t.id {direccion}
             LIMIT ?"
        );

        // El desplazamiento sigue disponible para saltos arbitrarios (por
        // ejemplo, "ir a la letra M"), pero no es el camino del scroll.
        let usa_offset = cursor.is_none() && offset > 0;
        if usa_offset {
            sql.push_str(" OFFSET ?");
        }

        // Contar exige recorrer el conjunto entero. Solo se hace en la primera
        // página, que es donde la interfaz muestra "N canciones"; al seguir
        // desplazándose, el total ya lo conoce el cliente.
        let (where_total, params_total) = construir_filtro(filter);
        let sql_total = format!("SELECT COUNT(*) FROM tracks t {joins} {where_total}");

        Self {
            sql,
            sql_total,
            params,
            params_total,
            contar: cursor.is_none() && offset == 0,
            usa_offset,
            clave_es_texto: orden.texto,
        }
    }

    fn contar_total(&self, conn: &rusqlite::Connection) -> DbResult<Option<u64>> {
        if !self.contar {
            return Ok(None);
        }
        let refs: Vec<&dyn rusqlite::ToSql> = self
            .params_total
            .iter()
            .map(|p| p.as_ref() as &dyn rusqlite::ToSql)
            .collect();
        let n: i64 = conn.query_row(&self.sql_total, refs.as_slice(), |r| r.get(0))?;
        Ok(Some(n.max(0).unsigned_abs()))
    }

    fn ejecutar(
        &self,
        conn: &rusqlite::Connection,
        limite: u32,
        offset: u32,
    ) -> DbResult<(Vec<(TrackRow, ValorClave)>, bool)> {
        let mut refs: Vec<&dyn rusqlite::ToSql> = self
            .params
            .iter()
            .map(|p| p.as_ref() as &dyn rusqlite::ToSql)
            .collect();

        let limite_i = i64::from(limite);
        let offset_i = i64::from(offset);
        refs.push(&limite_i);
        if self.usa_offset {
            refs.push(&offset_i);
        }

        let es_texto = self.clave_es_texto;
        let mut stmt = conn.prepare_cached(&self.sql)?;
        let filas = stmt
            .query_map(refs.as_slice(), |row| {
                let clave = if es_texto {
                    ValorClave::Texto(row.get("clave_orden")?)
                } else {
                    ValorClave::Numero(row.get("clave_orden")?)
                };
                Ok(a_track_row(row).map(|fila| (fila, clave)))
            })?
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .collect::<DbResult<Vec<_>>>()?;

        Ok((filas, es_texto))
    }
}

/// Construye el `WHERE` a partir del filtro.
///
/// Devuelve la cláusula y los parámetros por separado. Nunca se interpola valor
/// del usuario en el SQL: todo va por `?`.
fn construir_filtro(filter: &TrackFilter) -> (String, Vec<Box<dyn rusqlite::ToSql + Send>>) {
    let mut condiciones: Vec<String> = Vec::new();
    let mut params: Vec<Box<dyn rusqlite::ToSql + Send>> = Vec::new();

    if filter.local_only {
        condiciones.push("af.track_id IS NOT NULL".into());
    }
    if filter.favorites_only {
        condiciones.push("f.track_id IS NOT NULL".into());
    }
    if let Some(album) = &filter.album_id {
        condiciones.push("t.album_id = ?".into());
        params.push(Box::new(album.as_str().to_owned()));
    }
    if let Some(artist) = &filter.artist_id {
        condiciones.push(
            "EXISTS (SELECT 1 FROM track_artists ta WHERE ta.track_id = t.id AND ta.artist_id = ?)"
                .into(),
        );
        params.push(Box::new(artist.as_str().to_owned()));
    }
    if let Some(genero) = filter.genre_id {
        condiciones.push(
            "EXISTS (SELECT 1 FROM track_artists ta
                     JOIN artist_genres ag ON ag.artist_id = ta.artist_id
                     WHERE ta.track_id = t.id AND ag.genre_id = ?)"
                .into(),
        );
        params.push(Box::new(genero));
    }
    if let Some(texto) = &filter.text {
        let normalizado = text::normalize(texto);
        if !normalizado.is_empty() {
            // Filtro sobre la biblioteca ya cargada, distinto de la búsqueda
            // global: aquí no hay FTS5 porque el conjunto ya viene acotado por
            // el resto del filtro y un LIKE sobre las columnas normalizadas es
            // suficiente y más simple.
            condiciones.push("(t.title_norm LIKE ? OR t.artist_norm LIKE ?)".into());
            let patron = format!("%{normalizado}%");
            params.push(Box::new(patron.clone()));
            params.push(Box::new(patron));
        }
    }

    let clausula = if condiciones.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", condiciones.join(" AND "))
    };
    (clausula, params)
}

/// Recalcula `artist_display` y `artist_norm` desde `track_artists`.
///
/// Es la operación que mantiene coherente la denormalización de ADR-011. Se
/// ejecuta en la misma transacción que escribe las relaciones, así que no puede
/// quedar desfasada.
pub(crate) fn refrescar_artist_display(tx: &Transaction<'_>, track_id: &str) -> DbResult<()> {
    let display: String = tx.query_row(
        "SELECT COALESCE(GROUP_CONCAT(nombre, ', '), '')
         FROM (SELECT ar.name AS nombre
               FROM track_artists ta
               JOIN artists ar ON ar.id = ta.artist_id
               WHERE ta.track_id = ?1
               ORDER BY ta.position)",
        [track_id],
        |r| r.get(0),
    )?;

    tx.execute(
        "UPDATE tracks SET artist_display = ?2, artist_norm = ?3 WHERE id = ?1",
        params![track_id, display, text::normalize(&display)],
    )?;
    Ok(())
}

/// Inserta una pista y sus relaciones dentro de una transacción ya abierta.
fn upsert_track(tx: &Transaction<'_>, track: &Track) -> DbResult<()> {
    // Los álbumes y artistas referenciados deben existir antes que la pista:
    // `foreign_keys` está activo y rechazaría la inserción. Se crean como
    // referencias mínimas (`metadata_at` a NULL) y el MetadataService los
    // completará cuando haga falta.
    if let Some(album) = &track.album {
        tx.execute(
            "INSERT INTO albums (id, title, title_norm) VALUES (?1, ?2, ?3)
             ON CONFLICT (id) DO UPDATE SET title = ?2, title_norm = ?3",
            params![
                album.id.as_str(),
                album.title,
                text::normalize(&album.title)
            ],
        )?;
    }

    // Los artistas se escriben **antes** que la pista, y el identificador con el
    // que quedan no tiene por qué ser el que traían: uno local puede resolverse
    // a un artista que ya existe. Ver `asegurar_artista`.
    let artistas: Vec<String> = track
        .artists
        .iter()
        .map(|a| asegurar_artista(tx, a))
        .collect::<DbResult<_>>()?;

    tx.execute(
        "INSERT INTO tracks (
             id, title, title_norm, album_id, duration_ms, track_number, disc_number,
             explicit, isrc, popularity, metadata_at, added_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
         ON CONFLICT (id) DO UPDATE SET
             title        = ?2,
             title_norm   = ?3,
             album_id     = ?4,
             duration_ms  = ?5,
             track_number = ?6,
             disc_number  = ?7,
             explicit     = ?8,
             isrc         = ?9,
             popularity   = ?10,
             metadata_at  = ?11",
        params![
            track.id.as_str(),
            track.title,
            text::normalize(&track.title),
            track.album.as_ref().map(|a| a.id.as_str()),
            i64::from(track.duration.as_ms()),
            track.track_number,
            track.disc_number,
            i64::from(track.explicit),
            track.isrc,
            track.popularity,
            de_fecha(chrono::Utc::now()),
            de_fecha(track.added_at),
        ],
    )?;

    // Reemplazar las relaciones en bloque: si un artista deja de figurar en la
    // pista, quedarse con la fila antigua produciría un `artist_display`
    // incorrecto de forma permanente.
    tx.execute(
        "DELETE FROM track_artists WHERE track_id = ?1",
        [track.id.as_str()],
    )?;
    // Un artista repetido en la misma pista se ignora en vez de reventar.
    //
    // `PRIMARY KEY (track_id, artist_id)` dice que la pareja es única, y un
    // proveedor puede mandarla dos veces sin que eso sea un error de nadie:
    // MusicBrainz acredita al mismo artista con su alias y con su nombre real.
    // Dejar que el `INSERT` fallara abortaba la transacción **entera**, así que
    // una grabación con el crédito repetido se llevaba por delante las otras
    // treinta y nueve de la misma búsqueda.
    //
    // Se conserva la primera aparición, que es la que trae la posición buena: la
    // posición 0 es el artista principal y de ella depende `artist_display`.
    // Se comparan los identificadores **ya canonizados**: dos locales distintos
    // pueden resolverse al mismo artista, y comparando los de origen la pareja
    // repetida se colaría igual.
    let mut vistos = std::collections::HashSet::new();
    let mut posicion = 0_i64;
    for id in &artistas {
        if !vistos.insert(id.clone()) {
            continue;
        }
        tx.execute(
            "INSERT INTO track_artists (track_id, artist_id, position) VALUES (?1, ?2, ?3)",
            params![track.id.as_str(), id, posicion],
        )?;
        posicion += 1;
    }

    refrescar_artist_display(tx, track.id.as_str())?;
    Ok(())
}

#[async_trait]
impl TrackRepository for SqliteTrackRepository {
    async fn get(&self, id: &TrackId) -> CoreResult<Option<Track>> {
        let id = id.clone();
        self.pool
            .leer(move |conn| {
                let mut stmt = conn.prepare_cached(
                    "SELECT t.id, t.title, t.album_id, a.title AS album_title, t.duration_ms,
                            t.track_number, t.disc_number, t.explicit, t.isrc,
                            t.popularity, t.added_at, a.release_date
                     FROM tracks t
                     LEFT JOIN albums a ON a.id = t.album_id
                     WHERE t.id = ?1",
                )?;

                let base = stmt
                    .query_row([id.as_str()], |row| {
                        Ok((
                            row.get::<_, String>("title")?,
                            row.get::<_, Option<String>>("album_id")?,
                            row.get::<_, Option<String>>("album_title")?,
                            row.get::<_, i64>("duration_ms")?,
                            row.get::<_, Option<u16>>("track_number")?,
                            row.get::<_, Option<u16>>("disc_number")?,
                            row.get::<_, i64>("explicit")?,
                            row.get::<_, Option<String>>("isrc")?,
                            row.get::<_, Option<u8>>("popularity")?,
                            row.get::<_, i64>("added_at")?,
                            row.get::<_, Option<String>>("release_date")?,
                        ))
                    })
                    .optional()?;

                let Some(b) = base else { return Ok(None) };

                let mut stmt = conn.prepare_cached(
                    "SELECT ar.id, ar.name
                     FROM track_artists ta
                     JOIN artists ar ON ar.id = ta.artist_id
                     WHERE ta.track_id = ?1
                     ORDER BY ta.position",
                )?;
                let artists = stmt
                    .query_map([id.as_str()], |r| {
                        Ok(ArtistRef {
                            id: ArtistId::from_trusted(r.get::<_, String>(0)?),
                            name: r.get(1)?,
                        })
                    })?
                    .collect::<Result<Vec<_>, _>>()?;

                Ok(Some(Track {
                    id: id.clone(),
                    title: b.0,
                    album: b.1.map(|aid| AlbumRef {
                        id: AlbumId::from_trusted(aid),
                        title: b.2.unwrap_or_default(),
                    }),
                    artists,
                    duration: localify_core::domain::audio::DurationMs::new(
                        u32::try_from(b.3).unwrap_or(0),
                    ),
                    track_number: b.4,
                    disc_number: b.5,
                    explicit: b.6 != 0,
                    isrc: b.7,
                    popularity: b.8,
                    release_date: a_fecha_lanzamiento(b.10),
                    added_at: a_fecha(b.9),
                }))
            })
            .await
            .to_core()
    }

    async fn get_many(&self, ids: &[TrackId]) -> CoreResult<Vec<Track>> {
        // Sin batch: `get` está cacheado por `prepare_cached` y esta ruta solo
        // se usa con puñados de IDs. Optimizarla antes de tener una medida sería
        // adivinar.
        let mut resultado = Vec::with_capacity(ids.len());
        for id in ids {
            if let Some(t) = self.get(id).await? {
                resultado.push(t);
            }
        }
        Ok(resultado)
    }

    async fn upsert(&self, tracks: &[Track]) -> CoreResult<()> {
        if tracks.is_empty() {
            return Ok(());
        }
        let tracks = tracks.to_vec();
        self.pool
            .escribir(move |tx| {
                for track in &tracks {
                    upsert_track(tx, track)?;
                }
                Ok(())
            })
            .await
            .to_core()
    }

    async fn list_rows(
        &self,
        filter: &TrackFilter,
        sort: TrackSort,
        page: &PageRequest,
    ) -> CoreResult<Page<TrackRow>> {
        let plan = ConsultaLista::construir(filter, sort, page);
        let limite = page.limit();
        let offset = page.offset();

        self.pool
            .leer(move |conn| {
                let total = plan.contar_total(conn)?;
                let (filas, _) = plan.ejecutar(conn, limite, offset)?;

                // Sin página completa no hay más resultados: emitir un cursor
                // provocaría una petición extra que devolvería vacío.
                let hay_mas = u32::try_from(filas.len()).unwrap_or(u32::MAX) == limite;
                let next = hay_mas
                    .then(|| filas.last().map(|(fila, clave)| clave.a_cursor(&fila.id)))
                    .flatten();

                let items = filas.into_iter().map(|(fila, _)| fila).collect();
                Ok(Page::new(items, total, next))
            })
            .await
            .to_core()
    }

    async fn rows_by_ids(&self, ids: &[TrackId]) -> CoreResult<Vec<TrackRow>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let ids: Vec<String> = ids.iter().map(|i| i.as_str().to_owned()).collect();

        self.pool
            .leer(move |conn| {
                // `rarray` requiere la feature `array` de rusqlite. Con un
                // número acotado de IDs (la ventana visible de una lista), una
                // lista de marcadores generada es más simple y no añade una
                // extensión al build.
                let marcadores = vec!["?"; ids.len()].join(",");
                let columnas = COLUMNAS_TRACK_ROW;
                let joins = JOINS_TRACK_ROW;
                let sql =
                    format!("SELECT {columnas} FROM tracks t {joins} WHERE t.id IN ({marcadores})");

                let refs: Vec<&dyn rusqlite::ToSql> =
                    ids.iter().map(|s| s as &dyn rusqlite::ToSql).collect();

                let mut stmt = conn.prepare(&sql)?;
                let filas = stmt
                    .query_map(refs.as_slice(), |row| Ok(a_track_row(row)))?
                    .collect::<Result<Vec<_>, _>>()?
                    .into_iter()
                    .collect::<DbResult<Vec<_>>>()?;

                // Se devuelve en el orden pedido, no en el que salga de SQLite:
                // quien llama suele estar pintando una lista ya ordenada.
                let mut por_id: std::collections::HashMap<String, TrackRow> = filas
                    .into_iter()
                    .map(|f| (f.id.as_str().to_owned(), f))
                    .collect();
                Ok(ids.iter().filter_map(|id| por_id.remove(id)).collect())
            })
            .await
            .to_core()
    }

    async fn stale(&self, older_than_secs: u64, limit: u32) -> CoreResult<Vec<TrackId>> {
        let corte = de_fecha(chrono::Utc::now()) - i64::try_from(older_than_secs).unwrap_or(0);
        self.pool
            .leer(move |conn| {
                let mut stmt = conn.prepare_cached(
                    "SELECT id FROM tracks
                     WHERE metadata_at IS NULL OR metadata_at < ?1
                     ORDER BY metadata_at ASC NULLS FIRST
                     LIMIT ?2",
                )?;
                let ids = stmt
                    .query_map(params![corte, limit], |r| r.get::<_, String>(0))?
                    .collect::<Result<Vec<_>, _>>()?
                    .into_iter()
                    .map(TrackId::from_trusted)
                    .collect();
                Ok(ids)
            })
            .await
            .to_core()
    }

    async fn stats(&self) -> CoreResult<LibraryStats> {
        self.pool
            .leer(|conn| {
                let (track_count, total_duration_ms): (i64, i64) = conn.query_row(
                    "SELECT COUNT(*), COALESCE(SUM(duration_ms), 0) FROM tracks",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )?;
                let (local_count, total_bytes): (i64, i64) = conn.query_row(
                    "SELECT COUNT(*), COALESCE(SUM(size_bytes), 0) FROM audio_files",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )?;
                let album_count: i64 =
                    conn.query_row("SELECT COUNT(*) FROM albums", [], |r| r.get(0))?;
                let artist_count: i64 =
                    conn.query_row("SELECT COUNT(*) FROM artists", [], |r| r.get(0))?;
                let failed_count: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM download_jobs WHERE state = 'failed'",
                    [],
                    |r| r.get(0),
                )?;

                Ok(LibraryStats {
                    track_count: track_count.max(0).unsigned_abs(),
                    local_count: local_count.max(0).unsigned_abs(),
                    album_count: album_count.max(0).unsigned_abs(),
                    artist_count: artist_count.max(0).unsigned_abs(),
                    total_duration_ms: total_duration_ms.max(0).unsigned_abs(),
                    total_bytes: total_bytes.max(0).unsigned_abs(),
                    failed_count: failed_count.max(0).unsigned_abs(),
                })
            })
            .await
            .to_core()
    }
}

/// Comprobación de coherencia: ninguna pista debe tener un `artist_display`
/// distinto del que produce `track_artists`.
///
/// Es el invariante que sostiene la denormalización de ADR-011. Se verifica en
/// los tests tras cada camino de escritura.
#[cfg(test)]
async fn artist_display_coherente(pool: &Pool) -> DbResult<bool> {
    pool.leer(|conn| {
        let mut stmt = conn.prepare(
            "SELECT t.id, t.artist_display,
                    COALESCE((SELECT GROUP_CONCAT(nombre, ', ')
                              FROM (SELECT ar.name AS nombre
                                    FROM track_artists ta
                                    JOIN artists ar ON ar.id = ta.artist_id
                                    WHERE ta.track_id = t.id
                                    ORDER BY ta.position)), '') AS esperado
             FROM tracks t",
        )?;
        let discrepancias: Vec<(String, String, String)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .filter(|(_, actual, esperado)| actual != esperado)
            .collect();
        Ok(discrepancias.is_empty())
    })
    .await
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use localify_core::domain::audio::DurationMs;

    use super::*;
    use crate::pool::TempDbGuard;

    async fn repo() -> (SqliteTrackRepository, Pool, TempDbGuard) {
        let (pool, guard) = Pool::temporal().expect("abre");
        crate::migrations::ejecutar(&pool).await.expect("migra");
        (SqliteTrackRepository::new(pool.clone()), pool, guard)
    }

    fn artista(nombre: &str) -> ArtistRef {
        ArtistRef {
            id: ArtistId::nuevo_local(),
            name: nombre.into(),
        }
    }

    /// Cuántos artistas hay guardados con ese nombre normalizado.
    async fn cuantos_llamados(pool: &Pool, nombre: &str) -> i64 {
        let n = text::normalize(nombre);
        pool.leer(move |conn| {
            Ok(conn.query_row(
                "SELECT COUNT(*) FROM artists WHERE name_norm = ?1",
                [&n],
                |r| r.get::<_, i64>(0),
            )?)
        })
        .await
        .expect("cuenta")
    }

    #[tokio::test]
    async fn dos_pistas_del_mismo_artista_sin_identidad_no_lo_duplican() {
        // La página de incrustación de Spotify solo da nombres, así que cada
        // canción llegaba con un artista local recién inventado. Importar treinta
        // temas de un grupo creaba treinta «coldrain», todos con una canción y
        // sin foto.
        let (repo, pool, _g) = repo().await;
        repo.upsert(&[
            pista("Una", vec![artista("coldrain")]),
            pista("Otra", vec![artista("coldrain")]),
        ])
        .await
        .expect("guarda");

        assert_eq!(cuantos_llamados(&pool, "coldrain").await, 1);
    }

    #[tokio::test]
    async fn un_artista_del_catalogo_absorbe_al_local_del_mismo_nombre() {
        // Es lo que hace que las canciones importadas salgan bajo el artista
        // bueno, con su foto y sus géneros, en vez de bajo un duplicado mudo.
        let (repo, pool, _g) = repo().await;
        let del_catalogo = ArtistRef {
            id: ArtistId::from_trusted("UC1WV_zQ6vd52xitFX2cANYg"),
            name: "coldrain".into(),
        };
        repo.upsert(&[pista("Del catálogo", vec![del_catalogo.clone()])])
            .await
            .expect("guarda");
        repo.upsert(&[pista("Importada", vec![artista("coldrain")])])
            .await
            .expect("guarda");

        assert_eq!(cuantos_llamados(&pool, "coldrain").await, 1);
        let leida = repo
            .get(
                &repo
                    .list_rows(
                        &TrackFilter::default(),
                        TrackSort::TitleAsc,
                        &PageRequest::new(0, 10),
                    )
                    .await
                    .expect("lista")
                    .items[1]
                    .id,
            )
            .await
            .expect("lee")
            .expect("existe");
        assert_eq!(
            leida.artists[0].id.as_str(),
            "UC1WV_zQ6vd52xitFX2cANYg",
            "la importada tiene que colgar del artista con identidad"
        );
    }

    #[tokio::test]
    async fn dos_artistas_del_mismo_catalogo_y_el_mismo_nombre_no_se_funden() {
        // Pueden ser dos personas distintas: unirlos sería inventarse un dato.
        // Hay más de un Nirvana en MusicBrainz.
        let (repo, pool, _g) = repo().await;
        let uno = ArtistRef {
            id: ArtistId::from_trusted("UCaaaaaaaaaaaaaaaaaaaaaa"),
            name: "Nirvana".into(),
        };
        let otro = ArtistRef {
            id: ArtistId::from_trusted("UCbbbbbbbbbbbbbbbbbbbbbb"),
            name: "Nirvana".into(),
        };
        repo.upsert(&[pista("A", vec![uno]), pista("B", vec![otro])])
            .await
            .expect("guarda");

        assert_eq!(cuantos_llamados(&pool, "Nirvana").await, 2);
    }

    #[tokio::test]
    async fn el_mismo_artista_visto_por_dos_catalogos_se_funde_en_el_canal() {
        // Un canal de YouTube y un UUID de MusicBrainz no son dos personas: son
        // dos formas de nombrar a la misma, y tener las dos es el residuo de
        // haber cambiado de proveedor. Gana el canal porque es el que trae foto
        // y el único al que el InnerTube responde: a un UUID contesta 400.
        let (repo, pool, _g) = repo().await;
        let de_musicbrainz = ArtistRef {
            id: ArtistId::from_trusted("dd6aeb09-60b7-400d-b9e7-b1e5800bb84a"),
            name: "Casey Edwards".into(),
        };
        let de_youtube = ArtistRef {
            id: ArtistId::from_trusted("UCLlchLQvkIB_QWxH6J2tLIA"),
            name: "Casey Edwards".into(),
        };

        let antigua = pista("Bury the Light", vec![de_musicbrainz]);
        repo.upsert(std::slice::from_ref(&antigua))
            .await
            .expect("guarda");
        repo.upsert(&[pista("Devil Trigger", vec![de_youtube])])
            .await
            .expect("guarda");

        assert_eq!(cuantos_llamados(&pool, "Casey Edwards").await, 1);

        // Y la canción vieja se queda colgando del superviviente, no huérfana.
        let leida = repo.get(&antigua.id).await.expect("lee").expect("existe");
        assert_eq!(leida.artists[0].id.as_str(), "UCLlchLQvkIB_QWxH6J2tLIA");
    }

    #[tokio::test]
    async fn fundir_entre_catalogos_recompone_el_nombre_visible() {
        // «kittydog» y «Kittydog» comparten `name_norm` y son dos filas: al
        // fundirlas cambia la grafía que se lee en la lista, y `artist_display`
        // está denormalizado (ADR-011).
        let (repo, pool, _g) = repo().await;
        let minusculas = ArtistRef {
            id: ArtistId::from_trusted("df256f61-301f-4bc8-8396-8bd931a0739d"),
            name: "kittydog".into(),
        };
        let canal = ArtistRef {
            id: ArtistId::from_trusted("UCLPvTw05UjNlxdeaXMbY2xw"),
            name: "Kittydog".into(),
        };

        let vieja = pista("Una", vec![minusculas]);
        repo.upsert(std::slice::from_ref(&vieja))
            .await
            .expect("guarda");
        repo.upsert(&[pista("Otra", vec![canal])])
            .await
            .expect("guarda");

        let filas = repo.rows_by_ids(&[vieja.id]).await.expect("lee");
        assert_eq!(filas[0].artist_display, "Kittydog");
        assert!(artist_display_coherente(&pool).await.expect("comprueba"));
    }

    #[tokio::test]
    async fn con_dos_candidatos_del_otro_catalogo_no_se_funde_nada() {
        // Dos UUIDs con el mismo nombre son la señal de que sí son dos personas
        // distintas. Ante la duda, un duplicado es mejor que fusionar a quien no
        // toca.
        let (repo, pool, _g) = repo().await;
        let uno = ArtistRef {
            id: ArtistId::from_trusted("aaaaaaaa-0000-0000-0000-000000000001"),
            name: "Nirvana".into(),
        };
        let otro = ArtistRef {
            id: ArtistId::from_trusted("aaaaaaaa-0000-0000-0000-000000000002"),
            name: "Nirvana".into(),
        };
        let canal = ArtistRef {
            id: ArtistId::from_trusted("UCcccccccccccccccccccccc"),
            name: "Nirvana".into(),
        };
        repo.upsert(&[pista("A", vec![uno]), pista("B", vec![otro])])
            .await
            .expect("guarda");
        repo.upsert(&[pista("C", vec![canal])])
            .await
            .expect("guarda");

        assert_eq!(cuantos_llamados(&pool, "Nirvana").await, 3);
    }

    fn pista(titulo: &str, artistas: Vec<ArtistRef>) -> Track {
        Track {
            id: TrackId::nuevo_local(),
            title: titulo.into(),
            album: Some(AlbumRef {
                id: AlbumId::nuevo_local(),
                title: "Hot Space".into(),
            }),
            artists: artistas,
            duration: DurationMs::new(248_000),
            track_number: Some(1),
            disc_number: Some(1),
            explicit: false,
            isrc: Some("GBUM71029604".into()),
            release_date: None,
            popularity: Some(80),
            added_at: chrono::Utc::now(),
        }
    }

    #[tokio::test]
    async fn un_artista_repetido_en_la_misma_pista_no_tumba_el_lote() {
        // MusicBrainz acredita al mismo artista dos veces cuando aparece con su
        // alias y con su nombre real. Como `track_artists` tiene la pareja como
        // clave primaria, el segundo INSERT abortaba la transacción **entera**:
        // una grabación así se llevaba por delante las otras treinta y nueve de
        // la misma búsqueda, y el catálogo parecía no responder.
        let (repo, _pool, _g) = repo().await;

        let doble = artista("Casey Edwards");
        let repetida = pista("Bury the Light", vec![doble.clone(), doble.clone()]);
        let acompanante = pista("Fire Inside", vec![artista("Victor Borba")]);

        repo.upsert(&[repetida.clone(), acompanante.clone()])
            .await
            .expect("el lote entero tiene que guardarse");

        let guardada = repo
            .get(&repetida.id)
            .await
            .expect("consulta")
            .expect("la pista con el crédito repetido existe");
        assert_eq!(
            guardada.artists.len(),
            1,
            "el artista repetido cuenta una vez"
        );

        assert!(
            repo.get(&acompanante.id).await.expect("consulta").is_some(),
            "la otra pista del lote no puede perderse por culpa de la primera"
        );
    }

    #[tokio::test]
    async fn una_pista_guardada_se_recupera_igual() {
        let (repo, _pool, _g) = repo().await;
        let original = pista(
            "Under Pressure",
            vec![artista("Queen"), artista("David Bowie")],
        );

        repo.upsert(std::slice::from_ref(&original))
            .await
            .expect("guarda");
        let leida = repo.get(&original.id).await.expect("lee").expect("existe");

        assert_eq!(leida.title, original.title);
        assert_eq!(leida.duration, original.duration);
        assert_eq!(leida.isrc, original.isrc);
        assert_eq!(
            leida
                .artists
                .iter()
                .map(|a| a.name.clone())
                .collect::<Vec<_>>(),
            vec!["Queen", "David Bowie"],
            "el orden de artistas define quién es el principal"
        );
        assert_eq!(leida.album.map(|a| a.title), Some("Hot Space".into()));
    }

    #[tokio::test]
    async fn el_upsert_mantiene_coherente_artist_display() {
        let (repo, pool, _g) = repo().await;
        let mut t = pista(
            "Under Pressure",
            vec![artista("Queen"), artista("David Bowie")],
        );
        repo.upsert(std::slice::from_ref(&t)).await.expect("guarda");

        let filas = repo.rows_by_ids(&[t.id.clone()]).await.expect("lee");
        assert_eq!(filas[0].artist_display, "Queen, David Bowie");
        assert!(artist_display_coherente(&pool).await.expect("comprueba"));

        // Reescribir con menos artistas debe reflejarse: si quedara la fila
        // antigua en track_artists, el display sería incorrecto para siempre.
        t.artists = vec![artista("Queen")];
        repo.upsert(std::slice::from_ref(&t))
            .await
            .expect("reescribe");

        let filas = repo.rows_by_ids(&[t.id.clone()]).await.expect("lee");
        assert_eq!(filas[0].artist_display, "Queen");
        assert!(artist_display_coherente(&pool).await.expect("comprueba"));
    }

    #[tokio::test]
    async fn una_pista_inexistente_devuelve_none_y_no_error() {
        let (repo, _pool, _g) = repo().await;
        let ausente = repo.get(&TrackId::nuevo_local()).await.expect("consulta");
        assert!(ausente.is_none());
    }

    #[tokio::test]
    async fn rows_by_ids_respeta_el_orden_pedido() {
        let (repo, _pool, _g) = repo().await;
        let a = pista("A", vec![artista("X")]);
        let b = pista("B", vec![artista("Y")]);
        let c = pista("C", vec![artista("Z")]);
        repo.upsert(&[a.clone(), b.clone(), c.clone()])
            .await
            .expect("guarda");

        let pedido = vec![c.id.clone(), a.id.clone(), b.id.clone()];
        let filas = repo.rows_by_ids(&pedido).await.expect("lee");

        assert_eq!(
            filas.iter().map(|f| f.title.clone()).collect::<Vec<_>>(),
            vec!["C", "A", "B"],
            "quien llama suele estar pintando una lista ya ordenada"
        );
    }

    #[tokio::test]
    async fn rows_by_ids_ignora_los_ids_que_no_existen() {
        let (repo, _pool, _g) = repo().await;
        let a = pista("A", vec![artista("X")]);
        repo.upsert(std::slice::from_ref(&a)).await.expect("guarda");

        let filas = repo
            .rows_by_ids(&[TrackId::nuevo_local(), a.id.clone()])
            .await
            .expect("lee");
        assert_eq!(filas.len(), 1);
        assert_eq!(filas[0].id, a.id);
    }

    #[tokio::test]
    async fn una_pista_sin_fichero_aparece_como_ausente() {
        let (repo, _pool, _g) = repo().await;
        let t = pista("Sin descargar", vec![artista("X")]);
        repo.upsert(std::slice::from_ref(&t)).await.expect("guarda");

        let filas = repo.rows_by_ids(&[t.id]).await.expect("lee");
        assert_eq!(
            filas[0].availability,
            localify_core::domain::Availability::Absent
        );
        assert!(!filas[0].is_favorite);
    }

    #[tokio::test]
    async fn el_filtro_local_only_excluye_lo_no_descargado() {
        let (repo, pool, _g) = repo().await;
        let descargada = pista("Descargada", vec![artista("X")]);
        let ausente = pista("Ausente", vec![artista("Y")]);
        repo.upsert(&[descargada.clone(), ausente])
            .await
            .expect("guarda");

        let id = descargada.id.as_str().to_owned();
        pool.escribir(move |tx| {
            tx.execute(
                "INSERT INTO audio_files
                 (track_id, rel_path, format, codec, size_bytes, duration_ms, verified_at)
                 VALUES (?1, 'audio/aa/x.opus', 'opus', 'opus', 4000000, 248000, 0)",
                [&id],
            )?;
            Ok(())
        })
        .await
        .expect("registra fichero");

        let filtro = TrackFilter {
            local_only: true,
            ..TrackFilter::default()
        };
        let pagina = repo
            .list_rows(&filtro, TrackSort::AddedDesc, &PageRequest::new(0, 50))
            .await
            .expect("lista");

        assert_eq!(pagina.total, Some(1));
        assert_eq!(pagina.items.len(), 1);
        assert_eq!(pagina.items[0].title, "Descargada");
        assert!(pagina.items[0].availability.es_local());
    }

    #[tokio::test]
    async fn la_paginacion_no_repite_ni_pierde_filas() {
        let (repo, _pool, _g) = repo().await;
        let pistas: Vec<Track> = (0..25)
            .map(|i| pista(&format!("Pista {i:02}"), vec![artista("X")]))
            .collect();
        repo.upsert(&pistas).await.expect("guarda");

        let mut vistos: HashSet<String> = HashSet::new();
        let mut offset = 0_u32;
        loop {
            let pagina = repo
                .list_rows(
                    &TrackFilter::default(),
                    TrackSort::TitleAsc,
                    &PageRequest::new(offset, 10),
                )
                .await
                .expect("lista");

            for fila in &pagina.items {
                assert!(
                    vistos.insert(fila.id.as_str().to_owned()),
                    "fila repetida entre páginas"
                );
            }
            if pagina.next_cursor.is_none() {
                break;
            }
            offset += 10;
            assert!(offset < 100, "la paginación no termina");
        }

        assert_eq!(vistos.len(), 25, "se perdieron filas al paginar");
    }

    #[tokio::test]
    async fn el_scroll_por_cursor_recorre_todo_sin_repetir() {
        let (repo, _pool, _g) = repo().await;
        let pistas: Vec<Track> = (0..47)
            .map(|i| pista(&format!("Pista {i:02}"), vec![artista("X")]))
            .collect();
        repo.upsert(&pistas).await.expect("guarda");

        for orden in [
            TrackSort::AddedDesc,
            TrackSort::TitleAsc,
            TrackSort::ArtistAsc,
            TrackSort::AlbumAsc,
            TrackSort::DurationAsc,
            TrackSort::PlayCountDesc,
            TrackSort::LastPlayedDesc,
        ] {
            let mut vistos: Vec<String> = Vec::new();
            let mut cursor = None;
            let mut vueltas = 0;

            loop {
                let peticion = match cursor {
                    Some(c) => PageRequest::from_cursor(c, 10),
                    None => PageRequest::new(0, 10),
                };
                let pagina = repo
                    .list_rows(&TrackFilter::default(), orden, &peticion)
                    .await
                    .expect("lista");

                for fila in &pagina.items {
                    vistos.push(fila.id.as_str().to_owned());
                }

                vueltas += 1;
                assert!(vueltas < 50, "la paginación no termina con {orden:?}");

                match pagina.next_cursor {
                    Some(c) => cursor = Some(c),
                    None => break,
                }
            }

            let unicos: HashSet<_> = vistos.iter().collect();
            assert_eq!(
                unicos.len(),
                47,
                "con {orden:?} se perdieron o repitieron filas ({} leídas, {} únicas)",
                vistos.len(),
                unicos.len()
            );
        }
    }

    #[tokio::test]
    async fn el_total_solo_se_cuenta_en_la_primera_pagina() {
        // Contar exige recorrer el conjunto entero: repetirlo en cada página
        // del scroll sería pagar un escaneo completo por cada 100 filas.
        let (repo, _pool, _g) = repo().await;
        let pistas: Vec<Track> = (0..25)
            .map(|i| pista(&format!("P{i:02}"), vec![artista("X")]))
            .collect();
        repo.upsert(&pistas).await.expect("guarda");

        let primera = repo
            .list_rows(
                &TrackFilter::default(),
                TrackSort::TitleAsc,
                &PageRequest::new(0, 10),
            )
            .await
            .expect("lista");
        assert_eq!(primera.total, Some(25));

        let cursor = primera.next_cursor.expect("hay más páginas");
        let segunda = repo
            .list_rows(
                &TrackFilter::default(),
                TrackSort::TitleAsc,
                &PageRequest::from_cursor(cursor, 10),
            )
            .await
            .expect("lista");
        assert_eq!(segunda.total, None);
        assert_eq!(segunda.items.len(), 10);
    }

    #[tokio::test]
    async fn el_cursor_respeta_el_filtro() {
        let (repo, pool, _g) = repo().await;
        let pistas: Vec<Track> = (0..30)
            .map(|i| pista(&format!("P{i:02}"), vec![artista("X")]))
            .collect();
        repo.upsert(&pistas).await.expect("guarda");

        // Solo la mitad tiene fichero.
        for (i, t) in pistas.iter().enumerate().filter(|(i, _)| i % 2 == 0) {
            let id = t.id.as_str().to_owned();
            let ruta = format!("audio/aa/{i}.opus");
            pool.escribir(move |tx| {
                tx.execute(
                    "INSERT INTO audio_files
                     (track_id, rel_path, format, codec, size_bytes, duration_ms, verified_at)
                     VALUES (?1, ?2, 'opus', 'opus', 100, 200000, 0)",
                    params![id, ruta],
                )?;
                Ok(())
            })
            .await
            .expect("registra");
        }

        let filtro = TrackFilter {
            local_only: true,
            ..TrackFilter::default()
        };
        let mut total_vistas = 0;
        let mut cursor = None;
        loop {
            let peticion = match cursor {
                Some(c) => PageRequest::from_cursor(c, 5),
                None => PageRequest::new(0, 5),
            };
            let pagina = repo
                .list_rows(&filtro, TrackSort::TitleAsc, &peticion)
                .await
                .expect("lista");
            assert!(
                pagina.items.iter().all(|f| f.availability.es_local()),
                "el filtro debe seguir aplicándose al paginar con cursor"
            );
            total_vistas += pagina.items.len();
            match pagina.next_cursor {
                Some(c) => cursor = Some(c),
                None => break,
            }
        }
        assert_eq!(total_vistas, 15);
    }

    #[tokio::test]
    async fn el_limite_de_pagina_se_acota_al_maximo() {
        let (repo, _pool, _g) = repo().await;
        let pistas: Vec<Track> = (0..250)
            .map(|i| pista(&format!("P{i:03}"), vec![artista("X")]))
            .collect();
        repo.upsert(&pistas).await.expect("guarda");

        let pagina = repo
            .list_rows(
                &TrackFilter::default(),
                TrackSort::TitleAsc,
                &PageRequest::new(0, 10_000),
            )
            .await
            .expect("lista");

        assert_eq!(
            pagina.items.len(),
            localify_core::page::LIMITE_MAXIMO as usize,
            "el tope protege el puente IPC aunque el cliente pida más"
        );
    }

    #[tokio::test]
    async fn las_estadisticas_cuentan_catalogo_y_ficheros_por_separado() {
        let (repo, pool, _g) = repo().await;
        let pistas: Vec<Track> = (0..5)
            .map(|i| pista(&format!("P{i}"), vec![artista("X")]))
            .collect();
        repo.upsert(&pistas).await.expect("guarda");

        let id = pistas[0].id.as_str().to_owned();
        pool.escribir(move |tx| {
            tx.execute(
                "INSERT INTO audio_files
                 (track_id, rel_path, format, codec, size_bytes, duration_ms, verified_at)
                 VALUES (?1, 'audio/aa/x.opus', 'opus', 'opus', 4000000, 248000, 0)",
                [&id],
            )?;
            Ok(())
        })
        .await
        .expect("registra");

        let stats = repo.stats().await.expect("stats");
        assert_eq!(stats.track_count, 5, "el catálogo incluye lo no descargado");
        assert_eq!(stats.local_count, 1, "solo una está realmente en disco");
        assert_eq!(stats.total_bytes, 4_000_000);
        assert_eq!(stats.total_duration_ms, 5 * 248_000);
    }

    #[tokio::test]
    async fn stale_devuelve_primero_lo_que_nunca_se_refresco() {
        let (repo, pool, _g) = repo().await;
        let t = pista("X", vec![artista("A")]);
        repo.upsert(std::slice::from_ref(&t)).await.expect("guarda");

        // upsert marca metadata_at = ahora, así que nada está caducado todavía.
        let recientes = repo.stale(3600, 10).await.expect("consulta");
        assert!(recientes.is_empty());

        pool.escribir(|tx| {
            tx.execute("UPDATE tracks SET metadata_at = NULL", [])?;
            Ok(())
        })
        .await
        .expect("envejece");

        let caducados = repo.stale(3600, 10).await.expect("consulta");
        assert_eq!(
            caducados,
            vec![t.id],
            "sin metadata_at, la pista debe refrescarse"
        );
    }

    #[tokio::test]
    async fn borrar_una_pista_arrastra_sus_relaciones() {
        let (repo, pool, _g) = repo().await;
        let t = pista("X", vec![artista("A"), artista("B")]);
        repo.upsert(std::slice::from_ref(&t)).await.expect("guarda");

        let id = t.id.as_str().to_owned();
        pool.escribir(move |tx| {
            tx.execute("DELETE FROM tracks WHERE id = ?1", [&id])?;
            Ok(())
        })
        .await
        .expect("borra");

        let huerfanos: i64 = pool
            .leer(|c| Ok(c.query_row("SELECT COUNT(*) FROM track_artists", [], |r| r.get(0))?))
            .await
            .expect("cuenta");
        assert_eq!(huerfanos, 0, "ON DELETE CASCADE debe limpiar track_artists");
    }
}

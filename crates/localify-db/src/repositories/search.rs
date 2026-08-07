//! Búsqueda de texto completo sobre la biblioteca local.
//!
//! Es la **primera parada de toda búsqueda**, siempre, antes de consultar a
//! ningún proveedor externo. Debe responder en menos de 30 ms con 50 000
//! pistas, porque se ejecuta en cada pulsación de tecla.

use async_trait::async_trait;
use localify_core::domain::album::AlbumRow;
use localify_core::domain::artist::ArtistRow;
use localify_core::domain::ids::{AlbumId, ArtistId};
use localify_core::domain::playlist::PlaylistSummary;
use localify_core::domain::track::TrackRow;
use localify_core::error::CoreResult;
use localify_core::page::{Cursor, Page, PageRequest};
use localify_core::ports::database::SearchRepository;
use localify_core::text;
use rusqlite::params;

use crate::error::{DbResult, ToCore};
use crate::mappers::{COLUMNAS_TRACK_ROW, JOINS_TRACK_ROW, a_track_row, anyo_de};
use crate::pool::Pool;

pub struct SqliteSearchRepository {
    pool: Pool,
}

impl std::fmt::Debug for SqliteSearchRepository {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteSearchRepository")
            .finish_non_exhaustive()
    }
}

impl SqliteSearchRepository {
    #[must_use]
    pub const fn new(pool: Pool) -> Self {
        Self { pool }
    }
}

/// Convierte lo que teclea el usuario en una consulta FTS5 segura.
///
/// **No se puede pasar el texto crudo**: FTS5 tiene sintaxis propia (`AND`,
/// `NOT`, `*`, `"`, `:`, `^`) y un usuario que escriba `AC/DC` o una comilla
/// provocaría un error de sintaxis en lugar de una búsqueda. Se normaliza igual
/// que las columnas indexadas y cada término se entrecomilla.
///
/// El último término lleva `*` para que la búsqueda sea por prefijo: quien
/// escribe "bohem" espera ver "Bohemian Rhapsody" antes de terminar la palabra.
/// Es lo que aprovecha el `prefix='2 3'` del índice.
#[must_use]
pub fn a_consulta_fts(entrada: &str) -> Option<String> {
    let normalizado = text::normalize(entrada);
    let terminos: Vec<&str> = normalizado.split(' ').filter(|t| !t.is_empty()).collect();
    if terminos.is_empty() {
        return None;
    }

    let ultimo = terminos.len() - 1;
    let consulta = terminos
        .iter()
        .enumerate()
        .map(|(i, t)| {
            // Las comillas dobles internas no pueden existir tras `normalize`
            // (elimina todo lo no alfanumérico), pero se escapan igualmente:
            // depender de una propiedad de otra función para la seguridad es
            // frágil.
            let seguro = t.replace('"', "");
            if i == ultimo {
                format!("\"{seguro}\"*")
            } else {
                format!("\"{seguro}\"")
            }
        })
        .collect::<Vec<_>>()
        .join(" ");

    Some(consulta)
}

#[async_trait]
impl SearchRepository for SqliteSearchRepository {
    async fn search_tracks(&self, query: &str, page: &PageRequest) -> CoreResult<Page<TrackRow>> {
        let Some(fts) = a_consulta_fts(query) else {
            return Ok(Page::empty());
        };

        let limite = i64::from(page.limit());
        let offset = i64::from(page.offset());
        let columnas = COLUMNAS_TRACK_ROW;
        let joins = JOINS_TRACK_ROW;

        // `is_local DESC` implementa "lo que ya tengo, primero" en SQL, no en la
        // aplicación. Los pesos de bm25 reflejan que buscar por título es lo más
        // común: título 10, artista 8, álbum 3. bm25 devuelve valores negativos
        // donde el más negativo es el mejor, de ahí el orden ascendente.
        let sql = format!(
            "SELECT {columnas}, (af.track_id IS NOT NULL) AS is_local
             FROM tracks_fts fts
             JOIN tracks t ON t.rowid = fts.rowid
             {joins}
             WHERE tracks_fts MATCH ?1
             ORDER BY is_local DESC, bm25(tracks_fts, 10.0, 8.0, 3.0) ASC
             LIMIT ?2 OFFSET ?3"
        );

        self.pool
            .leer(move |conn| {
                let total: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM tracks_fts WHERE tracks_fts MATCH ?1",
                    [&fts],
                    |r| r.get(0),
                )?;
                let total = total.max(0).unsigned_abs();

                let mut stmt = conn.prepare_cached(&sql)?;
                let items = stmt
                    .query_map(params![fts, limite, offset], |row| Ok(a_track_row(row)))?
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

    async fn search_albums(&self, query: &str, limit: u8) -> CoreResult<Vec<AlbumRow>> {
        let normalizado = text::normalize(query);
        if normalizado.is_empty() {
            return Ok(Vec::new());
        }
        let patron = format!("%{normalizado}%");

        // Los álbumes no tienen índice FTS propio: son dos órdenes de magnitud
        // menos que las pistas y un LIKE sobre la columna normalizada indexada
        // basta. Añadir otra tabla virtual sería coste sin beneficio medible.
        self.pool
            .leer(move |conn| {
                let mut stmt = conn.prepare_cached(
                    "SELECT al.id, al.title, al.release_date, al.cover_cached,
                            COALESCE((SELECT GROUP_CONCAT(nombre, ', ')
                                      FROM (SELECT ar.name AS nombre
                                            FROM album_artists aa
                                            JOIN artists ar ON ar.id = aa.artist_id
                                            WHERE aa.album_id = al.id
                                            ORDER BY aa.position)), '') AS artist_display,
                            (SELECT COUNT(*) FROM tracks t WHERE t.album_id = al.id) AS track_count,
                            (SELECT COUNT(*) FROM tracks t
                              JOIN audio_files af ON af.track_id = t.id
                              WHERE t.album_id = al.id) AS local_count
                     FROM albums al
                     WHERE al.title_norm LIKE ?1
                     ORDER BY local_count DESC, al.title_norm ASC
                     LIMIT ?2",
                )?;

                let filas = stmt
                    .query_map(params![patron, i64::from(limit)], |r| {
                        let release: Option<String> = r.get("release_date")?;
                        let cacheada: i64 = r.get("cover_cached")?;
                        let id: String = r.get("id")?;
                        Ok(AlbumRow {
                            year: anyo_de(release.as_deref()),
                            cover: (cacheada != 0).then(|| id.clone()),
                            id: AlbumId::from_trusted(id),
                            title: r.get("title")?,
                            artist_display: r.get("artist_display")?,
                            track_count: u16::try_from(r.get::<_, i64>("track_count")?)
                                .unwrap_or(u16::MAX),
                            local_count: u16::try_from(r.get::<_, i64>("local_count")?)
                                .unwrap_or(u16::MAX),
                        })
                    })?
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(filas)
            })
            .await
            .to_core()
    }

    async fn search_artists(&self, query: &str, limit: u8) -> CoreResult<Vec<ArtistRow>> {
        let normalizado = text::normalize(query);
        if normalizado.is_empty() {
            return Ok(Vec::new());
        }
        let patron = format!("%{normalizado}%");
        let exacto = normalizado;

        self.pool
            .leer(move |conn| {
                let mut stmt = conn.prepare_cached(
                    "SELECT ar.id, ar.name, ar.image_url,
                            (SELECT COUNT(*) FROM track_artists ta
                              WHERE ta.artist_id = ar.id) AS track_count,
                            (SELECT COUNT(*) FROM track_artists ta
                              JOIN audio_files af ON af.track_id = ta.track_id
                              WHERE ta.artist_id = ar.id) AS local_track_count
                     FROM artists ar
                     WHERE ar.name_norm LIKE ?1
                     ORDER BY (ar.name_norm = ?2) DESC,
                              local_track_count DESC,
                              ar.name_norm ASC
                     LIMIT ?3",
                )?;

                let filas = stmt
                    .query_map(params![patron, exacto, i64::from(limit)], |r| {
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

    async fn search_playlists(&self, query: &str, limit: u8) -> CoreResult<Vec<PlaylistSummary>> {
        let normalizado = text::normalize(query);
        if normalizado.is_empty() {
            return Ok(Vec::new());
        }
        let patron = format!("%{normalizado}%");

        self.pool
            .leer(move |conn| {
                let mut stmt = conn.prepare_cached(
                    "SELECT p.id, p.name, p.cover_path, p.updated_at, p.source,
                            (SELECT COUNT(*) FROM playlist_items pi
                              WHERE pi.playlist_id = p.id) AS track_count
                     FROM playlists p
                     WHERE p.name_norm LIKE ?1
                     ORDER BY p.updated_at DESC
                     LIMIT ?2",
                )?;

                let filas = stmt
                    .query_map(params![patron, i64::from(limit)], |r| {
                        let id: String = r.get("id")?;
                        let origen: String = r.get("source")?;
                        Ok(PlaylistSummary {
                            id: localify_core::domain::ids::PlaylistId::parse(&id).unwrap_or_else(
                                |_| localify_core::domain::ids::PlaylistId::nuevo(),
                            ),
                            name: r.get("name")?,
                            track_count: u32::try_from(r.get::<_, i64>("track_count")?)
                                .unwrap_or(0),
                            // Sin imagen: en un resultado de búsqueda la
                            // playlist se muestra como una línea de texto, y
                            // componer el mosaico serían cuatro subconsultas
                            // por fila para algo que nadie pinta.
                            cover_albums: Vec::new(),
                            has_own_cover: false,
                            updated_at: crate::mappers::a_fecha(r.get::<_, i64>("updated_at")?),
                            source: crate::mappers::a_origen_playlist(&origen),
                        })
                    })?
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(filas)
            })
            .await
            .to_core()
    }
}

#[cfg(test)]
mod tests {
    use localify_core::domain::album::{Album, AlbumType, CoverSet};
    use localify_core::domain::audio::DurationMs;
    use localify_core::domain::ids::TrackId;
    use localify_core::domain::track::{AlbumRef, ArtistRef, Track};
    use localify_core::ports::database::{AlbumRepository, TrackRepository};

    use super::*;
    use crate::pool::TempDbGuard;
    use crate::repositories::albums::SqliteAlbumRepository;
    use crate::repositories::tracks::SqliteTrackRepository;

    struct Ctx {
        search: SqliteSearchRepository,
        tracks: SqliteTrackRepository,
        albums: SqliteAlbumRepository,
        pool: Pool,
        _guard: TempDbGuard,
    }

    async fn ctx() -> Ctx {
        let (pool, guard) = Pool::temporal().expect("abre");
        crate::migrations::ejecutar(&pool).await.expect("migra");
        Ctx {
            search: SqliteSearchRepository::new(pool.clone()),
            tracks: SqliteTrackRepository::new(pool.clone()),
            albums: SqliteAlbumRepository::new(pool.clone()),
            pool,
            _guard: guard,
        }
    }

    fn pista(titulo: &str, artista: &str) -> Track {
        Track {
            id: TrackId::nuevo_local(),
            title: titulo.into(),
            album: None,
            artists: vec![ArtistRef {
                id: ArtistId::nuevo_local(),
                name: artista.into(),
            }],
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

    async fn titulos(c: &Ctx, consulta: &str) -> Vec<String> {
        c.search
            .search_tracks(consulta, &PageRequest::new(0, 50))
            .await
            .expect("busca")
            .items
            .into_iter()
            .map(|t| t.title)
            .collect()
    }

    #[test]
    fn la_consulta_fts_entrecomilla_y_anade_prefijo_al_ultimo_termino() {
        assert_eq!(
            a_consulta_fts("bohemian rha"),
            Some(r#""bohemian" "rha"*"#.into())
        );
        assert_eq!(a_consulta_fts("queen"), Some(r#""queen"*"#.into()));
    }

    #[test]
    fn la_consulta_fts_neutraliza_la_sintaxis_de_fts5() {
        // Sin entrecomillar, `AND`, `NOT` o `*` serían operadores y `AC/DC`
        // rompería el parser.
        for entrada in [
            "AC/DC",
            "rock AND roll",
            "NOT me",
            "a*b",
            r#"comi"llas"#,
            "x:y",
        ] {
            let consulta = a_consulta_fts(entrada).expect("produce consulta");
            assert!(consulta.starts_with('"'), "'{entrada}' → {consulta}");
            assert!(
                !consulta.contains(r#"""""#),
                "comillas sin escapar en {consulta}"
            );
        }
    }

    #[test]
    fn una_consulta_vacia_no_produce_busqueda() {
        assert_eq!(a_consulta_fts(""), None);
        assert_eq!(a_consulta_fts("   "), None);
        assert_eq!(a_consulta_fts("!!! ---"), None);
    }

    #[tokio::test]
    async fn una_consulta_con_solo_simbolos_no_falla_ni_devuelve_nada() {
        let c = ctx().await;
        c.tracks
            .upsert(&[pista("Bohemian Rhapsody", "Queen")])
            .await
            .expect("guarda");

        let pagina = c
            .search
            .search_tracks("!!!", &PageRequest::new(0, 10))
            .await
            .expect("no debe fallar");
        assert!(pagina.is_empty());
    }

    #[tokio::test]
    async fn encuentra_por_titulo_por_artista_y_por_prefijo() {
        let c = ctx().await;
        c.tracks
            .upsert(&[
                pista("Bohemian Rhapsody", "Queen"),
                pista("Stairway to Heaven", "Led Zeppelin"),
            ])
            .await
            .expect("guarda");

        assert_eq!(titulos(&c, "bohemian").await, vec!["Bohemian Rhapsody"]);
        assert_eq!(titulos(&c, "queen").await, vec!["Bohemian Rhapsody"]);
        // Prefijo: lo que se busca al teclear.
        assert_eq!(titulos(&c, "bohem").await, vec!["Bohemian Rhapsody"]);
        assert_eq!(titulos(&c, "zepp").await, vec!["Stairway to Heaven"]);
    }

    #[tokio::test]
    async fn la_busqueda_ignora_los_diacriticos() {
        let c = ctx().await;
        c.tracks
            .upsert(&[pista("Jóga", "Björk"), pista("Cafe Tacvba", "Café Tacvba")])
            .await
            .expect("guarda");

        assert_eq!(titulos(&c, "joga").await, vec!["Jóga"]);
        assert_eq!(titulos(&c, "bjork").await, vec!["Jóga"]);
        assert_eq!(titulos(&c, "café").await, vec!["Cafe Tacvba"]);
    }

    #[tokio::test]
    async fn los_terminos_se_combinan_con_and() {
        let c = ctx().await;
        c.tracks
            .upsert(&[
                pista("Under Pressure", "Queen"),
                pista("Under the Bridge", "Red Hot Chili Peppers"),
            ])
            .await
            .expect("guarda");

        assert_eq!(titulos(&c, "under").await.len(), 2);
        assert_eq!(titulos(&c, "under queen").await, vec!["Under Pressure"]);
    }

    #[tokio::test]
    async fn lo_descargado_aparece_antes_que_lo_que_no_esta() {
        let c = ctx().await;
        let remota = pista("Bohemian Rhapsody", "Queen");
        let local = pista("Bohemian Rhapsody Live", "Queen");
        c.tracks
            .upsert(&[remota.clone(), local.clone()])
            .await
            .expect("guarda");

        let id = local.id.as_str().to_owned();
        c.pool
            .escribir(move |tx| {
                tx.execute(
                    "INSERT INTO audio_files
                     (track_id, rel_path, format, codec, size_bytes, duration_ms, verified_at)
                     VALUES (?1, 'audio/aa/x.opus', 'opus', 'opus', 100, 200000, 0)",
                    [&id],
                )?;
                Ok(())
            })
            .await
            .expect("registra");

        let resultados = titulos(&c, "bohemian").await;
        assert_eq!(
            resultados[0], "Bohemian Rhapsody Live",
            "lo que ya está en disco debe encabezar los resultados"
        );
    }

    #[tokio::test]
    async fn el_titulo_pesa_mas_que_el_album() {
        let c = ctx().await;

        // Un álbum llamado "Nevermind" con una pista que no lo menciona, y una
        // pista titulada "Nevermind" en otro álbum. Debe ganar el título.
        let al = Album {
            id: AlbumId::nuevo_local(),
            title: "Nevermind".into(),
            artists: vec![],
            album_type: AlbumType::Album,
            release_date: None,
            total_tracks: None,
            cover_url: None,
            covers: CoverSet::default(),
            label: None,
        };
        c.albums
            .upsert(std::slice::from_ref(&al))
            .await
            .expect("guarda álbum");

        let mut en_album = pista("Smells Like Teen Spirit", "Nirvana");
        en_album.album = Some(AlbumRef {
            id: al.id.clone(),
            title: al.title.clone(),
        });

        let por_titulo = pista("Nevermind", "Otro Artista");
        c.tracks
            .upsert(&[en_album, por_titulo])
            .await
            .expect("guarda pistas");

        let resultados = titulos(&c, "nevermind").await;
        assert_eq!(resultados.len(), 2);
        assert_eq!(
            resultados[0], "Nevermind",
            "bm25 pondera el título por encima del álbum"
        );
    }

    #[tokio::test]
    async fn borrar_una_pista_la_saca_del_indice() {
        let c = ctx().await;
        let t = pista("Bohemian Rhapsody", "Queen");
        c.tracks
            .upsert(std::slice::from_ref(&t))
            .await
            .expect("guarda");
        assert_eq!(titulos(&c, "bohemian").await.len(), 1);

        let id = t.id.as_str().to_owned();
        c.pool
            .escribir(move |tx| {
                tx.execute("DELETE FROM tracks WHERE id = ?1", [&id])?;
                Ok(())
            })
            .await
            .expect("borra");

        assert!(
            titulos(&c, "bohemian").await.is_empty(),
            "el trigger de borrado debe limpiar el índice sin dejar residuo"
        );
    }

    #[tokio::test]
    async fn renombrar_una_pista_actualiza_el_indice_en_ambos_sentidos() {
        let c = ctx().await;
        let mut t = pista("Titulo Viejo", "Queen");
        c.tracks
            .upsert(std::slice::from_ref(&t))
            .await
            .expect("guarda");

        t.title = "Titulo Nuevo".into();
        c.tracks
            .upsert(std::slice::from_ref(&t))
            .await
            .expect("renombra");

        assert_eq!(titulos(&c, "nuevo").await, vec!["Titulo Nuevo"]);
        assert!(
            titulos(&c, "viejo").await.is_empty(),
            "el término viejo debe desaparecer"
        );
    }

    #[tokio::test]
    async fn la_busqueda_de_artistas_prioriza_la_coincidencia_exacta() {
        let c = ctx().await;
        c.tracks
            .upsert(&[pista("A", "Queen"), pista("B", "Queens of the Stone Age")])
            .await
            .expect("guarda");

        let artistas = c.search.search_artists("queen", 10).await.expect("busca");
        assert_eq!(
            artistas[0].name, "Queen",
            "la coincidencia exacta va primero"
        );
        assert_eq!(artistas.len(), 2);
    }

    #[tokio::test]
    async fn la_busqueda_de_albumes_encuentra_por_fragmento() {
        let c = ctx().await;
        let al = Album {
            id: AlbumId::nuevo_local(),
            title: "The Dark Side of the Moon".into(),
            artists: vec![],
            album_type: AlbumType::Album,
            release_date: None,
            total_tracks: None,
            cover_url: None,
            covers: CoverSet::default(),
            label: None,
        };
        c.albums
            .upsert(std::slice::from_ref(&al))
            .await
            .expect("guarda");

        let encontrados = c
            .search
            .search_albums("dark side", 10)
            .await
            .expect("busca");
        assert_eq!(encontrados.len(), 1);
        assert_eq!(encontrados[0].title, "The Dark Side of the Moon");
    }

    #[tokio::test]
    async fn la_paginacion_de_resultados_es_estable() {
        let c = ctx().await;
        let pistas: Vec<Track> = (0..30)
            .map(|i| pista(&format!("Cancion {i:02}"), "Artista"))
            .collect();
        c.tracks.upsert(&pistas).await.expect("guarda");

        let mut vistos = std::collections::HashSet::new();
        let mut offset = 0_u32;
        loop {
            let pagina = c
                .search
                .search_tracks("cancion", &PageRequest::new(offset, 10))
                .await
                .expect("busca");
            for fila in &pagina.items {
                assert!(
                    vistos.insert(fila.id.as_str().to_owned()),
                    "resultado repetido"
                );
            }
            if pagina.next_cursor.is_none() {
                break;
            }
            offset += 10;
            assert!(offset < 200, "la paginación no termina");
        }
        assert_eq!(vistos.len(), 30);
    }
}

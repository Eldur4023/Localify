//! Repositorio de álbumes.

use async_trait::async_trait;
use localify_core::domain::album::{Album, AlbumFilter, AlbumRow, AlbumType, CoverSet};
use localify_core::domain::ids::{AlbumId, ArtistId};
use localify_core::domain::track::{ArtistRef, TrackRow};
use localify_core::error::CoreResult;
use localify_core::page::{Cursor, Page, PageRequest};
use localify_core::ports::database::AlbumRepository;
use localify_core::text;
use rusqlite::{OptionalExtension, Row, params};

use crate::error::{DbResult, ToCore};
use crate::mappers::{
    COLUMNAS_TRACK_ROW, JOINS_TRACK_ROW, a_fecha_lanzamiento, a_track_row, anyo_de, de_tipo_album,
};
use crate::pool::Pool;
use crate::repositories::artists::asegurar_artista;

pub struct SqliteAlbumRepository {
    pool: Pool,
}

impl std::fmt::Debug for SqliteAlbumRepository {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteAlbumRepository")
            .finish_non_exhaustive()
    }
}

impl SqliteAlbumRepository {
    #[must_use]
    pub const fn new(pool: Pool) -> Self {
        Self { pool }
    }
}

/// Columnas de una fila de rejilla de álbumes.
///
/// Las dos subconsultas de recuento resuelven "12 de 14 descargadas" sin
/// consultar pista a pista. Es la diferencia entre una consulta y N+1 al pintar
/// la vista de álbumes.
pub(crate) const COLUMNAS_ALBUM_ROW: &str = "
    al.id,
    al.title,
    al.release_date,
    al.cover_cached,
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
";

pub(crate) fn a_album_row(row: &Row<'_>) -> rusqlite::Result<AlbumRow> {
    let release: Option<String> = row.get("release_date")?;
    let cacheada: i64 = row.get("cover_cached")?;
    let id: String = row.get("id")?;

    Ok(AlbumRow {
        year: anyo_de(release.as_deref()),
        // La ruta de portada la resuelve `AppPaths` en la capa de arriba: la
        // base de datos solo sabe si existe, no dónde vive la biblioteca.
        cover: (cacheada != 0).then(|| id.clone()),
        id: AlbumId::from_trusted(id),
        title: row.get("title")?,
        artist_display: row.get("artist_display")?,
        track_count: u16::try_from(row.get::<_, i64>("track_count")?).unwrap_or(u16::MAX),
        local_count: u16::try_from(row.get::<_, i64>("local_count")?).unwrap_or(u16::MAX),
    })
}

#[async_trait]
impl AlbumRepository for SqliteAlbumRepository {
    async fn get(&self, id: &AlbumId) -> CoreResult<Option<Album>> {
        let id = id.clone();
        self.pool
            .leer(move |conn| {
                let mut stmt = conn.prepare_cached(
                    "SELECT title, album_type, release_date, total_tracks,
                            cover_url, cover_cached, label
                     FROM albums WHERE id = ?1",
                )?;

                let base = stmt
                    .query_row([id.as_str()], |r| {
                        Ok((
                            r.get::<_, String>(0)?,
                            r.get::<_, String>(1)?,
                            r.get::<_, Option<String>>(2)?,
                            r.get::<_, Option<u16>>(3)?,
                            r.get::<_, Option<String>>(4)?,
                            r.get::<_, i64>(5)?,
                            r.get::<_, Option<String>>(6)?,
                        ))
                    })
                    .optional()?;

                let Some(base) = base else { return Ok(None) };

                let mut stmt = conn.prepare_cached(
                    "SELECT ar.id, ar.name
                     FROM album_artists aa
                     JOIN artists ar ON ar.id = aa.artist_id
                     WHERE aa.album_id = ?1
                     ORDER BY aa.position",
                )?;
                let artists = stmt
                    .query_map([id.as_str()], |r| {
                        Ok(ArtistRef {
                            id: ArtistId::from_trusted(r.get::<_, String>(0)?),
                            name: r.get(1)?,
                        })
                    })?
                    .collect::<Result<Vec<_>, _>>()?;

                Ok(Some(Album {
                    id: id.clone(),
                    title: base.0,
                    artists,
                    album_type: AlbumType::from_str_lax(&base.1),
                    release_date: a_fecha_lanzamiento(base.2),
                    total_tracks: base.3,
                    cover_url: base.4,
                    // El juego de portadas cacheadas lo compone la capa que
                    // conoce las rutas; aquí solo se sabe si existe.
                    covers: CoverSet::default(),
                    label: base.6,
                }))
            })
            .await
            .to_core()
    }

    async fn upsert(&self, albums: &[Album]) -> CoreResult<()> {
        if albums.is_empty() {
            return Ok(());
        }
        let albums = albums.to_vec();

        self.pool
            .escribir(move |tx| {
                for album in &albums {
                    // El identificador con el que queda cada artista no tiene
                    // por qué ser el que traía: uno local se resuelve contra el
                    // que ya hubiera con ese nombre. Escribirlos a pelo aquí era
                    // la puerta por la que un álbum importado seguía creando
                    // artistas repetidos aunque el camino de las pistas ya no lo
                    // hiciera.
                    let artistas: Vec<String> = album
                        .artists
                        .iter()
                        .map(|a| asegurar_artista(tx, a))
                        .collect::<DbResult<_>>()?;

                    tx.execute(
                        "INSERT INTO albums (
                             id, title, title_norm, album_type, release_date,
                             total_tracks, cover_url, label, metadata_at
                         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, unixepoch())
                         ON CONFLICT (id) DO UPDATE SET
                             title        = ?2,
                             title_norm   = ?3,
                             album_type   = ?4,
                             release_date = ?5,
                             total_tracks = ?6,
                             cover_url    = ?7,
                             label        = ?8,
                             metadata_at  = unixepoch()",
                        params![
                            album.id.as_str(),
                            album.title,
                            text::normalize(&album.title),
                            de_tipo_album(album.album_type),
                            album.release_date.map(|d| d.format("%Y-%m-%d").to_string()),
                            album.total_tracks,
                            album.cover_url,
                            album.label,
                        ],
                    )?;

                    // Reemplazo en bloque, igual que en `tracks`: dejar filas
                    // antiguas produciría un `artist_display` incorrecto.
                    tx.execute(
                        "DELETE FROM album_artists WHERE album_id = ?1",
                        [album.id.as_str()],
                    )?;
                    // Se deduplica por el identificador **ya canonizado**, igual
                    // que en `tracks`: dos locales distintos pueden resolverse
                    // al mismo artista, y la pareja repetida violaría la clave
                    // primaria abortando la transacción entera.
                    let mut vistos = std::collections::HashSet::new();
                    let mut posicion = 0_i64;
                    for id in &artistas {
                        if !vistos.insert(id.clone()) {
                            continue;
                        }
                        tx.execute(
                            "INSERT INTO album_artists (album_id, artist_id, position)
                             VALUES (?1, ?2, ?3)",
                            params![album.id.as_str(), id, posicion],
                        )?;
                        posicion += 1;
                    }
                }
                Ok(())
            })
            .await
            .to_core()
    }

    async fn list_rows(
        &self,
        filter: &AlbumFilter,
        page: &PageRequest,
    ) -> CoreResult<Page<AlbumRow>> {
        let mut condiciones: Vec<String> = Vec::new();
        let mut params: Vec<Box<dyn rusqlite::ToSql + Send>> = Vec::new();

        if let Some(artista) = &filter.artist_id {
            condiciones.push(
                "EXISTS (SELECT 1 FROM album_artists aa
                         WHERE aa.album_id = al.id AND aa.artist_id = ?)"
                    .into(),
            );
            params.push(Box::new(artista.as_str().to_owned()));
        }
        if filter.local_only {
            condiciones.push(
                "EXISTS (SELECT 1 FROM tracks t
                         JOIN audio_files af ON af.track_id = t.id
                         WHERE t.album_id = al.id)"
                    .into(),
            );
        }
        if let Some(texto) = &filter.text {
            let normalizado = text::normalize(texto);
            if !normalizado.is_empty() {
                condiciones.push("al.title_norm LIKE ?".into());
                params.push(Box::new(format!("%{normalizado}%")));
            }
        }

        let where_sql = if condiciones.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", condiciones.join(" AND "))
        };

        let columnas = COLUMNAS_ALBUM_ROW;
        let sql = format!(
            "SELECT {columnas} FROM albums al {where_sql}
             ORDER BY al.title_norm ASC, al.id ASC
             LIMIT ? OFFSET ?"
        );
        let sql_total = format!("SELECT COUNT(*) FROM albums al {where_sql}");

        let limite = page.limit();
        let offset = page.offset();

        self.pool
            .leer(move |conn| {
                let mut refs: Vec<&dyn rusqlite::ToSql> = params
                    .iter()
                    .map(|p| p.as_ref() as &dyn rusqlite::ToSql)
                    .collect();

                let total: i64 = conn.query_row(&sql_total, refs.as_slice(), |r| r.get(0))?;
                let total = total.max(0).unsigned_abs();

                let limite_i = i64::from(limite);
                let offset_i = i64::from(offset);
                refs.push(&limite_i);
                refs.push(&offset_i);

                let mut stmt = conn.prepare(&sql)?;
                let items = stmt
                    .query_map(refs.as_slice(), a_album_row)?
                    .collect::<Result<Vec<_>, _>>()?;

                let consumidos = u64::from(offset) + items.len() as u64;
                let next = (consumidos < total).then(|| Cursor::new(consumidos.to_string()));

                Ok(Page::new(items, Some(total), next))
            })
            .await
            .to_core()
    }

    async fn tracks_of(&self, id: &AlbumId) -> CoreResult<Vec<TrackRow>> {
        let id = id.as_str().to_owned();
        let columnas = COLUMNAS_TRACK_ROW;
        let joins = JOINS_TRACK_ROW;
        // Un álbum rara vez pasa de 50 pistas: no se pagina.
        let sql = format!(
            "SELECT {columnas} FROM tracks t {joins}
             WHERE t.album_id = ?1
             ORDER BY t.disc_number ASC, t.track_number ASC, t.title_norm ASC"
        );

        self.pool
            .leer(move |conn| {
                let mut stmt = conn.prepare(&sql)?;
                let filas = stmt
                    .query_map([&id], |row| Ok(a_track_row(row)))?
                    .collect::<Result<Vec<_>, _>>()?
                    .into_iter()
                    .collect::<DbResult<Vec<_>>>()?;
                Ok(filas)
            })
            .await
            .to_core()
    }

    async fn set_cover_cached(&self, id: &AlbumId, cached: bool) -> CoreResult<()> {
        let id = id.as_str().to_owned();
        self.pool
            .escribir(move |tx| {
                tx.execute(
                    "UPDATE albums SET cover_cached = ?2 WHERE id = ?1",
                    params![id, i64::from(cached)],
                )?;
                Ok(())
            })
            .await
            .to_core()
    }

    async fn find_by_title_and_artist(
        &self,
        title_norm: &str,
        artist_name_norm: &str,
    ) -> CoreResult<Option<AlbumId>> {
        let title_norm = title_norm.to_owned();
        let artist_name_norm = artist_name_norm.to_owned();

        self.pool
            .leer(move |conn| {
                let id: Option<String> = conn
                    .query_row(
                        "SELECT al.id FROM albums al
                         JOIN album_artists aa ON aa.album_id = al.id
                         JOIN artists ar ON ar.id = aa.artist_id
                         WHERE al.title_norm = ?1 AND ar.name_norm = ?2
                         LIMIT 1",
                        params![title_norm, artist_name_norm],
                        |r| r.get(0),
                    )
                    .optional()?;
                Ok(id.map(AlbumId::from_trusted))
            })
            .await
            .to_core()
    }
}

#[cfg(test)]
mod tests {
    use localify_core::domain::audio::DurationMs;
    use localify_core::domain::ids::TrackId;
    use localify_core::domain::track::{AlbumRef, Track};
    use localify_core::ports::database::TrackRepository;

    use super::*;
    use crate::pool::TempDbGuard;
    use crate::repositories::tracks::SqliteTrackRepository;

    async fn repos() -> (
        SqliteAlbumRepository,
        SqliteTrackRepository,
        Pool,
        TempDbGuard,
    ) {
        let (pool, guard) = Pool::temporal().expect("abre");
        crate::migrations::ejecutar(&pool).await.expect("migra");
        (
            SqliteAlbumRepository::new(pool.clone()),
            SqliteTrackRepository::new(pool.clone()),
            pool,
            guard,
        )
    }

    fn album(titulo: &str, artistas: Vec<ArtistRef>) -> Album {
        Album {
            id: AlbumId::nuevo_local(),
            title: titulo.into(),
            artists: artistas,
            album_type: AlbumType::Album,
            release_date: chrono::NaiveDate::from_ymd_opt(1982, 5, 21),
            total_tracks: Some(10),
            cover_url: Some("https://ejemplo/x.jpg".into()),
            covers: CoverSet::default(),
            label: Some("EMI".into()),
        }
    }

    fn artista(nombre: &str) -> ArtistRef {
        ArtistRef {
            id: ArtistId::nuevo_local(),
            name: nombre.into(),
        }
    }

    fn pista_de(album: &Album, titulo: &str, disco: u16, numero: u16) -> Track {
        Track {
            id: TrackId::nuevo_local(),
            title: titulo.into(),
            album: Some(AlbumRef {
                id: album.id.clone(),
                title: album.title.clone(),
            }),
            artists: vec![artista("Queen")],
            duration: DurationMs::new(200_000),
            track_number: Some(numero),
            disc_number: Some(disco),
            explicit: false,
            isrc: None,
            release_date: None,
            popularity: None,
            added_at: chrono::Utc::now(),
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
    async fn guardar_un_album_no_duplica_a_su_artista() {
        // El camino de las pistas ya canonizaba los identificadores locales,
        // pero este escribía los suyos a pelo: un álbum bastaba para volver a
        // sembrar el «coldrain» repetido que la migración V4 acababa de limpiar.
        let (repo, _tracks, pool, _g) = repos().await;

        repo.upsert(&[album("Uno", vec![artista("Queen")])])
            .await
            .expect("guarda");
        repo.upsert(&[album("Otro", vec![artista("Queen")])])
            .await
            .expect("guarda");

        assert_eq!(cuantos_llamados(&pool, "Queen").await, 1);
    }

    #[tokio::test]
    async fn el_artista_de_un_album_se_engancha_al_del_catalogo() {
        // Es lo que hace que el álbum importado herede la foto y los géneros en
        // lugar de colgar de un duplicado mudo.
        let (repo, _tracks, pool, _g) = repos().await;
        let del_catalogo = ArtistRef {
            id: ArtistId::from_trusted("UC1WV_zQ6vd52xitFX2cANYg"),
            name: "coldrain".into(),
        };

        repo.upsert(&[album("Del catálogo", vec![del_catalogo])])
            .await
            .expect("guarda");
        let importado = album("Importado", vec![artista("coldrain")]);
        repo.upsert(std::slice::from_ref(&importado))
            .await
            .expect("guarda");

        assert_eq!(cuantos_llamados(&pool, "coldrain").await, 1);
        let leido = repo.get(&importado.id).await.expect("lee").expect("existe");
        assert_eq!(leido.artists[0].id.as_str(), "UC1WV_zQ6vd52xitFX2cANYg");
    }

    #[tokio::test]
    async fn un_artista_repetido_en_el_mismo_album_no_tumba_el_lote() {
        // Dos locales con el mismo nombre se canonizan al mismo identificador, y
        // la pareja repetida violaría la clave primaria de `album_artists`
        // abortando la transacción entera.
        let (repo, _tracks, _pool, _g) = repos().await;
        let uno = album(
            "Con crédito repetido",
            vec![artista("Casey Edwards"), artista("Casey Edwards")],
        );
        let otro = album("Acompañante", vec![artista("Victor Borba")]);

        repo.upsert(&[uno.clone(), otro.clone()])
            .await
            .expect("el lote entero tiene que guardarse");

        let leido = repo.get(&uno.id).await.expect("lee").expect("existe");
        assert_eq!(leido.artists.len(), 1);
        assert!(
            repo.get(&otro.id).await.expect("lee").is_some(),
            "el otro álbum del lote no puede perderse por culpa del primero"
        );
    }

    #[tokio::test]
    async fn un_album_guardado_se_recupera_igual() {
        let (repo, _tracks, _pool, _g) = repos().await;
        let original = album("Hot Space", vec![artista("Queen")]);
        repo.upsert(std::slice::from_ref(&original))
            .await
            .expect("guarda");

        let leido = repo.get(&original.id).await.expect("lee").expect("existe");
        assert_eq!(leido.title, "Hot Space");
        assert_eq!(leido.album_type, AlbumType::Album);
        assert_eq!(leido.release_date, original.release_date);
        assert_eq!(leido.label.as_deref(), Some("EMI"));
        assert_eq!(leido.artists.len(), 1);
        assert_eq!(leido.artists[0].name, "Queen");
    }

    #[tokio::test]
    async fn las_pistas_salen_ordenadas_por_disco_y_numero() {
        let (repo, tracks, _pool, _g) = repos().await;
        let al = album("Doble", vec![artista("Queen")]);
        repo.upsert(std::slice::from_ref(&al))
            .await
            .expect("guarda álbum");

        // Insertadas a propósito en desorden.
        tracks
            .upsert(&[
                pista_de(&al, "D2-01", 2, 1),
                pista_de(&al, "D1-02", 1, 2),
                pista_de(&al, "D1-01", 1, 1),
            ])
            .await
            .expect("guarda pistas");

        let filas = repo.tracks_of(&al.id).await.expect("lee");
        assert_eq!(
            filas.iter().map(|f| f.title.clone()).collect::<Vec<_>>(),
            vec!["D1-01", "D1-02", "D2-01"]
        );
    }

    #[tokio::test]
    async fn la_fila_de_rejilla_cuenta_pistas_totales_y_descargadas() {
        let (repo, tracks, pool, _g) = repos().await;
        let al = album("Hot Space", vec![artista("Queen"), artista("David Bowie")]);
        repo.upsert(std::slice::from_ref(&al))
            .await
            .expect("guarda");

        let pistas: Vec<Track> = (1..=3)
            .map(|i| pista_de(&al, &format!("P{i}"), 1, i))
            .collect();
        tracks.upsert(&pistas).await.expect("guarda pistas");

        let id = pistas[0].id.as_str().to_owned();
        pool.escribir(move |tx| {
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

        let pagina = repo
            .list_rows(&AlbumFilter::default(), &PageRequest::new(0, 50))
            .await
            .expect("lista");

        assert_eq!(pagina.items.len(), 1);
        let fila = &pagina.items[0];
        assert_eq!(fila.track_count, 3);
        assert_eq!(fila.local_count, 1, "solo una está en disco");
        assert_eq!(fila.artist_display, "Queen, David Bowie");
        assert_eq!(fila.year, Some(1982));
    }

    #[tokio::test]
    async fn el_filtro_local_only_excluye_albumes_sin_ninguna_pista_en_disco() {
        let (repo, tracks, _pool, _g) = repos().await;
        let al = album("Sin descargar", vec![artista("Queen")]);
        repo.upsert(std::slice::from_ref(&al))
            .await
            .expect("guarda");
        tracks
            .upsert(&[pista_de(&al, "P1", 1, 1)])
            .await
            .expect("guarda pista");

        let filtro = AlbumFilter {
            local_only: true,
            ..AlbumFilter::default()
        };
        let pagina = repo
            .list_rows(&filtro, &PageRequest::new(0, 50))
            .await
            .expect("lista");

        assert!(pagina.items.is_empty());
        assert_eq!(pagina.total, Some(0));
    }

    #[tokio::test]
    async fn marcar_la_portada_como_cacheada_se_refleja_en_la_fila() {
        let (repo, _tracks, _pool, _g) = repos().await;
        let al = album("Hot Space", vec![artista("Queen")]);
        repo.upsert(std::slice::from_ref(&al))
            .await
            .expect("guarda");

        let antes = repo
            .list_rows(&AlbumFilter::default(), &PageRequest::new(0, 10))
            .await
            .expect("lista");
        assert!(antes.items[0].cover.is_none());

        repo.set_cover_cached(&al.id, true).await.expect("marca");

        let despues = repo
            .list_rows(&AlbumFilter::default(), &PageRequest::new(0, 10))
            .await
            .expect("lista");
        assert_eq!(despues.items[0].cover.as_deref(), Some(al.id.as_str()));
    }

    #[tokio::test]
    async fn renombrar_un_album_reindexa_sus_pistas_en_fts() {
        // Verifica el trigger `albums_fts_au` de la migración V2: sin él,
        // buscar por el título nuevo no encontraría nada.
        let (repo, tracks, pool, _g) = repos().await;
        let mut al = album("Titulo Viejo", vec![artista("Queen")]);
        repo.upsert(std::slice::from_ref(&al))
            .await
            .expect("guarda");
        tracks
            .upsert(&[pista_de(&al, "Cancion", 1, 1)])
            .await
            .expect("guarda pista");

        al.title = "Titulo Nuevo".into();
        repo.upsert(std::slice::from_ref(&al))
            .await
            .expect("renombra");

        let por_nuevo: i64 = pool
            .leer(|c| {
                Ok(c.query_row(
                    "SELECT COUNT(*) FROM tracks_fts WHERE tracks_fts MATCH 'nuevo'",
                    [],
                    |r| r.get(0),
                )?)
            })
            .await
            .expect("busca");
        assert_eq!(por_nuevo, 1, "el título nuevo debe encontrarse");

        let por_viejo: i64 = pool
            .leer(|c| {
                Ok(c.query_row(
                    "SELECT COUNT(*) FROM tracks_fts WHERE tracks_fts MATCH 'viejo'",
                    [],
                    |r| r.get(0),
                )?)
            })
            .await
            .expect("busca");
        assert_eq!(
            por_viejo, 0,
            "el título viejo no debe dejar residuo en el índice"
        );
    }

    #[tokio::test]
    async fn se_encuentra_un_album_existente_por_titulo_y_artista() {
        let (repo, _tracks, _pool, _g) = repos().await;
        let al = album("Hot Space", vec![artista("Queen")]);
        repo.upsert(std::slice::from_ref(&al))
            .await
            .expect("guarda");

        let encontrado = repo
            .find_by_title_and_artist(&text::normalize("Hot Space"), &text::normalize("Queen"))
            .await
            .expect("consulta");
        assert_eq!(encontrado, Some(al.id));
    }

    #[tokio::test]
    async fn un_titulo_igual_con_otro_artista_no_se_confunde() {
        let (repo, _tracks, _pool, _g) = repos().await;
        repo.upsert(&[album("Greatest Hits", vec![artista("Queen")])])
            .await
            .expect("guarda");

        let encontrado = repo
            .find_by_title_and_artist(
                &text::normalize("Greatest Hits"),
                &text::normalize("David Bowie"),
            )
            .await
            .expect("consulta");
        assert!(encontrado.is_none());
    }

    #[tokio::test]
    async fn un_album_inexistente_no_se_encuentra() {
        let (repo, _tracks, _pool, _g) = repos().await;
        let encontrado = repo
            .find_by_title_and_artist(&text::normalize("Nada"), &text::normalize("Nadie"))
            .await
            .expect("consulta");
        assert!(encontrado.is_none());
    }
}

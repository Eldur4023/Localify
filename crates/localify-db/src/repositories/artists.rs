//! Repositorio de artistas.

use async_trait::async_trait;
use localify_core::domain::album::AlbumRow;
use localify_core::domain::artist::{Artist, ArtistRow};
use localify_core::domain::ids::ArtistId;
use localify_core::domain::track::TrackRow;
use localify_core::error::CoreResult;
use localify_core::page::{Cursor, Page, PageRequest};
use localify_core::ports::database::ArtistRepository;
use localify_core::text;
use rusqlite::params;

use crate::error::{DbResult, ToCore};
use crate::mappers::{COLUMNAS_TRACK_ROW, JOINS_TRACK_ROW, a_track_row, anyo_de};
use crate::pool::Pool;

pub struct SqliteArtistRepository {
    pool: Pool,
}

impl std::fmt::Debug for SqliteArtistRepository {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteArtistRepository")
            .finish_non_exhaustive()
    }
}

impl SqliteArtistRepository {
    #[must_use]
    pub const fn new(pool: Pool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl ArtistRepository for SqliteArtistRepository {
    async fn get(&self, id: &ArtistId) -> CoreResult<Option<Artist>> {
        let id = id.clone();
        self.pool
            .leer(move |conn| {
                let mut stmt = conn.prepare_cached(
                    "SELECT name, image_url, popularity, followers FROM artists WHERE id = ?1",
                )?;

                let base = stmt.query_row([id.as_str()], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, Option<String>>(1)?,
                        r.get::<_, Option<u8>>(2)?,
                        r.get::<_, Option<u64>>(3)?,
                    ))
                });

                let base = match base {
                    Ok(b) => b,
                    Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
                    Err(e) => return Err(e.into()),
                };

                let mut stmt = conn.prepare_cached(
                    "SELECT g.name FROM artist_genres ag
                     JOIN genres g ON g.id = ag.genre_id
                     WHERE ag.artist_id = ?1
                     ORDER BY g.name",
                )?;
                let genres = stmt
                    .query_map([id.as_str()], |r| r.get::<_, String>(0))?
                    .collect::<Result<Vec<_>, _>>()?;

                Ok(Some(Artist {
                    id: id.clone(),
                    name: base.0,
                    image_url: base.1,
                    genres,
                    popularity: base.2,
                    followers: base.3,
                }))
            })
            .await
            .to_core()
    }

    async fn upsert(&self, artists: &[Artist]) -> CoreResult<()> {
        if artists.is_empty() {
            return Ok(());
        }
        let artists = artists.to_vec();

        self.pool
            .escribir(move |tx| {
                for artista in &artists {
                    tx.execute(
                        "INSERT INTO artists (
                             id, name, name_norm, image_url, popularity, followers, metadata_at
                         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, unixepoch())
                         ON CONFLICT (id) DO UPDATE SET
                             name        = ?2,
                             name_norm   = ?3,
                             image_url   = ?4,
                             popularity  = ?5,
                             followers   = ?6,
                             metadata_at = unixepoch()",
                        params![
                            artista.id.as_str(),
                            artista.name,
                            text::normalize(&artista.name),
                            artista.image_url,
                            artista.popularity,
                            artista.followers,
                        ],
                    )?;

                    // Los géneros se normalizan a su propia tabla: Spotify los
                    // devuelve como texto repetido en miles de artistas, y
                    // guardarlos así permite consultar "todo el synth-pop de mi
                    // biblioteca" con un índice en lugar de un LIKE.
                    tx.execute(
                        "DELETE FROM artist_genres WHERE artist_id = ?1",
                        [artista.id.as_str()],
                    )?;
                    for genero in &artista.genres {
                        tx.execute(
                            "INSERT INTO genres (name) VALUES (?1) ON CONFLICT (name) DO NOTHING",
                            [genero],
                        )?;
                        tx.execute(
                            "INSERT INTO artist_genres (artist_id, genre_id)
                             VALUES (?1, (SELECT id FROM genres WHERE name = ?2))",
                            params![artista.id.as_str(), genero],
                        )?;
                    }
                }
                Ok(())
            })
            .await
            .to_core()
    }

    async fn list_rows(&self, page: &PageRequest) -> CoreResult<Page<ArtistRow>> {
        let limite = i64::from(page.limit());
        let offset = i64::from(page.offset());

        self.pool
            .leer(move |conn| {
                let total: i64 =
                    conn.query_row("SELECT COUNT(*) FROM artists", [], |r| r.get(0))?;
                let total = total.max(0).unsigned_abs();

                let mut stmt = conn.prepare_cached(
                    "SELECT ar.id, ar.name, ar.image_url,
                            (SELECT COUNT(*) FROM track_artists ta
                              WHERE ta.artist_id = ar.id) AS track_count,
                            (SELECT COUNT(*) FROM track_artists ta
                              JOIN audio_files af ON af.track_id = ta.track_id
                              WHERE ta.artist_id = ar.id) AS local_track_count
                     FROM artists ar
                     ORDER BY ar.name_norm ASC, ar.id ASC
                     LIMIT ?1 OFFSET ?2",
                )?;

                let items = stmt
                    .query_map(params![limite, offset], |r| {
                        Ok(ArtistRow {
                            id: ArtistId::from_trusted(r.get::<_, String>(0)?),
                            name: r.get(1)?,
                            image_url: r.get(2)?,
                            track_count: u32::try_from(r.get::<_, i64>(3)?).unwrap_or(0),
                            local_track_count: u32::try_from(r.get::<_, i64>(4)?).unwrap_or(0),
                        })
                    })?
                    .collect::<Result<Vec<_>, _>>()?;

                let consumidos = offset.max(0).unsigned_abs() + items.len() as u64;
                let next = (consumidos < total).then(|| Cursor::new(consumidos.to_string()));

                Ok(Page::new(items, Some(total), next))
            })
            .await
            .to_core()
    }

    async fn albums_of(&self, id: &ArtistId) -> CoreResult<Vec<AlbumRow>> {
        let id = id.as_str().to_owned();
        self.pool
            .leer(move |conn| {
                // Incluye tanto los álbumes donde figura como artista principal
                // como aquellos en los que solo aparece en alguna pista: para
                // el usuario, "los álbumes de X" son ambos.
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
                     WHERE EXISTS (SELECT 1 FROM album_artists aa
                                   WHERE aa.album_id = al.id AND aa.artist_id = ?1)
                        OR EXISTS (SELECT 1 FROM tracks t
                                   JOIN track_artists ta ON ta.track_id = t.id
                                   WHERE t.album_id = al.id AND ta.artist_id = ?1)
                     ORDER BY al.release_date DESC NULLS LAST, al.title_norm ASC",
                )?;

                let filas = stmt
                    .query_map([&id], |r| {
                        let release: Option<String> = r.get("release_date")?;
                        let cacheada: i64 = r.get("cover_cached")?;
                        let aid: String = r.get("id")?;
                        Ok(AlbumRow {
                            year: anyo_de(release.as_deref()),
                            cover: (cacheada != 0).then(|| aid.clone()),
                            id: localify_core::domain::ids::AlbumId::from_trusted(aid),
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

    async fn top_tracks_of(&self, id: &ArtistId, limit: u8) -> CoreResult<Vec<TrackRow>> {
        let id = id.as_str().to_owned();
        let columnas = COLUMNAS_TRACK_ROW;
        let joins = JOINS_TRACK_ROW;

        // El orden combina lo que el usuario ha escuchado con la popularidad de
        // Spotify. Sin el historial, la lista sería idéntica para todos; sin la
        // popularidad, una biblioteca recién creada no tendría ningún criterio.
        let sql = format!(
            "SELECT {columnas},
                    (SELECT COUNT(*) FROM play_history h WHERE h.track_id = t.id) AS reproducciones
             FROM tracks t
             {joins}
             JOIN track_artists ta ON ta.track_id = t.id
             WHERE ta.artist_id = ?1
             ORDER BY reproducciones DESC, t.popularity DESC NULLS LAST, t.title_norm ASC
             LIMIT ?2"
        );

        self.pool
            .leer(move |conn| {
                let mut stmt = conn.prepare(&sql)?;
                let filas = stmt
                    .query_map(params![id, i64::from(limit)], |row| Ok(a_track_row(row)))?
                    .collect::<Result<Vec<_>, _>>()?
                    .into_iter()
                    .collect::<DbResult<Vec<_>>>()?;
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
    use localify_core::domain::ids::{AlbumId, TrackId};
    use localify_core::domain::track::{AlbumRef, ArtistRef, Track};
    use localify_core::ports::database::{AlbumRepository, TrackRepository};

    use super::*;
    use crate::pool::TempDbGuard;
    use crate::repositories::albums::SqliteAlbumRepository;
    use crate::repositories::tracks::SqliteTrackRepository;

    struct Ctx {
        artists: SqliteArtistRepository,
        albums: SqliteAlbumRepository,
        tracks: SqliteTrackRepository,
        pool: Pool,
        _guard: TempDbGuard,
    }

    async fn ctx() -> Ctx {
        let (pool, guard) = Pool::temporal().expect("abre");
        crate::migrations::ejecutar(&pool).await.expect("migra");
        Ctx {
            artists: SqliteArtistRepository::new(pool.clone()),
            albums: SqliteAlbumRepository::new(pool.clone()),
            tracks: SqliteTrackRepository::new(pool.clone()),
            pool,
            _guard: guard,
        }
    }

    fn artista(nombre: &str, generos: &[&str]) -> Artist {
        Artist {
            id: ArtistId::nuevo_local(),
            name: nombre.into(),
            image_url: Some("https://ejemplo/a.jpg".into()),
            genres: generos.iter().map(|g| (*g).to_owned()).collect(),
            popularity: Some(90),
            followers: Some(1_000_000),
        }
    }

    #[tokio::test]
    async fn un_artista_guardado_se_recupera_con_sus_generos() {
        let c = ctx().await;
        let original = artista("Queen", &["glam rock", "classic rock"]);
        c.artists
            .upsert(std::slice::from_ref(&original))
            .await
            .expect("guarda");

        let leido = c
            .artists
            .get(&original.id)
            .await
            .expect("lee")
            .expect("existe");
        assert_eq!(leido.name, "Queen");
        assert_eq!(leido.followers, Some(1_000_000));
        let mut generos = leido.genres.clone();
        generos.sort();
        assert_eq!(generos, vec!["classic rock", "glam rock"]);
    }

    #[tokio::test]
    async fn los_generos_se_comparten_entre_artistas_sin_duplicarse() {
        let c = ctx().await;
        c.artists
            .upsert(&[
                artista("Queen", &["classic rock"]),
                artista("Bowie", &["classic rock"]),
            ])
            .await
            .expect("guarda");

        let generos: i64 = c
            .pool
            .leer(|conn| Ok(conn.query_row("SELECT COUNT(*) FROM genres", [], |r| r.get(0))?))
            .await
            .expect("cuenta");
        assert_eq!(generos, 1, "un mismo género no debe duplicarse en la tabla");

        let relaciones: i64 = c
            .pool
            .leer(
                |conn| Ok(conn.query_row("SELECT COUNT(*) FROM artist_genres", [], |r| r.get(0))?),
            )
            .await
            .expect("cuenta");
        assert_eq!(relaciones, 2);
    }

    #[tokio::test]
    async fn reescribir_un_artista_con_menos_generos_elimina_los_anteriores() {
        let c = ctx().await;
        let mut a = artista("Queen", &["glam rock", "classic rock"]);
        c.artists
            .upsert(std::slice::from_ref(&a))
            .await
            .expect("guarda");

        a.genres = vec!["classic rock".into()];
        c.artists
            .upsert(std::slice::from_ref(&a))
            .await
            .expect("reescribe");

        let leido = c.artists.get(&a.id).await.expect("lee").expect("existe");
        assert_eq!(leido.genres, vec!["classic rock"]);
    }

    #[tokio::test]
    async fn las_top_tracks_priorizan_lo_mas_escuchado() {
        let c = ctx().await;
        let a = artista("Queen", &[]);
        c.artists
            .upsert(std::slice::from_ref(&a))
            .await
            .expect("guarda artista");

        let referencia = ArtistRef {
            id: a.id.clone(),
            name: a.name.clone(),
        };
        let hacer = |titulo: &str, popularidad: u8| Track {
            id: TrackId::nuevo_local(),
            title: titulo.into(),
            album: None,
            artists: vec![referencia.clone()],
            duration: DurationMs::new(200_000),
            track_number: None,
            disc_number: None,
            explicit: false,
            isrc: None,
            release_date: None,
            popularity: Some(popularidad),
            added_at: chrono::Utc::now(),
        };

        // "Poco popular" tiene menos popularidad en Spotify pero el usuario la
        // ha escuchado: debe ganar.
        let popular = hacer("Muy popular", 95);
        let escuchada = hacer("Poco popular", 10);
        c.tracks
            .upsert(&[popular.clone(), escuchada.clone()])
            .await
            .expect("guarda pistas");

        let id = escuchada.id.as_str().to_owned();
        c.pool
            .escribir(move |tx| {
                for _ in 0..5 {
                    tx.execute(
                        "INSERT INTO play_history (track_id, ms_played, completed)
                         VALUES (?1, 200000, 1)",
                        [&id],
                    )?;
                }
                Ok(())
            })
            .await
            .expect("registra escuchas");

        let top = c.artists.top_tracks_of(&a.id, 10).await.expect("consulta");
        assert_eq!(
            top[0].title, "Poco popular",
            "el historial del usuario manda sobre la popularidad global"
        );
        assert_eq!(top[1].title, "Muy popular");
    }

    #[tokio::test]
    async fn las_top_tracks_usan_la_popularidad_cuando_no_hay_historial() {
        let c = ctx().await;
        let a = artista("Queen", &[]);
        c.artists
            .upsert(std::slice::from_ref(&a))
            .await
            .expect("guarda");

        let referencia = ArtistRef {
            id: a.id.clone(),
            name: a.name.clone(),
        };
        let hacer = |titulo: &str, popularidad: u8| Track {
            id: TrackId::nuevo_local(),
            title: titulo.into(),
            album: None,
            artists: vec![referencia.clone()],
            duration: DurationMs::new(200_000),
            track_number: None,
            disc_number: None,
            explicit: false,
            isrc: None,
            release_date: None,
            popularity: Some(popularidad),
            added_at: chrono::Utc::now(),
        };
        c.tracks
            .upsert(&[hacer("Media", 50), hacer("Alta", 95), hacer("Baja", 5)])
            .await
            .expect("guarda");

        let top = c.artists.top_tracks_of(&a.id, 10).await.expect("consulta");
        assert_eq!(
            top.iter().map(|t| t.title.clone()).collect::<Vec<_>>(),
            vec!["Alta", "Media", "Baja"]
        );
    }

    #[tokio::test]
    async fn los_albumes_incluyen_aquellos_donde_solo_colabora() {
        let c = ctx().await;
        let principal = artista("Queen", &[]);
        let invitado = artista("David Bowie", &[]);
        c.artists
            .upsert(&[principal.clone(), invitado.clone()])
            .await
            .expect("guarda artistas");

        let al = Album {
            id: AlbumId::nuevo_local(),
            title: "Hot Space".into(),
            // El álbum es solo de Queen.
            artists: vec![ArtistRef {
                id: principal.id.clone(),
                name: "Queen".into(),
            }],
            album_type: AlbumType::Album,
            release_date: chrono::NaiveDate::from_ymd_opt(1982, 5, 21),
            total_tracks: Some(1),
            cover_url: None,
            covers: CoverSet::default(),
            label: None,
        };
        c.albums
            .upsert(std::slice::from_ref(&al))
            .await
            .expect("guarda álbum");

        // Pero Bowie aparece en una de sus pistas.
        c.tracks
            .upsert(&[Track {
                id: TrackId::nuevo_local(),
                title: "Under Pressure".into(),
                album: Some(AlbumRef {
                    id: al.id.clone(),
                    title: al.title.clone(),
                }),
                artists: vec![
                    ArtistRef {
                        id: principal.id.clone(),
                        name: "Queen".into(),
                    },
                    ArtistRef {
                        id: invitado.id.clone(),
                        name: "David Bowie".into(),
                    },
                ],
                duration: DurationMs::new(248_000),
                track_number: Some(1),
                disc_number: Some(1),
                explicit: false,
                isrc: None,
                release_date: None,
                popularity: None,
                added_at: chrono::Utc::now(),
            }])
            .await
            .expect("guarda pista");

        let albumes = c.artists.albums_of(&invitado.id).await.expect("consulta");
        assert_eq!(
            albumes.len(),
            1,
            "un álbum donde colabora también es suyo para el usuario"
        );
        assert_eq!(albumes[0].title, "Hot Space");
    }

    #[tokio::test]
    async fn la_lista_de_artistas_cuenta_pistas_totales_y_locales() {
        let c = ctx().await;
        let a = artista("Queen", &[]);
        c.artists
            .upsert(std::slice::from_ref(&a))
            .await
            .expect("guarda");

        let referencia = ArtistRef {
            id: a.id.clone(),
            name: a.name.clone(),
        };
        let pistas: Vec<Track> = (0..3)
            .map(|i| Track {
                id: TrackId::nuevo_local(),
                title: format!("P{i}"),
                album: None,
                artists: vec![referencia.clone()],
                duration: DurationMs::new(100_000),
                track_number: None,
                disc_number: None,
                explicit: false,
                isrc: None,
                release_date: None,
                popularity: None,
                added_at: chrono::Utc::now(),
            })
            .collect();
        c.tracks.upsert(&pistas).await.expect("guarda");

        let id = pistas[0].id.as_str().to_owned();
        c.pool
            .escribir(move |tx| {
                tx.execute(
                    "INSERT INTO audio_files
                     (track_id, rel_path, format, codec, size_bytes, duration_ms, verified_at)
                     VALUES (?1, 'audio/aa/x.opus', 'opus', 'opus', 100, 100000, 0)",
                    [&id],
                )?;
                Ok(())
            })
            .await
            .expect("registra");

        let pagina = c
            .artists
            .list_rows(&PageRequest::new(0, 50))
            .await
            .expect("lista");
        assert_eq!(pagina.items.len(), 1);
        assert_eq!(pagina.items[0].track_count, 3);
        assert_eq!(pagina.items[0].local_track_count, 1);
    }
}

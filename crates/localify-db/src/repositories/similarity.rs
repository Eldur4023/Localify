//! Similitud entre pistas, para las recomendaciones locales.
//!
//! **Nada de esto sale a la red.** El modelo v1 es un vector disperso por pista
//! con cuatro señales ponderadas: artistas compartidos, géneros del artista,
//! mismo álbum y co-ocurrencia en las playlists del usuario.
//!
//! Se resuelve entero en SQL. Traer 50 000 pistas a memoria para calcular un
//! coseno sería absurdo cuando SQLite ya tiene los índices que hacen falta.
//!
//! El trait no expone el modelo: sustituirlo más adelante por embeddings de
//! audio o filtrado colaborativo no cambia una línea fuera de este crate.

use async_trait::async_trait;
use localify_core::domain::ids::TrackId;
use localify_core::error::CoreResult;
use localify_core::ports::database::SimilarityRepository;
use rusqlite::params;

use crate::error::ToCore;
use crate::pool::Pool;

/// Pesos de cada señal. Son **datos**, no código: ajustarlos no toca lógica.
///
/// El artista domina porque es la señal más fiable de que algo va a gustar. La
/// co-ocurrencia en playlists pesa poco de entrada, pero es la única señal que
/// refleja el criterio propio del usuario y ganará peso cuando haya historia.
mod pesos {
    pub(super) const ARTISTA: f32 = 0.45;
    pub(super) const GENERO: f32 = 0.25;
    pub(super) const ALBUM: f32 = 0.15;
    pub(super) const PLAYLIST: f32 = 0.15;
}

pub struct SqliteSimilarityRepository {
    pool: Pool,
}

impl std::fmt::Debug for SqliteSimilarityRepository {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteSimilarityRepository")
            .finish_non_exhaustive()
    }
}

impl SqliteSimilarityRepository {
    #[must_use]
    pub const fn new(pool: Pool) -> Self {
        Self { pool }
    }
}

/// Consulta de similitud respecto a un conjunto de pistas semilla.
///
/// `?1` es una lista de IDs separados por comas contra la que se compara, y
/// `?2` el límite. Se construye una sola vez porque es idéntica para "similar a
/// una pista" y "similar a una playlist": la única diferencia es el tamaño de la
/// semilla.
///
/// Cada CTE aporta una señal; la unión se agrega por pista y se ordena por peso
/// total.
///
/// ## No se exige que la canción esté descargada
///
/// La exigía, y era el motivo de que Inicio fuera un resumen de lo ya escuchado.
/// Solo hay fichero en disco de lo que ya sonó alguna vez, así que "recomiéndame
/// algo" respondía "esto que ya conoces". Una recomendación que solo puede
/// proponer lo que ya has oído no es una recomendación.
///
/// La condición se puso por inmediatez, y la inmediatez la resuelve otra cosa:
/// pulsar una canción la reproduce **siempre**, esté descargada o no. Es la
/// promesa central del proyecto, y la que hace que esta condición sobre.
const SQL_SIMILITUD: &str = "
WITH semilla(id) AS (
    SELECT value FROM json_each(?1)
),
artistas_semilla AS (
    SELECT DISTINCT ta.artist_id
    FROM track_artists ta
    JOIN semilla s ON s.id = ta.track_id
),
generos_semilla AS (
    SELECT DISTINCT ag.genre_id
    FROM artist_genres ag
    JOIN artistas_semilla a ON a.artist_id = ag.artist_id
),
albumes_semilla AS (
    SELECT DISTINCT t.album_id
    FROM tracks t
    JOIN semilla s ON s.id = t.id
    WHERE t.album_id IS NOT NULL
),
por_artista AS (
    SELECT DISTINCT ta.track_id, ?3 AS peso
    FROM track_artists ta
    JOIN artistas_semilla a ON a.artist_id = ta.artist_id
),
por_genero AS (
    SELECT DISTINCT ta.track_id, ?4 AS peso
    FROM artist_genres ag
    JOIN generos_semilla g ON g.genre_id = ag.genre_id
    JOIN track_artists ta  ON ta.artist_id = ag.artist_id
),
por_album AS (
    SELECT DISTINCT t.id AS track_id, ?5 AS peso
    FROM tracks t
    JOIN albumes_semilla al ON al.album_id = t.album_id
),
por_playlist AS (
    SELECT DISTINCT pi2.track_id, ?6 AS peso
    FROM playlist_items pi1
    JOIN semilla s          ON s.id = pi1.track_id
    JOIN playlist_items pi2 ON pi2.playlist_id = pi1.playlist_id
),
candidatos AS (
    SELECT track_id, peso FROM por_artista
    UNION ALL SELECT track_id, peso FROM por_genero
    UNION ALL SELECT track_id, peso FROM por_album
    UNION ALL SELECT track_id, peso FROM por_playlist
)
SELECT c.track_id, SUM(c.peso) AS puntuacion
FROM candidatos c
WHERE c.track_id NOT IN (SELECT id FROM semilla)
GROUP BY c.track_id
ORDER BY puntuacion DESC, c.track_id ASC
LIMIT ?2
";

/// Descubrimiento: lo del catálogo que encaja contigo y **no has puesto nunca**.
///
/// Es la única sección de Inicio que puede enseñar algo nuevo. Las demás son
/// proyecciones del historial —lo más oído, lo más reciente, tus artistas— y por
/// definición no descubren nada.
///
/// La semilla es lo que has escuchado de verdad en los últimos días, y el filtro
/// es al revés que en el resto: en vez de excluir la semilla, excluye **todo lo
/// que aparece en el historial**. Sin ese filtro esta sección volvería a ser la
/// misma lista de siempre con otro título.
const SQL_DESCUBRIR: &str = "
WITH semilla(id) AS (
    SELECT DISTINCT track_id
    FROM play_history
    WHERE played_at >= unixepoch() - (?1 * 86400)
),
artistas_semilla AS (
    SELECT DISTINCT ta.artist_id
    FROM track_artists ta
    JOIN semilla s ON s.id = ta.track_id
),
generos_semilla AS (
    SELECT DISTINCT ag.genre_id
    FROM artist_genres ag
    JOIN artistas_semilla a ON a.artist_id = ag.artist_id
),
albumes_semilla AS (
    SELECT DISTINCT t.album_id
    FROM tracks t
    JOIN semilla s ON s.id = t.id
    WHERE t.album_id IS NOT NULL
),
por_artista AS (
    SELECT DISTINCT ta.track_id, ?3 AS peso
    FROM track_artists ta
    JOIN artistas_semilla a ON a.artist_id = ta.artist_id
),
por_genero AS (
    SELECT DISTINCT ta.track_id, ?4 AS peso
    FROM artist_genres ag
    JOIN generos_semilla g ON g.genre_id = ag.genre_id
    JOIN track_artists ta  ON ta.artist_id = ag.artist_id
),
por_album AS (
    SELECT DISTINCT t.id AS track_id, ?5 AS peso
    FROM tracks t
    JOIN albumes_semilla al ON al.album_id = t.album_id
),
por_playlist AS (
    SELECT DISTINCT pi2.track_id, ?6 AS peso
    FROM playlist_items pi1
    JOIN semilla s          ON s.id = pi1.track_id
    JOIN playlist_items pi2 ON pi2.playlist_id = pi1.playlist_id
),
candidatos AS (
    SELECT track_id, peso FROM por_artista
    UNION ALL SELECT track_id, peso FROM por_genero
    UNION ALL SELECT track_id, peso FROM por_album
    UNION ALL SELECT track_id, peso FROM por_playlist
)
SELECT c.track_id, SUM(c.peso) AS puntuacion
FROM candidatos c
WHERE c.track_id NOT IN (SELECT track_id FROM play_history)
GROUP BY c.track_id
ORDER BY puntuacion DESC, c.track_id ASC
LIMIT ?2
";

/// La puntuación es una suma de cuatro pesos pequeños: la precisión de `f32`
/// sobra, y el dominio la expone así para no arrastrar `f64` por toda la API.
#[allow(clippy::cast_possible_truncation, reason = "el rango real es [0, 1]")]
fn puntuacion_a_f32(valor: f64) -> f32 {
    valor as f32
}

fn ids_a_json(ids: &[TrackId]) -> String {
    let valores: Vec<&str> = ids.iter().map(TrackId::as_str).collect();
    serde_json::to_string(&valores).unwrap_or_else(|_| "[]".to_owned())
}

#[async_trait]
impl SimilarityRepository for SqliteSimilarityRepository {
    async fn similar_to_track(
        &self,
        track: &TrackId,
        limit: u8,
    ) -> CoreResult<Vec<(TrackId, f32)>> {
        self.similar_to_set(std::slice::from_ref(track), limit)
            .await
    }

    async fn similar_to_set(
        &self,
        tracks: &[TrackId],
        limit: u8,
    ) -> CoreResult<Vec<(TrackId, f32)>> {
        if tracks.is_empty() {
            return Ok(Vec::new());
        }
        let semilla = ids_a_json(tracks);

        self.pool
            .leer(move |conn| {
                let mut stmt = conn.prepare_cached(SQL_SIMILITUD)?;
                let filas = stmt
                    .query_map(
                        params![
                            semilla,
                            i64::from(limit),
                            f64::from(pesos::ARTISTA),
                            f64::from(pesos::GENERO),
                            f64::from(pesos::ALBUM),
                            f64::from(pesos::PLAYLIST),
                        ],
                        |r| {
                            Ok((
                                TrackId::from_trusted(r.get::<_, String>(0)?),
                                puntuacion_a_f32(r.get::<_, f64>(1)?),
                            ))
                        },
                    )?
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(filas)
            })
            .await
            .to_core()
    }

    async fn because_you_listened(&self, limit: u8) -> CoreResult<Vec<(TrackId, TrackId, f32)>> {
        // Devuelve tríos (semilla, sugerencia, puntuación) para poder titular la
        // sección "Porque escuchaste X". Sin la semilla, la sección no podría
        // explicar de dónde sale la recomendación.
        self.pool
            .leer(move |conn| {
                // Las cinco pistas completadas más recientes. No se exige que
                // sigan descargadas: lo que las hace buena semilla es que se
                // escucharan enteras, no que su fichero siga ahí. Con el `JOIN`
                // a `audio_files`, vaciar las descargas dejaba esta sección sin
                // semillas y por tanto sin existir.
                let mut stmt = conn.prepare_cached(
                    "SELECT DISTINCT h.track_id
                     FROM play_history h
                     WHERE h.completed = 1
                     ORDER BY h.played_at DESC
                     LIMIT 5",
                )?;
                let semillas: Vec<String> = stmt
                    .query_map([], |r| r.get(0))?
                    .collect::<Result<Vec<_>, _>>()?;

                let mut resultado = Vec::new();
                let mut stmt = conn.prepare_cached(SQL_SIMILITUD)?;

                for semilla in semillas {
                    let json =
                        serde_json::to_string(&[&semilla]).unwrap_or_else(|_| "[]".to_owned());
                    let filas = stmt
                        .query_map(
                            params![
                                json,
                                i64::from(limit),
                                f64::from(pesos::ARTISTA),
                                f64::from(pesos::GENERO),
                                f64::from(pesos::ALBUM),
                                f64::from(pesos::PLAYLIST),
                            ],
                            |r| {
                                Ok((
                                    TrackId::from_trusted(r.get::<_, String>(0)?),
                                    puntuacion_a_f32(r.get::<_, f64>(1)?),
                                ))
                            },
                        )?
                        .collect::<Result<Vec<_>, _>>()?;

                    let origen = TrackId::from_trusted(semilla);
                    for (sugerida, puntuacion) in filas {
                        resultado.push((origen.clone(), sugerida, puntuacion));
                    }
                }
                Ok(resultado)
            })
            .await
            .to_core()
    }

    async fn discover(&self, days: u16, limit: u8) -> CoreResult<Vec<(TrackId, f32)>> {
        self.pool
            .leer(move |conn| {
                let mut stmt = conn.prepare_cached(SQL_DESCUBRIR)?;
                let filas = stmt
                    .query_map(
                        params![
                            i64::from(days),
                            i64::from(limit),
                            f64::from(pesos::ARTISTA),
                            f64::from(pesos::GENERO),
                            f64::from(pesos::ALBUM),
                            f64::from(pesos::PLAYLIST),
                        ],
                        |r| {
                            Ok((
                                TrackId::from_trusted(r.get::<_, String>(0)?),
                                puntuacion_a_f32(r.get::<_, f64>(1)?),
                            ))
                        },
                    )?
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
    use localify_core::domain::artist::Artist;
    use localify_core::domain::audio::DurationMs;
    use localify_core::domain::ids::{AlbumId, ArtistId};
    use localify_core::domain::track::{AlbumRef, ArtistRef, Track};
    use localify_core::ports::database::{AlbumRepository, ArtistRepository, TrackRepository};

    use super::*;
    use crate::pool::TempDbGuard;
    use crate::repositories::albums::SqliteAlbumRepository;
    use crate::repositories::artists::SqliteArtistRepository;
    use crate::repositories::tracks::SqliteTrackRepository;

    struct Ctx {
        sim: SqliteSimilarityRepository,
        tracks: SqliteTrackRepository,
        albums: SqliteAlbumRepository,
        artists: SqliteArtistRepository,
        pool: Pool,
        _guard: TempDbGuard,
    }

    async fn ctx() -> Ctx {
        let (pool, guard) = Pool::temporal().expect("abre");
        crate::migrations::ejecutar(&pool).await.expect("migra");
        Ctx {
            sim: SqliteSimilarityRepository::new(pool.clone()),
            tracks: SqliteTrackRepository::new(pool.clone()),
            albums: SqliteAlbumRepository::new(pool.clone()),
            artists: SqliteArtistRepository::new(pool.clone()),
            pool,
            _guard: guard,
        }
    }

    /// Registra un fichero de audio: sin él, una pista nunca se sugiere.
    async fn descargar(pool: &Pool, id: &TrackId) {
        let id = id.as_str().to_owned();
        let ruta = format!("audio/aa/{id}.opus");
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
        .expect("registra fichero");
    }

    fn pista(titulo: &str, artistas: Vec<ArtistRef>, album: Option<AlbumRef>) -> Track {
        Track {
            id: TrackId::nuevo_local(),
            title: titulo.into(),
            album,
            artists: artistas,
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

    fn ref_artista(a: &Artist) -> ArtistRef {
        ArtistRef {
            id: a.id.clone(),
            name: a.name.clone(),
        }
    }

    fn artista(nombre: &str, generos: &[&str]) -> Artist {
        Artist {
            id: ArtistId::nuevo_local(),
            name: nombre.into(),
            image_url: None,
            genres: generos.iter().map(|g| (*g).to_owned()).collect(),
            popularity: None,
            followers: None,
        }
    }

    #[tokio::test]
    async fn una_pista_del_mismo_artista_puntua_mas_que_una_del_mismo_genero() {
        let c = ctx().await;
        let queen = artista("Queen", &["glam rock"]);
        let bowie = artista("David Bowie", &["glam rock"]);
        c.artists
            .upsert(&[queen.clone(), bowie.clone()])
            .await
            .expect("guarda artistas");

        let semilla = pista("Semilla", vec![ref_artista(&queen)], None);
        let mismo_artista = pista("Mismo artista", vec![ref_artista(&queen)], None);
        let mismo_genero = pista("Mismo género", vec![ref_artista(&bowie)], None);
        c.tracks
            .upsert(&[semilla.clone(), mismo_artista.clone(), mismo_genero.clone()])
            .await
            .expect("guarda pistas");

        for t in [&semilla, &mismo_artista, &mismo_genero] {
            descargar(&c.pool, &t.id).await;
        }

        let similares = c
            .sim
            .similar_to_track(&semilla.id, 10)
            .await
            .expect("consulta");
        assert_eq!(similares.len(), 2);
        assert_eq!(similares[0].0, mismo_artista.id);
        assert!(
            similares[0].1 > similares[1].1,
            "el artista compartido debe pesar más que el género"
        );
    }

    #[tokio::test]
    async fn la_semilla_nunca_se_recomienda_a_si_misma() {
        let c = ctx().await;
        let a = artista("Queen", &[]);
        c.artists
            .upsert(std::slice::from_ref(&a))
            .await
            .expect("guarda");

        let semilla = pista("Semilla", vec![ref_artista(&a)], None);
        let otra = pista("Otra", vec![ref_artista(&a)], None);
        c.tracks
            .upsert(&[semilla.clone(), otra.clone()])
            .await
            .expect("guarda");
        descargar(&c.pool, &semilla.id).await;
        descargar(&c.pool, &otra.id).await;

        let similares = c
            .sim
            .similar_to_track(&semilla.id, 10)
            .await
            .expect("consulta");
        assert!(!similares.iter().any(|(id, _)| *id == semilla.id));
        assert_eq!(similares.len(), 1);
    }

    #[tokio::test]
    async fn se_sugiere_tambien_lo_que_no_esta_descargado() {
        // Este test decía lo contrario, y su razón —"obligaría a esperar a que
        // baje"— no se sostiene: pulsar una canción la reproduce esté o no en
        // disco, que es la promesa central del proyecto. Lo que sí provocaba era
        // que solo se pudiera recomendar lo ya escuchado, porque solo eso tiene
        // fichero, y con ello que Inicio fuera un resumen del pasado.
        let c = ctx().await;
        let a = artista("Queen", &[]);
        c.artists
            .upsert(std::slice::from_ref(&a))
            .await
            .expect("guarda");

        let semilla = pista("Semilla", vec![ref_artista(&a)], None);
        let sin_fichero = pista("Sin fichero", vec![ref_artista(&a)], None);
        c.tracks
            .upsert(&[semilla.clone(), sin_fichero.clone()])
            .await
            .expect("guarda");
        descargar(&c.pool, &semilla.id).await;

        let similares = c
            .sim
            .similar_to_track(&semilla.id, 10)
            .await
            .expect("consulta");
        assert_eq!(
            similares.iter().map(|(id, _)| id).collect::<Vec<_>>(),
            vec![&sin_fichero.id],
            "no tener el fichero no es motivo para no proponerla"
        );
    }

    #[tokio::test]
    async fn las_senales_se_acumulan() {
        let c = ctx().await;
        let queen = artista("Queen", &["glam rock"]);
        c.artists
            .upsert(std::slice::from_ref(&queen))
            .await
            .expect("guarda");

        let al = Album {
            id: AlbumId::nuevo_local(),
            title: "Hot Space".into(),
            artists: vec![ref_artista(&queen)],
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
        let ref_album = AlbumRef {
            id: al.id.clone(),
            title: al.title.clone(),
        };

        let semilla = pista(
            "Semilla",
            vec![ref_artista(&queen)],
            Some(ref_album.clone()),
        );
        // Comparte artista, género Y álbum: debe puntuar más que cualquiera de
        // las señales por separado.
        let todo = pista("Todo en común", vec![ref_artista(&queen)], Some(ref_album));
        // Solo artista.
        let solo_artista = pista("Solo artista", vec![ref_artista(&queen)], None);

        c.tracks
            .upsert(&[semilla.clone(), todo.clone(), solo_artista.clone()])
            .await
            .expect("guarda");
        for t in [&semilla, &todo, &solo_artista] {
            descargar(&c.pool, &t.id).await;
        }

        let similares = c
            .sim
            .similar_to_track(&semilla.id, 10)
            .await
            .expect("consulta");
        let por_id: std::collections::HashMap<_, _> = similares.into_iter().collect();

        assert!(
            por_id[&todo.id] > por_id[&solo_artista.id],
            "compartir álbum además del artista debe sumar"
        );
    }

    #[tokio::test]
    async fn la_coocurrencia_en_playlists_cuenta_como_senal() {
        let c = ctx().await;
        // Artistas sin nada en común: la única relación será la playlist.
        let uno = artista("Artista A", &[]);
        let otro = artista("Artista B", &[]);
        c.artists
            .upsert(&[uno.clone(), otro.clone()])
            .await
            .expect("guarda");

        let semilla = pista("Semilla", vec![ref_artista(&uno)], None);
        let compañera = pista("En la misma lista", vec![ref_artista(&otro)], None);
        let ajena = pista("Sin relación", vec![ref_artista(&otro)], None);
        c.tracks
            .upsert(&[semilla.clone(), compañera.clone(), ajena.clone()])
            .await
            .expect("guarda");
        for t in [&semilla, &compañera, &ajena] {
            descargar(&c.pool, &t.id).await;
        }

        let (s, p) = (
            semilla.id.as_str().to_owned(),
            compañera.id.as_str().to_owned(),
        );
        c.pool
            .escribir(move |tx| {
                tx.execute(
                    "INSERT INTO playlists (id, name, name_norm) VALUES ('pl1', 'Lista', 'lista')",
                    [],
                )?;
                tx.execute(
                    "INSERT INTO playlist_items (id, playlist_id, track_id, position)
                     VALUES ('e1', 'pl1', ?1, 0.0)",
                    [&s],
                )?;
                tx.execute(
                    "INSERT INTO playlist_items (id, playlist_id, track_id, position)
                     VALUES ('e2', 'pl1', ?1, 1.0)",
                    [&p],
                )?;
                Ok(())
            })
            .await
            .expect("crea playlist");

        let similares = c
            .sim
            .similar_to_track(&semilla.id, 10)
            .await
            .expect("consulta");
        let ids: Vec<_> = similares.iter().map(|(id, _)| id.clone()).collect();

        assert!(
            ids.contains(&compañera.id),
            "estar en la misma playlist es una señal"
        );
        assert!(
            !ids.contains(&ajena.id),
            "sin ninguna señal compartida no debería aparecer"
        );
    }

    #[tokio::test]
    async fn similar_a_un_conjunto_agrega_las_semillas() {
        let c = ctx().await;
        let uno = artista("A", &[]);
        let otro = artista("B", &[]);
        c.artists
            .upsert(&[uno.clone(), otro.clone()])
            .await
            .expect("guarda");

        let s1 = pista("Semilla 1", vec![ref_artista(&uno)], None);
        let s2 = pista("Semilla 2", vec![ref_artista(&otro)], None);
        let de_uno = pista("De A", vec![ref_artista(&uno)], None);
        let de_otro = pista("De B", vec![ref_artista(&otro)], None);
        c.tracks
            .upsert(&[s1.clone(), s2.clone(), de_uno.clone(), de_otro.clone()])
            .await
            .expect("guarda");
        for t in [&s1, &s2, &de_uno, &de_otro] {
            descargar(&c.pool, &t.id).await;
        }

        let similares = c
            .sim
            .similar_to_set(&[s1.id.clone(), s2.id.clone()], 10)
            .await
            .expect("consulta");
        let ids: Vec<_> = similares.iter().map(|(id, _)| id.clone()).collect();

        assert!(ids.contains(&de_uno.id));
        assert!(ids.contains(&de_otro.id));
        assert!(
            !ids.contains(&s1.id) && !ids.contains(&s2.id),
            "las semillas se excluyen"
        );
    }

    #[tokio::test]
    async fn una_semilla_vacia_no_devuelve_nada_ni_falla() {
        let c = ctx().await;
        assert!(
            c.sim
                .similar_to_set(&[], 10)
                .await
                .expect("consulta")
                .is_empty()
        );
    }

    #[tokio::test]
    async fn because_you_listened_parte_de_lo_escuchado_entero() {
        let c = ctx().await;
        let a = artista("Queen", &[]);
        c.artists
            .upsert(std::slice::from_ref(&a))
            .await
            .expect("guarda");

        let escuchada = pista("Escuchada", vec![ref_artista(&a)], None);
        let sugerible = pista("Sugerible", vec![ref_artista(&a)], None);
        c.tracks
            .upsert(&[escuchada.clone(), sugerible.clone()])
            .await
            .expect("guarda");
        descargar(&c.pool, &escuchada.id).await;
        descargar(&c.pool, &sugerible.id).await;

        let id = escuchada.id.as_str().to_owned();
        c.pool
            .escribir(move |tx| {
                tx.execute(
                    "INSERT INTO play_history (track_id, ms_played, completed)
                     VALUES (?1, 200000, 1)",
                    [&id],
                )?;
                Ok(())
            })
            .await
            .expect("registra escucha");

        let recomendaciones = c.sim.because_you_listened(5).await.expect("consulta");
        assert_eq!(recomendaciones.len(), 1);
        assert_eq!(
            recomendaciones[0].0, escuchada.id,
            "la semilla titula la sección"
        );
        assert_eq!(recomendaciones[0].1, sugerible.id);
    }

    #[tokio::test]
    async fn because_you_listened_ignora_lo_que_se_salto() {
        let c = ctx().await;
        let a = artista("Queen", &[]);
        c.artists
            .upsert(std::slice::from_ref(&a))
            .await
            .expect("guarda");

        let saltada = pista("Saltada", vec![ref_artista(&a)], None);
        let otra = pista("Otra", vec![ref_artista(&a)], None);
        c.tracks
            .upsert(&[saltada.clone(), otra.clone()])
            .await
            .expect("guarda");
        descargar(&c.pool, &saltada.id).await;
        descargar(&c.pool, &otra.id).await;

        let id = saltada.id.as_str().to_owned();
        c.pool
            .escribir(move |tx| {
                tx.execute(
                    "INSERT INTO play_history (track_id, ms_played, completed)
                     VALUES (?1, 8000, 0)",
                    [&id],
                )?;
                Ok(())
            })
            .await
            .expect("registra salto");

        let recomendaciones = c.sim.because_you_listened(5).await.expect("consulta");
        assert!(
            recomendaciones.is_empty(),
            "algo saltado a los ocho segundos no es base para recomendar"
        );
    }

    /// Registra una escucha completa de `track`.
    async fn escuchar(pool: &Pool, track: &TrackId) {
        let id = track.as_str().to_owned();
        pool.escribir(move |tx| {
            tx.execute(
                "INSERT INTO play_history (track_id, ms_played, completed)
                 VALUES (?1, 200000, 1)",
                [&id],
            )?;
            Ok(())
        })
        .await
        .expect("registra escucha");
    }

    #[tokio::test]
    async fn descubrir_excluye_todo_lo_que_ya_sono() {
        // Es la diferencia entre una recomendación y un resumen: si lo ya
        // escuchado puede volver a salir, la sección repite lo que hay en las
        // otras cinco.
        let c = ctx().await;
        let a = artista("Queen", &[]);
        c.artists
            .upsert(std::slice::from_ref(&a))
            .await
            .expect("guarda");

        let escuchada = pista("Escuchada", vec![ref_artista(&a)], None);
        let nueva = pista("Nueva", vec![ref_artista(&a)], None);
        c.tracks
            .upsert(&[escuchada.clone(), nueva.clone()])
            .await
            .expect("guarda");
        escuchar(&c.pool, &escuchada.id).await;

        let ids: Vec<_> = c
            .sim
            .discover(30, 10)
            .await
            .expect("consulta")
            .into_iter()
            .map(|(t, _)| t)
            .collect();
        assert_eq!(ids, vec![nueva.id], "solo lo que no ha sonado nunca");
    }

    #[tokio::test]
    async fn sin_historial_no_hay_nada_que_descubrir() {
        // Sin semilla no se inventa una fila al azar: Inicio omite la sección.
        let c = ctx().await;
        let a = artista("Queen", &[]);
        c.artists
            .upsert(std::slice::from_ref(&a))
            .await
            .expect("guarda");
        let t = pista("Suelta", vec![ref_artista(&a)], None);
        c.tracks
            .upsert(std::slice::from_ref(&t))
            .await
            .expect("guarda");

        assert!(
            c.sim.discover(30, 10).await.expect("consulta").is_empty(),
            "sin escuchas no hay gusto del que partir"
        );
    }
}

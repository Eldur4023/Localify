//! Conversión de los resultados de InnerTube al dominio.
//!
//! ## El subtítulo no tiene formato fijo
//!
//! La segunda columna de un resultado es una lista de tramos separados por `•`,
//! y **su composición cambia**. Estos tres vienen de la misma búsqueda:
//!
//! ```text
//! Queen • Bohemian Rhapsody (The Original Soundtrack) • 2:28
//! Álbum • Queen • 2018
//! Artista • 108 M usuarios mensuales
//! ```
//!
//! Leer por posición —"el primero es el artista, el segundo el álbum"— funciona
//! con el primero y falla con los otros dos. Y no es un caso raro: la misma
//! búsqueda de canciones devuelve unos resultados con "Canción •" delante y
//! otros sin él, según de dónde venga la pista.
//!
//! ## Por eso se clasifica por identificador, no por posición
//!
//! Cada tramo que es un enlace trae el `browseId` de aquello a lo que apunta, y
//! el prefijo dice qué es: `UC` un canal de artista, `MPRE` un álbum. Los
//! tramos sin enlace son texto suelto —el año, las reproducciones, la
//! duración— y se distinguen por su forma.
//!
//! Es más código que contar posiciones, pero es lo único que no se rompe
//! cuando YouTube añade o quita un tramo, que es algo que hace sin avisar.

use localify_core::domain::album::{Album, AlbumType, CoverSet};
use localify_core::domain::artist::Artist;
use localify_core::domain::audio::DurationMs;
use localify_core::domain::ids::{AlbumId, ArtistId, TrackId};
use localify_core::domain::track::{AlbumRef, ArtistRef, Track};
use serde_json::Value;

use crate::innertube::{
    browse_id, buscar_uno, columna, duracion_de_columna_fija, duracion_ms, elementos_de_carrusel,
    elementos_de_lista, texto_de, tramos, video_id,
};

/// Prefijo de los identificadores de canal, que en YouTube Music son artistas.
const PREFIJO_ARTISTA: &str = "UC";
/// Prefijo de los identificadores de álbum.
const PREFIJO_ALBUM: &str = "MPRE";

/// Un tramo del subtítulo, ya clasificado.
enum Tramo {
    Artista(ArtistRef),
    Album(AlbumRef),
    Duracion(DurationMs),
    /// Año de publicación: cuatro dígitos sueltos.
    Anyo(i32),
    /// Cualquier otra cosa: "Canción", "1,5 M reproducciones", separadores.
    Otro,
}

/// Clasifica un tramo por su identificador y, si no lo tiene, por su forma.
fn clasificar(texto: &str, id: Option<&str>) -> Tramo {
    if let Some(id) = id {
        if id.starts_with(PREFIJO_ARTISTA) {
            return Tramo::Artista(ArtistRef {
                id: ArtistId::from_trusted(id),
                name: texto.trim().to_owned(),
            });
        }
        if id.starts_with(PREFIJO_ALBUM) {
            return Tramo::Album(AlbumRef {
                id: AlbumId::from_trusted(id),
                title: texto.trim().to_owned(),
            });
        }
    }

    let limpio = texto.trim();
    if let Some(ms) = duracion_ms(limpio) {
        return Tramo::Duracion(DurationMs::new(ms));
    }
    // Cuatro dígitos exactos: un año. Se comprueba la longitud además del
    // parseo porque "2:28" ya lo ha cogido la duración, pero "304" no es un año
    // y "20180" tampoco.
    if limpio.len() == 4
        && let Ok(a) = limpio.parse::<i32>()
        && (1900..=2100).contains(&a)
    {
        return Tramo::Anyo(a);
    }

    Tramo::Otro
}

/// Todo lo que se ha podido sacar de un subtítulo.
#[derive(Default)]
struct Subtitulo {
    artistas: Vec<ArtistRef>,
    album: Option<AlbumRef>,
    duracion: Option<DurationMs>,
    anyo: Option<i32>,
}

fn leer_subtitulo(elemento: &Value, columnas: &[usize]) -> Subtitulo {
    let mut s = Subtitulo::default();

    // Se recorren varias columnas porque la información se reparte de forma
    // distinta según el tipo: en una canción la duración va en la columna 1 y
    // en un álbum el año también, pero en otras respuestas aparece en la 2.
    for &n in columnas {
        for (texto, id) in tramos(elemento, n) {
            match clasificar(&texto, id.as_deref()) {
                Tramo::Artista(a) => s.artistas.push(a),
                Tramo::Album(a) => s.album = s.album.or(Some(a)),
                Tramo::Duracion(d) => s.duracion = s.duracion.or(Some(d)),
                Tramo::Anyo(a) => s.anyo = s.anyo.or(Some(a)),
                Tramo::Otro => {}
            }
        }
    }
    s
}

/// Reproducciones de un resultado, tal y como las escribe YouTube Music.
///
/// ## Es un número abreviado, y aun así vale
///
/// Aquí ponía que convertir "7,3 M" daría "un número inventado" y por eso no se
/// leía. Era un error de criterio: 7,3 M **es** un número, escrito con menos
/// precisión de la que tenía. Redondearlo pierde decenas de miles de
/// reproducciones y no pierde nada de lo que hace falta, porque esto solo se usa
/// para **comparar**: entre 1,1 B y 665 K no hay ninguna duda que la precisión
/// pudiera resolver.
///
/// El precio de no leerlo era grande: sin ninguna señal de popularidad, elegir
/// entre seis canciones que se llaman igual era imposible, y buscar "judas"
/// destacaba lo que el proveedor pusiera primero.
///
/// ## Dos idiomas, dos formatos
///
/// InnerTube responde en el idioma que se le pida: `"1.1B plays"` en inglés y
/// `"2,1 M reproducciones"` en español. Cambian el separador decimal, el espacio
/// y la palabra final, así que no se puede parsear un formato y confiar: se
/// buscan las cifras y el multiplicador, y se ignora el resto.
fn reproducciones(texto: &str) -> Option<u64> {
    let limpio = texto.trim();
    // El número: dígitos con coma o punto de por medio. Se corta en cuanto
    // aparece otra cosa, que es la letra del multiplicador o un espacio.
    let cifras: String = limpio
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == ',' || *c == '.')
        .collect();
    if cifras.is_empty() {
        return None;
    }

    // Da igual cuál sea el separador decimal del idioma: solo hay uno y siempre
    // separa la parte entera de una fracción de un dígito.
    let valor: f64 = cifras.replace(',', ".").parse().ok()?;

    // La letra puede ir pegada al número o tras un espacio. `B` es "billion"
    // (mil millones) en inglés; en español YouTube Music usa `M` y `mil`.
    let resto = limpio[cifras.len()..].trim_start();
    let multiplicador = match resto.chars().next() {
        Some('K' | 'k') => 1_000.0,
        Some('M') => 1_000_000.0,
        Some('B' | 'b') => 1_000_000_000.0,
        // "mil" en español, en minúscula, frente a la M de millones.
        Some('m') if resto.starts_with("mil") => 1_000.0,
        _ => 1.0,
    };

    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "el producto es positivo y cabe de sobra en u64"
    )]
    Some((valor * multiplicador) as u64)
}

/// Traduce reproducciones a la escala 0-100 de [`Track::popularity`].
///
/// ## Logarítmica, no lineal
///
/// En lineal, todo lo que no sea un éxito mundial queda aplastado contra el
/// cero: con mil millones arriba, una canción de un millón —que es muchísimo—
/// sacaría un 0. Lo que se compara es el orden de magnitud, y ahí sí se
/// distinguen 100 K, 10 M y 1 B.
///
/// La escala llega a 100 en diez mil millones, que es más que la canción más
/// reproducida de la plataforma: así el tope no se satura y las de arriba siguen
/// pudiendo ordenarse entre ellas.
#[must_use]
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "acotado a 0..=100 antes de convertir"
)]
#[allow(
    clippy::cast_precision_loss,
    reason = "solo se usa su logaritmo; los bits bajos de un recuento de reproducciones no cambian el orden de magnitud"
)]
pub fn popularidad_de(reproducciones: u64) -> Option<u8> {
    /// Exponente al que la escala llega a 100: diez mil millones, más que la
    /// canción más reproducida. Así el tope no se satura y las de arriba
    /// siguen distinguiéndose entre ellas.
    const TECHO: f64 = 10.0;

    if reproducciones == 0 {
        return None;
    }
    let escala = (reproducciones as f64).log10() / TECHO;
    Some((escala.clamp(0.0, 1.0) * 100.0).round() as u8)
}

/// Convierte un resultado de búsqueda de canciones.
///
/// Devuelve `None` si falta el `videoId` o el título: sin identidad no hay
/// pista, y sin título no hay nada que enseñar. Todo lo demás es opcional.
#[must_use]
pub fn cancion(elemento: &Value, ahora: chrono::DateTime<chrono::Utc>) -> Option<Track> {
    let id = video_id(elemento)?;
    let titulo = columna(elemento, 0)?;
    if titulo.trim().is_empty() {
        return None;
    }

    let sub = leer_subtitulo(elemento, &[1, 2]);

    Some(Track {
        id: TrackId::from_trusted(id),
        title: titulo.trim().to_owned(),
        album: sub.album,
        artists: sub.artistas,
        // Sin duración no se puede validar nada, pero tampoco es motivo para
        // descartar la pista: se guarda a cero y el fichero descargado dirá la
        // real. Inventarla sería peor.
        duration: sub.duracion.unwrap_or(DurationMs::new(0)),
        track_number: None,
        disc_number: None,
        // YouTube Music marca lo explícito con una insignia aparte, no en el
        // subtítulo. Se deja en falso hasta leerla; decir "no" cuando no se
        // sabe es menos dañino que decir "sí".
        explicit: false,
        // No hay ISRC en este catálogo. Es la pérdida real frente a Spotify, y
        // la razón de que aquí no haga falta: el identificador ya **es** el del
        // vídeo que se va a descargar, así que no hay nada que emparejar.
        isrc: None,
        release_date: None,
        // Las reproducciones viven en la tercera columna y son la única señal de
        // popularidad que da este catálogo. Sin ellas no había forma de decidir
        // cuál de seis canciones homónimas es la que se busca.
        popularity: columna(elemento, 2)
            .as_deref()
            .and_then(reproducciones)
            .and_then(popularidad_de),
        added_at: ahora,
    })
}

/// Convierte un resultado de búsqueda de álbumes.
///
/// ## El artista casi nunca viene enlazado aquí
///
/// En la estantería de álbumes, el nombre del artista es **texto plano sin
/// `browseId`**, y no por ser artistas menores: se ha comprobado con Queen y
/// con Benson Boone, que tienen canal de sobra. YouTube Music sencillamente no
/// enlaza artistas desde ese listado.
///
/// La consecuencia es que `artists` sale vacío en la búsqueda de álbumes. No se
/// rellena con el texto porque un [`ArtistRef`] necesita identidad, y la única
/// forma de dársela sería inventarla: o un identificador nuevo por resultado
/// —que duplicaría el artista en cada búsqueda— o uno derivado del nombre, que
/// mezclaría a dos artistas homónimos en la misma fila.
///
/// El dato existe y se obtiene al abrir el álbum, que es otro endpoint y ahí sí
/// enlaza. Mientras eso no esté implementado, la búsqueda de álbumes muestra
/// título, tipo y año, y el artista aparece al entrar.
#[must_use]
pub fn album(elemento: &Value) -> Option<Album> {
    let id = browse_id(elemento)?;
    let titulo = columna(elemento, 0)?;
    if titulo.trim().is_empty() {
        return None;
    }

    let sub = leer_subtitulo(elemento, &[1, 2]);

    // El tipo va como texto traducido ("Álbum", "Single", "EP"), así que no se
    // puede comparar con una constante en un idioma. Se toma el primer tramo
    // sin enlace, que es donde va siempre, y se mapea por palabra clave; lo que
    // no se reconozca cae en álbum, que es el caso mayoritario.
    let tipo = tramos(elemento, 1)
        .first()
        .map_or(AlbumType::Album, |(t, _)| tipo_de_album(t));

    Some(Album {
        id: AlbumId::from_trusted(id),
        title: titulo.trim().to_owned(),
        artists: sub.artistas,
        album_type: tipo,
        // Solo se conoce el año, no el día. Se guarda como 1 de enero y se
        // marca así en la interfaz mostrando únicamente el año.
        release_date: sub
            .anyo
            .and_then(|a| chrono::NaiveDate::from_ymd_opt(a, 1, 1)),
        total_tracks: None,
        cover_url: miniatura(elemento),
        covers: CoverSet::default(),
        label: None,
    })
}

/// Convierte un resultado de búsqueda de artistas.
#[must_use]
pub fn artista(elemento: &Value) -> Option<Artist> {
    let id = browse_id(elemento)?;
    let nombre = columna(elemento, 0)?;
    if nombre.trim().is_empty() {
        return None;
    }

    Some(Artist {
        id: ArtistId::from_trusted(id),
        name: nombre.trim().to_owned(),
        image_url: miniatura(elemento),
        // YouTube Music no expone géneros. Es una pérdida con consecuencias: el
        // motor de recomendaciones los usa como señal principal (los hereda del
        // artista), así que con este proveedor se apoyará solo en el historial
        // y en la coocurrencia. Se deja vacío en vez de inventarlos.
        genres: Vec::new(),
        popularity: None,
        followers: None,
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// Respuestas de navegación
// ─────────────────────────────────────────────────────────────────────────────

/// Álbum completo desde su página, con artista enlazado.
///
/// A diferencia de la búsqueda, aquí el artista **sí** viene con su
/// identificador: está en `straplineTextOne`, que es la línea bajo el título.
#[must_use]
pub fn album_de_pagina(respuesta: &Value, id: &AlbumId) -> Option<Album> {
    let cabecera = buscar_uno(respuesta, "musicResponsiveHeaderRenderer")?;
    let titulo = texto_de(cabecera.get("title"))?;

    let artistas: Vec<ArtistRef> = runs_con_id(cabecera.get("straplineTextOne"))
        .into_iter()
        .filter_map(|(texto, id)| {
            let id = id?;
            id.starts_with(PREFIJO_ARTISTA).then(|| ArtistRef {
                id: ArtistId::from_trusted(id),
                name: texto.trim().to_owned(),
            })
        })
        .collect();

    // El subtítulo trae "Álbum • 2018" o similar; el mismo criterio de siempre.
    let subtitulo = runs_con_id(cabecera.get("subtitle"));
    let anyo = subtitulo
        .iter()
        .find_map(|(t, _)| match clasificar(t, None) {
            Tramo::Anyo(a) => Some(a),
            _ => None,
        });
    let tipo = subtitulo
        .first()
        .map_or(AlbumType::Album, |(t, _)| tipo_de_album(t));

    let pistas = u16::try_from(elementos_de_lista(respuesta).len()).ok();

    Some(Album {
        id: id.clone(),
        title: titulo.trim().to_owned(),
        artists: artistas,
        album_type: tipo,
        release_date: anyo.and_then(|a| chrono::NaiveDate::from_ymd_opt(a, 1, 1)),
        total_tracks: pistas,
        cover_url: miniatura(cabecera),
        covers: CoverSet::default(),
        label: None,
    })
}

/// Pistas del listado de un álbum.
///
/// Las filas de un álbum son más pobres que las de una búsqueda: no repiten el
/// artista ni el álbum en cada línea, porque ya están en la cabecera. Se pasan
/// como argumento para que las pistas salgan completas.
#[must_use]
pub fn pistas_de_album(
    respuesta: &Value,
    album: &AlbumRef,
    artistas: &[ArtistRef],
    ahora: chrono::DateTime<chrono::Utc>,
) -> Vec<Track> {
    elementos_de_lista(respuesta)
        .into_iter()
        .enumerate()
        .filter_map(|(i, e)| {
            let id = video_id(e)?;
            let titulo = columna(e, 0)?;
            if titulo.trim().is_empty() {
                return None;
            }

            // Cada fila puede traer su propio artista (los álbumes de varios
            // artistas lo hacen); si no, hereda el del álbum.
            let sub = leer_subtitulo(e, &[1, 2]);
            let propios = if sub.artistas.is_empty() {
                artistas.to_vec()
            } else {
                sub.artistas
            };

            Some(Track {
                id: TrackId::from_trusted(id),
                title: titulo.trim().to_owned(),
                album: Some(album.clone()),
                artists: propios,
                duration: duracion_de_columna_fija(e)
                    .map(DurationMs::new)
                    .or(sub.duracion)
                    .unwrap_or(DurationMs::new(0)),
                // El orden en el listado **es** el número de pista: el álbum
                // llega ordenado y no hay otro sitio donde venga el número.
                track_number: u16::try_from(i + 1).ok(),
                disc_number: None,
                explicit: false,
                isrc: None,
                release_date: None,
                popularity: None,
                added_at: ahora,
            })
        })
        .collect()
}

/// Artista desde su página.
#[must_use]
pub fn artista_de_pagina(respuesta: &Value, id: &ArtistId) -> Option<Artist> {
    let cabecera = buscar_uno(respuesta, "musicImmersiveHeaderRenderer")
        .or_else(|| buscar_uno(respuesta, "musicResponsiveHeaderRenderer"))?;
    let nombre = texto_de(cabecera.get("title"))?;

    Some(Artist {
        id: id.clone(),
        name: nombre.trim().to_owned(),
        image_url: miniatura(cabecera),
        genres: Vec::new(),
        popularity: None,
        // "108 M usuarios mensuales" no son seguidores, y convertir la cifra
        // abreviada daría un número inventado. Se deja vacío.
        followers: None,
    })
}

/// Álbumes de la discografía de un artista, sacados de sus carruseles.
#[must_use]
pub fn albumes_de_carrusel(respuesta: &Value) -> Vec<Album> {
    elementos_de_carrusel(respuesta)
        .into_iter()
        .filter_map(|e| {
            let id = e
                .pointer("/navigationEndpoint/browseEndpoint/browseId")
                .and_then(Value::as_str)?;
            // Los carruseles de un artista mezclan álbumes, sencillos, vídeos y
            // artistas parecidos. El prefijo es lo que separa unos de otros.
            if !id.starts_with(PREFIJO_ALBUM) {
                return None;
            }
            let titulo = texto_de(e.get("title"))?;
            let sub = runs_con_id(e.get("subtitle"));
            let anyo = sub.iter().find_map(|(t, _)| match clasificar(t, None) {
                Tramo::Anyo(a) => Some(a),
                _ => None,
            });

            Some(Album {
                id: AlbumId::from_trusted(id),
                title: titulo.trim().to_owned(),
                artists: Vec::new(),
                album_type: sub
                    .first()
                    .map_or(AlbumType::Album, |(t, _)| tipo_de_album(t)),
                release_date: anyo.and_then(|a| chrono::NaiveDate::from_ymd_opt(a, 1, 1)),
                total_tracks: None,
                cover_url: miniatura(e),
                covers: CoverSet::default(),
                label: None,
            })
        })
        .collect()
}

/// Pista a partir de la respuesta del reproductor.
///
/// Es la vía para un `videoId` suelto. Da menos que la búsqueda —el artista
/// llega sin identificador, porque `videoDetails` solo trae el nombre del
/// canal— pero es lo único que funciona sin conocer nada más de la pista.
#[must_use]
pub fn pista_de_reproductor(
    respuesta: &Value,
    id: &TrackId,
    ahora: chrono::DateTime<chrono::Utc>,
) -> Option<Track> {
    let detalles = respuesta.get("videoDetails")?;
    let titulo = detalles.get("title")?.as_str()?;

    // La duración viene como cadena de segundos.
    let duracion = detalles
        .get("lengthSeconds")
        .and_then(Value::as_str)
        .and_then(|s| s.parse::<u32>().ok())
        .map_or(DurationMs::new(0), |s| DurationMs::new(s * 1_000));

    let artistas = detalles
        .get("channelId")
        .and_then(Value::as_str)
        .zip(detalles.get("author").and_then(Value::as_str))
        .map(|(canal, autor)| {
            vec![ArtistRef {
                id: ArtistId::from_trusted(canal),
                name: autor.to_owned(),
            }]
        })
        .unwrap_or_default();

    Some(Track {
        id: id.clone(),
        title: titulo.trim().to_owned(),
        album: None,
        artists: artistas,
        duration: duracion,
        track_number: None,
        disc_number: None,
        explicit: false,
        isrc: None,
        release_date: None,
        popularity: None,
        added_at: ahora,
    })
}

/// Tramos de un nodo `{ runs: [...] }` con su identificador de navegación.
fn runs_con_id(nodo: Option<&Value>) -> Vec<(String, Option<String>)> {
    let Some(runs) = nodo.and_then(|n| n.get("runs")).and_then(Value::as_array) else {
        return Vec::new();
    };
    runs.iter()
        .filter_map(|r| {
            let texto = r.get("text")?.as_str()?.to_owned();
            let id = r
                .pointer("/navigationEndpoint/browseEndpoint/browseId")
                .and_then(Value::as_str)
                .map(str::to_owned);
            Some((texto, id))
        })
        .collect()
}

/// Mapea el texto del tipo de álbum, sea cual sea el idioma.
fn tipo_de_album(texto: &str) -> AlbumType {
    let t = texto.trim().to_lowercase();
    if t.starts_with("single") {
        AlbumType::Single
    } else if t.starts_with("ep") {
        // Un EP no tiene variante propia en el dominio y es más single que
        // recopilatorio: cuatro canciones de un artista, no una colección.
        AlbumType::Single
    } else if t.starts_with("recopil") || t.starts_with("compilation") {
        AlbumType::Compilation
    } else {
        AlbumType::Album
    }
}

/// URL de la miniatura de mayor tamaño que ofrezca el elemento.
/// Igual que [`miniatura`], para quien la necesite desde fuera del módulo.
#[must_use]
pub fn miniatura_publica(elemento: &Value) -> Option<String> {
    miniatura(elemento)
}

fn miniatura(elemento: &Value) -> Option<String> {
    let miniaturas = elemento
        .pointer("/thumbnail/musicThumbnailRenderer/thumbnail/thumbnails")
        .and_then(Value::as_array)?;

    // Vienen ordenadas de menor a mayor, pero confiar en el orden es
    // innecesario: se coge la de mayor anchura declarada.
    miniaturas
        .iter()
        .max_by_key(|t| t.get("width").and_then(Value::as_u64).unwrap_or(0))
        .and_then(|t| t.get("url"))
        .and_then(Value::as_str)
        .map(str::to_owned)
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "en un test, un `expect` que falla es el fallo"
)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn se_leen_las_reproducciones_en_los_dos_idiomas() {
        // Cadenas reales capturadas con `--example explorar`, en `en`/`US` y en
        // `es`/`ES`. Cambian el separador decimal, el espacio y la palabra
        // final, así que parsear un formato y confiar habría dejado el otro a
        // cero sin que nadie se enterase.
        assert_eq!(reproducciones("1.1B plays"), Some(1_100_000_000));
        assert_eq!(reproducciones("6.1M plays"), Some(6_100_000));
        assert_eq!(reproducciones("221K plays"), Some(221_000));
        assert_eq!(reproducciones("2,1 M reproducciones"), Some(2_100_000));
        assert_eq!(reproducciones("665 K reproducciones"), Some(665_000));
        assert_eq!(reproducciones("348 reproducciones"), Some(348));
    }

    #[test]
    fn lo_que_no_son_reproducciones_no_se_lee() {
        // La tercera columna no siempre las trae: en algunos resultados no hay
        // nada, y en otros hay otra cosa.
        assert_eq!(reproducciones(""), None);
        assert_eq!(reproducciones("Album"), None);
        assert_eq!(reproducciones("Single • 2026"), None);
    }

    #[test]
    fn la_popularidad_ordena_por_orden_de_magnitud() {
        // Lo que importa es que el orden se conserve. En escala lineal, todo lo
        // que no fuera un éxito mundial quedaría aplastado contra el cero.
        let gaga = popularidad_de(1_100_000_000).expect("hay reproducciones");
        let mediana = popularidad_de(6_100_000).expect("hay reproducciones");
        let pequena = popularidad_de(221_000).expect("hay reproducciones");

        assert!(gaga > mediana, "{gaga} debería superar a {mediana}");
        assert!(mediana > pequena, "{mediana} debería superar a {pequena}");
        assert!(
            pequena > 0,
            "doscientas mil reproducciones no pueden puntuar cero"
        );
        assert!(gaga <= 100);
    }

    #[test]
    fn sin_reproducciones_no_hay_popularidad() {
        // Cero no es "poco popular", es "no se sabe". Guardarlo como 0 pondría
        // a la misma altura lo desconocido y lo que nadie escucha.
        assert_eq!(popularidad_de(0), None);
    }

    /// Construye un elemento con la forma que devuelve InnerTube.
    ///
    /// Los datos son los de una respuesta real capturada con el ejemplo
    /// `explorar`; inventarlos habría sido codificar mis suposiciones sobre el
    /// formato en vez de el formato.
    fn elemento(columnas: Vec<Vec<(&str, Option<&str>)>>, video: Option<&str>) -> Value {
        let flex: Vec<Value> = columnas
            .into_iter()
            .map(|runs| {
                let runs: Vec<Value> = runs
                    .into_iter()
                    .map(|(texto, id)| match id {
                        Some(i) => json!({
                            "text": texto,
                            "navigationEndpoint": { "browseEndpoint": { "browseId": i } }
                        }),
                        None => json!({ "text": texto }),
                    })
                    .collect();
                json!({
                    "musicResponsiveListItemFlexColumnRenderer": { "text": { "runs": runs } }
                })
            })
            .collect();

        let mut v = json!({ "flexColumns": flex });
        if let Some(id) = video {
            v["playlistItemData"] = json!({ "videoId": id });
        }
        v
    }

    fn ahora() -> chrono::DateTime<chrono::Utc> {
        chrono::Utc::now()
    }

    #[test]
    fn una_cancion_real_se_convierte_entera() {
        let e = elemento(
            vec![
                vec![("Bohemian Rhapsody (Live)", None)],
                vec![
                    ("Queen", Some("UCEPMVbUzImPl4p8k4LkGevA")),
                    (" • ", None),
                    ("Queen Rock Montreal", Some("MPREb_Pe45z4cBo4l")),
                    (" • ", None),
                    ("5:29", None),
                ],
                vec![("1,5 M reproducciones", None)],
            ],
            Some("6Wg1_YOfiM0"),
        );

        let t = cancion(&e, ahora()).expect("se convierte");
        assert_eq!(t.id.as_str(), "6Wg1_YOfiM0");
        assert_eq!(t.title, "Bohemian Rhapsody (Live)");
        assert_eq!(t.artists.len(), 1);
        assert_eq!(t.artists[0].name, "Queen");
        assert_eq!(
            t.album.as_ref().expect("hay álbum").title,
            "Queen Rock Montreal"
        );
        assert_eq!(t.duration.as_ms(), 329_000);
    }

    #[test]
    fn el_orden_de_los_tramos_no_importa() {
        // Es lo que compra clasificar por identificador: si YouTube pone el
        // álbum antes que el artista, o mete un tramo nuevo en medio, el
        // resultado es el mismo. Leyendo por posición, no.
        let e = elemento(
            vec![
                vec![("Una canción", None)],
                vec![
                    ("Canción", None),
                    (" • ", None),
                    ("3:05", None),
                    (" • ", None),
                    ("El Álbum", Some("MPREb_algo1234")),
                    (" • ", None),
                    ("El Artista", Some("UCabcdefghijklmnopqrstu")),
                ],
            ],
            Some("abcdefghijk"),
        );

        let t = cancion(&e, ahora()).expect("se convierte");
        assert_eq!(t.artists[0].name, "El Artista");
        assert_eq!(t.album.expect("hay álbum").title, "El Álbum");
        assert_eq!(t.duration.as_ms(), 185_000);
    }

    #[test]
    fn varios_artistas_se_recogen_todos() {
        let e = elemento(
            vec![
                vec![("Colaboración", None)],
                vec![
                    ("Uno", Some("UCaaaaaaaaaaaaaaaaaaaaa")),
                    (" y ", None),
                    ("Dos", Some("UCbbbbbbbbbbbbbbbbbbbbb")),
                    (" • ", None),
                    ("4:00", None),
                ],
            ],
            Some("abcdefghijk"),
        );

        let t = cancion(&e, ahora()).expect("se convierte");
        assert_eq!(t.artists.len(), 2);
        assert_eq!(t.artists[1].name, "Dos");
    }

    #[test]
    fn una_cancion_sin_album_sigue_siendo_una_cancion() {
        // Pasa con los sencillos y con lo subido por el propio artista. Que
        // falte el álbum no puede descartar la pista.
        let e = elemento(
            vec![
                vec![("Suelta", None)],
                vec![
                    ("Alguien", Some("UCaaaaaaaaaaaaaaaaaaaaa")),
                    (" • ", None),
                    ("2:10", None),
                ],
            ],
            Some("abcdefghijk"),
        );

        let t = cancion(&e, ahora()).expect("se convierte");
        assert!(t.album.is_none());
        assert_eq!(t.duration.as_ms(), 130_000);
    }

    #[test]
    fn sin_video_id_no_hay_pista() {
        // Sin identidad no se puede ni guardar ni descargar: descartarla es lo
        // único correcto.
        let e = elemento(vec![vec![("Algo", None)], vec![("X", None)]], None);
        assert!(cancion(&e, ahora()).is_none());
    }

    #[test]
    fn las_reproducciones_no_se_confunden_con_una_duracion() {
        let e = elemento(
            vec![
                vec![("Tema", None)],
                vec![("Alguien", Some("UCaaaaaaaaaaaaaaaaaaaaa"))],
                vec![("304 M reproducciones", None)],
            ],
            Some("abcdefghijk"),
        );
        let t = cancion(&e, ahora()).expect("se convierte");
        assert_eq!(
            t.duration.as_ms(),
            0,
            "mejor cero que una duración inventada"
        );
    }

    #[test]
    fn un_album_lee_tipo_artista_y_anyo() {
        let e = elemento(
            vec![
                vec![("Bohemian Rhapsody (The Original Soundtrack)", None)],
                vec![
                    ("Álbum", None),
                    (" • ", None),
                    ("Queen", Some("UCEPMVbUzImPl4p8k4LkGevA")),
                    (" • ", None),
                    ("2018", None),
                ],
            ],
            None,
        );
        let mut e = e;
        e["navigationEndpoint"] = json!({ "browseEndpoint": { "browseId": "MPREb_m2xZZHGzRl1" } });

        let a = album(&e).expect("se convierte");
        assert_eq!(a.id.as_str(), "MPREb_m2xZZHGzRl1");
        assert_eq!(a.album_type, AlbumType::Album);
        assert_eq!(a.artists[0].name, "Queen");
        assert_eq!(
            a.release_date.expect("hay fecha").format("%Y").to_string(),
            "2018"
        );
    }

    #[test]
    fn un_single_no_se_confunde_con_un_album() {
        let mut e = elemento(
            vec![
                vec![("Algo", None)],
                vec![("Single", None), (" • ", None), ("2026", None)],
            ],
            None,
        );
        e["navigationEndpoint"] = json!({ "browseEndpoint": { "browseId": "MPREb_xcENJfhKFTF" } });
        assert_eq!(
            album(&e).expect("se convierte").album_type,
            AlbumType::Single
        );
    }

    #[test]
    fn el_anyo_no_se_confunde_con_otros_numeros() {
        // "304" o "20180" no son años; aceptarlos daría fechas absurdas.
        assert!(matches!(clasificar("304", None), Tramo::Otro));
        assert!(matches!(clasificar("20180", None), Tramo::Otro));
        assert!(matches!(clasificar("1975", None), Tramo::Anyo(1975)));
    }

    #[test]
    fn un_artista_se_convierte_sin_generos() {
        let mut e = elemento(
            vec![
                vec![("Queen", None)],
                vec![
                    ("Artista", None),
                    (" • ", None),
                    ("108 M usuarios mensuales", None),
                ],
            ],
            None,
        );
        e["navigationEndpoint"] =
            json!({ "browseEndpoint": { "browseId": "UCEPMVbUzImPl4p8k4LkGevA" } });

        let a = artista(&e).expect("se convierte");
        assert_eq!(a.name, "Queen");
        assert!(
            a.genres.is_empty(),
            "este catálogo no los da; no se inventan"
        );
    }

    #[test]
    fn la_miniatura_elegida_es_la_mayor() {
        let e = json!({
            "flexColumns": [],
            "thumbnail": { "musicThumbnailRenderer": { "thumbnail": { "thumbnails": [
                { "url": "chica", "width": 60, "height": 60 },
                { "url": "grande", "width": 544, "height": 544 },
                { "url": "media", "width": 226, "height": 226 }
            ]}}}
        });
        assert_eq!(miniatura(&e).as_deref(), Some("grande"));
    }
}

//! El proveedor entero contra YouTube Music de verdad.
//!
//! `#[ignore]` porque sale a la red. Los tests unitarios comprueban la lógica
//! con fixtures; estos comprueban lo único que un fixture no puede: que la API
//! sigue devolviendo lo que el parser espera.
//!
//! ```text
//! cargo test -p localify-ytmusic --test provider_real -- --ignored --nocapture
//! ```

#![allow(clippy::expect_used, clippy::print_stdout, clippy::panic)]

use localify_core::domain::ids::{AlbumId, ArtistId, TrackId};
use localify_core::ports::metadata_provider::MetadataProvider;
use localify_ytmusic::YtMusicProvider;

/// Identificadores estables usados en los tests.
///
/// Son de Queen a propósito: llevan décadas en el catálogo y no van a
/// desaparecer, que es lo que se necesita de un dato fijo en un test que sale a
/// la red.
const ALBUM_NIGHT_AT_THE_OPERA: &str = "MPREb_m2xZZHGzRl1";
const ARTISTA_QUEEN: &str = "UCEPMVbUzImPl4p8k4LkGevA";

fn proveedor() -> YtMusicProvider {
    YtMusicProvider::nuevo("es", "ES").expect("cliente")
}

#[tokio::test]
#[ignore = "sale a la red: ejecutar con --ignored"]
async fn buscar_devuelve_pistas_completas() {
    let p = proveedor();
    let pagina = p
        .search_tracks("bohemian rhapsody", 10, 0)
        .await
        .expect("busca");

    assert!(!pagina.items.is_empty(), "la búsqueda no devolvió nada");
    println!("{} pistas", pagina.items.len());

    // Lo que importa no es que haya resultados sino que estén **completos**:
    // sin identificador no se puede descargar, sin duración no se puede
    // validar, y sin artista la biblioteca queda inservible.
    let con_artista = pagina
        .items
        .iter()
        .filter(|t| !t.artists.is_empty())
        .count();
    let con_duracion = pagina
        .items
        .iter()
        .filter(|t| t.duration.as_ms() > 0)
        .count();

    for t in pagina.items.iter().take(3) {
        println!(
            "  {} · {} · {} ms",
            t.title,
            t.artists.first().map_or("—", |a| a.name.as_str()),
            t.duration.as_ms()
        );
    }

    assert_eq!(
        con_artista,
        pagina.items.len(),
        "toda pista debe traer artista"
    );
    assert_eq!(
        con_duracion,
        pagina.items.len(),
        "toda pista debe traer duración"
    );
}

#[tokio::test]
#[ignore = "sale a la red: ejecutar con --ignored"]
async fn el_limite_y_el_desplazamiento_se_respetan() {
    let p = proveedor();
    let cinco = p.search_tracks("queen", 5, 0).await.expect("busca");
    assert_eq!(cinco.items.len(), 5);

    let desde_tres = p.search_tracks("queen", 5, 3).await.expect("busca");
    assert!(!desde_tres.items.is_empty());
    assert_ne!(
        cinco.items[0].id, desde_tres.items[0].id,
        "el desplazamiento tiene que mover la ventana"
    );
}

#[tokio::test]
#[ignore = "sale a la red: ejecutar con --ignored"]
async fn un_album_trae_cabecera_y_pistas_numeradas() {
    let p = proveedor();
    let id = AlbumId::from_trusted(ALBUM_NIGHT_AT_THE_OPERA);

    let album = p.album(&id).await.expect("álbum");
    println!("{} ({:?})", album.title, album.album_type);
    assert!(!album.title.is_empty());
    assert!(
        !album.artists.is_empty(),
        "en la página del álbum el artista SÍ viene enlazado, \
         a diferencia de en la búsqueda"
    );

    let pistas = p.album_tracks(&id).await.expect("pistas");
    println!("  {} pistas", pistas.len());
    assert!(pistas.len() > 5);

    for t in pistas.iter().take(3) {
        println!(
            "   {:?} · {} · {} ms",
            t.track_number,
            t.title,
            t.duration.as_ms()
        );
    }

    // El número de pista sale del orden del listado: si se perdiera, el álbum
    // se mostraría desordenado y no habría forma de notarlo salvo mirando.
    assert_eq!(pistas[0].track_number, Some(1));
    assert!(pistas.iter().all(|t| t.album.is_some()));
    assert!(
        pistas.iter().all(|t| t.duration.as_ms() > 0),
        "la duración de un álbum va en columna fija, no en la flexible"
    );
}

#[tokio::test]
#[ignore = "sale a la red: ejecutar con --ignored"]
async fn un_artista_trae_nombre_canciones_y_discografia() {
    let p = proveedor();
    let id = ArtistId::from_trusted(ARTISTA_QUEEN);

    let artista = p.artist(&id).await.expect("artista");
    println!("{}", artista.name);
    assert_eq!(artista.name, "Queen");

    let top = p.artist_top_tracks(&id).await.expect("top");
    println!("  {} canciones destacadas", top.len());
    assert!(!top.is_empty());

    let albumes = p.artist_albums(&id).await.expect("álbumes");
    println!("  {} álbumes", albumes.len());
    for a in albumes.iter().take(3) {
        println!(
            "   {} ({:?})",
            a.title,
            a.release_date.map(|d| d.format("%Y").to_string())
        );
    }
    assert!(!albumes.is_empty());
}

#[tokio::test]
#[ignore = "sale a la red: ejecutar con --ignored"]
async fn una_pista_por_identificador_se_recupera() {
    let p = proveedor();
    // Se busca primero para no fijar un videoId en el test: los vídeos sí
    // desaparecen del catálogo, y un identificador quemado convertiría este
    // test en una alarma falsa dentro de dos años.
    let pagina = p
        .search_tracks("bohemian rhapsody queen", 1, 0)
        .await
        .expect("busca");
    let id: TrackId = pagina.items.first().expect("hay resultado").id.clone();

    let pista = p.track(&id).await.expect("pista");
    println!("{} · {} ms", pista.title, pista.duration.as_ms());
    assert_eq!(pista.id, id);
    assert!(!pista.title.is_empty());
    assert!(pista.duration.as_ms() > 0);
}

#[tokio::test]
#[ignore = "sale a la red: ejecutar con --ignored"]
async fn el_estado_es_operativo_sin_configurar_nada() {
    // Es la diferencia práctica con Spotify: nadie tiene que crear una
    // aplicación en ningún sitio para poder buscar.
    let estado = proveedor().status().await;
    assert!(estado.esta_operativo(), "estado: {estado:?}");
}

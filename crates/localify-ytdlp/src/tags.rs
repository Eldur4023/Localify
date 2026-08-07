//! Escritura y lectura de etiquetas.
//!
//! Etiquetar hace la biblioteca **portable**: sigue siendo válida aunque se
//! borre la base de datos, se abre en cualquier reproductor, y permite
//! reconstruir el catálogo escaneando la carpeta.
//!
//! ## Cómo se recupera la identidad de un fichero
//!
//! Por dos vías, y hacen falta las dos:
//!
//! 1. **El nombre del fichero.** Por construcción es
//!    `audio/<shard>/<track_id>.<ext>`, así que el identificador está siempre
//!    ahí. Es la vía fiable.
//! 2. **La etiqueta `LOCALIFY_SPOTIFY_ID`.** Sobrevive a que el usuario mueva o
//!    renombre el fichero, que es justo cuando la primera vía falla.
//!
//! La segunda tiene una limitación conocida: **ID3v2 no admite claves
//! personalizadas a través de la API genérica de `lofty`**, que las descarta al
//! convertir a marcos porque un identificador de marco ID3v2 tiene exactamente
//! cuatro caracteres. Se escribe igualmente —funciona en Vorbis comments, que es
//! el formato de nuestros ficheros Opus, y en MP4— y para MP3 queda la primera
//! vía. Se documenta aquí en lugar de fingir que funciona en todas partes.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use localify_core::domain::track::Track;
use localify_core::error::CoreResult;
use localify_core::ports::youtube::TagWriter;
use lofty::config::WriteOptions;
use lofty::file::{AudioFile, TaggedFileExt};
use lofty::picture::{Picture, PictureType};
use lofty::probe::Probe;
use lofty::tag::{Accessor, ItemKey, ItemValue, Tag, TagItem};
use tracing::{debug, warn};

use crate::error::{YtDlpError, YtDlpResult};

/// Clave del identificador de Spotify en las etiquetas.
///
/// Es la que permite a `rescan` recuperar la identidad de un fichero sin
/// adivinar.
pub const CLAVE_ID: &str = "LOCALIFY_SPOTIFY_ID";

/// Etiquetador basado en `lofty`.
#[derive(Debug, Clone, Default)]
pub struct EtiquetadorLofty;

impl EtiquetadorLofty {
    #[must_use]
    pub const fn nuevo() -> Self {
        Self
    }
}

/// Escribe las etiquetas de forma síncrona.
///
/// `lofty` es síncrono y toca el disco, así que la llamada asíncrona lo
/// traslada al pool bloqueante: escribir una portada de medio megabyte no debe
/// ocupar un hilo del runtime.
fn escribir_sincrono(ruta: &Path, track: &Track, portada: Option<Vec<u8>>) -> YtDlpResult<()> {
    let mut fichero = Probe::open(ruta)
        .map_err(|e| YtDlpError::VerificacionFallida(format!("no se pudo abrir: {e}")))?
        .read()
        .map_err(|e| YtDlpError::VerificacionFallida(format!("no se pudo leer: {e}")))?;

    let tipo = fichero.primary_tag_type();
    if fichero.primary_tag().is_none() {
        fichero.insert_tag(Tag::new(tipo));
    }
    let Some(tag) = fichero.primary_tag_mut() else {
        return Err(YtDlpError::VerificacionFallida(
            "el contenedor no admite etiquetas".to_owned(),
        ));
    };

    tag.set_title(track.title.clone());
    tag.set_artist(track.artist_display());

    if let Some(album) = &track.album {
        tag.set_album(album.title.clone());
    }
    if let Some(numero) = track.track_number {
        tag.set_track(u32::from(numero));
    }
    if let Some(disco) = track.disc_number {
        tag.set_disk(u32::from(disco));
    }
    if let Some(fecha) = track.release_date {
        use chrono::Datelike;
        let anyo = fecha.year();
        if anyo > 0 {
            tag.set_year(anyo.unsigned_abs());
            // La fecha completa va como texto ISO: es lo que entienden todos
            // los reproductores, y evita depender del tipo `Timestamp` de
            // lofty, que cambia entre versiones.
            tag.insert(TagItem::new(
                ItemKey::RecordingDate,
                ItemValue::Text(fecha.format("%Y-%m-%d").to_string()),
            ));
        }
    }
    if let Some(isrc) = &track.isrc {
        tag.insert(TagItem::new(ItemKey::Isrc, ItemValue::Text(isrc.clone())));
    }

    // El artista del álbum es el principal, no la lista completa: es lo que
    // agrupa correctamente los recopilatorios en cualquier reproductor.
    if let Some(principal) = track.artista_principal() {
        tag.insert(TagItem::new(
            ItemKey::AlbumArtist,
            ItemValue::Text(principal.name.clone()),
        ));
    }

    // La marca que hace reconstruible la biblioteca.
    tag.insert(TagItem::new(
        ItemKey::Unknown(CLAVE_ID.to_owned()),
        ItemValue::Text(track.id.as_str().to_owned()),
    ));

    if let Some(bytes) = portada {
        // `from_reader` detecta el formato por su firma: si los bytes no son
        // una imagen reconocible, falla en lugar de incrustar basura.
        match Picture::from_reader(&mut bytes.as_slice()) {
            Ok(mut imagen) => {
                imagen.set_pic_type(PictureType::CoverFront);
                tag.push_picture(imagen);
            }
            // Una portada ilegible no debe impedir que la pista entre en la
            // biblioteca: la música importa más que la carátula.
            Err(e) => warn!(error = %e, "no se pudo incrustar la portada"),
        }
    }

    fichero
        .save_to_path(ruta, WriteOptions::default())
        .map_err(|e| YtDlpError::VerificacionFallida(format!("no se pudo etiquetar: {e}")))?;

    Ok(())
}

/// Lee el identificador de Spotify de las etiquetas de un fichero.
fn leer_id_de_etiquetas(ruta: &Path) -> Option<String> {
    let fichero = Probe::open(ruta).ok()?.read().ok()?;

    // Se miran todas las etiquetas, no solo la principal: un fichero puede
    // llevar ID3v2 y APE a la vez, y la marca podría estar en cualquiera.
    fichero
        .tags()
        .iter()
        .find_map(|tag| {
            tag.get_string(&ItemKey::Unknown(CLAVE_ID.to_owned()))
                .map(str::to_owned)
        })
        .filter(|v| !v.is_empty())
}

/// Extrae el identificador del nombre del fichero.
///
/// Por construcción, todo fichero de la biblioteca se llama
/// `<track_id>.<ext>`. Es la vía fiable, y la única que funciona en formatos
/// cuyas etiquetas no admiten claves personalizadas.
#[must_use]
pub fn id_desde_nombre(ruta: &Path) -> Option<String> {
    let nombre = ruta.file_stem()?.to_str()?;
    // `<id>.opus.part` deja `<id>.opus` como raíz: hay que quitar otra capa.
    let nombre = Path::new(nombre)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(nombre);

    // La regla de qué es un identificador vive en el dominio y **no se copia
    // aquí**. Tenerla duplicada fue exactamente lo que hizo que esta función se
    // quedara pidiendo base62 de 22 cuando el catálogo pasó a admitir también
    // identificadores de YouTube: ningún fichero de ese origen se habría
    // recuperado nunca, y el fallo es invisible hasta que alguien restaura una
    // copia vieja de la base de datos.
    //
    // Lo que no tenga forma de identificador es un fichero que no puso
    // Localify, y adivinar sería peor que no responder.
    localify_core::domain::ids::tiene_forma_de_id(nombre).then(|| nombre.to_owned())
}

/// Recupera la identidad de un fichero por cualquiera de las dos vías.
fn leer_id_sincrono(ruta: &Path) -> Option<String> {
    // Las etiquetas primero: sobreviven a que el usuario renombre el fichero,
    // que es justo cuando el nombre deja de servir.
    leer_id_de_etiquetas(ruta).or_else(|| id_desde_nombre(ruta))
}

#[async_trait]
impl TagWriter for EtiquetadorLofty {
    async fn write(&self, path: &Path, track: &Track, cover: Option<&[u8]>) -> CoreResult<()> {
        let ruta: PathBuf = path.to_path_buf();
        let pista = track.clone();
        let portada = cover.map(<[u8]>::to_vec);

        tokio::task::spawn_blocking(move || escribir_sincrono(&ruta, &pista, portada))
            .await
            .map_err(|e| {
                localify_core::error::CoreError::internal(format!("el etiquetado se cayó: {e}"))
            })??;

        debug!(pista = %track.id, "etiquetas escritas");
        Ok(())
    }

    async fn read_track_id(&self, path: &Path) -> CoreResult<Option<String>> {
        let ruta = path.to_path_buf();

        let id = tokio::task::spawn_blocking(move || leer_id_sincrono(&ruta))
            .await
            .map_err(|e| {
                localify_core::error::CoreError::internal(format!("la lectura se cayó: {e}"))
            })?;

        Ok(id)
    }
}

#[cfg(test)]
mod tests {
    use localify_core::domain::audio::DurationMs;
    use localify_core::domain::ids::{AlbumId, ArtistId, TrackId};
    use localify_core::domain::track::{AlbumRef, ArtistRef};

    use super::*;

    /// Crea un MP3 mínimo pero válido.
    ///
    /// Se usa MP3 y no Opus porque generar un fichero Opus válido a mano es
    /// mucho más trabajo, y lo que se prueba aquí es el etiquetado, que es
    /// independiente del contenedor.
    fn mp3_de_prueba(nombre: &str) -> PathBuf {
        let dir = std::env::temp_dir().join("localify-test-tags");
        let _ = std::fs::create_dir_all(&dir);
        let ruta = dir.join(nombre);

        // Una trama MPEG-1 Layer III, 128 kbps, 44.1 kHz, repetida.
        let cabecera = [0xFF_u8, 0xFB, 0x90, 0x00];
        let mut datos = Vec::with_capacity(417 * 40);
        for _ in 0..40 {
            datos.extend_from_slice(&cabecera);
            datos.extend_from_slice(&[0_u8; 413]);
        }
        std::fs::write(&ruta, &datos).expect("escribe");
        ruta
    }

    fn pista() -> Track {
        Track {
            id: TrackId::from_trusted("3z8h0TU7ReDPLIbEnYhWZb"),
            title: "Under Pressure".into(),
            album: Some(AlbumRef {
                id: AlbumId::from_trusted("1GbtB4zTqAsyfZEsm1RZfx"),
                title: "Hot Space".into(),
            }),
            artists: vec![
                ArtistRef {
                    id: ArtistId::from_trusted("1dfeR4HaWDbWqFHLkxsg1d"),
                    name: "Queen".into(),
                },
                ArtistRef {
                    id: ArtistId::from_trusted("0oSGxfWSnnOXhD2fKuz2Gy"),
                    name: "David Bowie".into(),
                },
            ],
            duration: DurationMs::new(248_000),
            track_number: Some(11),
            disc_number: Some(1),
            explicit: false,
            isrc: Some("GBUM71029604".into()),
            release_date: chrono::NaiveDate::from_ymd_opt(1982, 5, 21),
            popularity: Some(80),
            added_at: chrono::Utc::now(),
        }
    }

    #[tokio::test]
    async fn las_etiquetas_hacen_ida_y_vuelta() {
        let ruta = mp3_de_prueba("etiquetado.mp3");
        let etiquetador = EtiquetadorLofty::nuevo();
        let p = pista();

        etiquetador.write(&ruta, &p, None).await.expect("etiqueta");

        let fichero = Probe::open(&ruta).expect("abre").read().expect("lee");
        let tag = fichero.primary_tag().expect("hay etiqueta");

        assert_eq!(tag.title().as_deref(), Some("Under Pressure"));
        assert_eq!(tag.artist().as_deref(), Some("Queen, David Bowie"));
        assert_eq!(tag.album().as_deref(), Some("Hot Space"));
        assert_eq!(tag.track(), Some(11));
        assert_eq!(tag.year(), Some(1982));

        let _ = std::fs::remove_file(ruta);
    }

    #[tokio::test]
    async fn el_identificador_se_recupera_del_nombre_del_fichero() {
        // Es la vía fiable: el nombre lo pone Localify y siempre es el ID.
        let ruta = mp3_de_prueba("3z8h0TU7ReDPLIbEnYhWZb.mp3");
        let etiquetador = EtiquetadorLofty::nuevo();

        etiquetador
            .write(&ruta, &pista(), None)
            .await
            .expect("etiqueta");

        let leido = etiquetador.read_track_id(&ruta).await.expect("lee");
        assert_eq!(leido.as_deref(), Some("3z8h0TU7ReDPLIbEnYhWZb"));

        let _ = std::fs::remove_file(ruta);
    }

    #[test]
    fn el_nombre_de_fichero_solo_se_acepta_si_parece_un_id() {
        // Adivinar sería peor que no responder: un fichero que no puso
        // Localify no debe asociarse a ninguna pista.
        assert_eq!(
            id_desde_nombre(Path::new("3z8h0TU7ReDPLIbEnYhWZb.opus")).as_deref(),
            Some("3z8h0TU7ReDPLIbEnYhWZb")
        );
        assert_eq!(
            id_desde_nombre(Path::new("audio/3z/3z8h0TU7ReDPLIbEnYhWZb.opus.part")).as_deref(),
            Some("3z8h0TU7ReDPLIbEnYhWZb"),
            "el sufijo .part no debe estorbar"
        );

        for nombre in [
            "Queen - Under Pressure.mp3",
            "corto.opus",
            "3z8h0TU7ReDPLIbEnYhW$b.opus",
            "sin-extension",
        ] {
            assert!(
                id_desde_nombre(Path::new(nombre)).is_none(),
                "'{nombre}' no debería reconocerse como identificador"
            );
        }
    }

    #[tokio::test]
    async fn id3v2_no_admite_claves_personalizadas_y_queda_documentado() {
        // Limitación conocida de la API genérica de lofty: un identificador de
        // marco ID3v2 tiene cuatro caracteres, y las claves personalizadas se
        // descartan al convertir. Para MP3, la identidad viene del nombre.
        let ruta = mp3_de_prueba("otro-nombre-cualquiera.mp3");
        EtiquetadorLofty::nuevo()
            .write(&ruta, &pista(), None)
            .await
            .expect("etiqueta");

        assert!(
            leer_id_de_etiquetas(&ruta).is_none(),
            "si esto empieza a funcionar, lofty lo ha arreglado y se puede simplificar"
        );

        let _ = std::fs::remove_file(ruta);
    }

    #[tokio::test]
    async fn el_artista_del_album_es_el_principal_y_no_la_lista() {
        // Es lo que agrupa bien los recopilatorios en cualquier reproductor.
        let ruta = mp3_de_prueba("albumartist.mp3");
        EtiquetadorLofty::nuevo()
            .write(&ruta, &pista(), None)
            .await
            .expect("etiqueta");

        let fichero = Probe::open(&ruta).expect("abre").read().expect("lee");
        let tag = fichero.primary_tag().expect("hay etiqueta");

        assert_eq!(
            tag.get_string(&ItemKey::AlbumArtist),
            Some("Queen"),
            "el artista del álbum no debe llevar los invitados"
        );

        let _ = std::fs::remove_file(ruta);
    }

    #[tokio::test]
    async fn el_isrc_se_conserva() {
        let ruta = mp3_de_prueba("isrc.mp3");
        EtiquetadorLofty::nuevo()
            .write(&ruta, &pista(), None)
            .await
            .expect("etiqueta");

        let fichero = Probe::open(&ruta).expect("abre").read().expect("lee");
        let tag = fichero.primary_tag().expect("hay etiqueta");
        assert_eq!(tag.get_string(&ItemKey::Isrc), Some("GBUM71029604"));

        let _ = std::fs::remove_file(ruta);
    }

    #[tokio::test]
    async fn una_portada_ilegible_no_impide_etiquetar() {
        // La música importa más que la carátula.
        let ruta = mp3_de_prueba("portada-rota.mp3");
        let basura = vec![0xDE_u8, 0xAD, 0xBE, 0xEF];

        EtiquetadorLofty::nuevo()
            .write(&ruta, &pista(), Some(&basura))
            .await
            .expect("debe etiquetar igualmente");

        let fichero = Probe::open(&ruta).expect("abre").read().expect("lee");
        let tag = fichero.primary_tag().expect("hay etiqueta");
        assert_eq!(tag.title().as_deref(), Some("Under Pressure"));
        assert!(tag.pictures().is_empty(), "la basura no debe incrustarse");

        let _ = std::fs::remove_file(ruta);
    }

    #[tokio::test]
    async fn una_pista_sin_album_ni_fecha_se_etiqueta_igual() {
        let ruta = mp3_de_prueba("minima.mp3");
        let mut p = pista();
        p.album = None;
        p.release_date = None;
        p.track_number = None;
        p.isrc = None;

        EtiquetadorLofty::nuevo()
            .write(&ruta, &p, None)
            .await
            .expect("etiqueta");

        let fichero = Probe::open(&ruta).expect("abre").read().expect("lee");
        let tag = fichero.primary_tag().expect("hay etiqueta");
        assert_eq!(tag.title().as_deref(), Some("Under Pressure"));

        let _ = std::fs::remove_file(ruta);
    }

    #[tokio::test]
    async fn un_fichero_ajeno_no_se_asocia_a_ninguna_pista() {
        let ruta = mp3_de_prueba("musica-del-usuario.mp3");
        let leido = EtiquetadorLofty::nuevo()
            .read_track_id(&ruta)
            .await
            .expect("consulta");
        assert!(
            leido.is_none(),
            "sin etiqueta ni nombre reconocible, no hay identidad que inferir"
        );

        let _ = std::fs::remove_file(ruta);
    }

    #[tokio::test]
    async fn un_fichero_inexistente_no_revienta_al_leer() {
        let leido = EtiquetadorLofty::nuevo()
            .read_track_id(Path::new("no-existe.mp3"))
            .await
            .expect("consulta");
        assert!(leido.is_none());
    }

    #[tokio::test]
    async fn etiquetar_un_fichero_invalido_devuelve_error() {
        let dir = std::env::temp_dir().join("localify-test-tags");
        let _ = std::fs::create_dir_all(&dir);
        let ruta = dir.join("no-es-audio.mp3");
        std::fs::write(&ruta, b"esto no es un fichero de audio").expect("escribe");

        assert!(
            EtiquetadorLofty::nuevo()
                .write(&ruta, &pista(), None)
                .await
                .is_err()
        );

        let _ = std::fs::remove_file(ruta);
    }
}

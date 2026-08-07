//! Verificación de los ficheros descargados.
//!
//! Es la barrera que hace cierta la regla "nunca dejar archivos corruptos". Un
//! fichero solo entra en la biblioteca si supera **todas** estas comprobaciones,
//! y hasta entonces vive en `.tmp/` con extensión `.part`.
//!
//! No basta con que yt-dlp devuelva código cero: una descarga puede terminar
//! con un fichero truncado si la conexión se corta en el último fragmento, y
//! ese fichero sonaría cortado para siempre.

use std::path::Path;
use std::sync::Arc;

use localify_core::domain::audio::DurationMs;
use localify_core::ports::youtube::MediaInfo;
use serde::Deserialize;
use tracing::debug;

use crate::error::{YtDlpError, YtDlpResult};
use crate::proceso::Ejecutor;

const BINARIO: &str = "ffprobe";

/// Desviación máxima admisible entre la duración de Spotify y la del fichero.
///
/// Dos segundos absorben la diferencia normal entre másteres y el silencio
/// final que a veces añade YouTube. Más allá, o el fichero está truncado o el
/// emparejamiento era otra grabación, y ambas cosas exigen descartarlo.
pub const TOLERANCIA_DURACION_MS: u32 = 2_000;

/// Tamaño mínimo plausible de un fichero de audio.
///
/// Por debajo de esto no hay canción: es una página de error guardada como si
/// fuera audio, que es un fallo que ocurre de verdad.
pub const TAMANO_MINIMO_BYTES: u64 = 16 * 1024;

/// Salida de `ffprobe` en JSON.
#[derive(Debug, Deserialize)]
struct SalidaFfprobe {
    format: Option<FormatoFfprobe>,
    #[serde(default)]
    streams: Vec<StreamFfprobe>,
}

#[derive(Debug, Deserialize)]
struct FormatoFfprobe {
    /// Segundos, como texto.
    duration: Option<String>,
    bit_rate: Option<String>,
}

#[derive(Debug, Deserialize)]
struct StreamFfprobe {
    codec_type: Option<String>,
    codec_name: Option<String>,
    sample_rate: Option<String>,
    channels: Option<u8>,
    bit_rate: Option<String>,
}

/// Inspecciona un fichero de audio.
pub struct Inspector {
    ejecutor: Arc<dyn Ejecutor>,
}

impl std::fmt::Debug for Inspector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Inspector").finish_non_exhaustive()
    }
}

impl Inspector {
    #[must_use]
    pub fn nuevo(ejecutor: Arc<dyn Ejecutor>) -> Self {
        Self { ejecutor }
    }

    /// Lee los metadatos técnicos del fichero.
    ///
    /// # Errors
    /// Si ffprobe falla o el fichero no contiene audio.
    pub async fn inspeccionar(&self, ruta: &Path) -> YtDlpResult<MediaInfo> {
        let args = vec![
            "-v".to_owned(),
            "quiet".to_owned(),
            "-print_format".to_owned(),
            "json".to_owned(),
            "-show_format".to_owned(),
            "-show_streams".to_owned(),
            ruta.to_string_lossy().into_owned(),
        ];

        let salida = self.ejecutor.ejecutar(BINARIO, &args).await?;
        if !salida.es_ok() {
            return Err(crate::proceso::clasificar(BINARIO, &salida));
        }

        let info = interpretar(&salida.stdout)?;
        let seekable = moov_al_principio(ruta).await;

        Ok(MediaInfo {
            seekable_from_start: seekable,
            ..info
        })
    }

    /// Comprueba que el fichero es utilizable como pista de la biblioteca.
    ///
    /// # Errors
    /// [`YtDlpError::VerificacionFallida`] con el motivo concreto.
    pub async fn verificar(
        &self,
        ruta: &Path,
        duracion_esperada: DurationMs,
    ) -> YtDlpResult<MediaInfo> {
        let bytes = tokio::fs::metadata(ruta).await?.len();
        if bytes < TAMANO_MINIMO_BYTES {
            return Err(YtDlpError::VerificacionFallida(format!(
                "el fichero solo ocupa {bytes} bytes"
            )));
        }

        // Que ffprobe pueda demuxearlo entero es la prueba de que no está
        // truncado: si lo estuviera, no podría calcular la duración.
        let info = self.inspeccionar(ruta).await?;

        if info.duration.is_zero() {
            return Err(YtDlpError::VerificacionFallida(
                "el fichero no declara duración".to_owned(),
            ));
        }

        let diferencia = info.duration.diff(duracion_esperada);
        if diferencia.as_ms() > TOLERANCIA_DURACION_MS {
            return Err(YtDlpError::VerificacionFallida(format!(
                "dura {} y se esperaban {} (diferencia de {})",
                info.duration, duracion_esperada, diferencia
            )));
        }

        debug!(
            ruta = %ruta.display(),
            bytes,
            codec = %info.codec,
            "fichero verificado"
        );
        Ok(info)
    }
}

/// Interpreta la salida JSON de ffprobe.
///
/// # Errors
/// Si el JSON no se puede leer o no hay pista de audio.
pub fn interpretar(json: &str) -> YtDlpResult<MediaInfo> {
    let salida: SalidaFfprobe = serde_json::from_str(json).map_err(|e| YtDlpError::Salida {
        binario: BINARIO,
        detalle: e.to_string(),
    })?;

    let audio = salida
        .streams
        .iter()
        .find(|s| s.codec_type.as_deref() == Some("audio"))
        .ok_or_else(|| {
            // Ocurre cuando lo descargado no era audio: una página de error, o
            // un vídeo sin pista de sonido.
            YtDlpError::VerificacionFallida("el fichero no contiene audio".to_owned())
        })?;

    let segundos: f64 = salida
        .format
        .as_ref()
        .and_then(|f| f.duration.as_deref())
        .and_then(|d| d.parse().ok())
        .unwrap_or(0.0);

    // El bitrate del contenedor incluye la sobrecarga; el de la pista es el
    // dato real. Se prefiere el segundo y se cae al primero.
    let bitrate = audio
        .bit_rate
        .as_deref()
        .or_else(|| salida.format.as_ref().and_then(|f| f.bit_rate.as_deref()))
        .and_then(|b| b.parse::<u32>().ok())
        .map(|b| b / 1000);

    Ok(MediaInfo {
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "una duración positiva en segundos cabe de sobra en u32 ms"
        )]
        duration: DurationMs::new((segundos.max(0.0) * 1000.0) as u32),
        codec: audio
            .codec_name
            .clone()
            .unwrap_or_else(|| "desconocido".to_owned()),
        bitrate_kbps: bitrate,
        sample_rate: audio.sample_rate.as_deref().and_then(|s| s.parse().ok()),
        channels: audio.channels,
        // Lo rellena `inspeccionar` leyendo la cabecera; aquí no hay fichero.
        seekable_from_start: false,
    })
}

/// `true` si un MP4/M4A tiene el átomo `moov` antes que `mdat`.
///
/// Es lo que decide si se puede reproducir mientras baja: con `moov` al final
/// —que es como lo genera un codificador que no hace *faststart*— el
/// decodificador no sabe nada del contenido hasta tener el fichero entero.
///
/// Para los contenedores que sí son de flujo por diseño (WebM, Ogg, MP3) la
/// respuesta es siempre `true` y no hace falta mirar nada.
pub async fn moov_al_principio(ruta: &Path) -> bool {
    let extension = ruta
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();

    // `.part` esconde la extensión real: se mira la anterior.
    let extension = if extension == "part" {
        ruta.file_stem()
            .and_then(|s| Path::new(s).extension())
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .unwrap_or(extension)
    } else {
        extension
    };

    if crate::formats::admite_reproduccion_progresiva(&extension) {
        return true;
    }
    if !matches!(extension.as_str(), "m4a" | "mp4" | "m4b") {
        return false;
    }

    // Basta con leer la cabecera: si `moov` no aparece en el primer tramo, está
    // detrás de los datos de audio.
    let Ok(datos) = leer_cabecera(ruta, 64 * 1024).await else {
        return false;
    };

    let posicion_moov = buscar(&datos, b"moov");
    let posicion_mdat = buscar(&datos, b"mdat");

    match (posicion_moov, posicion_mdat) {
        (Some(m), Some(d)) => m < d,
        (Some(_), None) => true,
        _ => false,
    }
}

async fn leer_cabecera(ruta: &Path, bytes: usize) -> std::io::Result<Vec<u8>> {
    use tokio::io::AsyncReadExt;

    let mut fichero = tokio::fs::File::open(ruta).await?;
    let mut buffer = vec![0_u8; bytes];
    let leidos = fichero.read(&mut buffer).await?;
    buffer.truncate(leidos);
    Ok(buffer)
}

fn buscar(datos: &[u8], patron: &[u8]) -> Option<usize> {
    if patron.is_empty() || datos.len() < patron.len() {
        return None;
    }
    datos.windows(patron.len()).position(|v| v == patron)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proceso::falso::EjecutorFalso;

    fn json_ffprobe(segundos: &str, codec: &str) -> String {
        format!(
            r#"{{"format":{{"duration":"{segundos}","bit_rate":"165000"}},
                 "streams":[
                   {{"codec_type":"audio","codec_name":"{codec}","sample_rate":"48000",
                     "channels":2,"bit_rate":"160000"}}
                 ]}}"#
        )
    }

    fn fichero_temporal(nombre: &str, bytes: usize) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join("localify-test-verify");
        let _ = std::fs::create_dir_all(&dir);
        let ruta = dir.join(nombre);
        std::fs::write(&ruta, vec![0_u8; bytes]).expect("escribe");
        ruta
    }

    #[test]
    fn se_interpretan_los_metadatos_tecnicos() {
        let info = interpretar(&json_ffprobe("248.123", "opus")).expect("interpreta");

        assert_eq!(info.duration, DurationMs::new(248_123));
        assert_eq!(info.codec, "opus");
        assert_eq!(info.bitrate_kbps, Some(160), "manda el bitrate de la pista");
        assert_eq!(info.sample_rate, Some(48_000));
        assert_eq!(info.channels, Some(2));
    }

    #[test]
    fn sin_bitrate_de_pista_se_usa_el_del_contenedor() {
        let json = r#"{"format":{"duration":"200","bit_rate":"128000"},
            "streams":[{"codec_type":"audio","codec_name":"aac"}]}"#;
        assert_eq!(
            interpretar(json).expect("interpreta").bitrate_kbps,
            Some(128)
        );
    }

    #[test]
    fn un_fichero_sin_pista_de_audio_se_rechaza() {
        // Ocurre cuando lo descargado era una página de error.
        let json = r#"{"format":{"duration":"10"},
            "streams":[{"codec_type":"video","codec_name":"h264"}]}"#;
        let error = interpretar(json).expect_err("debe fallar");
        assert!(matches!(error, YtDlpError::VerificacionFallida(_)));
    }

    #[test]
    fn una_salida_ilegible_se_rechaza() {
        assert!(interpretar("{roto").is_err());
        assert!(interpretar("").is_err());
    }

    #[tokio::test]
    async fn un_fichero_con_la_duracion_esperada_pasa_la_verificacion() {
        let ruta = fichero_temporal("bueno.opus", 100_000);
        let e = Arc::new(EjecutorFalso::nuevo().con_stdout(&json_ffprobe("248.5", "opus")));
        let inspector = Inspector::nuevo(e);

        let info = inspector
            .verificar(&ruta, DurationMs::new(248_000))
            .await
            .expect("verifica");
        assert_eq!(info.codec, "opus");

        let _ = std::fs::remove_file(ruta);
    }

    #[tokio::test]
    async fn un_fichero_truncado_no_pasa() {
        // Dura la mitad de lo esperado: la conexión se cortó a medias.
        let ruta = fichero_temporal("truncado.opus", 100_000);
        let e = Arc::new(EjecutorFalso::nuevo().con_stdout(&json_ffprobe("120.0", "opus")));
        let inspector = Inspector::nuevo(e);

        let error = inspector
            .verificar(&ruta, DurationMs::new(248_000))
            .await
            .expect_err("debe fallar");

        match error {
            YtDlpError::VerificacionFallida(m) => {
                assert!(m.contains("diferencia"), "{m}");
            }
            otro => panic!("se esperaba VerificacionFallida, llegó {otro:?}"),
        }

        let _ = std::fs::remove_file(ruta);
    }

    #[tokio::test]
    async fn una_diferencia_dentro_de_la_tolerancia_se_acepta() {
        // Los másteres difieren en décimas y YouTube añade silencio al final.
        let ruta = fichero_temporal("casi.opus", 100_000);
        let e = Arc::new(EjecutorFalso::nuevo().con_stdout(&json_ffprobe("249.9", "opus")));
        let inspector = Inspector::nuevo(e);

        assert!(
            inspector
                .verificar(&ruta, DurationMs::new(248_000))
                .await
                .is_ok()
        );

        let _ = std::fs::remove_file(ruta);
    }

    #[tokio::test]
    async fn un_fichero_diminuto_se_rechaza_sin_llamar_a_ffprobe() {
        // Una página de error guardada como audio: no merece un proceso.
        let ruta = fichero_temporal("error.opus", 512);
        let e = Arc::new(EjecutorFalso::nuevo());
        let inspector = Inspector::nuevo(e.clone());

        let error = inspector
            .verificar(&ruta, DurationMs::new(248_000))
            .await
            .expect_err("debe fallar");
        assert!(matches!(error, YtDlpError::VerificacionFallida(_)));
        assert_eq!(e.cuantas(), 0);

        let _ = std::fs::remove_file(ruta);
    }

    #[tokio::test]
    async fn un_fichero_sin_duracion_declarada_se_rechaza() {
        let ruta = fichero_temporal("sinduracion.opus", 100_000);
        let json = r#"{"format":{},"streams":[{"codec_type":"audio","codec_name":"opus"}]}"#;
        let e = Arc::new(EjecutorFalso::nuevo().con_stdout(json));
        let inspector = Inspector::nuevo(e);

        assert!(
            inspector
                .verificar(&ruta, DurationMs::new(248_000))
                .await
                .is_err()
        );

        let _ = std::fs::remove_file(ruta);
    }

    #[tokio::test]
    async fn los_contenedores_de_flujo_son_progresivos_sin_inspeccionar() {
        for nombre in ["a.opus", "b.webm", "c.ogg", "d.mp3"] {
            let ruta = std::path::PathBuf::from(nombre);
            assert!(moov_al_principio(&ruta).await, "{nombre}");
        }
    }

    #[tokio::test]
    async fn la_extension_real_se_ve_a_traves_del_sufijo_part() {
        // Durante la descarga, el fichero se llama `<id>.opus.part`.
        assert!(moov_al_principio(Path::new("abc.opus.part")).await);
        assert!(moov_al_principio(Path::new("abc.webm.part")).await);
    }

    #[tokio::test]
    async fn un_m4a_con_moov_al_principio_es_progresivo() {
        let dir = std::env::temp_dir().join("localify-test-verify");
        let _ = std::fs::create_dir_all(&dir);
        let ruta = dir.join("faststart.m4a");

        let mut datos = b"\x00\x00\x00\x20ftypM4A ".to_vec();
        datos.extend_from_slice(b"\x00\x00\x10\x00moov");
        datos.extend_from_slice(&[0_u8; 100]);
        datos.extend_from_slice(b"\x00\x00\x10\x00mdat");
        std::fs::write(&ruta, &datos).expect("escribe");

        assert!(moov_al_principio(&ruta).await);
        let _ = std::fs::remove_file(ruta);
    }

    #[tokio::test]
    async fn un_m4a_con_moov_al_final_no_es_progresivo() {
        // Reproducirlo mientras baja se cortaría a los pocos segundos.
        let dir = std::env::temp_dir().join("localify-test-verify");
        let _ = std::fs::create_dir_all(&dir);
        let ruta = dir.join("sinfaststart.m4a");

        let mut datos = b"\x00\x00\x00\x20ftypM4A ".to_vec();
        datos.extend_from_slice(b"\x00\x00\x10\x00mdat");
        datos.extend_from_slice(&[0_u8; 100]);
        datos.extend_from_slice(b"\x00\x00\x10\x00moov");
        std::fs::write(&ruta, &datos).expect("escribe");

        assert!(!moov_al_principio(&ruta).await);
        let _ = std::fs::remove_file(ruta);
    }

    #[tokio::test]
    async fn un_fichero_inexistente_no_se_da_por_progresivo() {
        assert!(!moov_al_principio(Path::new("no-existe.m4a")).await);
    }

    #[test]
    fn la_busqueda_de_patrones_maneja_los_casos_degenerados() {
        assert_eq!(buscar(b"hola mundo", b"mundo"), Some(5));
        assert_eq!(buscar(b"hola", b"adios"), None);
        assert_eq!(buscar(b"", b"x"), None);
        assert_eq!(buscar(b"x", b""), None);
    }
}

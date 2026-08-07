//! Cambio de contenedor sin recodificar.
//!
//! ## Por qué hace falta
//!
//! YouTube sirve el audio Opus dentro de un contenedor **WebM**, no Ogg.
//! Guardarlo con extensión `.opus` sería mentir sobre el formato, y trae dos
//! problemas concretos:
//!
//! - Los reproductores que confían en la extensión fallarían al abrirlo.
//! - Las etiquetas de Matroska tienen soporte pobre; las de Ogg (Vorbis
//!   comments) admiten claves arbitrarias, que es lo que necesita
//!   `LOCALIFY_SPOTIFY_ID`.
//!
//! La solución es remuxear a Ogg con `-c copy`: **no se toca el audio**, solo
//! el envoltorio. Cuesta milisegundos y no degrada un solo bit, que es
//! justamente la diferencia entre remuxear y recodificar.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tracing::debug;

use crate::error::{YtDlpError, YtDlpResult};
use crate::proceso::{Ejecutor, clasificar};

const BINARIO: &str = "ffmpeg";

/// Decide si un fichero necesita cambiar de contenedor.
///
/// Devuelve la extensión de destino, o `None` si ya está donde debe.
#[must_use]
pub fn destino_para(extension_actual: &str, codec: &str) -> Option<&'static str> {
    let codec = codec.to_ascii_lowercase();
    let actual = extension_actual.to_ascii_lowercase();

    match (actual.as_str(), codec.as_str()) {
        // Opus dentro de WebM: a Ogg, que es su contenedor natural.
        ("webm", c) if c.contains("opus") => Some("opus"),
        // Vorbis dentro de WebM: también a Ogg.
        ("webm", c) if c.contains("vorbis") => Some("ogg"),
        // AAC dentro de un MP4 genérico: a m4a, que es lo mismo con el nombre
        // que esperan los reproductores.
        ("mp4", c) if c.contains("aac") || c.contains("mp4a") => Some("m4a"),
        _ => None,
    }
}

/// Remuxea un fichero a otro contenedor, sin recodificar.
pub struct Remuxeador {
    ejecutor: Arc<dyn Ejecutor>,
}

impl std::fmt::Debug for Remuxeador {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Remuxeador").finish_non_exhaustive()
    }
}

impl Remuxeador {
    #[must_use]
    pub fn nuevo(ejecutor: Arc<dyn Ejecutor>) -> Self {
        Self { ejecutor }
    }

    /// Cambia el contenedor de `origen`, dejando el resultado en un fichero
    /// nuevo junto a él.
    ///
    /// Devuelve la ruta del resultado. El original **no se borra**: de eso se
    /// encarga quien orquesta la descarga, después de verificar.
    ///
    /// # Errors
    /// Si ffmpeg falla o el resultado no aparece.
    pub async fn remuxear(&self, origen: &Path, extension: &str) -> YtDlpResult<PathBuf> {
        let destino = origen.with_extension(format!("remux.{extension}"));

        let args = vec![
            "-y".to_owned(),
            "-loglevel".to_owned(),
            "error".to_owned(),
            "-i".to_owned(),
            origen.to_string_lossy().into_owned(),
            // La instrucción que lo cambia todo: copiar el flujo tal cual.
            // Sin esto, ffmpeg recodificaría y perdería calidad de forma
            // irreversible.
            "-c".to_owned(),
            "copy".to_owned(),
            // Solo audio: si el origen trae carátula como flujo de vídeo, la
            // descartamos aquí y la reincrustamos como etiqueta.
            "-vn".to_owned(),
            "-map".to_owned(),
            "0:a".to_owned(),
            destino.to_string_lossy().into_owned(),
        ];

        let salida = self.ejecutor.ejecutar(BINARIO, &args).await?;
        if !salida.es_ok() {
            let _ = tokio::fs::remove_file(&destino).await;
            return Err(clasificar(BINARIO, &salida));
        }

        if !crate::proceso::existe(&destino).await {
            return Err(YtDlpError::VerificacionFallida(
                "ffmpeg terminó bien pero no dejó el fichero".to_owned(),
            ));
        }

        debug!(
            origen = %origen.display(),
            destino = %destino.display(),
            "contenedor cambiado sin recodificar"
        );
        Ok(destino)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proceso::falso::EjecutorFalso;

    #[test]
    fn el_opus_en_webm_va_a_ogg() {
        // Es el caso normal: todo el audio de YouTube en Opus llega así.
        assert_eq!(destino_para("webm", "opus"), Some("opus"));
        assert_eq!(destino_para("webm", "vorbis"), Some("ogg"));
    }

    #[test]
    fn un_mp4_generico_se_renombra_a_m4a() {
        assert_eq!(destino_para("mp4", "aac"), Some("m4a"));
        assert_eq!(destino_para("mp4", "mp4a.40.2"), Some("m4a"));
    }

    #[test]
    fn lo_que_ya_esta_en_su_contenedor_no_se_toca() {
        // Remuxear por remuxear solo añadiría riesgo y tiempo.
        assert_eq!(destino_para("opus", "opus"), None);
        assert_eq!(destino_para("m4a", "aac"), None);
        assert_eq!(destino_para("mp3", "mp3"), None);
        assert_eq!(destino_para("flac", "flac"), None);
        assert_eq!(destino_para("ogg", "vorbis"), None);
    }

    #[tokio::test]
    async fn el_remux_pide_copiar_el_flujo_y_nunca_recodificar() {
        let dir = std::env::temp_dir().join("localify-test-remux");
        let _ = std::fs::create_dir_all(&dir);
        let origen = dir.join("abc.webm");
        std::fs::write(&origen, b"contenido").expect("escribe");
        let esperado = dir.join("abc.remux.opus");
        std::fs::write(&esperado, b"resultado").expect("escribe");

        let e = Arc::new(EjecutorFalso::nuevo().con_stdout(""));
        let r = Remuxeador::nuevo(e.clone());

        let destino = r.remuxear(&origen, "opus").await.expect("remuxea");
        assert_eq!(destino, esperado);

        let args = e.args_de(0);
        let posicion = args.iter().position(|a| a == "-c").expect("hay -c");
        assert_eq!(
            args.get(posicion + 1).map(String::as_str),
            Some("copy"),
            "sin `-c copy` ffmpeg recodificaria y degradaria el audio: {args:?}"
        );
        assert!(args.contains(&"-vn".to_owned()), "solo audio: {args:?}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn el_original_no_se_borra_al_remuxear() {
        // Borrarlo aqui dejaria la descarga sin nada si la verificacion
        // posterior fallara.
        let dir = std::env::temp_dir().join("localify-test-remux-original");
        let _ = std::fs::create_dir_all(&dir);
        let origen = dir.join("abc.webm");
        std::fs::write(&origen, b"contenido").expect("escribe");
        std::fs::write(dir.join("abc.remux.opus"), b"resultado").expect("escribe");

        let e = Arc::new(EjecutorFalso::nuevo().con_stdout(""));
        Remuxeador::nuevo(e)
            .remuxear(&origen, "opus")
            .await
            .expect("remuxea");

        assert!(origen.exists(), "el original debe seguir ahi");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn si_ffmpeg_falla_no_queda_un_fichero_a_medias() {
        let dir = std::env::temp_dir().join("localify-test-remux-fallo");
        let _ = std::fs::create_dir_all(&dir);
        let origen = dir.join("abc.webm");
        std::fs::write(&origen, b"contenido").expect("escribe");
        let parcial = dir.join("abc.remux.opus");
        std::fs::write(&parcial, b"a medias").expect("escribe");

        let e = Arc::new(EjecutorFalso::nuevo().con_error(1, "Invalid data found"));
        let error = Remuxeador::nuevo(e)
            .remuxear(&origen, "opus")
            .await
            .expect_err("debe fallar");

        assert!(matches!(error, YtDlpError::Proceso { .. }));
        assert!(
            !parcial.exists(),
            "un resultado a medias no debe sobrevivir a un fallo"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn un_ffmpeg_que_miente_sobre_su_exito_se_detecta() {
        // Codigo cero pero sin fichero: ocurre con rutas invalidas.
        let dir = std::env::temp_dir().join("localify-test-remux-mentira");
        let _ = std::fs::create_dir_all(&dir);
        let origen = dir.join("abc.webm");
        std::fs::write(&origen, b"contenido").expect("escribe");

        let e = Arc::new(EjecutorFalso::nuevo().con_stdout(""));
        let error = Remuxeador::nuevo(e)
            .remuxear(&origen, "opus")
            .await
            .expect_err("debe fallar");

        assert!(matches!(error, YtDlpError::VerificacionFallida(_)));
        let _ = std::fs::remove_dir_all(&dir);
    }
}

//! Selección del formato de descarga.
//!
//! Dos criterios, y en este orden:
//!
//! 1. **Máxima calidad disponible.** Nunca se transcodifica: recodificar un
//!    códec con pérdida a otro no recupera nada y solo añade degradación.
//! 2. **Que se pueda reproducir mientras baja.** Es lo que hace posible pulsar
//!    play y oír música en dos segundos, en lugar de esperar a la descarga
//!    completa.
//!
//! Ambos apuntan al mismo sitio: **Opus en contenedor WebM**. Es el mejor audio
//! que sirve YouTube (~160 kbps VBR, perceptualmente superior a los 128 kbps de
//! AAC) y Matroska está diseñado para decodificarse en flujo. Los dos criterios
//! se refuerzan, no compiten.

use localify_core::domain::settings::FormatPreference;

/// Expresión de selección de formato para yt-dlp.
///
/// La sintaxis es la suya: `/` separa alternativas por orden de preferencia,
/// `[...]` filtra y `bestaudio` ordena por calidad.
#[must_use]
pub fn expresion(preferencia: FormatPreference) -> &'static str {
    match preferencia {
        // Opus primero; si el vídeo no lo ofrece, m4a; y si tampoco, lo mejor
        // que haya. Nunca se queda sin audio por ser exigente.
        FormatPreference::Opus => {
            "bestaudio[acodec=opus]/bestaudio[ext=webm]/bestaudio[ext=m4a]/bestaudio/best"
        }
        // La vía de escape si el decodificador Opus diera problemas: symphonia
        // decodifica AAC de forma nativa (ADR-003).
        FormatPreference::M4a => "bestaudio[ext=m4a]/bestaudio[acodec=aac]/bestaudio/best",
        FormatPreference::Best => "bestaudio/best",
    }
}

/// Extensión esperada según el códec y el contenedor que informe yt-dlp.
#[must_use]
pub fn extension_de(acodec: Option<&str>, ext: Option<&str>) -> &'static str {
    match (acodec, ext) {
        // El audio de YouTube en WebM es Opus. Guardarlo con extensión `.opus`
        // en lugar de `.webm` deja claro qué es y lo hace reconocible para
        // cualquier reproductor.
        (Some(c), _) if c.contains("opus") => "opus",
        (_, Some("webm")) => "opus",
        (Some(c), _) if c.contains("mp4a") || c.contains("aac") => "m4a",
        (_, Some("m4a" | "mp4")) => "m4a",
        (Some(c), _) if c.contains("mp3") => "mp3",
        (_, Some("mp3")) => "mp3",
        (Some(c), _) if c.contains("vorbis") => "ogg",
        (_, Some("ogg")) => "ogg",
        (_, Some("flac")) => "flac",
        _ => "m4a",
    }
}

/// `true` si el formato admite reproducción mientras el fichero crece.
///
/// - **Opus/WebM y Ogg**: sí. Matroska y Ogg intercalan datos en bloques
///   secuenciales con la cabecera al principio.
/// - **MP3**: sí. Es un flujo de tramas independientes.
/// - **M4A**: depende. Solo si el átomo `moov` está al principio, cosa que hay
///   que comprobar por fichero. Se responde `false` y la capa de descarga
///   decide tras inspeccionarlo.
/// - **FLAC**: no, en la práctica. YouTube no lo sirve, así que solo llegaría
///   de un fichero importado, que ya está completo.
#[must_use]
pub fn admite_reproduccion_progresiva(extension: &str) -> bool {
    matches!(extension, "opus" | "webm" | "ogg" | "mp3")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn la_expresion_por_defecto_prefiere_opus_pero_nunca_se_queda_sin_audio() {
        let e = expresion(FormatPreference::Opus);
        assert!(e.starts_with("bestaudio[acodec=opus]"));
        assert!(
            e.ends_with("/best"),
            "debe haber una alternativa final: quedarse sin audio por ser exigente sería peor"
        );
    }

    #[test]
    fn la_alternativa_m4a_apunta_a_lo_que_symphonia_decodifica_nativamente() {
        let e = expresion(FormatPreference::M4a);
        assert!(e.contains("ext=m4a"));
        assert!(e.contains("acodec=aac"));
    }

    #[test]
    fn el_audio_webm_de_youtube_se_guarda_como_opus() {
        assert_eq!(extension_de(Some("opus"), Some("webm")), "opus");
        assert_eq!(extension_de(None, Some("webm")), "opus");
        assert_eq!(extension_de(Some("opus"), None), "opus");
    }

    #[test]
    fn el_audio_aac_se_guarda_como_m4a() {
        assert_eq!(extension_de(Some("mp4a.40.2"), Some("m4a")), "m4a");
        assert_eq!(extension_de(Some("aac"), None), "m4a");
        assert_eq!(extension_de(None, Some("mp4")), "m4a");
    }

    #[test]
    fn un_formato_desconocido_cae_en_m4a() {
        // Elegir m4a como red de seguridad y no opus: symphonia decodifica AAC
        // de forma nativa, así que un formato inesperado tiene más
        // probabilidades de sonar.
        assert_eq!(extension_de(None, None), "m4a");
        assert_eq!(extension_de(Some("desconocido"), Some("raro")), "m4a");
    }

    #[test]
    fn los_formatos_en_flujo_admiten_reproduccion_progresiva() {
        for ext in ["opus", "webm", "ogg", "mp3"] {
            assert!(admite_reproduccion_progresiva(ext), "{ext}");
        }
    }

    #[test]
    fn m4a_no_se_da_por_progresivo_sin_inspeccionar() {
        // Depende de dónde esté el átomo `moov`, y dar por bueno lo contrario
        // produciría reproducciones que se cortan a los pocos segundos.
        assert!(!admite_reproduccion_progresiva("m4a"));
        assert!(!admite_reproduccion_progresiva("flac"));
    }
}

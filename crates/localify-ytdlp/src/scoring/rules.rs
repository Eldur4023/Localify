//! Tabla de pesos y penalizaciones del emparejador.
//!
//! Son **datos, no código**: ajustar el sistema es cambiar un número aquí, sin
//! tocar la lógica que los aplica. Eso permite calibrar contra un corpus real
//! sin arriesgarse a romper el algoritmo.
//!
//! Los valores están pensados para que la puntuación final caiga en `[0, 100]`
//! y los umbrales de confianza (`75` alta, `55` media) signifiquen lo que
//! aparentan. Ver `docs/architecture/02-modules.md` para la justificación.

/// Punto de partida de un candidato sin ninguna señal.
///
/// Con `30`, un candidato anónimo de un canal cualquiera necesita señales de
/// texto para alcanzar la confianza media, y un directo o un karaoke caen por
/// debajo del umbral aunque vengan de un canal oficial.
pub const BASE: f32 = 30.0;

// ─── Bonificaciones por fuente ───────────────────────────────────────────────

/// Resultado de music.youtube.com.
pub const BONO_YOUTUBE_MUSIC: f32 = 30.0;
/// Canal terminado en `- Topic`: subidas automáticas de la discográfica.
pub const BONO_CANAL_TOPIC: f32 = 28.0;
/// "Provided to YouTube by ..." en la descripción: marca inequívoca de subida
/// por el titular de los derechos.
pub const BONO_PROVIDED_TO_YOUTUBE: f32 = 25.0;
/// El nombre del canal coincide con el del artista principal.
pub const BONO_CANAL_DEL_ARTISTA: f32 = 22.0;
/// El álbum aparece en el título o la descripción.
pub const BONO_ALBUM: f32 = 10.0;

// ─── Bonificaciones por texto ────────────────────────────────────────────────

/// Máximo por similitud del título.
pub const BONO_TITULO_MAX: f32 = 20.0;
/// Máximo por coincidencia del artista.
pub const BONO_ARTISTA_MAX: f32 = 15.0;
/// Similitud por debajo de la cual no se concede nada.
///
/// Sin este suelo, una coincidencia del 30 % —es decir, ninguna— seguiría
/// sumando seis puntos.
pub const UMBRAL_SIMILITUD: f64 = 0.55;

// ─── Penalizaciones ──────────────────────────────────────────────────────────

/// Directo: es otra grabación, no la del álbum.
pub const PENALIZA_DIRECTO: f32 = 45.0;
/// Versión distinta: cover, karaoke, remix, instrumental…
pub const PENALIZA_VERSION: f32 = 40.0;
/// Manipulación del audio: acelerado, ralentizado, 8D, con graves realzados.
pub const PENALIZA_MANIPULADO: f32 = 40.0;
/// Vídeo musical: se prefiere el audio, que no lleva intro ni diálogos.
pub const PENALIZA_VIDEOCLIP: f32 = 8.0;
/// Recopilatorio o mezcla continua.
pub const PENALIZA_RECOPILATORIO: f32 = 60.0;
/// Canal desconocido con muy pocas reproducciones.
pub const PENALIZA_SIN_RECORRIDO: f32 = 15.0;
/// Falta un término que el título de Spotify **sí** tiene.
///
/// Es la otra cara de la excepción: si el usuario busca "Smells Like Teen
/// Spirit (Live)", un candidato que no diga "live" es la versión equivocada.
pub const PENALIZA_FALTA_REQUERIDO: f32 = 35.0;

/// Reproducciones por debajo de las cuales un canal desconocido resulta
/// sospechoso.
pub const VISTAS_MINIMAS: u64 = 1_000;

// ─── Duración ────────────────────────────────────────────────────────────────

/// Factor multiplicativo por diferencia de duración, en milisegundos.
///
/// Es multiplicativo y no aditivo a propósito: la duración es la señal más
/// fiable de que dos grabaciones son la misma, y ninguna cantidad de
/// bonificaciones debería rescatar un candidato que dura medio minuto más.
pub const FACTOR_DURACION: &[(u32, f32)] = &[
    (2_000, 1.00),
    (5_000, 0.90),
    (10_000, 0.70),
    (20_000, 0.40),
    (45_000, 0.15),
];

/// Diferencia a partir de la cual el candidato se descarta sin puntuar.
pub const DESCARTE_DURACION_MS: u32 = 45_000;

/// Factor para una diferencia dada.
#[must_use]
pub fn factor_duracion(diferencia_ms: u32) -> f32 {
    FACTOR_DURACION
        .iter()
        .find(|(limite, _)| diferencia_ms <= *limite)
        .map_or(0.0, |(_, factor)| *factor)
}

// ─── Vocabulario ─────────────────────────────────────────────────────────────

// El vocabulario vive en `localify_core::domain::versiones` desde que la
// búsqueda necesitó lo mismo para agrupar versiones. Se reexporta en vez de
// copiarse: dos listas que empiezan iguales terminan distintas, y la que se
// queda atrás falla en silencio.
pub use localify_core::domain::versiones::{
    TERMINOS_DIRECTO, TERMINOS_MANIPULADO, TERMINOS_RECOPILATORIO, TERMINOS_VERSION,
    TERMINOS_VIDEOCLIP,
};

/// Duración a partir de la cual un candidato huele a recopilatorio.
pub const DURACION_RECOPILATORIO_MS: u32 = 10 * 60 * 1000;
/// Duración de pista por debajo de la cual la anterior es sospechosa.
pub const DURACION_PISTA_NORMAL_MS: u32 = 8 * 60 * 1000;

/// Sufijo que identifica los canales de subida automática.
pub const SUFIJO_TOPIC: &str = "topic";

/// Marca de subida por el titular de los derechos.
pub const MARCA_PROVIDED: &str = "provided to youtube by";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn el_factor_de_duracion_decrece_de_forma_monotona() {
        let mut anterior = f32::INFINITY;
        for (limite, _) in FACTOR_DURACION {
            let f = factor_duracion(*limite);
            assert!(f <= anterior, "el factor debe decrecer con la diferencia");
            anterior = f;
        }
    }

    #[test]
    fn una_coincidencia_exacta_no_penaliza_la_duracion() {
        assert!((factor_duracion(0) - 1.0).abs() < f32::EPSILON);
        assert!((factor_duracion(2_000) - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn una_diferencia_grande_anula_practicamente_el_candidato() {
        assert!(factor_duracion(30_000) <= 0.15);
        assert!(
            (factor_duracion(60_000) - 0.0).abs() < f32::EPSILON,
            "más allá del descarte, el factor es cero"
        );
    }

    #[test]
    fn el_descarte_coincide_con_el_ultimo_tramo() {
        let (ultimo, _) = FACTOR_DURACION[FACTOR_DURACION.len() - 1];
        assert_eq!(ultimo, DESCARTE_DURACION_MS);
    }

    #[test]
    fn los_vocabularios_estan_normalizados() {
        // Se comparan contra texto que pasó por `normalize`: una entrada con
        // mayúsculas o acentos nunca coincidiría.
        for lista in [
            TERMINOS_DIRECTO,
            TERMINOS_VERSION,
            TERMINOS_MANIPULADO,
            TERMINOS_VIDEOCLIP,
            TERMINOS_RECOPILATORIO,
        ] {
            for termino in lista {
                assert_eq!(
                    *termino,
                    localify_core::text::normalize(termino),
                    "'{termino}' no está normalizado"
                );
            }
        }
    }

    #[test]
    fn ningun_termino_se_repite_entre_grupos() {
        // Un mismo término en dos grupos produciría dos penalizaciones por lo
        // mismo. La coincidencia es por palabra completa, así que `mix` y
        // `remix` sí pueden convivir en grupos distintos.
        let listas: Vec<(&str, &[&str])> = vec![
            ("directo", TERMINOS_DIRECTO),
            ("version", TERMINOS_VERSION),
            ("manipulado", TERMINOS_MANIPULADO),
            ("videoclip", TERMINOS_VIDEOCLIP),
            ("recopilatorio", TERMINOS_RECOPILATORIO),
        ];

        for (i, (nombre_a, a)) in listas.iter().enumerate() {
            for (nombre_b, b) in listas.iter().skip(i + 1) {
                for termino in *a {
                    assert!(
                        !b.contains(termino),
                        "'{termino}' está en '{nombre_a}' y en '{nombre_b}'"
                    );
                }
            }
        }
    }

    #[test]
    fn una_puntuacion_perfecta_supera_el_umbral_alto() {
        // Todas las señales a favor, sin penalizaciones.
        let total = BASE + BONO_YOUTUBE_MUSIC + BONO_TITULO_MAX + BONO_ARTISTA_MAX + BONO_ALBUM;
        assert!(
            total >= localify_core::domain::download::UMBRAL_ALTA,
            "un candidato perfecto debe alcanzar confianza alta ({total})"
        );
    }

    #[test]
    fn un_karaoke_perfecto_de_duracion_no_llega_al_umbral_medio() {
        // Es el caso que más importa: una duración idéntica no debe rescatar a
        // un karaoke, porque lo descargado no se vuelve a descargar.
        let total = BASE + BONO_TITULO_MAX + BONO_ARTISTA_MAX - PENALIZA_VERSION;
        assert!(
            total < localify_core::domain::download::UMBRAL_MEDIA,
            "un karaoke no debería descargarse nunca ({total})"
        );
    }

    #[test]
    fn un_directo_desde_canal_oficial_tampoco_llega() {
        let total = BASE + BONO_CANAL_TOPIC + BONO_TITULO_MAX + BONO_ARTISTA_MAX - PENALIZA_DIRECTO;
        assert!(
            total < localify_core::domain::download::UMBRAL_MEDIA,
            "un directo oficial sigue siendo la grabación equivocada ({total})"
        );
    }

    #[test]
    fn un_canal_topic_con_buen_texto_alcanza_confianza_alta() {
        let total = BASE + BONO_CANAL_TOPIC + BONO_TITULO_MAX + BONO_ARTISTA_MAX;
        assert!(
            total >= localify_core::domain::download::UMBRAL_ALTA,
            "los canales Topic son la mejor fuente disponible ({total})"
        );
    }
}

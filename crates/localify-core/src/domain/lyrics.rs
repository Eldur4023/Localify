//! Letras.
//!
//! Si no hay letra, no hay error: la UI simplemente no muestra el panel.

use serde::{Deserialize, Serialize};

use super::audio::DurationMs;

/// Una línea sincronizada con el audio.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LyricLine {
    pub at: DurationMs,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Lyrics {
    /// Letra sincronizada, ordenada por tiempo. Habilita el resaltado línea a
    /// línea.
    pub synced: Option<Vec<LyricLine>>,
    pub plain: Option<String>,
    pub source: String,
}

impl Lyrics {
    #[must_use]
    pub fn esta_vacia(&self) -> bool {
        self.synced.as_ref().is_none_or(Vec::is_empty)
            && self.plain.as_ref().is_none_or(|t| t.trim().is_empty())
    }

    #[must_use]
    pub fn tiene_sincronizacion(&self) -> bool {
        self.synced.as_ref().is_some_and(|l| !l.is_empty())
    }

    /// Índice de la línea activa en una posición dada.
    ///
    /// Búsqueda binaria: se llama en cada fotograma mientras el panel está
    /// abierto, así que un recorrido lineal sobre 80 líneas sería trabajo
    /// desperdiciado 60 veces por segundo.
    #[must_use]
    pub fn linea_en(&self, posicion: DurationMs) -> Option<usize> {
        let lineas = self.synced.as_ref()?;
        if lineas.is_empty() || lineas[0].at.as_ms() > posicion.as_ms() {
            return None;
        }
        let idx = lineas.partition_point(|l| l.at.as_ms() <= posicion.as_ms());
        idx.checked_sub(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn letra() -> Lyrics {
        Lyrics {
            synced: Some(vec![
                LyricLine {
                    at: DurationMs::new(1000),
                    text: "primera".into(),
                },
                LyricLine {
                    at: DurationMs::new(5000),
                    text: "segunda".into(),
                },
                LyricLine {
                    at: DurationMs::new(9000),
                    text: "tercera".into(),
                },
            ]),
            plain: None,
            source: "lrclib".into(),
        }
    }

    #[test]
    fn antes_de_la_primera_linea_no_hay_ninguna_activa() {
        assert_eq!(letra().linea_en(DurationMs::new(500)), None);
    }

    #[test]
    fn la_linea_activa_es_la_ultima_ya_comenzada() {
        let l = letra();
        assert_eq!(l.linea_en(DurationMs::new(1000)), Some(0));
        assert_eq!(l.linea_en(DurationMs::new(4999)), Some(0));
        assert_eq!(l.linea_en(DurationMs::new(5000)), Some(1));
        assert_eq!(l.linea_en(DurationMs::new(999_999)), Some(2));
    }

    #[test]
    fn una_letra_sin_sincronizar_no_tiene_linea_activa() {
        let l = Lyrics {
            synced: None,
            plain: Some("texto suelto".into()),
            source: "lrclib".into(),
        };
        assert!(!l.tiene_sincronizacion());
        assert!(!l.esta_vacia());
        assert_eq!(l.linea_en(DurationMs::new(1000)), None);
    }

    #[test]
    fn una_letra_en_blanco_se_considera_vacia() {
        let l = Lyrics {
            synced: Some(vec![]),
            plain: Some("   ".into()),
            source: "x".into(),
        };
        assert!(l.esta_vacia());
    }
}

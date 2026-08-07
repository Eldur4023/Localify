//! Conversión a estéreo.
//!
//! Todo el resto del motor —mezclador, ecualizador, limitador, salida— trabaja
//! con dos canales intercalados. Normalizar aquí, en un único sitio, evita que
//! cada etapa tenga que preguntarse cuántos canales le llegan.
//!
//! ## Por qué no basta con quedarse los dos primeros canales
//!
//! En una mezcla 5.1 la voz principal va casi entera en el canal **central**.
//! Quedarse con los dos primeros la haría desaparecer. La mezcla a estéreo usa
//! los coeficientes de la ITU-R BS.775, que es lo que hace cualquier receptor
//! de AV al reproducir 5.1 por dos altavoces.

/// Coeficiente del canal central y de los surround al mezclar a estéreo.
///
/// `1/√2` (−3 dB): al repartir un canal entre dos, mantiene la potencia.
const ATENUACION: f32 = std::f32::consts::FRAC_1_SQRT_2;

/// De dónde sale cada canal de la fuente.
///
/// Se resuelve una sola vez al abrir el fichero, no por bloque.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mezcla {
    /// Un canal: se copia a los dos.
    Mono,
    /// Ya son dos: se pasa tal cual.
    Estereo,
    /// Más de dos: se aplica la mezcla estándar.
    Multicanal {
        canales: usize,
        izquierdo: usize,
        derecho: usize,
        central: Option<usize>,
        surround_izq: Option<usize>,
        surround_der: Option<usize>,
    },
}

impl Mezcla {
    /// Decide la mezcla a partir del número de canales y sus posiciones.
    ///
    /// `posiciones` trae, para cada canal de la fuente, su papel. Si el
    /// contenedor no lo declara, se asume el orden canónico
    /// (FL, FR, FC, LFE, BL, BR), que es el que usan WAV, FLAC y Vorbis.
    #[must_use]
    pub fn decidir(canales: usize, posiciones: Option<&[Posicion]>) -> Self {
        match canales {
            0 | 1 => Self::Mono,
            2 => Self::Estereo,
            _ => {
                let buscar =
                    |p: Posicion| -> Option<usize> { posiciones?.iter().position(|x| *x == p) };
                Self::Multicanal {
                    canales,
                    izquierdo: buscar(Posicion::FrontalIzq).unwrap_or(0),
                    derecho: buscar(Posicion::FrontalDer).unwrap_or(1),
                    // Sin mapa de canales se asume el orden canónico, donde el
                    // central es el tercero y los traseros el quinto y sexto.
                    central: buscar(Posicion::FrontalCentro)
                        .or(Some(2))
                        .filter(|i| *i < canales),
                    surround_izq: buscar(Posicion::TraseroIzq)
                        .or(Some(4))
                        .filter(|i| *i < canales),
                    surround_der: buscar(Posicion::TraseroDer)
                        .or(Some(5))
                        .filter(|i| *i < canales),
                }
            }
        }
    }

    /// Canales que consume de la fuente.
    #[must_use]
    pub const fn canales_origen(&self) -> usize {
        match self {
            Self::Mono => 1,
            Self::Estereo => 2,
            Self::Multicanal { canales, .. } => *canales,
        }
    }

    /// Convierte `origen` (intercalado, `canales_origen()` canales) a estéreo
    /// intercalado, **añadiendo** al final de `destino`.
    pub fn aplicar(&self, origen: &[f32], destino: &mut Vec<f32>) {
        match self {
            Self::Mono => {
                destino.reserve(origen.len() * 2);
                for m in origen {
                    destino.push(*m);
                    destino.push(*m);
                }
            }
            Self::Estereo => destino.extend_from_slice(origen),
            Self::Multicanal {
                canales,
                izquierdo,
                derecho,
                central,
                surround_izq,
                surround_der,
            } => {
                let n = *canales;
                destino.reserve(origen.len() / n * 2);
                for marco in origen.chunks_exact(n) {
                    let c = central.map_or(0.0, |i| marco[i] * ATENUACION);
                    let si = surround_izq.map_or(0.0, |i| marco[i] * ATENUACION);
                    let sd = surround_der.map_or(0.0, |i| marco[i] * ATENUACION);
                    // El LFE se descarta a propósito: son frecuencias que unos
                    // altavoces de escritorio no reproducen, y sumarlas solo
                    // haría trabajar al limitador.
                    destino.push(marco[*izquierdo] + c + si);
                    destino.push(marco[*derecho] + c + sd);
                }
            }
        }
    }
}

/// Papel de un canal dentro de la mezcla.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Posicion {
    FrontalIzq,
    FrontalDer,
    FrontalCentro,
    Lfe,
    TraseroIzq,
    TraseroDer,
    Otro,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn el_mono_suena_por_los_dos_altavoces() {
        let m = Mezcla::decidir(1, None);
        let mut salida = Vec::new();
        m.aplicar(&[0.5, -0.25], &mut salida);
        assert_eq!(salida, vec![0.5, 0.5, -0.25, -0.25]);
    }

    #[test]
    fn el_estereo_pasa_intacto() {
        let m = Mezcla::decidir(2, None);
        let mut salida = Vec::new();
        m.aplicar(&[0.1, 0.2, 0.3, 0.4], &mut salida);
        assert_eq!(salida, vec![0.1, 0.2, 0.3, 0.4]);
    }

    #[test]
    fn el_canal_central_no_desaparece_en_una_mezcla_multicanal() {
        // Es el fallo que se evita: en 5.1 la voz principal va en el central.
        // Quedarse con los dos primeros canales la borraria por completo.
        let m = Mezcla::decidir(6, None);
        let mut salida = Vec::new();
        // Frontales en silencio, voz solo en el central.
        m.aplicar(&[0.0, 0.0, 1.0, 0.0, 0.0, 0.0], &mut salida);

        assert!(salida[0] > 0.5, "la voz debe llegar al canal izquierdo");
        assert!(salida[1] > 0.5, "y al derecho");
        assert!(
            (salida[0] - salida[1]).abs() < 1e-6,
            "el central debe repartirse por igual"
        );
    }

    #[test]
    fn el_central_se_atenua_para_no_ganar_potencia() {
        let m = Mezcla::decidir(6, None);
        let mut salida = Vec::new();
        m.aplicar(&[0.0, 0.0, 1.0, 0.0, 0.0, 0.0], &mut salida);
        assert!(
            (salida[0] - ATENUACION).abs() < 1e-6,
            "esperado -3 dB, obtenido {}",
            salida[0]
        );
    }

    #[test]
    fn el_lfe_se_descarta() {
        let m = Mezcla::decidir(6, None);
        let mut salida = Vec::new();
        m.aplicar(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0], &mut salida);
        assert_eq!(salida, vec![0.0, 0.0], "el LFE no debe sumarse a la mezcla");
    }

    #[test]
    fn los_surround_van_a_su_lado() {
        let m = Mezcla::decidir(6, None);
        let mut salida = Vec::new();
        m.aplicar(&[0.0, 0.0, 0.0, 0.0, 1.0, 0.0], &mut salida);
        assert!(salida[0] > 0.5, "el surround izquierdo va al izquierdo");
        assert!(salida[1].abs() < 1e-6, "y no al derecho");
    }

    #[test]
    fn se_respeta_el_mapa_de_canales_cuando_el_contenedor_lo_declara() {
        // Un contenedor puede ordenarlos de otra forma. Asumir el orden
        // canonico entonces intercambiaria los altavoces.
        let posiciones = [
            Posicion::FrontalCentro,
            Posicion::FrontalIzq,
            Posicion::FrontalDer,
            Posicion::Lfe,
        ];
        let m = Mezcla::decidir(4, Some(&posiciones));
        let mut salida = Vec::new();
        // Senal solo en el canal 1, que aqui es el frontal izquierdo.
        m.aplicar(&[0.0, 1.0, 0.0, 0.0], &mut salida);
        assert!((salida[0] - 1.0).abs() < 1e-6);
        assert!(salida[1].abs() < 1e-6);
    }

    #[test]
    fn una_mezcla_de_tres_canales_no_indexa_fuera_de_rango() {
        // Con 3 canales no hay traseros. Sin el filtro por indice, esto
        // entraria en panico en el hilo de decodificacion.
        let m = Mezcla::decidir(3, None);
        let mut salida = Vec::new();
        m.aplicar(&[0.2, 0.3, 0.4], &mut salida);
        assert_eq!(salida.len(), 2);
        assert!(salida.iter().all(|v| v.is_finite()));
    }

    #[test]
    fn cero_canales_se_trata_como_mono() {
        // Un contenedor corrupto puede declarar cero. Es preferible sonar en
        // mono que entrar en panico o dividir por cero.
        assert_eq!(Mezcla::decidir(0, None), Mezcla::Mono);
    }
}

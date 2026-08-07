//! Rampas de fundido cruzado de potencia constante.
//!
//! ## Por qué no una rampa lineal
//!
//! Si dos pistas no correlacionadas se funden con rampas lineales (`1−t` y
//! `t`), a mitad del fundido cada una vale 0.5, y la **potencia** total —que va
//! con el cuadrado— cae a 0.5 en vez de 1. Se oye como un bajón de volumen
//! justo en medio de cada transición.
//!
//! Con `cos(π t/2)` y `sin(π t/2)` la suma de cuadrados vale 1 en todo el
//! recorrido, porque `cos² + sin² = 1`. La potencia se mantiene y el fundido es
//! inaudible como tal.
//!
//! ## El caso de duración cero
//!
//! Es el modo *gapless*, no un caso degenerado: el corte es inmediato y sin
//! hueco. Se apoya en que la voz siguiente ya está decodificada y lista, que es
//! justo para lo que existe la segunda voz.

/// Fundido en curso entre dos voces.
///
/// Avanza por marcos, no por tiempo de reloj: el hilo de audio no mira el
/// reloj, y contar marcos es exacto por construcción.
#[derive(Debug, Clone, Copy)]
pub struct Crossfade {
    marcos_totales: u32,
    marcos_hechos: u32,
}

impl Crossfade {
    /// Prepara un fundido de `duracion_marcos`. Con cero, es un corte seco.
    #[must_use]
    pub const fn nuevo(duracion_marcos: u32) -> Self {
        Self {
            marcos_totales: duracion_marcos,
            marcos_hechos: 0,
        }
    }

    /// Ganancias `(saliente, entrante)` del marco actual.
    ///
    /// Solo lee: consultar dos veces el mismo marco da lo mismo. Avanzar es
    /// cosa de [`Self::avanzar`].
    #[inline]
    #[must_use]
    pub fn ganancias(&self) -> (f32, f32) {
        if self.marcos_totales == 0 {
            return (0.0, 1.0);
        }
        if self.marcos_hechos >= self.marcos_totales {
            return (0.0, 1.0);
        }
        #[allow(
            clippy::cast_precision_loss,
            reason = "12 s a 96 kHz son 1.15 M marcos, exactos en f32"
        )]
        let t = self.marcos_hechos as f32 / self.marcos_totales as f32;
        let angulo = t * std::f32::consts::FRAC_PI_2;
        let (sin, cos) = angulo.sin_cos();
        (cos, sin)
    }

    /// Consume un marco.
    #[inline]
    pub const fn avanzar(&mut self) {
        if self.marcos_hechos < self.marcos_totales {
            self.marcos_hechos += 1;
        }
    }

    /// Consume `n` marcos de golpe, para cuando un bloque se procesa entero.
    #[inline]
    pub const fn avanzar_n(&mut self, n: u32) {
        self.marcos_hechos = if self.marcos_hechos.saturating_add(n) > self.marcos_totales {
            self.marcos_totales
        } else {
            self.marcos_hechos + n
        };
    }

    /// `true` cuando la voz saliente ya no aporta nada y se puede liberar.
    #[inline]
    #[must_use]
    pub const fn ha_terminado(&self) -> bool {
        self.marcos_hechos >= self.marcos_totales
    }

    /// Marcos que quedan.
    #[must_use]
    pub const fn restantes(&self) -> u32 {
        self.marcos_totales.saturating_sub(self.marcos_hechos)
    }
}

/// Convierte una duración en marcos a la frecuencia de muestreo dada.
///
/// Satura en vez de desbordar. El ajuste llega como mucho a 12 s, pero un
/// valor absurdo por una configuración corrupta debe dar un fundido larguísimo,
/// no uno de cero marcos por dar la vuelta el contador.
#[must_use]
pub const fn marcos_de(duracion_ms: u32, sample_rate: u32) -> u32 {
    // En u64 para que el producto no desborde a mitad del cálculo.
    let marcos = (duracion_ms as u64 * sample_rate as u64) / 1000;
    if marcos > u32::MAX as u64 {
        u32::MAX
    } else {
        #[allow(
            clippy::cast_possible_truncation,
            reason = "la rama comprueba que cabe"
        )]
        {
            marcos as u32
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn con_duracion_cero_el_cambio_es_inmediato() {
        // Es el modo gapless: la voz nueva entra a plena ganancia desde el
        // primer marco, sin un solo marco de silencio.
        let f = Crossfade::nuevo(0);
        assert_eq!(f.ganancias(), (0.0, 1.0));
        assert!(f.ha_terminado());
    }

    #[test]
    fn empieza_con_la_saliente_y_acaba_con_la_entrante() {
        let mut f = Crossfade::nuevo(1000);
        let (sale, entra) = f.ganancias();
        assert!((sale - 1.0).abs() < 1e-6, "deberia empezar con la saliente");
        assert!(entra.abs() < 1e-6);

        f.avanzar_n(1000);
        let (sale, entra) = f.ganancias();
        assert!(sale.abs() < 1e-6);
        assert!((entra - 1.0).abs() < 1e-6);
    }

    #[test]
    fn la_potencia_se_mantiene_constante_durante_todo_el_fundido() {
        // Es la razon de ser del fundido equal-power. Con rampas lineales,
        // esta suma bajaria a 0.5 en el punto medio y se oiria el bajon.
        let mut f = Crossfade::nuevo(4800);
        for _ in 0..=4800 {
            let (a, b) = f.ganancias();
            let potencia = a * a + b * b;
            assert!(
                (potencia - 1.0).abs() < 1e-5,
                "potencia {potencia} en el marco {}",
                f.marcos_hechos
            );
            f.avanzar();
        }
    }

    #[test]
    fn en_el_punto_medio_las_dos_voces_suenan_por_igual() {
        let mut f = Crossfade::nuevo(1000);
        f.avanzar_n(500);
        let (a, b) = f.ganancias();
        assert!((a - b).abs() < 1e-5, "{a} != {b}");
        assert!(
            (a - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-5,
            "en el medio cada voz debe valer 1/raiz(2), no 0.5"
        );
    }

    #[test]
    fn las_ganancias_son_monotonas() {
        // Un vaiven en la rampa se oiria como un temblor del volumen.
        let mut f = Crossfade::nuevo(2000);
        let (mut sale_ant, mut entra_ant) = f.ganancias();
        for _ in 0..2000 {
            f.avanzar();
            let (sale, entra) = f.ganancias();
            assert!(sale <= sale_ant + 1e-6, "la saliente subio");
            assert!(entra >= entra_ant - 1e-6, "la entrante bajo");
            sale_ant = sale;
            entra_ant = entra;
        }
    }

    #[test]
    fn consultar_no_avanza() {
        let f = Crossfade::nuevo(100);
        let primera = f.ganancias();
        assert_eq!(f.ganancias(), primera);
        assert_eq!(f.restantes(), 100);
    }

    #[test]
    fn avanzar_de_mas_no_desborda() {
        let mut f = Crossfade::nuevo(10);
        f.avanzar_n(u32::MAX);
        assert!(f.ha_terminado());
        assert_eq!(f.restantes(), 0);
        assert_eq!(f.ganancias(), (0.0, 1.0));
    }

    #[test]
    fn la_conversion_a_marcos_es_exacta_en_los_casos_habituales() {
        assert_eq!(marcos_de(0, 48_000), 0);
        assert_eq!(marcos_de(1000, 48_000), 48_000);
        assert_eq!(marcos_de(3000, 44_100), 132_300);
        assert_eq!(marcos_de(12_000, 192_000), 2_304_000);
    }

    #[test]
    fn doce_segundos_a_la_maxima_tasa_no_desbordan() {
        // El limite superior del ajuste. Si esto desbordara, el fundido mas
        // largo del reproductor daria una duracion absurda.
        let marcos = marcos_de(12_000, 192_000);
        let mut f = Crossfade::nuevo(marcos);
        f.avanzar_n(marcos / 2);
        let (a, b) = f.ganancias();
        assert!((a * a + b * b - 1.0).abs() < 1e-5);
    }
}

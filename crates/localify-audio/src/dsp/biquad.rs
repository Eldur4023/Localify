//! Filtro biquad de segundo orden, forma directa transpuesta II.
//!
//! Es el ladrillo del ecualizador: diez de estos en cascada por canal.
//!
//! ## Por qué la forma transpuesta II
//!
//! De las cuatro formas canónicas, la transpuesta II es la que menos ruido de
//! redondeo acumula en aritmética de coma flotante, y solo necesita dos
//! variables de estado por canal. En un filtro que corre 48 000 veces por
//! segundo y por canal, esas dos cosas importan.
//!
//! ## Reparto de responsabilidades
//!
//! Calcular coeficientes es caro (dos `sin`, un `cos`, una raíz). Aplicarlos es
//! barato (cinco multiplicaciones y cuatro sumas). Por eso [`Coeficientes`] y
//! [`Biquad`] están separados: los coeficientes se calculan al cambiar de
//! perfil, fuera del hilo de audio, y el callback solo aplica.

/// Coeficientes normalizados (ya divididos por `a0`).
///
/// Se copian enteros en el hilo de audio, así que son `Copy` y pequeños.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Coeficientes {
    pub b0: f32,
    pub b1: f32,
    pub b2: f32,
    pub a1: f32,
    pub a2: f32,
}

impl Coeficientes {
    /// Filtro que deja pasar la señal sin tocarla.
    pub const PASO: Self = Self {
        b0: 1.0,
        b1: 0.0,
        b2: 0.0,
        a1: 0.0,
        a2: 0.0,
    };

    /// Peaking EQ: realza o atenúa alrededor de `centro_hz` sin tocar el resto.
    ///
    /// Las fórmulas son las del *Audio EQ Cookbook* de Robert Bristow-Johnson,
    /// que es la referencia que implementan todos los ecualizadores gráficos.
    ///
    /// `q` controla la anchura: más alto, más estrecho. 1.41 reparte el trabajo
    /// de forma razonable entre bandas separadas por una octava, que es como
    /// están repartidas las diez de [`localify_core::domain::audio::BANDAS_EQ_HZ`].
    ///
    /// Devuelve [`Self::PASO`] si la banda no es realizable a esta frecuencia
    /// de muestreo: por encima de Nyquist las fórmulas producen coeficientes
    /// inestables, y un filtro inestable no atenúa, **explota**. La banda de
    /// 16 kHz con un dispositivo a 22 050 Hz es exactamente ese caso.
    #[must_use]
    pub fn peaking(centro_hz: f32, ganancia_db: f32, q: f32, sample_rate: u32) -> Self {
        #[allow(clippy::cast_precision_loss, reason = "48000 cabe exacto en f32")]
        let sr = sample_rate as f32;

        // Margen del 5 % por debajo de Nyquist: justo en el límite, `tan`
        // tiende a infinito y los coeficientes pierden todo el sentido.
        if !centro_hz.is_finite() || centro_hz <= 0.0 || centro_hz >= sr * 0.475 {
            return Self::PASO;
        }
        if !ganancia_db.is_finite() || ganancia_db.abs() < 1e-6 {
            return Self::PASO;
        }

        let a = 10.0_f32.powf(ganancia_db / 40.0);
        let w0 = 2.0 * std::f32::consts::PI * centro_hz / sr;
        let (sin_w0, cos_w0) = w0.sin_cos();
        let alpha = sin_w0 / (2.0 * q);

        let a0 = 1.0 + alpha / a;

        Self {
            b0: (1.0 + alpha * a) / a0,
            b1: (-2.0 * cos_w0) / a0,
            b2: (1.0 - alpha * a) / a0,
            a1: (-2.0 * cos_w0) / a0,
            a2: (1.0 - alpha / a) / a0,
        }
    }

    /// `true` si el filtro no altera la señal.
    #[must_use]
    pub fn es_paso(&self) -> bool {
        *self == Self::PASO
    }
}

/// Un biquad con su estado. Uno por banda **y por canal**: compartir el estado
/// entre canales mezclaría el izquierdo con el derecho.
#[derive(Debug, Clone, Copy, Default)]
pub struct Biquad {
    z1: f32,
    z2: f32,
}

impl Biquad {
    #[must_use]
    pub const fn nuevo() -> Self {
        Self { z1: 0.0, z2: 0.0 }
    }

    /// Procesa una muestra.
    ///
    /// Sin asignaciones, sin ramas de datos, sin llamadas: apto para el hilo de
    /// tiempo real.
    #[inline]
    #[must_use]
    pub fn procesar(&mut self, x: f32, c: &Coeficientes) -> f32 {
        let y = c.b0 * x + self.z1;
        self.z1 = c.b1 * x - c.a1 * y + self.z2;
        self.z2 = c.b2 * x - c.a2 * y;
        y
    }

    /// Vacía el estado. Se llama al cambiar de pista: arrastrar el estado de la
    /// canción anterior produce un chasquido en la primera muestra.
    pub const fn reiniciar(&mut self) {
        self.z1 = 0.0;
        self.z2 = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: u32 = 48_000;

    /// Amplitud de salida en régimen permanente para una senoide de `hz`.
    ///
    /// Medir la respuesta real y no comparar coeficientes es lo que hace que
    /// estos tests detecten un error de signo o un `a1` intercambiado.
    fn respuesta(c: &Coeficientes, hz: f32) -> f32 {
        let mut f = Biquad::nuevo();
        #[allow(clippy::cast_precision_loss, reason = "48000 cabe exacto en f32")]
        let sr = SR as f32;
        let total = SR as usize / 4;
        // El primer cuarto se descarta: es el transitorio del filtro.
        let arranque = total / 2;

        let mut pico: f32 = 0.0;
        for n in 0..total {
            #[allow(clippy::cast_precision_loss, reason = "n < 12000")]
            let t = n as f32 / sr;
            let y = f.procesar((2.0 * std::f32::consts::PI * hz * t).sin(), c);
            if n >= arranque {
                pico = pico.max(y.abs());
            }
        }
        pico
    }

    fn a_db(amplitud: f32) -> f32 {
        20.0 * amplitud.log10()
    }

    #[test]
    fn el_filtro_paso_no_altera_la_senal() {
        let mut f = Biquad::nuevo();
        for x in [0.0, 1.0, -0.5, 0.25, -1.0] {
            assert!((f.procesar(x, &Coeficientes::PASO) - x).abs() < 1e-6);
        }
    }

    #[test]
    fn un_realce_de_seis_db_sube_seis_db_en_su_frecuencia() {
        let c = Coeficientes::peaking(1000.0, 6.0, 1.41, SR);
        let ganancia = a_db(respuesta(&c, 1000.0));
        assert!(
            (ganancia - 6.0).abs() < 0.5,
            "esperados +6 dB en 1 kHz, medidos {ganancia:.2} dB"
        );
    }

    #[test]
    fn una_atenuacion_de_seis_db_baja_seis_db() {
        let c = Coeficientes::peaking(1000.0, -6.0, 1.41, SR);
        let ganancia = a_db(respuesta(&c, 1000.0));
        assert!(
            (ganancia + 6.0).abs() < 0.5,
            "esperados -6 dB en 1 kHz, medidos {ganancia:.2} dB"
        );
    }

    #[test]
    fn el_realce_no_se_extiende_a_frecuencias_lejanas() {
        // Si tocar los graves subiera también los agudos, el ecualizador
        // seria un control de volumen con pasos raros.
        let c = Coeficientes::peaking(1000.0, 12.0, 1.41, SR);
        let lejos = a_db(respuesta(&c, 60.0));
        assert!(
            lejos.abs() < 1.0,
            "a 60 Hz deberia notarse poco, medido {lejos:.2} dB"
        );
    }

    #[test]
    fn una_banda_por_encima_de_nyquist_no_desestabiliza_el_filtro() {
        // A 22 050 Hz, la banda de 16 kHz esta por encima del limite util. Sin
        // esta guarda, los coeficientes divergen y la salida se va a infinito.
        let c = Coeficientes::peaking(16_000.0, 12.0, 1.41, 22_050);
        assert!(c.es_paso(), "la banda irrealizable debe neutralizarse");

        let mut f = Biquad::nuevo();
        let mut salida = 0.0_f32;
        for n in 0..10_000 {
            #[allow(clippy::cast_precision_loss, reason = "n < 10000")]
            let t = n as f32 / 22_050.0;
            salida = f.procesar((2.0 * std::f32::consts::PI * 1000.0 * t).sin(), &c);
        }
        assert!(
            salida.is_finite() && salida.abs() <= 1.001,
            "salida {salida}"
        );
    }

    #[test]
    fn ganancia_cero_equivale_a_no_filtrar() {
        // Importa por rendimiento: el perfil plano es el caso comun, y saltarse
        // diez biquads por canal ahorra trabajo en cada muestra.
        assert!(Coeficientes::peaking(1000.0, 0.0, 1.41, SR).es_paso());
    }

    #[test]
    fn el_filtro_es_estable_con_la_ganancia_maxima_en_todas_las_bandas() {
        use localify_core::domain::audio::{BANDAS_EQ_HZ, GANANCIA_MAX_DB};

        for sr in [22_050, 44_100, 48_000, 96_000] {
            for hz in BANDAS_EQ_HZ {
                let c = Coeficientes::peaking(hz, GANANCIA_MAX_DB, 1.41, sr);
                let mut f = Biquad::nuevo();
                let mut ultima = 0.0_f32;
                for n in 0..20_000 {
                    #[allow(clippy::cast_precision_loss, reason = "n < 20000")]
                    let t = n as f32;
                    // Impulso seguido de silencio: si el filtro es inestable,
                    // la cola crece en vez de apagarse.
                    let x = if n == 0 { 1.0 } else { 0.0 };
                    ultima = f.procesar(x, &c);
                    let _ = t;
                }
                assert!(
                    ultima.abs() < 1e-3,
                    "la respuesta al impulso no se apaga: {hz} Hz a {sr} Hz -> {ultima}"
                );
            }
        }
    }

    #[test]
    fn reiniciar_borra_la_cola_del_filtro() {
        let c = Coeficientes::peaking(1000.0, 12.0, 1.41, SR);
        let mut f = Biquad::nuevo();
        let _ = f.procesar(1.0, &c);

        f.reiniciar();
        assert!(
            f.procesar(0.0, &c).abs() < f32::EPSILON,
            "tras reiniciar, una entrada nula debe dar salida nula"
        );
    }
}

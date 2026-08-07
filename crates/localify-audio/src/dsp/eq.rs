//! Ecualizador gráfico de diez bandas.
//!
//! ## El problema del cambio de perfil
//!
//! Calcular diez juegos de coeficientes por canal cuesta veinte funciones
//! trigonométricas. Eso no puede pasar en el callback de audio. Pero el
//! callback tampoco puede tomar un lock para leer los coeficientes nuevos: si
//! el hilo que escribe se ve interrumpido por el planificador mientras tiene el
//! lock, el callback se pierde su plazo y se oye un corte.
//!
//! La solución es la que describe el diseño: los coeficientes se calculan
//! entero fuera del hilo de audio y se publican con un intercambio atómico
//! ([`EqCompartido`]). El callback lee un puntero, y si ha cambiado, se queda
//! con el nuevo. Nunca espera a nadie.
//!
//! El estado de los filtros (`z1`, `z2`) **no** viaja en ese intercambio: vive
//! en el callback, porque es suyo y de nadie más.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use localify_core::domain::audio::{BANDAS_EQ_HZ, EqProfile};

use super::biquad::{Biquad, Coeficientes};

/// Anchura de cada banda. Con bandas separadas por una octava, 1.41 (≈ √2) es
/// el valor que hace que las curvas contiguas se crucen a mitad de camino: ni
/// dejan un hueco entre ellas ni se suman en exceso al subirlas todas.
const Q: f32 = 1.41;

/// Número de bandas. Es una constante del dominio, no una elección del motor.
pub const BANDAS: usize = BANDAS_EQ_HZ.len();

/// Los coeficientes de un perfil ya resueltos para una frecuencia de muestreo.
#[derive(Debug, Clone)]
pub struct CoeficientesEq {
    bandas: [Coeficientes; BANDAS],
    /// `true` si ninguna banda altera la señal. El callback lo consulta para
    /// saltarse el filtrado entero, que es el caso más común.
    plano: bool,
}

impl CoeficientesEq {
    #[must_use]
    pub fn calcular(perfil: &EqProfile, sample_rate: u32) -> Self {
        let mut bandas = [Coeficientes::PASO; BANDAS];
        for (i, c) in bandas.iter_mut().enumerate() {
            *c = Coeficientes::peaking(BANDAS_EQ_HZ[i], perfil.gains_db[i], Q, sample_rate);
        }
        let plano = bandas.iter().all(Coeficientes::es_paso);
        Self { bandas, plano }
    }

    #[must_use]
    pub fn plano() -> Self {
        Self {
            bandas: [Coeficientes::PASO; BANDAS],
            plano: true,
        }
    }

    #[must_use]
    pub const fn es_plano(&self) -> bool {
        self.plano
    }
}

/// Los coeficientes vigentes, publicables desde otro hilo.
///
/// `arc-swap` haría esto mismo, pero traer una dependencia para un
/// `Mutex<Arc<_>>` que solo se toma fuera del hilo de audio no compensa: el
/// callback usa [`EstadoEq::actualizar_si_procede`], que **no** toma el lock
/// salvo cuando la bandera dice que hay algo nuevo.
#[derive(Debug)]
pub struct EqCompartido {
    hay_novedad: AtomicBool,
    actual: std::sync::Mutex<Arc<CoeficientesEq>>,
}

impl EqCompartido {
    #[must_use]
    pub fn nuevo() -> Self {
        Self {
            hay_novedad: AtomicBool::new(false),
            actual: std::sync::Mutex::new(Arc::new(CoeficientesEq::plano())),
        }
    }

    /// Publica un perfil nuevo. Se llama desde el hilo de control.
    pub fn publicar(&self, perfil: &EqProfile, sample_rate: u32) {
        let nuevos = Arc::new(CoeficientesEq::calcular(perfil, sample_rate));
        if let Ok(mut g) = self.actual.lock() {
            *g = nuevos;
            self.hay_novedad.store(true, Ordering::Release);
        }
    }

    /// Recoge los coeficientes si hay novedad. Devuelve `None` si no la hay.
    ///
    /// El caso normal —sin cambios— es una lectura atómica y nada más. Solo
    /// cuando el usuario mueve el ecualizador se intenta tomar el lock, y con
    /// `try_lock`: si estuviera ocupado, el callback sigue con los coeficientes
    /// de antes y lo recoge en la siguiente vuelta, 10 ms después. Inaudible, y
    /// preferible a bloquear.
    #[must_use]
    pub fn recoger(&self) -> Option<Arc<CoeficientesEq>> {
        if !self.hay_novedad.load(Ordering::Acquire) {
            return None;
        }
        let g = self.actual.try_lock().ok()?;
        self.hay_novedad.store(false, Ordering::Release);
        Some(Arc::clone(&g))
    }
}

impl Default for EqCompartido {
    fn default() -> Self {
        Self::nuevo()
    }
}

/// El ecualizador tal y como lo ve el callback: coeficientes vigentes más el
/// estado de los filtros de cada canal.
#[derive(Debug)]
pub struct EstadoEq {
    coeficientes: Arc<CoeficientesEq>,
    /// `[canal][banda]`. Dos canales: la salida siempre se mezcla a estéreo.
    filtros: [[Biquad; BANDAS]; 2],
}

impl EstadoEq {
    #[must_use]
    pub fn nuevo() -> Self {
        Self {
            coeficientes: Arc::new(CoeficientesEq::plano()),
            filtros: [[Biquad::nuevo(); BANDAS]; 2],
        }
    }

    /// Recoge coeficientes nuevos si los hay. Llamar **una vez por bloque**,
    /// no por muestra.
    ///
    /// Cambiar de coeficientes a mitad de bloque no rompe nada: los biquads
    /// conservan su estado, así que la transición es continua. No hace falta
    /// reiniciarlos ni rampar.
    pub fn actualizar_si_procede(&mut self, compartido: &EqCompartido) {
        if let Some(nuevos) = compartido.recoger() {
            self.coeficientes = nuevos;
        }
    }

    /// `true` si procesar no cambiaría nada.
    #[must_use]
    pub fn es_plano(&self) -> bool {
        self.coeficientes.es_plano()
    }

    /// Filtra un bloque estéreo intercalado, en el sitio.
    ///
    /// Sin asignaciones ni locks: apto para el hilo de tiempo real.
    pub fn procesar(&mut self, intercalado: &mut [f32]) {
        if self.coeficientes.es_plano() {
            return;
        }
        let bandas = &self.coeficientes.bandas;

        for marco in intercalado.chunks_exact_mut(2) {
            for (canal, muestra) in marco.iter_mut().enumerate() {
                let filtros = &mut self.filtros[canal];
                let mut x = *muestra;
                for (f, c) in filtros.iter_mut().zip(bandas.iter()) {
                    x = f.procesar(x, c);
                }
                *muestra = x;
            }
        }
    }

    /// Vacía el estado de todos los filtros. Al cambiar de pista sin fundido,
    /// la cola de la anterior sonaría como un chasquido sobre la nueva.
    pub fn reiniciar(&mut self) {
        for canal in &mut self.filtros {
            for f in canal {
                f.reiniciar();
            }
        }
    }
}

impl Default for EstadoEq {
    fn default() -> Self {
        Self::nuevo()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: u32 = 48_000;

    /// Genera un bloque estéreo con una senoide de `hz` en ambos canales.
    fn senoide(hz: f32, marcos: usize) -> Vec<f32> {
        #[allow(clippy::cast_precision_loss, reason = "48000 cabe exacto en f32")]
        let sr = SR as f32;
        (0..marcos)
            .flat_map(|n| {
                #[allow(clippy::cast_precision_loss, reason = "marcos acotado en tests")]
                let t = n as f32 / sr;
                let v = (2.0 * std::f32::consts::PI * hz * t).sin();
                [v, v]
            })
            .collect()
    }

    /// Valor eficaz de la segunda mitad, ya pasado el transitorio.
    ///
    /// Se mide RMS y no pico a propósito. A 8 kHz muestreados a 48 kHz solo hay
    /// seis muestras por ciclo, y ninguna tiene por qué caer en la cresta: el
    /// pico medido puede quedarse 1.25 dB corto por dónde caen las muestras, no
    /// por lo que haga el filtro. El RMS no depende de eso.
    fn rms(bloque: &[f32]) -> f32 {
        let cola = &bloque[bloque.len() / 2..];
        #[allow(clippy::cast_precision_loss, reason = "bloques acotados en tests")]
        let n = cola.len() as f32;
        (cola.iter().map(|v| v * v).sum::<f32>() / n).sqrt()
    }

    /// Ganancia en dB que el ecualizador aplica a una senoide de `hz`.
    fn ganancia_db(eq: &mut EstadoEq, hz: f32) -> f32 {
        let entrada = senoide(hz, 24_000);
        let mut salida = entrada.clone();
        eq.procesar(&mut salida);
        20.0 * (rms(&salida) / rms(&entrada)).log10()
    }

    fn perfil(ganancias: [f32; 10]) -> EqProfile {
        EqProfile::new("test", "test", ganancias).expect("ganancias validas")
    }

    #[test]
    fn el_perfil_plano_no_toca_la_senal() {
        let mut eq = EstadoEq::nuevo();
        let original = senoide(1000.0, 4096);
        let mut bloque = original.clone();
        eq.procesar(&mut bloque);
        assert_eq!(bloque, original, "plano debe ser bit a bit identico");
    }

    #[test]
    fn subir_la_banda_de_mil_hercios_sube_una_senoide_de_mil_hercios() {
        let compartido = EqCompartido::nuevo();
        let mut ganancias = [0.0; 10];
        ganancias[5] = 6.0; // 1000 Hz
        compartido.publicar(&perfil(ganancias), SR);

        let mut eq = EstadoEq::nuevo();
        eq.actualizar_si_procede(&compartido);

        let db = ganancia_db(&mut eq, 1000.0);
        assert!(
            (db - 6.0).abs() < 1.0,
            "esperados +6 dB, medidos {db:.2} dB"
        );
    }

    #[test]
    fn subir_los_graves_no_toca_los_agudos() {
        let compartido = EqCompartido::nuevo();
        let mut ganancias = [0.0; 10];
        ganancias[0] = 12.0; // 31 Hz
        ganancias[1] = 12.0; // 62 Hz
        compartido.publicar(&perfil(ganancias), SR);

        let mut eq = EstadoEq::nuevo();
        eq.actualizar_si_procede(&compartido);

        let db = ganancia_db(&mut eq, 8000.0);
        assert!(db.abs() < 0.5, "8 kHz deberia quedar intacto, {db:.2} dB");
    }

    #[test]
    fn los_canales_se_filtran_por_separado() {
        // Compartir el estado entre canales mezclaria el izquierdo con el
        // derecho: en una grabacion con instrumentos panoramizados se oiria.
        let compartido = EqCompartido::nuevo();
        let mut ganancias = [0.0; 10];
        ganancias[5] = 12.0;
        compartido.publicar(&perfil(ganancias), SR);

        let mut eq = EstadoEq::nuevo();
        eq.actualizar_si_procede(&compartido);

        // Izquierdo con señal, derecho en silencio.
        let mut bloque: Vec<f32> = (0..8192)
            .flat_map(|n| {
                #[allow(clippy::cast_precision_loss, reason = "n < 8192")]
                let t = n as f32 / 48_000.0;
                [(2.0 * std::f32::consts::PI * 1000.0 * t).sin(), 0.0]
            })
            .collect();
        eq.procesar(&mut bloque);

        let derechos: f32 = bloque
            .iter()
            .skip(1)
            .step_by(2)
            .fold(0.0_f32, |m, v| m.max(v.abs()));
        assert!(
            derechos < 1e-6,
            "el canal en silencio se contamino: {derechos}"
        );
    }

    #[test]
    fn sin_novedad_no_se_recoge_nada() {
        // El caso comun del callback: una lectura atomica y a otra cosa.
        let compartido = EqCompartido::nuevo();
        assert!(compartido.recoger().is_none());

        compartido.publicar(&perfil([0.0; 10]), SR);
        assert!(compartido.recoger().is_some());
        assert!(
            compartido.recoger().is_none(),
            "recoger dos veces no debe repetir la novedad"
        );
    }

    #[test]
    fn un_perfil_plano_publicado_se_detecta_como_plano() {
        let compartido = EqCompartido::nuevo();
        compartido.publicar(&EqProfile::plano(), SR);
        let mut eq = EstadoEq::nuevo();
        eq.actualizar_si_procede(&compartido);
        assert!(eq.es_plano(), "es la ruta rapida del callback");
    }

    #[test]
    fn cambiar_de_perfil_no_produce_un_salto_en_la_senal() {
        // Un salto de amplitud entre bloques se oye como un chasquido. Se parte
        // de UNA senoide continua y se procesa en dos mitades: si se generaran
        // dos senoides independientes, ambas arrancarian en fase cero y el
        // salto medido seria del generador, no del ecualizador.
        let compartido = EqCompartido::nuevo();
        let mut eq = EstadoEq::nuevo();

        let continua = senoide(1000.0, 8192);
        let mitad = continua.len() / 2;

        let mut primera = continua[..mitad].to_vec();
        eq.procesar(&mut primera);

        let mut ganancias = [0.0; 10];
        ganancias[5] = 12.0;
        compartido.publicar(&perfil(ganancias), SR);
        eq.actualizar_si_procede(&compartido);

        let mut segunda = continua[mitad..].to_vec();
        eq.procesar(&mut segunda);

        // La senoide misma avanza 0.13 por muestra a 1 kHz; el margen cubre eso
        // mas el transitorio de arranque de los filtros.
        let salto = (segunda[0] - primera[primera.len() - 2]).abs();
        assert!(
            salto < 0.25,
            "discontinuidad de {salto} al cambiar de perfil"
        );
    }

    #[test]
    fn reiniciar_deja_los_filtros_en_silencio() {
        let compartido = EqCompartido::nuevo();
        let mut ganancias = [0.0; 10];
        ganancias[5] = 12.0;
        compartido.publicar(&perfil(ganancias), SR);

        let mut eq = EstadoEq::nuevo();
        eq.actualizar_si_procede(&compartido);
        let mut bloque = senoide(1000.0, 2048);
        eq.procesar(&mut bloque);

        eq.reiniciar();
        let mut silencio = vec![0.0_f32; 512];
        eq.procesar(&mut silencio);
        assert!(
            silencio.iter().all(|v| v.abs() < 1e-9),
            "quedo cola del filtro tras reiniciar"
        );
    }

    #[test]
    fn todos_los_perfiles_de_fabrica_se_calculan_sin_desestabilizarse() {
        for perfil in EqProfile::predefinidos() {
            for sr in [22_050, 44_100, 48_000, 96_000] {
                let compartido = EqCompartido::nuevo();
                compartido.publicar(&perfil, sr);
                let mut eq = EstadoEq::nuevo();
                eq.actualizar_si_procede(&compartido);

                let mut bloque = senoide(1000.0, 8192);
                eq.procesar(&mut bloque);
                assert!(
                    bloque.iter().all(|v| v.is_finite() && v.abs() < 10.0),
                    "el perfil '{}' se desestabiliza a {sr} Hz",
                    perfil.id
                );
            }
        }
    }
}

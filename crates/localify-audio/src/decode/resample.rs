//! Conversión de frecuencia de muestreo.
//!
//! ## Cuándo hace falta
//!
//! Opus siempre decodifica a 48 kHz y la mayoría de dispositivos de Windows
//! trabajan ahí, así que el caso común es **no remuestrear nada**. Pero un FLAC
//! o un MP3 de la biblioteca del usuario suele venir a 44,1 kHz, y ese sí hay
//! que convertirlo.
//!
//! ## Por qué no interpolación lineal
//!
//! 44 100 → 48 000 no es una relación entera. Con interpolación lineal, el
//! error de reconstrucción aparece como distorsión de intermodulación repartida
//! por todo el espectro: se oye como una aspereza en los agudos, y encima es de
//! las cosas más difíciles de atribuir después ("suena raro, no sé por qué").
//!
//! Se usa `rubato` con su remuestreador por FFT, que aplica el filtro
//! antialiasing correcto. Va en el hilo de decodificación, no en el de audio,
//! así que su coste no pone en riesgo ningún plazo de tiempo real.

use audioadapter_buffers::direct::InterleavedSlice;
use rubato::{FixedSync, Resampler};

use localify_core::ports::audio_engine::AudioError;

/// Marcos por llamada. Con 1024 el remuestreador trabaja en bloques
/// suficientemente grandes para ser eficiente y suficientemente pequeños para
/// que el retardo que introduce sea despreciable.
const BLOQUE: usize = 1024;

/// Canales: todo llega aquí ya mezclado a estéreo.
const CANALES: usize = 2;

/// Convierte PCM estéreo intercalado de una frecuencia a otra.
pub struct Remuestreador {
    interior: rubato::Fft<f32>,
    /// Entrada acumulada a la espera de completar un bloque.
    pendiente: Vec<f32>,
    /// Buffer de salida reutilizado, para no asignar por bloque.
    salida: Vec<f32>,
    origen_hz: u32,
    destino_hz: u32,
}

impl std::fmt::Debug for Remuestreador {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Remuestreador")
            .field("origen_hz", &self.origen_hz)
            .field("destino_hz", &self.destino_hz)
            .field("pendiente", &self.pendiente.len())
            .finish_non_exhaustive()
    }
}

impl Remuestreador {
    /// Crea un remuestreador de `origen_hz` a `destino_hz`.
    ///
    /// Devuelve `None` si las dos frecuencias coinciden: no hay nada que hacer,
    /// y pasar la señal por un filtro para dejarla igual solo la degradaría.
    ///
    /// # Errors
    /// Si `rubato` no admite la relación entre ambas frecuencias.
    pub fn nuevo(origen_hz: u32, destino_hz: u32) -> Result<Option<Self>, AudioError> {
        if origen_hz == destino_hz || origen_hz == 0 || destino_hz == 0 {
            return Ok(None);
        }

        let interior = rubato::Fft::<f32>::new(
            origen_hz as usize,
            destino_hz as usize,
            BLOQUE,
            CANALES,
            FixedSync::Input,
        )
        .map_err(|e| {
            AudioError::UnsupportedFormat(format!("remuestreo {origen_hz}->{destino_hz}: {e}"))
        })?;

        let capacidad = interior.output_frames_max() * CANALES;
        Ok(Some(Self {
            interior,
            pendiente: Vec::with_capacity(BLOQUE * CANALES * 2),
            salida: vec![0.0; capacidad],
            origen_hz,
            destino_hz,
        }))
    }

    /// Remuestrea lo que pueda de `entrada` y **añade** el resultado a `destino`.
    ///
    /// Lo que no complete un bloque se queda guardado para la siguiente
    /// llamada. Es la razón de que exista `pendiente`: los paquetes que entrega
    /// un decodificador no miden lo mismo que los bloques del remuestreador, y
    /// tirar el sobrante sería perder audio en cada paquete.
    ///
    /// # Errors
    /// Si `rubato` rechaza el bloque.
    pub fn procesar(&mut self, entrada: &[f32], destino: &mut Vec<f32>) -> Result<(), AudioError> {
        self.pendiente.extend_from_slice(entrada);

        loop {
            let necesarios = self.interior.input_frames_next() * CANALES;
            if self.pendiente.len() < necesarios {
                return Ok(());
            }
            self.bombear(necesarios, destino)?;
        }
    }

    /// Saca el audio que el remuestreador tenga retenido en sus filtros.
    ///
    /// Sin esto, cada canción perdería sus últimos milisegundos, que es
    /// exactamente lo que rompe una reproducción sin huecos.
    ///
    /// # Errors
    /// Si `rubato` rechaza el bloque final.
    pub fn vaciar(&mut self, destino: &mut Vec<f32>) -> Result<(), AudioError> {
        // Se completa con silencio hasta un bloque entero: es lo que empuja la
        // cola de los filtros hacia la salida.
        let necesarios = self.interior.input_frames_next() * CANALES;
        self.pendiente
            .resize(necesarios.max(self.pendiente.len()), 0.0);
        self.bombear(necesarios, destino)
    }

    fn bombear(&mut self, necesarios: usize, destino: &mut Vec<f32>) -> Result<(), AudioError> {
        let marcos_entrada = necesarios / CANALES;

        let entrada = InterleavedSlice::new(&self.pendiente[..necesarios], CANALES, marcos_entrada)
            .map_err(|e| AudioError::Decode(format!("buffer de entrada invalido: {e}")))?;

        let marcos_salida = self.interior.output_frames_next();
        if self.salida.len() < marcos_salida * CANALES {
            self.salida.resize(marcos_salida * CANALES, 0.0);
        }
        let mut salida = InterleavedSlice::new_mut(
            &mut self.salida[..marcos_salida * CANALES],
            CANALES,
            marcos_salida,
        )
        .map_err(|e| AudioError::Decode(format!("buffer de salida invalido: {e}")))?;

        let (_, producidos) = self
            .interior
            .process_into_buffer(&entrada, &mut salida, None)
            .map_err(|e| AudioError::Decode(format!("remuestreo: {e}")))?;

        destino.extend_from_slice(&self.salida[..producidos * CANALES]);
        self.pendiente.drain(..necesarios);
        Ok(())
    }

    /// Vacía el estado. Se llama tras un salto de posición: los filtros
    /// arrastran audio de antes del salto y se oiría como un eco.
    pub fn reiniciar(&mut self) {
        self.interior.reset();
        self.pendiente.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn senoide(hz: f32, sr: u32, marcos: usize) -> Vec<f32> {
        (0..marcos)
            .flat_map(|n| {
                #[allow(clippy::cast_precision_loss, reason = "acotado en tests")]
                let t = n as f32 / sr as f32;
                let v = (2.0 * std::f32::consts::PI * hz * t).sin();
                [v, v]
            })
            .collect()
    }

    fn rms(bloque: &[f32]) -> f32 {
        if bloque.is_empty() {
            return 0.0;
        }
        #[allow(clippy::cast_precision_loss, reason = "acotado en tests")]
        let n = bloque.len() as f32;
        (bloque.iter().map(|v| v * v).sum::<f32>() / n).sqrt()
    }

    #[test]
    fn frecuencias_iguales_no_crean_remuestreador() {
        // Pasar la senal por un filtro para dejarla igual solo la degradaria.
        assert!(
            Remuestreador::nuevo(48_000, 48_000)
                .expect("valido")
                .is_none()
        );
    }

    #[test]
    fn una_frecuencia_de_cero_no_entra_en_panico() {
        // Un contenedor corrupto puede declararla. Dividir por cero en el hilo
        // de decodificacion tumbaria la reproduccion.
        assert!(Remuestreador::nuevo(0, 48_000).expect("valido").is_none());
        assert!(Remuestreador::nuevo(44_100, 0).expect("valido").is_none());
    }

    #[test]
    fn convierte_cuarenta_y_cuatro_uno_a_cuarenta_y_ocho() {
        let mut r = Remuestreador::nuevo(44_100, 48_000)
            .expect("valido")
            .expect("hay conversion");

        let entrada = senoide(1000.0, 44_100, 44_100);
        let mut salida = Vec::new();
        r.procesar(&entrada, &mut salida).expect("remuestrea");

        let marcos = salida.len() / 2;
        // Un segundo de entrada da aproximadamente un segundo de salida a la
        // frecuencia nueva. El margen cubre lo que queda en los filtros.
        assert!(
            (44_000..=48_100).contains(&marcos),
            "un segundo a 44.1 kHz deberia dar ~48000 marcos, dio {marcos}"
        );
    }

    #[test]
    fn la_amplitud_se_conserva() {
        // Un remuestreador que cambie el volumen se notaria al pasar de una
        // cancion a 44.1 a otra a 48.
        let mut r = Remuestreador::nuevo(44_100, 48_000)
            .expect("valido")
            .expect("hay conversion");

        let entrada = senoide(1000.0, 44_100, 44_100);
        let mut salida = Vec::new();
        r.procesar(&entrada, &mut salida).expect("remuestrea");

        // Se descarta el arranque, donde los filtros aun no estan cargados.
        let estable = &salida[salida.len() / 4..];
        let esperado = rms(&entrada);
        let obtenido = rms(estable);
        assert!(
            (obtenido / esperado - 1.0).abs() < 0.05,
            "la amplitud cambio: {esperado} -> {obtenido}"
        );
    }

    #[test]
    fn no_pierde_audio_entre_llamadas() {
        // Los paquetes de un decodificador no miden lo que los bloques del
        // remuestreador. Si el sobrante se tirara, se perderia audio en cada
        // paquete y la cancion sonaria entrecortada.
        let mut r = Remuestreador::nuevo(44_100, 48_000)
            .expect("valido")
            .expect("hay conversion");

        let entrada = senoide(1000.0, 44_100, 44_100);
        let mut a_trozos = Vec::new();
        // Trozos de 313 marcos: un tamano feo a proposito.
        for trozo in entrada.chunks(313 * 2) {
            r.procesar(trozo, &mut a_trozos).expect("remuestrea");
        }

        let mut r2 = Remuestreador::nuevo(44_100, 48_000)
            .expect("valido")
            .expect("hay conversion");
        let mut de_una = Vec::new();
        r2.procesar(&entrada, &mut de_una).expect("remuestrea");

        assert_eq!(
            a_trozos.len(),
            de_una.len(),
            "trocear la entrada no debe cambiar el total producido"
        );
    }

    #[test]
    fn vaciar_saca_la_cola_de_los_filtros() {
        // Sin esto, cada cancion perderia sus ultimos milisegundos, que es
        // justo lo que rompe la reproduccion sin huecos.
        let mut r = Remuestreador::nuevo(44_100, 48_000)
            .expect("valido")
            .expect("hay conversion");

        let entrada = senoide(1000.0, 44_100, 4410);
        let mut salida = Vec::new();
        r.procesar(&entrada, &mut salida).expect("remuestrea");
        let antes = salida.len();

        r.vaciar(&mut salida).expect("vacia");
        assert!(
            salida.len() > antes,
            "vaciar debe producir las muestras retenidas"
        );
    }

    #[test]
    fn reiniciar_borra_el_audio_anterior() {
        // Tras un salto de posicion, la cola de los filtros es audio de antes
        // del salto: sonaria como un eco.
        let mut r = Remuestreador::nuevo(44_100, 48_000)
            .expect("valido")
            .expect("hay conversion");

        let fuerte = senoide(1000.0, 44_100, 4410);
        let mut basura = Vec::new();
        r.procesar(&fuerte, &mut basura).expect("remuestrea");

        r.reiniciar();

        let silencio = vec![0.0_f32; 44_100 * 2];
        let mut salida = Vec::new();
        r.procesar(&silencio, &mut salida).expect("remuestrea");
        assert!(
            rms(&salida) < 1e-4,
            "quedo audio de antes del salto: rms {}",
            rms(&salida)
        );
    }

    #[test]
    fn tambien_convierte_hacia_abajo() {
        let mut r = Remuestreador::nuevo(48_000, 44_100)
            .expect("valido")
            .expect("hay conversion");

        let entrada = senoide(1000.0, 48_000, 48_000);
        let mut salida = Vec::new();
        r.procesar(&entrada, &mut salida).expect("remuestrea");

        let marcos = salida.len() / 2;
        assert!(
            (43_000..=44_200).contains(&marcos),
            "esperados ~44100 marcos, obtenidos {marcos}"
        );
    }
}

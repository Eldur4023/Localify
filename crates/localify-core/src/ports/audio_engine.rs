//! Puerto del motor de audio.
//!
//! ## Mecanismo, no política
//!
//! El motor mezcla voces y aplica DSP. **No sabe qué es una cola**, ni qué es
//! el modo aleatorio, ni qué suena después. Esa lógica depende de shuffle,
//! repetición, cola de usuario, contexto y disponibilidad de descarga: es
//! negocio, y no puede vivir en un componente cuyo código corre parcialmente en
//! un hilo de tiempo real (ADR-015).
//!
//! ## Contrato de tiempo real
//!
//! El callback de audio del sistema **no asigna memoria, no toma locks, no hace
//! I/O y no loguea**. Por eso los métodos de este trait son síncronos y no
//! bloqueantes: encolan órdenes en una estructura lock-free y vuelven de
//! inmediato. El efecto se observa después, por [`EngineEvent`].

use std::path::PathBuf;

use crate::domain::audio::{AudioDevice, DurationMs, EqProfile, Volume};

/// Identifica una voz de reproducción. Hay dos como máximo: la que suena y la
/// precargada para el crossfade.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct VoiceId(pub u32);

/// Origen del audio.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AudioSource {
    /// Fichero completo en la biblioteca.
    File(PathBuf),
    /// Fichero **en crecimiento**: un `.part` que se está descargando.
    ///
    /// Es la pieza que hace posible "pulsa play y suena en 2 segundos"
    /// (ADR-007). La implementación no devuelve EOF al llegar al final actual:
    /// espera a que el descargador señale más bytes.
    Growing {
        path: PathBuf,
        expected_bytes: Option<u64>,
    },
}

/// Lo que el motor comunica hacia arriba.
#[derive(Debug, Clone, PartialEq)]
pub enum EngineEvent {
    /// La voz empezó a producir muestras.
    Started {
        voice: VoiceId,
    },
    /// Quedan `remaining` para el final. Es la señal con la que
    /// `PlaybackService` decide si prepara un crossfade.
    ApproachingEnd {
        voice: VoiceId,
        remaining: DurationMs,
    },
    Ended {
        voice: VoiceId,
    },
    /// Se agotó el buffer: la descarga no va suficientemente rápida.
    Underrun {
        voice: VoiceId,
    },
    /// Avanzó lo decodificable de un fichero en crecimiento.
    BufferedChanged {
        voice: VoiceId,
        buffered: DurationMs,
    },
    /// El dispositivo de salida desapareció (desconexión de auriculares, por
    /// ejemplo). El motor intenta reconstruir el stream sin perder la posición.
    DeviceLost,
    DeviceChanged {
        device: AudioDevice,
    },
    Failed {
        voice: VoiceId,
        reason_key: String,
    },
}

/// Errores del motor. No usan `CoreError` porque parte de este código corre
/// donde no se puede asignar memoria; la conversión ocurre en la frontera.
#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    #[error("no hay dispositivo de salida disponible")]
    NoDevice,
    #[error("formato no soportado: {0}")]
    UnsupportedFormat(String),
    #[error("no se pudo decodificar: {0}")]
    Decode(String),
    #[error("no se pudo abrir el origen: {0}")]
    Source(String),
    #[error("no hay voces libres")]
    NoVoiceAvailable,
    #[error("el motor está cerrándose")]
    ShuttingDown,
}

impl From<AudioError> for crate::error::CoreError {
    fn from(e: AudioError) -> Self {
        Self::Audio(Box::new(e))
    }
}

pub trait AudioEngine: Send + Sync + 'static {
    /// Prepara una voz y decodifica el buffer inicial.
    ///
    /// # Errors
    /// Si el formato no se soporta, el origen no se puede abrir o no quedan
    /// voces libres.
    fn load(&self, source: AudioSource, start_at: DurationMs) -> Result<VoiceId, AudioError>;

    /// Empieza o reanuda. Es idempotente.
    fn play(&self, voice: VoiceId);

    fn pause(&self);

    /// Libera la voz y sus buffers.
    fn stop(&self, voice: VoiceId);

    /// Salta a una posición.
    ///
    /// Si cae más allá de lo descargado en un fichero en crecimiento, **no
    /// falla**: la voz queda esperando y emite `BufferedChanged` según avance.
    /// Fallar aquí obligaría a la UI a distinguir un caso que para el usuario
    /// es simplemente "está cargando".
    fn seek(&self, voice: VoiceId, position: DurationMs);

    /// Funde de la voz activa a `next` con rampas de potencia constante.
    /// Con `duration` a cero, el cambio es inmediato y sin hueco.
    fn crossfade_to(&self, next: VoiceId, duration: DurationMs);

    fn set_volume(&self, volume: Volume);

    /// Recalcula los coeficientes fuera del hilo de audio y los publica con un
    /// intercambio atómico de buffers.
    fn set_equalizer(&self, profile: &EqProfile);

    /// Posición actual. Lee un atómico: es lo bastante barato como para
    /// sondearla desde un comando varias veces por segundo, que es
    /// precisamente por lo que la posición no se emite como evento.
    fn position(&self) -> DurationMs;

    /// Cuánto hay decodificable. Solo difiere de la duración total durante una
    /// descarga progresiva.
    fn buffered(&self) -> DurationMs;

    fn devices(&self) -> Vec<AudioDevice>;

    /// # Errors
    /// Si el dispositivo no existe o no admite la configuración.
    fn set_device(&self, device_id: Option<&str>) -> Result<(), AudioError>;
}

/// Receptor de los eventos del motor.
///
/// Va aparte del trait principal porque solo lo consume el actor de
/// reproducción, mientras que [`AudioEngine`] lo comparten varios llamantes.
pub trait AudioEventSource: Send + 'static {
    /// Siguiente evento, o `None` si el motor se cerró.
    ///
    /// Bloquea hasta que haya uno. Se consume desde una tarea dedicada.
    fn recv(&mut self) -> Option<EngineEvent>;

    /// Variante no bloqueante, para drenar la cola sin esperar.
    fn try_recv(&mut self) -> Option<EngineEvent>;
}

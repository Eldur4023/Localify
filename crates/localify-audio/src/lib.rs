//! # localify-audio
//!
//! Motor de reproducción: decodificación, mezcla de voces, crossfade,
//! ecualizador y salida por el dispositivo del sistema.
//!
//! Implementa [`localify_core::ports::audio_engine::AudioEngine`].
//!
//! ## Por qué un motor propio
//!
//! Hace falta reproducir un fichero **mientras se descarga**, y ninguna
//! abstracción existente lo permite: `decodeAudioData` de Web Audio exige el
//! buffer completo, y `rodio` no expone el control necesario para un crossfade
//! con precisión de muestra (ADR-002). La pieza clave es un `MediaSource` que,
//! al llegar al final del fichero actual, espera más bytes en lugar de devolver
//! EOF (ADR-007).
//!
//! ## Contrato de tiempo real
//!
//! El callback de audio corre en un hilo de prioridad alta del sistema. Dentro
//! de él está **prohibido** asignar memoria, tomar un lock, hacer I/O o
//! loguear. La comunicación con el resto del programa es por estructuras
//! lock-free y atómicos.
//!
//! **Estado: en construcción (Fase 7).** Es la fase más larga y de mayor
//! riesgo técnico del proyecto.

// En los tests, `expect` con un mensaje es la forma correcta de fallar.
#![cfg_attr(test, allow(clippy::expect_used))]

pub mod decode;
pub mod dsp;
pub mod engine;
pub mod source;

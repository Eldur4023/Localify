//! El motor de audio.
//!
//! Tres hilos, con papeles muy distintos:
//!
//! - **El de control** (tokio, o el que llame al trait): pide cargar, tocar,
//!   pausar, saltar. Nunca toca PCM.
//! - **El de decodificación**: uno por voz. Lee del fichero —que puede estar
//!   creciendo—, decodifica, remuestrea y va llenando un anillo. Puede
//!   bloquearse todo lo que haga falta.
//! - **El de audio**: propiedad de WASAPI. Solo ejecuta
//!   [`mezclador::Mezclador::rellenar`], que no asigna, no bloquea y no loguea.
//!
//! Entre el segundo y el tercero hay una cola SPSC sin locks. Es la frontera
//! importante: es lo que permite que una lectura de disco lenta no se convierta
//! en un corte audible, siempre que el anillo tenga margen.

pub mod mezclador;
pub mod motor;
pub mod salida;
pub mod voz;

pub use mezclador::{EstadoVoz, Mezclador, VolumenCompartido, Voz};
pub use motor::{MotorAudio, ReceptorEventos};
pub use salida::Salida;
pub use voz::{ManejadorVoz, OrigenAudio};

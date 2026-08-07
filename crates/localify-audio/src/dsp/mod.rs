//! Procesado de señal.
//!
//! Todo lo de este módulo es matemática pura sobre bloques de `f32` estéreo
//! intercalado: sin I/O, sin dispositivo, sin hilos. Por eso se puede probar
//! entero midiendo la respuesta real a senoides e impulsos, que es lo que hacen
//! sus tests, en vez de comparar coeficientes contra números escritos a mano.
//!
//! La cadena, en orden:
//!
//! ```text
//! voces ─▶ mezclador (ganancias de crossfade) ─▶ EQ ─▶ limitador ─▶ salida
//! ```
//!
//! El limitador va el último a propósito: su trabajo es atrapar lo que el
//! ecualizador se haya llevado por encima del techo.

pub mod biquad;
pub mod crossfade;
pub mod eq;
pub mod limiter;

pub use crossfade::{Crossfade, marcos_de};
pub use eq::{EqCompartido, EstadoEq};
pub use limiter::Limitador;

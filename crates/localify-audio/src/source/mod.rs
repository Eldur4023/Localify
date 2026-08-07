//! Orígenes de audio.
//!
//! Un solo tipo, [`GrowingFileSource`], cubre los dos casos: un fichero de la
//! biblioteca y un `.part` a medio descargar. La diferencia está únicamente en
//! el [`EstadoDescarga`] con el que se construye.
//!
//! Unificarlos importa: si hubiera dos rutas, la de descarga progresiva sería
//! la que casi nunca se ejerce en los tests y la que fallaría en producción.

pub mod growing;

pub use growing::{EstadoDescarga, GrowingFileSource};

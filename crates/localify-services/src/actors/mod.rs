//! Servicios con estado, implementados como actores.
//!
//! `Playback`, `Queue` y `Download` poseen estado mutable con invariantes
//! temporales, y se acceden desde comandos IPC, desde el motor de audio y desde
//! tareas de fondo a la vez.
//!
//! Con locks, cualquier operación que toque dos de ellos introduce riesgo de
//! deadlock por orden de adquisición, y el estado puede observarse a medio
//! actualizar. Con actores, el estado tiene un único propietario, las
//! transiciones se serializan solas y los tests son deterministas: se envían
//! mensajes y se comprueban las respuestas (ADR-008).
//!
//! **Regla estricta:** un actor nunca espera a otro dentro de su bucle
//! principal. Las esperas largas se delegan a tareas hijas que devuelven el
//! resultado por el canal del propio actor.

pub mod cola;
pub mod download;
pub mod reproduccion;

pub use cola::{Dependencias as DependenciasCola, QueueActor};
pub use download::{BACKOFF_POR_DEFECTO, Dependencias as DependenciasDescarga, DownloadActor};
pub use reproduccion::{Dependencias as DependenciasReproduccion, PlaybackActor, conectar_eventos};

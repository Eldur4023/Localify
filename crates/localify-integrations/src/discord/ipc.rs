//! Protocolo IPC de Discord sobre named pipes de Windows.
//!
//! ## Por qué escrito a mano y no con una biblioteca
//!
//! El protocolo entero son dos enteros y un JSON: `[opcode u32 LE][longitud u32
//! LE][carga]`. Lo que sí es trabajo —reconexión con backoff, límite de
//! frecuencia, no bloquear nunca al reproductor— hay que escribirlo igual por
//! encima de cualquier biblioteca, porque depende de cómo esté montada esta
//! aplicación. Añadir una dependencia para ahorrar treinta líneas de
//! `write_all` y quedarse con todo lo demás no sale a cuenta.
//!
//! ## Discord no estando abierto es lo normal, no un error
//!
//! Si no hay tubería, no hay nada que hacer y no hay nada que decirle al
//! usuario: no ha pedido esto ahora, lo dejó activado hace semanas. Se reintenta
//! con espera creciente y se calla.

use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeClient};
use tracing::debug;

/// Discord abre `discord-ipc-0`; si hay varios clientes —estable, PTB, Canary—
/// numera hasta el 9. Se prueban todos porque cuál toca depende de cuál se abrió
/// primero, que no es algo que se pueda saber desde aquí.
const TUBERIAS: u8 = 10;

/// Opcode de saludo. Solo se manda una vez por conexión.
const OP_HANDSHAKE: u32 = 0;
/// Opcode de mensaje normal.
const OP_FRAME: u32 = 1;
/// Discord pide cerrar.
const OP_CLOSE: u32 = 2;
/// Discord comprueba que seguimos vivos; hay que devolver el mismo cuerpo.
const OP_PING: u32 = 3;
const OP_PONG: u32 = 4;

/// Tope de una respuesta. Las de Discord son de unos cientos de bytes; el límite
/// existe para que una longitud corrupta no reserve un gigabyte.
const MAXIMO_CARGA: u32 = 64 * 1024;

/// Versión del protocolo. Solo existe la 1.
const VERSION: u8 = 1;

/// Lo que se espera una respuesta antes de dar la tubería por muerta.
///
/// Sin este límite, `read_exact` espera para siempre. Y una tubería que acepta
/// la conexión y luego no contesta no es un caso raro: `discord-ipc-0` lo abre
/// quien llegue primero, y hay más programas que hablan este protocolo. Basta
/// con que uno de ellos —o un Discord a medio cerrar— se quede callado para que
/// esta tarea se duerma hasta que se cierre Localify. Sin error, sin traza y sin
/// volver a intentarlo: la integración deja de existir y no hay nada en el log
/// que lo explique.
const RESPUESTA: Duration = Duration::from_secs(5);

pub struct ConexionDiscord {
    tuberia: NamedPipeClient,
}

impl std::fmt::Debug for ConexionDiscord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConexionDiscord").finish_non_exhaustive()
    }
}

impl ConexionDiscord {
    /// Se conecta y saluda. `None` si Discord no está escuchando.
    pub async fn conectar(client_id: &str) -> Option<Self> {
        for n in 0..TUBERIAS {
            let ruta = format!(r"\\.\pipe\discord-ipc-{n}");
            let Ok(tuberia) = ClientOptions::new().open(&ruta) else {
                continue;
            };

            let mut conexion = Self { tuberia };
            let saludo = serde_json::json!({ "v": VERSION, "client_id": client_id });
            if conexion.enviar(OP_HANDSHAKE, &saludo).await.is_err() {
                continue;
            }
            // Discord responde con un `READY` que no hace falta mirar, pero sí
            // hay que leerlo: dejarlo en la tubería descuadra la siguiente
            // lectura, que empezaría a mitad de este mensaje.
            if let Err(e) = conexion.recibir().await {
                // Alguien tiene esa tubería y no es Discord, o es un Discord que
                // ya no atiende. Se deja constancia y se prueba la siguiente: en
                // silencio, "no hay Discord" y "hay algo que no contesta" se ven
                // exactamente igual desde fuera.
                debug!(tuberia = %ruta, error = %e, "la tubería no completó el saludo");
                continue;
            }

            debug!(tuberia = %ruta, "conectado a Discord");
            return Some(conexion);
        }
        None
    }

    async fn enviar(&mut self, opcode: u32, carga: &serde_json::Value) -> std::io::Result<()> {
        let cuerpo = serde_json::to_vec(carga)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let largo = u32::try_from(cuerpo.len())
            .map_err(|_| std::io::Error::other("carga demasiado grande"))?;

        // Cabecera y cuerpo en una sola escritura: en dos, una desconexión entre
        // medias deja a Discord esperando un cuerpo que no llega y la tubería
        // inservible sin que nadie lo note.
        let mut trama = Vec::with_capacity(8 + cuerpo.len());
        trama.extend_from_slice(&opcode.to_le_bytes());
        trama.extend_from_slice(&largo.to_le_bytes());
        trama.extend_from_slice(&cuerpo);

        self.tuberia.write_all(&trama).await?;
        self.tuberia.flush().await
    }

    /// `recibir` con el reloj puesto. Ver [`RESPUESTA`].
    async fn recibir(&mut self) -> std::io::Result<(u32, serde_json::Value)> {
        match tokio::time::timeout(RESPUESTA, self.leer()).await {
            Ok(r) => r,
            Err(_) => Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "Discord no contestó",
            )),
        }
    }

    async fn leer(&mut self) -> std::io::Result<(u32, serde_json::Value)> {
        let mut cabecera = [0_u8; 8];
        self.tuberia.read_exact(&mut cabecera).await?;

        let opcode = u32::from_le_bytes([cabecera[0], cabecera[1], cabecera[2], cabecera[3]]);
        let largo = u32::from_le_bytes([cabecera[4], cabecera[5], cabecera[6], cabecera[7]]);
        if largo > MAXIMO_CARGA {
            return Err(std::io::Error::other(format!(
                "carga desproporcionada: {largo} bytes"
            )));
        }

        let mut cuerpo = vec![0_u8; largo as usize];
        self.tuberia.read_exact(&mut cuerpo).await?;
        let valor = serde_json::from_slice(&cuerpo).unwrap_or(serde_json::Value::Null);
        Ok((opcode, valor))
    }

    /// Publica una actividad, o la retira si es `None`.
    ///
    /// La respuesta se lee **y se mira**. Leerla es obligatorio de todas formas
    /// —si no, se acumulan en la tubería y cada lectura devuelve la de la
    /// llamada anterior—, pero además dice si Discord aceptó lo que se le
    /// mandó: un `SET_ACTIVITY` mal formado se responde con un marco de error
    /// normal, no con un fallo de escritura. Darlo por bueno costó una tarde:
    /// la integración informaba de éxito y el perfil no cambiaba nunca.
    pub async fn publicar(
        &mut self,
        actividad: Option<serde_json::Value>,
    ) -> std::io::Result<Respuesta> {
        let orden = serde_json::json!({
            "cmd": "SET_ACTIVITY",
            "args": {
                "pid": std::process::id(),
                "activity": actividad,
            },
            // Discord exige un `nonce` por mensaje. No lo usamos para nada más
            // que cumplir: aquí solo hay una petición en vuelo cada vez.
            "nonce": uuid_sencillo(),
        });

        self.enviar(OP_FRAME, &orden).await?;

        let (opcode, cuerpo) = self.recibir().await?;
        match opcode {
            OP_CLOSE => Err(std::io::Error::other("Discord cerró la conexión")),
            // Un PING puede colarse entre la orden y su respuesta. Contestarlo
            // es obligatorio: sin PONG, Discord da la conexión por muerta. La
            // respuesta a la orden llegará después; se da por aceptada porque no
            // hay nada que mirar todavía.
            OP_PING => {
                self.enviar(OP_PONG, &serde_json::Value::Null).await?;
                Ok(Respuesta::Aceptada)
            }
            _ => Ok(Respuesta::de(&cuerpo)),
        }
    }
}

/// Qué hizo Discord con la actividad.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Respuesta {
    Aceptada,
    /// Discord la entendió y la rechazó. El motivo viene en texto y es lo único
    /// que dice qué campo está mal, así que se conserva para el log.
    ///
    /// No es un fallo de la tubería: reconectar no arregla nada y reintentar lo
    /// mismo da el mismo error. Es un caso aparte de `Err` a propósito.
    Rechazada(String),
}

impl Respuesta {
    /// Visible para el módulo padre para poder probarla con marcos reales sin
    /// levantar una tubería.
    pub(super) fn de(cuerpo: &serde_json::Value) -> Self {
        if cuerpo["evt"] != "ERROR" {
            return Self::Aceptada;
        }
        let motivo = cuerpo["data"]["message"]
            .as_str()
            .unwrap_or("sin motivo")
            .to_owned();
        Self::Rechazada(motivo)
    }
}

/// Identificador de un solo uso para el campo `nonce`.
///
/// No hace falta que sea un UUID de verdad ni que sea impredecible: solo que no
/// se repita dentro de una conexión. El reloj monótono del proceso lo garantiza
/// sin arrastrar una dependencia por un campo que nadie mira.
fn uuid_sencillo() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static CONTADOR: AtomicU64 = AtomicU64::new(0);
    format!("localify-{}", CONTADOR.fetch_add(1, Ordering::Relaxed))
}

/// Espera entre reintentos de conexión, creciendo hasta un tope.
///
/// Discord cerrado es un estado que puede durar días. Reintentar cada cinco
/// segundos para siempre sería abrir y cerrar una tubería veinte mil veces al
/// día para nada.
pub fn siguiente_espera(actual: Duration) -> Duration {
    const TOPE: Duration = Duration::from_secs(120);
    (actual * 2).min(TOPE)
}

/// Primera espera tras un fallo.
pub const ESPERA_INICIAL: Duration = Duration::from_secs(5);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn la_espera_crece_y_se_detiene_en_el_tope() {
        let mut e = ESPERA_INICIAL;
        for _ in 0..10 {
            e = siguiente_espera(e);
        }
        assert_eq!(e, Duration::from_secs(120));
    }

    #[test]
    fn cada_nonce_es_distinto() {
        assert_ne!(uuid_sencillo(), uuid_sencillo());
    }
}

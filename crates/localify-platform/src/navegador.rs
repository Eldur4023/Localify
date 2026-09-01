//! Abrir una URL en el navegador del usuario.
//!
//! Hace falta para las páginas de configuración externas —el portal de
//! desarrolladores de Discord, por ejemplo— que ocurren fuera de Localify.
//!
//! ## Por qué no lo hace el frontend
//!
//! `window.open` dentro del WebView no lleva al navegador del sistema: o lo
//! bloquea la política de la ventana, o abre la página **dentro** de Localify,
//! que es peor —una página de inicio de sesión ajena dentro de una ventana sin
//! barra de direcciones es exactamente la forma de una suplantación—.
//!
//! ## Por qué `rundll32` y no `cmd /c start`
//!
//! `start` es una orden interna del intérprete, así que hay que lanzar un
//! `cmd.exe` para usarla y eso hace parpadear una consola negra. `rundll32
//! url.dll,FileProtocolHandler` llama al mismo manejador de protocolo sin
//! proceso de consola por medio, y sin necesidad de FFI ni de `unsafe`.

use std::process::Command;

use localify_core::error::{CoreError, CoreResult};
use tracing::debug;

/// Abre `url` en el navegador predeterminado.
///
/// # Errors
/// Si el proceso no se puede lanzar, o si la URL no es `http`/`https`.
pub fn abrir(url: &str) -> CoreResult<()> {
    // El esquema se comprueba aquí y no en quien llama: esta función acaba
    // pasando una cadena a un manejador de protocolos del sistema, y `file:` o
    // un ejecutable suelto abrirían cosas que nadie ha pedido abrir.
    if !url.starts_with("https://") && !url.starts_with("http://") {
        return Err(CoreError::invalid("solo se abren URLs http o https"));
    }

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;

        Command::new("rundll32")
            .args(["url.dll,FileProtocolHandler", url])
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map_err(|e| CoreError::internal(format!("no se pudo abrir el navegador: {e}")))?;
    }
    #[cfg(not(windows))]
    {
        Command::new("xdg-open")
            .arg(url)
            .spawn()
            .map_err(|e| CoreError::internal(format!("no se pudo abrir el navegador: {e}")))?;
    }

    debug!(url, "abierto en el navegador del sistema");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn solo_se_abren_urls_de_web() {
        // Sin esta comprobación, una cadena que llegara hasta aquí acabaría en
        // el manejador de protocolos del sistema, que abre bastante más que
        // páginas web.
        for malo in [
            "file:///C:/Windows/System32/calc.exe",
            "C:\\Windows\\System32\\calc.exe",
            "javascript:alert(1)",
            "",
        ] {
            assert!(abrir(malo).is_err(), "{malo} no debería abrirse");
        }
    }
}

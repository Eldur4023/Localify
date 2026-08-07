//! Instancia única.
//!
//! Dos procesos de Localify abriendo la misma base de datos y el mismo
//! dispositivo de audio no es un caso a soportar: es un fallo. Se evita con un
//! mutex con nombre del sistema, que el SO libera solo aunque el proceso muera
//! de forma abrupta (a diferencia de un fichero de bloqueo, que dejaría un
//! candado huérfano).

use localify_core::error::{CoreError, CoreResult};

/// Guarda del bloqueo. Mientras viva, esta es la única instancia.
#[derive(Debug)]
pub struct InstanceGuard {
    #[cfg(windows)]
    handle: windows::Win32::Foundation::HANDLE,
}

// SAFETY: un HANDLE de mutex de Windows puede usarse desde cualquier hilo; el
// guard solo lo conserva para cerrarlo al soltarse.
#[cfg(windows)]
unsafe impl Send for InstanceGuard {}
#[cfg(windows)]
unsafe impl Sync for InstanceGuard {}

#[cfg(windows)]
const NOMBRE_MUTEX: &str = "Local\\Localify.SingleInstance";

/// Intenta adquirir el bloqueo de instancia única.
///
/// # Errors
/// [`CoreError::Conflict`] si ya hay otra instancia en marcha.
#[cfg(windows)]
pub fn adquirir() -> CoreResult<InstanceGuard> {
    adquirir_con_nombre(NOMBRE_MUTEX)
}

/// Adquiere un bloqueo con nombre arbitrario.
///
/// Existe para los tests. Comprobar el mecanismo con el nombre de producción
/// significaba competir con la aplicación de verdad: el test pasaba o fallaba
/// según si el usuario tenía Localify abierto en ese momento, que es lo
/// contrario de lo que hace un test.
#[cfg(windows)]
fn adquirir_con_nombre(nombre_mutex: &str) -> CoreResult<InstanceGuard> {
    use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError};
    use windows::Win32::System::Threading::CreateMutexW;
    use windows::core::HSTRING;

    let nombre = HSTRING::from(nombre_mutex);

    // SAFETY: `nombre` es una cadena UTF-16 válida y viva durante la llamada.
    let handle = unsafe { CreateMutexW(None, true, &nombre) }
        .map_err(|e| CoreError::internal(format!("no se pudo crear el mutex: {e}")))?;

    // SAFETY: `GetLastError` no tiene precondiciones.
    let ultimo = unsafe { GetLastError() };
    if ultimo == ERROR_ALREADY_EXISTS {
        // SAFETY: `handle` es válido y no se vuelve a usar tras cerrarlo.
        unsafe {
            let _ = windows::Win32::Foundation::CloseHandle(handle);
        }
        return Err(CoreError::conflict(
            "ya hay una instancia de Localify en ejecución",
        ));
    }

    Ok(InstanceGuard { handle })
}

#[cfg(not(windows))]
pub fn adquirir() -> CoreResult<InstanceGuard> {
    // Al portar se implementará con un socket de dominio Unix o un flock.
    Ok(InstanceGuard {})
}

#[cfg(windows)]
impl Drop for InstanceGuard {
    fn drop(&mut self) {
        // SAFETY: el handle se creó en `adquirir`, es válido y solo se cierra
        // aquí, una vez.
        unsafe {
            let _ = windows::Win32::Foundation::CloseHandle(self.handle);
        }
    }
}

#[cfg(not(windows))]
#[derive(Debug)]
pub struct InstanceGuard;

#[cfg(test)]
mod tests {
    use super::*;

    /// Nombre propio del test.
    ///
    /// Con el de producción, este test competía con la aplicación instalada:
    /// pasaba con Localify cerrado y fallaba con Localify abierto. Lo que se
    /// quiere comprobar es el **mecanismo**, no quién tiene cogido el nombre
    /// real. Lleva el identificador del proceso para que dos ejecuciones
    /// simultáneas de la suite tampoco choquen entre sí.
    #[cfg(windows)]
    fn nombre_de_prueba() -> String {
        format!("Local\\Localify.Test.{}", std::process::id())
    }

    #[cfg(windows)]
    #[test]
    fn la_segunda_adquisicion_en_el_mismo_proceso_es_rechazada() {
        let nombre = nombre_de_prueba();

        let primera =
            adquirir_con_nombre(&nombre).expect("la primera instancia debe poder arrancar");
        let segunda = adquirir_con_nombre(&nombre);
        assert!(
            segunda.is_err(),
            "una segunda instancia no debería poder arrancar"
        );
        drop(primera);

        // Tras soltar el guard, el nombre queda libre otra vez.
        let tercera = adquirir_con_nombre(&nombre);
        assert!(
            tercera.is_ok(),
            "el mutex debe liberarse al soltar el guard"
        );
    }
}

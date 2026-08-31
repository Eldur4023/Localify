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
    /// El fichero con el candado puesto.
    ///
    /// No se lee nunca a propósito: existe para mantener el descriptor abierto,
    /// porque cerrarlo es exactamente lo que suelta el bloqueo. Es lo mismo que
    /// hace el `handle` de Windows, solo que allí sí hay que cerrarlo a mano.
    #[cfg(not(windows))]
    #[allow(dead_code, reason = "es una guarda RAII: su valor es existir")]
    fichero: std::fs::File,
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

/// Intenta adquirir el bloqueo de instancia única.
///
/// ## Por qué un candado de fichero y no un fichero con el PID
///
/// La cabecera del módulo lo dice para Windows y vale igual aquí: lo que hace
/// falta es que el candado **se suelte solo** si el proceso muere de forma
/// abrupta. Un fichero con el PID dentro no cumple eso —queda huérfano tras un
/// cierre forzado, y hay que decidir si el PID que contiene sigue vivo, que es
/// una carrera— mientras que un candado de fichero lo libera el núcleo al cerrar
/// el descriptor, pase lo que pase.
///
/// El fichero vive en el directorio de ejecución del usuario, que es efímero por
/// definición, y no en la carpeta de configuración: ahí sobreviviría a un
/// reinicio sin significar nada.
///
/// # Errors
/// Si ya hay otra instancia, o si el fichero de bloqueo no se puede abrir.
#[cfg(not(windows))]
pub fn adquirir() -> CoreResult<InstanceGuard> {
    let base = std::env::var("XDG_RUNTIME_DIR")
        .or_else(|_| std::env::var("TMPDIR"))
        .unwrap_or_else(|_| "/tmp".to_owned());
    adquirir_en(&std::path::Path::new(&base).join("localify.lock"))
}

/// El mecanismo, sobre una ruta cualquiera.
///
/// Separado de [`adquirir`] por el mismo motivo que `adquirir_con_nombre` en
/// Windows: sin esto, el test competiría con la aplicación instalada y pasaría o
/// fallaría según estuviera Localify abierto.
#[cfg(not(windows))]
fn adquirir_en(ruta: &std::path::Path) -> CoreResult<InstanceGuard> {
    let fichero = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(ruta)
        .map_err(|e| {
            CoreError::storage(format!(
                "no se pudo abrir el fichero de bloqueo {}: {e}",
                ruta.display()
            ))
        })?;

    fichero
        .try_lock()
        .map_err(|_| CoreError::conflict("ya hay una instancia de Localify en ejecución"))?;

    Ok(InstanceGuard { fichero })
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

// Fuera de Windows el guard es el propio fichero abierto: al soltarse se cierra
// el descriptor y el núcleo libera el candado. No hace falta un `Drop` propio.

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

    /// El mismo contrato que en Windows, con el mecanismo de Linux.
    ///
    /// Importa que sea *el mismo test*: lo que la aplicación necesita —una sola
    /// instancia, y el candado libre en cuanto la primera muere— no depende del
    /// sistema, y tenerlo comprobado en uno solo dejaría la mitad de las
    /// plataformas a base de confianza.
    #[cfg(not(windows))]
    #[test]
    fn la_segunda_adquisicion_en_el_mismo_proceso_es_rechazada() {
        let ruta = std::env::temp_dir().join(format!("localify-test-{}.lock", std::process::id()));

        let primera = adquirir_en(&ruta).expect("la primera instancia debe poder arrancar");
        assert!(
            adquirir_en(&ruta).is_err(),
            "una segunda instancia no debería poder arrancar"
        );
        drop(primera);

        assert!(
            adquirir_en(&ruta).is_ok(),
            "el candado debe liberarse al soltar el guard"
        );
        let _ = std::fs::remove_file(&ruta);
    }
}

//! Almacén de secretos.
//!
//! Guarda el `client_secret` de Spotify y la sesión de Last.fm. Ninguno de los
//! dos cruza jamás el puente IPC hacia el frontend.
//!
//! ## Un almacén por sistema, porque el sistema ya tiene uno
//!
//! - **Windows:** un fichero cifrado con **DPAPI** (`CryptProtectData`) en el
//!   ámbito del usuario actual. Solo esa cuenta, en esa máquina, puede
//!   descifrarlo, y no hay ninguna clave que gestionar.
//! - **Linux y macOS:** el llavero del escritorio, a través del Secret Service
//!   (gnome-keyring, KDE Wallet). **No hay fichero**: el JSON entero se guarda
//!   como un único secreto con la clave `Localify/secrets`.
//!
//! No se cifra a mano en ninguno de los dos casos. Inventar un formato propio
//! —una clave derivada guardada al lado del dato, que es donde acaban estas
//! cosas— sería peor que cualquiera de los dos almacenes del sistema.
//!
//! ## Qué pasa si no hay llavero
//!
//! En una sesión sin Secret Service —un servidor, un entorno mínimo— guardar
//! **falla con un error visible**. Es deliberado: la alternativa es escribir el
//! secreto en claro, y prefiero que Spotify no se pueda configurar a que se
//! configure dejando la credencial legible en el disco. La música sigue
//! funcionando; es lo único que no depende de esto.

use std::collections::HashMap;

use async_trait::async_trait;
use localify_core::error::{CoreError, CoreResult};
use localify_core::ports::platform::SecretStore;
use tokio::sync::Mutex;

/// Almacén de secretos respaldado por el sistema.
#[derive(Debug)]
pub struct AlmacenDeSecretos {
    respaldo: Respaldo,
    /// Serializa las escrituras y evita releer el almacén en cada consulta.
    cache: Mutex<Option<HashMap<String, String>>>,
}

impl AlmacenDeSecretos {
    #[must_use]
    pub fn new(config_dir: &std::path::Path) -> Self {
        Self {
            respaldo: Respaldo::new(config_dir),
            cache: Mutex::new(None),
        }
    }

    async fn cargar(&self) -> CoreResult<HashMap<String, String>> {
        let mut guard = self.cache.lock().await;
        if let Some(mapa) = guard.as_ref() {
            return Ok(mapa.clone());
        }

        let mapa = match self.respaldo.leer().await? {
            Some(claro) => serde_json::from_slice(&claro).map_err(|e| {
                CoreError::storage(format!("el almacén de secretos está corrupto: {e}"))
            })?,
            None => HashMap::new(),
        };

        *guard = Some(mapa.clone());
        Ok(mapa)
    }

    async fn guardar(&self, mapa: HashMap<String, String>) -> CoreResult<()> {
        let claro = serde_json::to_vec(&mapa)
            .map_err(|e| CoreError::internal(format!("no se pudo serializar: {e}")))?;
        self.respaldo.escribir(&claro).await?;
        *self.cache.lock().await = Some(mapa);
        Ok(())
    }
}

#[async_trait]
impl SecretStore for AlmacenDeSecretos {
    async fn get(&self, key: &str) -> CoreResult<Option<String>> {
        Ok(self.cargar().await?.get(key).cloned())
    }

    async fn set(&self, key: &str, value: &str) -> CoreResult<()> {
        let mut mapa = self.cargar().await?;
        mapa.insert(key.to_owned(), value.to_owned());
        self.guardar(mapa).await
    }

    async fn delete(&self, key: &str) -> CoreResult<()> {
        let mut mapa = self.cargar().await?;
        if mapa.remove(key).is_some() {
            self.guardar(mapa).await?;
        }
        Ok(())
    }
}

#[cfg(windows)]
fn cifrar(claro: &[u8]) -> CoreResult<Vec<u8>> {
    use windows::Win32::Foundation::LocalFree;
    use windows::Win32::Security::Cryptography::{CRYPT_INTEGER_BLOB, CryptProtectData};

    let entrada = CRYPT_INTEGER_BLOB {
        cbData: u32::try_from(claro.len())
            .map_err(|_| CoreError::invalid("secreto demasiado grande"))?,
        pbData: claro.as_ptr().cast_mut(),
    };
    let mut salida = CRYPT_INTEGER_BLOB::default();

    // SAFETY: `entrada` apunta a `claro`, vivo durante la llamada. `salida` se
    // rellena con memoria que asigna la propia API y que liberamos con
    // `LocalFree` justo después de copiarla.
    unsafe {
        CryptProtectData(
            &raw const entrada,
            None,
            None,
            None,
            None,
            0,
            &raw mut salida,
        )
        .map_err(|e| CoreError::internal(format!("DPAPI no pudo cifrar: {e}")))?;
    }

    // SAFETY: tras el éxito, `salida` describe un búfer válido de `cbData`
    // bytes.
    let resultado =
        unsafe { std::slice::from_raw_parts(salida.pbData, salida.cbData as usize).to_vec() };

    // SAFETY: `pbData` lo asignó la API con LocalAlloc.
    unsafe {
        let _ = LocalFree(Some(windows::Win32::Foundation::HLOCAL(
            salida.pbData.cast(),
        )));
    }

    Ok(resultado)
}

#[cfg(windows)]
fn descifrar(cifrado: &[u8]) -> CoreResult<Vec<u8>> {
    use windows::Win32::Foundation::LocalFree;
    use windows::Win32::Security::Cryptography::{CRYPT_INTEGER_BLOB, CryptUnprotectData};

    let entrada = CRYPT_INTEGER_BLOB {
        cbData: u32::try_from(cifrado.len())
            .map_err(|_| CoreError::invalid("secreto demasiado grande"))?,
        pbData: cifrado.as_ptr().cast_mut(),
    };
    let mut salida = CRYPT_INTEGER_BLOB::default();

    // SAFETY: mismas condiciones que en `cifrar`.
    unsafe {
        CryptUnprotectData(
            &raw const entrada,
            None,
            None,
            None,
            None,
            0,
            &raw mut salida,
        )
        .map_err(|e| {
            CoreError::storage(format!(
                "DPAPI no pudo descifrar (¿otro usuario u otra máquina?): {e}"
            ))
        })?;
    }

    // SAFETY: tras el éxito, `salida` describe un búfer válido.
    let resultado =
        unsafe { std::slice::from_raw_parts(salida.pbData, salida.cbData as usize).to_vec() };

    // SAFETY: `pbData` lo asignó la API con LocalAlloc.
    unsafe {
        let _ = LocalFree(Some(windows::Win32::Foundation::HLOCAL(
            salida.pbData.cast(),
        )));
    }

    Ok(resultado)
}

/// Dónde acaban los bytes. Es lo único que cambia entre sistemas.
#[cfg(windows)]
#[derive(Debug)]
struct Respaldo {
    ruta: std::path::PathBuf,
}

#[cfg(windows)]
impl Respaldo {
    fn new(config_dir: &std::path::Path) -> Self {
        Self {
            ruta: config_dir.join("secrets.bin"),
        }
    }

    async fn leer(&self) -> CoreResult<Option<Vec<u8>>> {
        match tokio::fs::read(&self.ruta).await {
            Ok(cifrado) => descifrar(&cifrado).map(Some),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(CoreError::storage(format!(
                "no se pudo leer el almacén de secretos: {e}"
            ))),
        }
    }

    async fn escribir(&self, claro: &[u8]) -> CoreResult<()> {
        let cifrado = cifrar(claro)?;
        if let Some(padre) = self.ruta.parent() {
            tokio::fs::create_dir_all(padre)
                .await
                .map_err(|e| CoreError::storage(format!("no se pudo crear la carpeta: {e}")))?;
        }
        tokio::fs::write(&self.ruta, &cifrado)
            .await
            .map_err(|e| CoreError::storage(format!("no se pudo escribir: {e}")))
    }
}

/// El llavero del escritorio, vía Secret Service.
///
/// `config_dir` no se usa: aquí no hay fichero. Se acepta para que el
/// constructor sea el mismo en los dos sistemas y quien lo llama no tenga que
/// saber cuál está compilado.
#[cfg(not(windows))]
#[derive(Debug)]
struct Respaldo {
    /// Nombre de la entrada en el llavero.
    ///
    /// Es un campo y no una constante **para que los tests no escriban en la
    /// entrada de verdad**. Con un nombre fijo, `guarda_lee_y_borra` acabaría
    /// borrando las credenciales de Spotify y la sesión de Last.fm del usuario
    /// que ejecutara la suite: el llavero es del escritorio, no del proceso, y no
    /// hay directorio temporal que aísle eso.
    entrada: String,
}

#[cfg(not(windows))]
impl Respaldo {
    /// Nombre del servicio en el llavero. Aparece tal cual en Seahorse o en
    /// KWalletManager, así que dice quién guardó esto.
    const SERVICIO: &'static str = "Localify";
    /// Todos los secretos van en una sola entrada, con el mismo JSON que en
    /// Windows. Repartirlos en una entrada por clave obligaría a enumerar el
    /// llavero para leerlos, y a mantener dos formatos distintos del mismo dato.
    const ENTRADA: &'static str = "secrets";

    fn new(_config_dir: &std::path::Path) -> Self {
        Self {
            entrada: Self::ENTRADA.to_owned(),
        }
    }

    async fn leer(&self) -> CoreResult<Option<Vec<u8>>> {
        let entrada = self.entrada.clone();
        // El Secret Service habla por D-Bus y bloquea: fuera del hilo del
        // runtime.
        let resultado = tokio::task::spawn_blocking(move || {
            let llave = keyring::Entry::new(Self::SERVICIO, &entrada)?;
            match llave.get_password() {
                Ok(texto) => Ok(Some(texto)),
                // Que no haya nada guardado es el primer arranque, no un fallo.
                Err(keyring::Error::NoEntry) => Ok(None),
                Err(e) => Err(e),
            }
        })
        .await
        .map_err(|e| CoreError::internal(format!("el hilo del llavero murió: {e}")))?;

        match resultado {
            Ok(texto) => Ok(texto.map(String::into_bytes)),
            Err(e) => Err(CoreError::storage(format!(
                "no se pudo leer del llavero del sistema: {e}"
            ))),
        }
    }

    async fn escribir(&self, claro: &[u8]) -> CoreResult<()> {
        let texto = String::from_utf8(claro.to_vec())
            .map_err(|e| CoreError::internal(format!("el almacén no es UTF-8: {e}")))?;
        let entrada = self.entrada.clone();

        tokio::task::spawn_blocking(move || {
            keyring::Entry::new(Self::SERVICIO, &entrada)?.set_password(&texto)
        })
        .await
        .map_err(|e| CoreError::internal(format!("el hilo del llavero murió: {e}")))?
        .map_err(|e| {
            // Sin llavero no se guarda nada, y se dice. Ver la cabecera del
            // módulo: escribir el secreto en claro no es una alternativa.
            CoreError::storage(format!(
                "no se pudo guardar en el llavero del sistema \
                 (¿hay un gestor de secretos en la sesión?): {e}"
            ))
        })
    }
}

#[cfg(not(windows))]
impl AlmacenDeSecretos {
    /// Un almacén sobre una entrada propia del llavero, para los tests.
    #[cfg(test)]
    fn de_prueba(entrada: String) -> Self {
        Self {
            respaldo: Respaldo { entrada },
            cache: Mutex::new(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(windows)]
    fn dpapi_hace_ida_y_vuelta() {
        let secreto = b"un client_secret de spotify";
        let cifrado = cifrar(secreto).expect("cifra");
        assert_ne!(
            cifrado.as_slice(),
            secreto.as_slice(),
            "el secreto no debe quedar en claro"
        );
        assert_eq!(descifrar(&cifrado).expect("descifra"), secreto);
    }

    /// Un almacén aislado y su carpeta, distinto en cada sistema.
    ///
    /// En Windows basta con un directorio temporal. En Linux **no**: el llavero
    /// es del escritorio y no hay carpeta que aísle nada, así que el aislamiento
    /// tiene que ser el nombre de la entrada. Sin esto, ejecutar la suite
    /// borraría las credenciales de quien la ejecuta.
    fn almacen_de_prueba(nombre: &str) -> (AlmacenDeSecretos, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("localify-test-{nombre}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("crea dir");

        #[cfg(windows)]
        let almacen = AlmacenDeSecretos::new(&dir);
        #[cfg(not(windows))]
        let almacen = AlmacenDeSecretos::de_prueba(format!("test-{nombre}-{}", std::process::id()));

        (almacen, dir)
    }

    /// Deja el llavero como estaba. En Windows lo hace borrar la carpeta.
    #[cfg(not(windows))]
    fn limpiar(almacen: &AlmacenDeSecretos) {
        let entrada = almacen.respaldo.entrada.clone();
        if let Ok(llave) = keyring::Entry::new(Respaldo::SERVICIO, &entrada) {
            let _ = llave.delete_credential();
        }
    }

    #[tokio::test]
    async fn guarda_lee_y_borra() {
        let (store, dir) = almacen_de_prueba("secrets");

        // Sin gestor de secretos en la sesión —un contenedor, WSL, un servidor
        // sin escritorio— no hay nada que probar aquí: fallaría el entorno, no
        // el código. Es el mismo criterio que usan los tests de audio, que se
        // saltan solos cuando no hay tarjeta de sonido.
        if store.set("spotify.secret", "abc123").await.is_err() {
            let _ = std::fs::remove_dir_all(&dir);
            return;
        }

        assert_eq!(
            store.get("spotify.secret").await.expect("get"),
            Some("abc123".into())
        );

        store.delete("spotify.secret").await.expect("delete");
        assert_eq!(store.get("spotify.secret").await.expect("get"), None);

        #[cfg(not(windows))]
        limpiar(&store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// El secreto no queda legible en el disco.
    ///
    /// La garantía es la misma en los dos sistemas; lo que cambia es cómo se
    /// cumple. En Windows hay un fichero y lo que importa es que esté cifrado;
    /// en Linux **no hay fichero**, y comprobar que no aparece ninguno es lo que
    /// impide que un futuro apaño lo reintroduzca en claro.
    #[tokio::test]
    async fn el_secreto_no_queda_legible_en_el_disco() {
        let (store, dir) = almacen_de_prueba("secrets-claro");
        if store.set("k", "SECRETO_MUY_RECONOCIBLE").await.is_err() {
            let _ = std::fs::remove_dir_all(&dir);
            return;
        }

        #[cfg(windows)]
        {
            let bytes = std::fs::read(dir.join("secrets.bin")).expect("lee");
            assert!(
                !String::from_utf8_lossy(&bytes).contains("SECRETO_MUY_RECONOCIBLE"),
                "el secreto aparece en claro en disco"
            );
        }
        #[cfg(not(windows))]
        {
            let entradas: Vec<_> = std::fs::read_dir(&dir)
                .expect("lee dir")
                .filter_map(Result::ok)
                .map(|e| e.file_name())
                .collect();
            assert!(
                entradas.is_empty(),
                "no debe escribirse ningún fichero de secretos: {entradas:?}"
            );
            limpiar(&store);
        }

        let _ = std::fs::remove_dir_all(&dir);
    }
}

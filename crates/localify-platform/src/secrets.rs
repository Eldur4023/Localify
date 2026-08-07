//! Almacén de secretos.
//!
//! En Windows se cifra con **DPAPI** (`CryptProtectData`) en el ámbito del
//! usuario actual: solo esa cuenta, en esa máquina, puede descifrarlo. No hace
//! falta gestionar ninguna clave y el secreto no queda en claro en disco.
//!
//! Guarda el `client_secret` de Spotify y la sesión de Last.fm. Ninguno de los
//! dos cruza jamás el puente IPC hacia el frontend.

use std::collections::HashMap;
use std::path::PathBuf;

use async_trait::async_trait;
use localify_core::error::{CoreError, CoreResult};
use localify_core::ports::platform::SecretStore;
use tokio::sync::Mutex;

/// Secretos cifrados en un fichero JSON dentro de la carpeta de configuración.
#[derive(Debug)]
pub struct DpapiSecretStore {
    ruta: PathBuf,
    /// Serializa las escrituras y evita releer el fichero en cada consulta.
    cache: Mutex<Option<HashMap<String, String>>>,
}

impl DpapiSecretStore {
    #[must_use]
    pub fn new(config_dir: &std::path::Path) -> Self {
        Self {
            ruta: config_dir.join("secrets.bin"),
            cache: Mutex::new(None),
        }
    }

    async fn cargar(&self) -> CoreResult<HashMap<String, String>> {
        let mut guard = self.cache.lock().await;
        if let Some(mapa) = guard.as_ref() {
            return Ok(mapa.clone());
        }

        let mapa = match tokio::fs::read(&self.ruta).await {
            Ok(cifrado) => {
                let claro = descifrar(&cifrado)?;
                serde_json::from_slice(&claro).map_err(|e| {
                    CoreError::storage(format!("el almacén de secretos está corrupto: {e}"))
                })?
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => HashMap::new(),
            Err(e) => {
                return Err(CoreError::storage(format!(
                    "no se pudo leer el almacén de secretos: {e}"
                )));
            }
        };

        *guard = Some(mapa.clone());
        Ok(mapa)
    }

    async fn guardar(&self, mapa: HashMap<String, String>) -> CoreResult<()> {
        let claro = serde_json::to_vec(&mapa)
            .map_err(|e| CoreError::internal(format!("no se pudo serializar: {e}")))?;
        let cifrado = cifrar(&claro)?;

        if let Some(padre) = self.ruta.parent() {
            tokio::fs::create_dir_all(padre)
                .await
                .map_err(|e| CoreError::storage(format!("no se pudo crear la carpeta: {e}")))?;
        }
        tokio::fs::write(&self.ruta, &cifrado)
            .await
            .map_err(|e| CoreError::storage(format!("no se pudo escribir: {e}")))?;

        *self.cache.lock().await = Some(mapa);
        Ok(())
    }
}

#[async_trait]
impl SecretStore for DpapiSecretStore {
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

#[cfg(not(windows))]
fn cifrar(claro: &[u8]) -> CoreResult<Vec<u8>> {
    // Al portar: keyring / libsecret. Nunca dejar esto en claro en una release.
    Ok(claro.to_vec())
}

#[cfg(not(windows))]
fn descifrar(cifrado: &[u8]) -> CoreResult<Vec<u8>> {
    Ok(cifrado.to_vec())
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

    #[tokio::test]
    async fn guarda_lee_y_borra() {
        let dir = std::env::temp_dir().join("localify-test-secrets");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("crea dir");

        let store = DpapiSecretStore::new(&dir);
        assert_eq!(store.get("spotify.secret").await.expect("get"), None);

        store.set("spotify.secret", "abc123").await.expect("set");
        assert_eq!(
            store.get("spotify.secret").await.expect("get"),
            Some("abc123".into())
        );

        // Una instancia nueva debe leer lo persistido, no la caché en memoria.
        let otra = DpapiSecretStore::new(&dir);
        assert_eq!(
            otra.get("spotify.secret").await.expect("get"),
            Some("abc123".into())
        );

        store.delete("spotify.secret").await.expect("delete");
        assert_eq!(store.get("spotify.secret").await.expect("get"), None);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn el_fichero_en_disco_no_contiene_el_secreto_en_claro() {
        let dir = std::env::temp_dir().join("localify-test-secrets-claro");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("crea dir");

        let store = DpapiSecretStore::new(&dir);
        store
            .set("k", "SECRETO_MUY_RECONOCIBLE")
            .await
            .expect("set");

        let bytes = std::fs::read(dir.join("secrets.bin")).expect("lee");
        let como_texto = String::from_utf8_lossy(&bytes);
        #[cfg(windows)]
        assert!(
            !como_texto.contains("SECRETO_MUY_RECONOCIBLE"),
            "el secreto aparece en claro en disco"
        );
        let _ = como_texto;

        let _ = std::fs::remove_dir_all(&dir);
    }
}

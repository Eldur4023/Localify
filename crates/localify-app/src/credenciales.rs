//! Credenciales de aplicación de Spotify.
//!
//! Tres orígenes, en este orden:
//!
//! 1. **Almacén de secretos del sistema**: lo que el usuario pegó en Ajustes.
//!    Cifrado con DPAPI, nunca en claro en disco.
//! 2. **Incrustadas en tiempo de compilación**: para las compilaciones
//!    oficiales, vía variables de entorno. No están en el repositorio.
//! 3. **Ninguna**: la aplicación arranca igual y funciona por completo sobre la
//!    biblioteca local.
//!
//! El tercer caso no es un fallo degradado sino un modo de operación de primera
//! clase, y es además el estado de cualquiera que compile desde el código
//! fuente (ADR-005).

use std::sync::Arc;

use localify_core::ports::platform::SecretStore;
use localify_spotify::Credenciales;
use tracing::{debug, info};

/// Clave del `client_id` en el almacén de secretos.
const CLAVE_ID: &str = "spotify.client_id";
/// Clave del `client_secret`.
const CLAVE_SECRETO: &str = "spotify.client_secret";

/// Credenciales incrustadas en tiempo de compilación, si las hubo.
///
/// `option_env!` se resuelve al compilar: en una compilación sin estas
/// variables, las constantes son `None` y no queda rastro en el binario.
const ID_INCRUSTADO: Option<&str> = option_env!("LOCALIFY_SPOTIFY_CLIENT_ID");
const SECRETO_INCRUSTADO: Option<&str> = option_env!("LOCALIFY_SPOTIFY_CLIENT_SECRET");

/// Carga las credenciales disponibles.
///
/// Nunca falla: la ausencia de credenciales es un estado válido.
pub async fn cargar(store: &Arc<dyn SecretStore>) -> Option<Credenciales> {
    // Lo que puso el usuario manda sobre lo incrustado: si se molestó en
    // ponerlo, es porque quiere usar su propia aplicación.
    let id = store.get(CLAVE_ID).await.ok().flatten();
    let secreto = store.get(CLAVE_SECRETO).await.ok().flatten();

    if let (Some(client_id), Some(client_secret)) = (id, secreto)
        && !client_id.is_empty()
        && !client_secret.is_empty()
    {
        info!("credenciales de Spotify cargadas del almacén del sistema");
        return Some(Credenciales {
            client_id,
            client_secret,
        });
    }

    if let (Some(client_id), Some(client_secret)) = (ID_INCRUSTADO, SECRETO_INCRUSTADO)
        && !client_id.is_empty()
        && !client_secret.is_empty()
    {
        debug!("usando las credenciales de Spotify de la compilación");
        return Some(Credenciales {
            client_id: client_id.to_owned(),
            client_secret: client_secret.to_owned(),
        });
    }

    info!("sin credenciales de Spotify: la aplicación funcionará solo con la biblioteca local");
    None
}

/// Guarda las credenciales en el almacén del sistema.
///
/// # Errors
/// Si el almacén no responde.
pub async fn guardar(
    store: &Arc<dyn SecretStore>,
    client_id: &str,
    client_secret: &str,
) -> localify_core::error::CoreResult<Credenciales> {
    store.set(CLAVE_ID, client_id).await?;
    store.set(CLAVE_SECRETO, client_secret).await?;
    info!("credenciales de Spotify guardadas");

    Ok(Credenciales {
        client_id: client_id.to_owned(),
        client_secret: client_secret.to_owned(),
    })
}

/// Borra las credenciales guardadas.
///
/// # Errors
/// Si el almacén no responde.
pub async fn borrar(store: &Arc<dyn SecretStore>) -> localify_core::error::CoreResult<()> {
    store.delete(CLAVE_ID).await?;
    store.delete(CLAVE_SECRETO).await?;
    Ok(())
}

/// El `client_id` guardado, para mostrarlo en Ajustes.
///
/// El `client_secret` **no** tiene equivalente a propósito: no debe salir del
/// almacén.
pub async fn client_id_visible(store: &Arc<dyn SecretStore>) -> Option<String> {
    store
        .get(CLAVE_ID)
        .await
        .ok()
        .flatten()
        .or_else(|| ID_INCRUSTADO.map(str::to_owned))
        .filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use async_trait::async_trait;
    use localify_core::error::CoreResult;

    use super::*;

    #[derive(Debug, Default)]
    struct AlmacenFalso {
        datos: Mutex<HashMap<String, String>>,
    }

    #[async_trait]
    impl SecretStore for AlmacenFalso {
        async fn get(&self, key: &str) -> CoreResult<Option<String>> {
            Ok(self.datos.lock().ok().and_then(|d| d.get(key).cloned()))
        }
        async fn set(&self, key: &str, value: &str) -> CoreResult<()> {
            if let Ok(mut d) = self.datos.lock() {
                d.insert(key.to_owned(), value.to_owned());
            }
            Ok(())
        }
        async fn delete(&self, key: &str) -> CoreResult<()> {
            if let Ok(mut d) = self.datos.lock() {
                d.remove(key);
            }
            Ok(())
        }
    }

    fn almacen() -> Arc<dyn SecretStore> {
        Arc::new(AlmacenFalso::default())
    }

    #[tokio::test]
    async fn sin_nada_configurado_no_hay_credenciales() {
        let store = almacen();
        // En una compilación sin variables incrustadas, no debe haber nada.
        if ID_INCRUSTADO.is_none() {
            assert!(cargar(&store).await.is_none());
        }
    }

    #[tokio::test]
    async fn lo_guardado_se_recupera() {
        let store = almacen();
        guardar(&store, "mi-id", "mi-secreto")
            .await
            .expect("guarda");

        let c = cargar(&store).await.expect("hay credenciales");
        assert_eq!(c.client_id, "mi-id");
        assert_eq!(c.client_secret, "mi-secreto");
    }

    #[tokio::test]
    async fn unas_credenciales_a_medias_no_se_usan() {
        // Un secreto sin id, o al revés, no sirve para autenticar: usarlo solo
        // produciría un 400 desconcertante.
        let store = almacen();
        store
            .set("spotify.client_id", "solo-el-id")
            .await
            .expect("set");

        if ID_INCRUSTADO.is_none() {
            assert!(cargar(&store).await.is_none());
        }
    }

    #[tokio::test]
    async fn borrar_las_deja_sin_efecto() {
        let store = almacen();
        guardar(&store, "id", "secreto").await.expect("guarda");
        borrar(&store).await.expect("borra");

        if ID_INCRUSTADO.is_none() {
            assert!(cargar(&store).await.is_none());
        }
        assert!(client_id_visible(&store).await.is_none() || ID_INCRUSTADO.is_some());
    }

    #[tokio::test]
    async fn el_client_id_es_visible_pero_no_hay_forma_de_leer_el_secreto() {
        let store = almacen();
        guardar(&store, "id-publico", "secreto-privado")
            .await
            .expect("guarda");

        assert_eq!(
            client_id_visible(&store).await.as_deref(),
            Some("id-publico")
        );
        // No existe `client_secret_visible`: esa es la garantía, y es
        // estructural, no una convención.
    }
}

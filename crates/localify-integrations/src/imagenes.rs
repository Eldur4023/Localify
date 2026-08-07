//! Descarga de imágenes: portadas y fotos de artista.
//!
//! Está aquí y no en el adaptador de un proveedor concreto porque las URLs de
//! imagen son públicas y se descargan igual vengan de YouTube Music o de
//! Spotify. Es el mismo motivo por el que [`ImageFetcher`] es un puerto aparte
//! de `MetadataProvider`.
//!
//! ## Un tope de tamaño, no de confianza
//!
//! La URL la da el proveedor, así que no es hostil, pero sí puede ser un error:
//! una portada de 20 MB entraría entera en memoria antes de que nadie la mire.
//! El tope corta eso sin necesidad de comprobar nada más.

use async_trait::async_trait;
use localify_core::error::{CoreError, CoreResult};
use localify_core::ports::metadata_provider::ImageFetcher;
use tracing::debug;

/// Tope por imagen. Una portada de 1000×1000 en JPEG no llega a 500 KB; ocho
/// megabytes dan margen de sobra para PNG grandes sin dejar la puerta abierta.
const MAXIMO_BYTES: usize = 8 * 1024 * 1024;

/// Tope por descarga. Una portada que tarda más no vale la espera: la interfaz
/// ya está enseñando el hueco con su icono.
const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

#[derive(Debug, Clone)]
pub struct DescargadorDeImagenes {
    http: reqwest::Client,
}

impl DescargadorDeImagenes {
    /// # Errors
    /// Si el cliente HTTP no se puede construir.
    pub fn nuevo() -> Result<Self, reqwest::Error> {
        Ok(Self {
            http: reqwest::Client::builder().timeout(TIMEOUT).build()?,
        })
    }
}

#[async_trait]
impl ImageFetcher for DescargadorDeImagenes {
    async fn fetch(&self, url: &str) -> CoreResult<Vec<u8>> {
        let resp = self
            .http
            .get(url)
            .send()
            .await
            .map_err(|e| CoreError::provider_unavailable("imagenes", Box::new(e)))?
            .error_for_status()
            .map_err(|e| CoreError::provider_unavailable("imagenes", Box::new(e)))?;

        // Se mira la cabecera antes de leer: si el servidor declara un tamaño
        // absurdo, no hace falta descargarlo para descartarlo.
        if let Some(largo) = resp.content_length()
            && largo > MAXIMO_BYTES as u64
        {
            return Err(CoreError::invalid(format!(
                "imagen demasiado grande: {largo} bytes"
            )));
        }

        let bytes = resp
            .bytes()
            .await
            .map_err(|e| CoreError::provider_unavailable("imagenes", Box::new(e)))?;

        // Y después también: `Content-Length` es una promesa, no un hecho.
        if bytes.len() > MAXIMO_BYTES {
            return Err(CoreError::invalid(format!(
                "imagen demasiado grande: {} bytes",
                bytes.len()
            )));
        }

        debug!(url, bytes = bytes.len(), "imagen descargada");
        Ok(bytes.to_vec())
    }
}

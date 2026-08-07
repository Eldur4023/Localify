//! Letras desde LRCLIB.
//!
//! ## Por qué LRCLIB y no otro
//!
//! No pide clave, no pide cuenta y su catálogo está sincronizado línea a línea.
//! Cualquier alternativa con API key volvería a poner al usuario a crear una
//! aplicación en un sitio ajeno, que ya es el peaje de Spotify y no conviene
//! cobrarlo dos veces.
//!
//! ## La caché negativa es la mitad del trabajo
//!
//! La mayoría de las canciones **no** tienen letra. Sin recordar los fallos, la
//! aplicación consultaría LRCLIB cada vez que suena una de ellas: una petición
//! de red por reproducción, para recibir siempre el mismo 404. Con
//! `mark_not_found`, la segunda vez no toca la red.
//!
//! No caducar nunca sería igual de malo —una letra añadida después no
//! aparecería jamás— así que el negativo caduca, y de eso se encarga el
//! repositorio.
//!
//! ## Que no haya letra no es un error
//!
//! `Ok(None)` recorre todo el camino hasta la interfaz, que simplemente no
//! muestra el panel. Un `Err` haría aparecer un mensaje de fallo cada vez que
//! suena una canción instrumental.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use localify_core::domain::ids::TrackId;
use localify_core::domain::lyrics::Lyrics;
use localify_core::error::CoreResult;
use localify_core::ports::database::{LyricsRepository, TrackRepository};
use localify_core::ports::services::LyricsService;
use serde::Deserialize;
use tracing::{debug, warn};

/// Punto de entrada de la API pública.
const BASE: &str = "https://lrclib.net/api/get";

/// LRCLIB pide identificarse con nombre y versión, y es de buena educación
/// hacerlo: permite que puedan avisar si algo va mal por nuestra parte.
const AGENTE: &str = concat!(
    "Localify/",
    env!("CARGO_PKG_VERSION"),
    " (https://github.com/Eldur4023/Localify)"
);

/// Tope por petición. Una letra que tarda más que esto no vale la espera: la
/// canción ya está sonando.
const TIMEOUT: Duration = Duration::from_secs(6);

/// Respuesta de LRCLIB. Solo se leen los dos campos que interesan.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Respuesta {
    #[serde(default)]
    synced_lyrics: Option<String>,
    #[serde(default)]
    plain_lyrics: Option<String>,
}

pub struct Dependencias {
    pub repo: Arc<dyn LyricsRepository>,
    pub tracks: Arc<dyn TrackRepository>,
    pub http: reqwest::Client,
}

impl std::fmt::Debug for Dependencias {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Dependencias").finish_non_exhaustive()
    }
}

pub struct LyricsServiceImpl {
    deps: Dependencias,
}

impl std::fmt::Debug for LyricsServiceImpl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LyricsServiceImpl").finish_non_exhaustive()
    }
}

impl LyricsServiceImpl {
    #[must_use]
    pub fn nuevo(deps: Dependencias) -> Self {
        Self { deps }
    }

    /// Cliente HTTP con los valores que espera LRCLIB.
    ///
    /// # Errors
    /// Si no se puede construir el cliente (falta de entropía para TLS, por
    /// ejemplo). No es recuperable y hace que la integración quede inerte.
    pub fn cliente() -> Result<reqwest::Client, reqwest::Error> {
        reqwest::Client::builder()
            .user_agent(AGENTE)
            .timeout(TIMEOUT)
            .build()
    }
}

#[async_trait]
impl LyricsService for LyricsServiceImpl {
    async fn get(&self, track: &TrackId) -> CoreResult<Option<Lyrics>> {
        // 1. Lo que ya está guardado. Una letra no cambia, así que un acierto
        //    aquí termina el trabajo.
        if let Some(l) = self.deps.repo.get(track).await? {
            return Ok(Some(l));
        }

        // 2. Lo que ya se sabe que no existe. Es el caso mayoritario y el que
        //    justifica la caché negativa.
        if self.deps.repo.is_marked_not_found(track).await? {
            debug!(%track, "sin letra, según la caché negativa");
            return Ok(None);
        }

        // 3. Hace falta la pista para poder preguntar: LRCLIB busca por
        //    artista, título, álbum y duración, no por identificador.
        let Some(pista) = self.deps.tracks.get(track).await? else {
            return Ok(None);
        };

        let artista = pista
            .artists
            .first()
            .map(|a| a.name.clone())
            .unwrap_or_default();
        let album = pista.album.as_ref().map(|a| a.title.clone());

        let letra = match self
            .consultar(
                &artista,
                &pista.title,
                album.as_deref(),
                pista.duration.as_ms(),
            )
            .await
        {
            Ok(v) => v,
            Err(e) => {
                // Un fallo de red **no** se marca como "no existe": la letra
                // puede estar ahí perfectamente y quedaría descartada hasta que
                // caducara el negativo. Se devuelve `None` y se reintentará.
                warn!(%track, error = %e, "LRCLIB no respondió");
                return Ok(None);
            }
        };

        match letra {
            Some(l) if !l.esta_vacia() => {
                self.deps.repo.save(track, &l).await?;
                Ok(Some(l))
            }
            // Sin letra, o con una respuesta que viene vacía: las dos cosas
            // significan lo mismo para quien mira la pantalla.
            _ => {
                self.deps.repo.mark_not_found(track).await?;
                Ok(None)
            }
        }
    }
}

impl LyricsServiceImpl {
    /// Pregunta a LRCLIB. `Ok(None)` es un 404, que es una respuesta legítima.
    async fn consultar(
        &self,
        artista: &str,
        titulo: &str,
        album: Option<&str>,
        duracion_ms: u32,
    ) -> Result<Option<Lyrics>, reqwest::Error> {
        // La duración va en segundos y LRCLIB la usa para desempatar entre
        // versiones: sin ella, una canción con un remix del mismo nombre puede
        // devolver la letra del otro.
        let segundos = (duracion_ms / 1000).to_string();

        let mut params: Vec<(&str, &str)> = vec![
            ("artist_name", artista),
            ("track_name", titulo),
            ("duration", &segundos),
        ];
        if let Some(a) = album {
            params.push(("album_name", a));
        }

        let resp = self.deps.http.get(BASE).query(&params).send().await?;

        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let cuerpo: Respuesta = resp.error_for_status()?.json().await?;

        let sincronizada = cuerpo
            .synced_lyrics
            .as_deref()
            .and_then(crate::lrc::analizar);
        let plana = cuerpo.plain_lyrics.filter(|t| !t.trim().is_empty());

        if sincronizada.is_none() && plana.is_none() {
            return Ok(None);
        }

        Ok(Some(Lyrics {
            synced: sincronizada,
            plain: plana,
            source: "lrclib".to_owned(),
        }))
    }
}

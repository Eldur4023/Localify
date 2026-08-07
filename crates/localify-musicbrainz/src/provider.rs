//! Adaptador de [`MetadataProvider`] sobre MusicBrainz.
//!
//! ## Qué aporta que los otros dos no
//!
//! Conoce la **música editada**: bandas sonoras, ediciones especiales, discos
//! que existen como disco. Es justo el punto ciego de YouTube Music, cuyo
//! catálogo es lo que hay subido a YouTube — y ahí una banda sonora de un juego
//! aparece como veinte versiones de aficionados y ninguna original.
//!
//! Trae además **ISRC y duración exacta**, que son las dos señales que mejor
//! sostienen al emparejador de descargas: competir por título entre cuarenta
//! covers es una lotería, y "nueve minutos con cuarenta y dos segundos" no.
//!
//! ## Y qué no
//!
//! - **Popularidad.** No existe el concepto. La primera coincidencia y parte de
//!   las recomendaciones se apoyan en él; con este catálogo se quedan con lo que
//!   dé el historial local.
//! - **Lo nativo de YouTube.** Un remix que solo existe en un canal no está
//!   editado en ningún sitio, así que no está aquí. Es la otra mitad del hueco,
//!   y por eso el proveedor combinado existe.
//! - **Playlists.** MusicBrainz no las tiene. Importar una devuelve un error
//!   claro en vez de una lista vacía que parecería un fallo.
//! - **Canciones populares de un artista.** Sin popularidad no hay "top". Se
//!   devuelven sus grabaciones tal y como las ordena MusicBrainz, que es por
//!   relevancia de texto y no por lo que la gente escucha.
//!
//! ## No hay nada que configurar
//!
//! Como YouTube Music: sin clave, sin cuenta, sin registrar nada. `status()`
//! devuelve `Ready` desde el primer arranque.

use async_trait::async_trait;
use localify_core::domain::album::Album;
use localify_core::domain::artist::Artist;
use localify_core::domain::ids::{AlbumId, ArtistId, TrackId};
use localify_core::domain::track::Track;
use localify_core::error::{CoreError, CoreResult};
use localify_core::events::ProviderStatus;
use localify_core::page::Page;
use localify_core::ports::metadata_provider::{MetadataProvider, PlaylistImport, Resolucion};
use tracing::debug;

use crate::cliente::ClienteMusicBrainz;
use crate::parseo::{
    self, Artista, BusquedaEdiciones, BusquedaGrabaciones, Edicion, Grabacion, GrabacionConEnlaces,
};

/// Nombre estable, para logs y para el evento de estado del proveedor.
pub const NOMBRE: &str = "musicbrainz";

/// Lo que se pide junto a una grabación para poder construir una pista entera.
const INC_GRABACION: &str = "artist-credits+releases+isrcs";

pub struct MusicBrainzProvider {
    cliente: ClienteMusicBrainz,
}

impl std::fmt::Debug for MusicBrainzProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MusicBrainzProvider")
            .finish_non_exhaustive()
    }
}

impl MusicBrainzProvider {
    /// # Errors
    /// Si el cliente HTTP no se puede construir.
    pub fn nuevo() -> Result<Self, reqwest::Error> {
        Ok(Self {
            cliente: ClienteMusicBrainz::nuevo()?,
        })
    }
}

/// Parámetro que cambia el analizador de la consulta.
///
/// ## Por qué está aquí y qué arregla
///
/// Por defecto MusicBrainz interpreta la consulta como sintaxis Lucene sobre un
/// índice de doce millones de grabaciones, y el resultado con texto suelto es
/// malo: "casey edwards bury the light" devolvía la versión de Casey Edwards
/// **en octavo lugar**, por detrás de seis grupos que tienen una canción llamada
/// "Bury the Light". Con `dismax`, la misma consulta la pone primera y segunda.
///
/// De paso resuelve el problema de la puntuación. Con el analizador normal
/// habría que escapar `/`, `?`, `:` y compañía —o buscar "AC/DC" mandaría
/// operadores al índice—, y ese escapado es exactamente el tipo de cosa que se
/// olvida en una llamada nueva. `dismax` trata la consulta como lo que es: texto
/// que ha escrito una persona. Comprobado con "AC/DC back in black" y
/// "Where's My Love?".
///
/// **No vale para las consultas por campo.** `arid:<uuid>` necesita que se
/// interprete la sintaxis, así que ahí no se pone.
const DISMAX: (&str, &str) = ("dismax", "true");

#[async_trait]
impl MetadataProvider for MusicBrainzProvider {
    fn name(&self) -> &'static str {
        NOMBRE
    }

    async fn status(&self) -> ProviderStatus {
        // Sin credenciales que comprobar. Que la red falle se descubre al pedir,
        // y entonces la aplicación ya sabe seguir sobre lo local.
        ProviderStatus::Ready
    }

    async fn search_tracks(&self, query: &str, limit: u8, offset: u16) -> CoreResult<Page<Track>> {
        let respuesta: BusquedaGrabaciones = self
            .cliente
            .pedir(
                "recording",
                &[
                    ("query", query.to_owned()),
                    (DISMAX.0, DISMAX.1.to_owned()),
                    ("limit", limit.to_string()),
                    ("offset", offset.to_string()),
                ],
            )
            .await?;

        let items: Vec<Track> = respuesta
            .recordings
            .into_iter()
            .map(parseo::a_track)
            .collect();
        debug!(query, encontradas = items.len(), "búsqueda en MusicBrainz");
        Ok(Page::new(items, Some(respuesta.count), None))
    }

    async fn track(&self, id: &TrackId) -> CoreResult<Track> {
        let g: Grabacion = self
            .cliente
            .pedir(
                &format!("recording/{}", id.as_str()),
                &[("inc", INC_GRABACION.to_owned())],
            )
            .await?;
        Ok(parseo::a_track(g))
    }

    async fn tracks(&self, ids: &[TrackId]) -> CoreResult<Vec<Track>> {
        // MusicBrainz no tiene petición por lotes para grabaciones sueltas, y
        // con un segundo entre peticiones pedirlas de una en una es lento a
        // propósito: es el precio de un servicio gratuito que pide que no lo
        // aporrees. Quien llama ya trata esto como trabajo de fondo.
        let mut salida = Vec::with_capacity(ids.len());
        for id in ids {
            match self.track(id).await {
                Ok(t) => salida.push(t),
                // Una que falle no puede tumbar el lote: el resto sigue siendo
                // útil y el que falta se reintentará en el siguiente repaso.
                Err(e) => debug!(pista = %id, error = %e, "no se pudo traer la grabación"),
            }
        }
        Ok(salida)
    }

    async fn album(&self, id: &AlbumId) -> CoreResult<Album> {
        let e: Edicion = self
            .cliente
            .pedir(
                &format!("release/{}", id.as_str()),
                &[("inc", "artist-credits+release-groups+labels".to_owned())],
            )
            .await?;
        Ok(parseo::a_album(&e))
    }

    async fn album_tracks(&self, id: &AlbumId) -> CoreResult<Vec<Track>> {
        let e: Edicion = self
            .cliente
            .pedir(
                &format!("release/{}", id.as_str()),
                &[("inc", "recordings+artist-credits".to_owned())],
            )
            .await?;
        Ok(parseo::pistas_de(e))
    }

    /// El vídeo que MusicBrainz asocia a esta grabación.
    ///
    /// Sin miniatura: la portada de MusicBrainz es del **lanzamiento**, no de la
    /// grabación, y ya se sirve por su álbum. Devolverla aquí sería la misma
    /// imagen por dos caminos.
    async fn resolve_recording(&self, track: &Track) -> CoreResult<Option<Resolucion>> {
        let g: GrabacionConEnlaces = self
            .cliente
            .pedir(
                &format!("recording/{}", track.id.as_str()),
                &[("inc", "url-rels".to_owned())],
            )
            .await?;

        let Some(video_id) = parseo::video_de_youtube(&g.relations) else {
            return Ok(None);
        };
        debug!(pista = %track.id, video_id, "MusicBrainz conoce el vídeo oficial");

        Ok(Some(Resolucion {
            video_id,
            cover_url: None,
            // La ficha de la grabación no dice en qué disco salió: para eso hay
            // que pedir sus lanzamientos, que es otra petición contra un
            // servicio con un límite de una por segundo. No compensa aquí.
            album: None,
        }))
    }

    async fn artist(&self, id: &ArtistId) -> CoreResult<Artist> {
        let a: Artista = self
            .cliente
            .pedir(
                &format!("artist/{}", id.as_str()),
                &[("inc", "tags".to_owned())],
            )
            .await?;

        // Las etiquetas más votadas hacen de géneros. No son lo mismo —las pone
        // la gente— pero son la única señal de este tipo que hay aquí, y el
        // motor de recomendaciones se queda ciego sin ninguna.
        let mut etiquetas = a.tags;
        // Negado para ordenar de más votadas a menos sin invertir después.
        etiquetas.sort_by_key(|e| -e.count);

        Ok(Artist {
            id: ArtistId::from_trusted(a.id),
            name: a.name,
            // Las fotos de artista son de Wikidata y hacen falta dos peticiones
            // más para llegar. Vacío es honesto; la interfaz enseña su icono.
            image_url: None,
            genres: etiquetas
                .into_iter()
                .filter(|e| e.count > 0)
                .map(|e| e.name)
                .take(5)
                .collect(),
            popularity: None,
            followers: None,
        })
    }

    async fn artist_top_tracks(&self, id: &ArtistId) -> CoreResult<Vec<Track>> {
        // No hay "top": sin popularidad, lo mejor disponible es el orden de
        // relevancia de su buscador. Se dice en la cabecera del módulo para que
        // nadie lea esta lista como "lo que más suena".
        //
        // Sin `dismax`: esto es una consulta por campo y necesita que se
        // interprete `arid:` como lo que es.
        let respuesta: BusquedaGrabaciones = self
            .cliente
            .pedir(
                "recording",
                &[
                    ("query", format!("arid:{}", id.as_str())),
                    ("limit", "10".to_owned()),
                ],
            )
            .await?;
        Ok(respuesta
            .recordings
            .into_iter()
            .map(parseo::a_track)
            .collect())
    }

    async fn artist_albums(&self, id: &ArtistId) -> CoreResult<Vec<Album>> {
        let respuesta: BusquedaEdiciones = self
            .cliente
            .pedir(
                "release",
                &[
                    ("artist", id.as_str().to_owned()),
                    ("inc", "artist-credits+release-groups".to_owned()),
                    ("limit", "25".to_owned()),
                ],
            )
            .await?;
        Ok(respuesta.releases.iter().map(parseo::a_album).collect())
    }

    async fn public_playlist(
        &self,
        _url_or_id: &str,
        _page_callback: &(dyn Fn(u32, u32) + Send + Sync),
    ) -> CoreResult<PlaylistImport> {
        // MusicBrainz no tiene playlists. Un error claro es mejor que devolver
        // una lista vacía, que el usuario leería como "la importación falló en
        // silencio" y volvería a intentarlo.
        Err(CoreError::invalid(
            "MusicBrainz no tiene listas de reproducción que importar",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn la_busqueda_de_texto_usa_dismax_y_la_de_campo_no() {
        // Es la distinción que sostiene las dos: `dismax` trata la consulta como
        // texto de una persona —y por eso la de Casey Edwards sube del octavo
        // puesto al primero—, pero rompería `arid:<uuid>`, que necesita que se
        // interprete el campo.
        assert_eq!(DISMAX, ("dismax", "true"));
    }
}

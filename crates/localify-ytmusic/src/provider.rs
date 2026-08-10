//! Adaptador de [`MetadataProvider`] sobre YouTube Music.
//!
//! ## No hay nada que configurar
//!
//! Es la diferencia práctica más grande con Spotify: InnerTube no pide clave ni
//! cuenta, así que `status()` devuelve `Ready` desde el primer arranque. Nadie
//! tiene que crear una aplicación en ningún sitio para que la búsqueda
//! funcione.
//!
//! ## Lo que este catálogo no da
//!
//! - **ISRC.** Es la señal más fiable para localizar una grabación concreta, y
//!   aquí no hace falta: el identificador de la pista **es** el del vídeo que se
//!   va a descargar, así que no hay dos catálogos que emparejar.
//! - **Géneros.** El motor de recomendaciones los usa como señal principal
//!   heredándolos del artista; con este proveedor se apoyará solo en el
//!   historial y la coocurrencia.
//! - **Popularidad.** YouTube da reproducciones, que no es lo mismo y viene
//!   abreviada ("1,5 M"); convertirla daría un número inventado.
//!
//! Ninguna de las tres se rellena a ojo. Un campo vacío se ve; uno inventado,
//! no.
//!
//! ## Las peticiones fallidas no son errores del dominio
//!
//! Un fallo de red devuelve `ProviderUnavailable`, que la aplicación ya sabe
//! tratar: sigue funcionando sobre la biblioteca local y lo dice en la interfaz.

use async_trait::async_trait;
use localify_core::domain::album::Album;
use localify_core::domain::artist::Artist;
use localify_core::domain::ids::{AlbumId, ArtistId, TrackId};
use localify_core::domain::track::{AlbumRef, Track};
use localify_core::error::{CoreError, CoreResult};
use localify_core::events::ProviderStatus;
use localify_core::page::Page;
use localify_core::ports::metadata_provider::{MetadataProvider, PlaylistImport, Resolucion};
use tracing::{debug, warn};

use crate::innertube::{ClienteInnerTube, Filtro, buscar_uno, estanterias, texto_de};
use crate::parseo;

/// Nombre estable, para logs y para el evento de estado del proveedor.
pub const NOMBRE: &str = "ytmusic";

/// Longitud de un identificador de vídeo de YouTube.
const LARGO_VIDEO: usize = 11;

/// Margen al comparar duraciones, en milisegundos.
///
/// Diez segundos: la misma grabación difiere unos pocos entre catálogos —donde
/// uno corta el silencio final, otro no— y una versión distinta se aleja mucho
/// más. Con menos margen se descartarían coincidencias buenas; con más entrarían
/// las extendidas.
const MARGEN_MS: u32 = 10_000;

/// `true` si la cadena tiene forma de identificador de vídeo.
///
/// En este catálogo el identificador de una pista **es** el del vídeo, pero no
/// el de un álbum ni el de un artista, que también son `TrackId` en otros
/// contextos. Comprobar la forma evita mandar a descargar un `MPREb_…`.
fn es_video(id: &str) -> bool {
    id.len() == LARGO_VIDEO
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// `true` si la cadena tiene forma de identificador de canal de YouTube.
///
/// Los artistas de este catálogo **son** canales: `UC` y veintidós caracteres
/// más. Cualquier otra cosa —un UUID de MusicBrainz, un identificador local de
/// los que se inventan al importar una playlist, un id de Spotify— no existe
/// aquí, y pedirla devuelve `400` después de haber gastado la petición.
///
/// Comprobarlo antes de salir a la red no es una optimización: sin esto, Inicio
/// llenaba el log de errores de YouTube Music por artistas que este catálogo
/// nunca podría conocer, y el ruido tapa los fallos de verdad.
fn es_canal(id: &str) -> bool {
    const LARGO_CANAL: usize = 24;
    id.len() == LARGO_CANAL
        && id.starts_with("UC")
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// `true` si las dos duraciones son la misma canción.
///
/// Una duración desconocida —cero— no descarta: es lo que devuelve este
/// catálogo cuando no la trae, y exigirla dejaría fuera coincidencias buenas.
fn duracion_compatible(pista: &Track, candidato: &Track) -> bool {
    let (a, b) = (pista.duration.as_ms(), candidato.duration.as_ms());
    if a == 0 || b == 0 {
        return true;
    }
    a.abs_diff(b) <= MARGEN_MS
}

pub struct YtMusicProvider {
    cliente: ClienteInnerTube,
}

impl std::fmt::Debug for YtMusicProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("YtMusicProvider").finish_non_exhaustive()
    }
}

impl YtMusicProvider {
    /// # Errors
    /// Si el cliente HTTP no se puede construir.
    pub fn nuevo(idioma: &str, pais: &str) -> Result<Self, reqwest::Error> {
        Ok(Self {
            cliente: ClienteInnerTube::nuevo(idioma, pais)?,
        })
    }

    fn ahora() -> chrono::DateTime<chrono::Utc> {
        chrono::Utc::now()
    }
}

/// Traduce un fallo de red al error del dominio.
fn caido(e: &reqwest::Error) -> CoreError {
    warn!(error = %e, "YouTube Music no respondió");
    CoreError::provider_unavailable(
        "ytmusic",
        // El detalle técnico se queda en el log; el usuario recibe una clave
        // i18n a través de la capa de errores.
        Box::<dyn std::error::Error + Send + Sync>::from(e.to_string()),
    )
}

#[async_trait]
impl MetadataProvider for YtMusicProvider {
    fn name(&self) -> &'static str {
        NOMBRE
    }

    async fn status(&self) -> ProviderStatus {
        // No hay credenciales que comprobar. Se podría hacer una petición de
        // prueba, pero eso convertiría abrir Ajustes en tráfico de red para
        // confirmar algo que solo puede fallar si no hay internet, y eso ya se
        // ve al buscar.
        ProviderStatus::Ready
    }

    /// El vídeo de esta grabación, por identidad o buscándolo en el catálogo.
    ///
    /// ## Si el identificador ya es un vídeo, no hay nada que hacer
    ///
    /// Es el caso de todo lo que sale de la búsqueda: el id de la pista **es**
    /// el del vídeo. Emparejarla por texto era hacer tres consultas a YouTube
    /// para acabar eligiendo —con suerte— el vídeo que ya teníamos delante.
    ///
    /// ## Y si no, se busca **aquí** y no en YouTube a secas
    ///
    /// Es la diferencia que se nota en lo importado de una lista de Spotify.
    /// `ytsearch` recorre YouTube entero y devuelve vídeos de letras, covers y
    /// bucles de una hora; la búsqueda de YouTube Music es un catálogo de música
    /// y esas cosas ni las lista.
    ///
    /// Se exige que la duración cuadre: sin ese filtro, una canción que aquí no
    /// esté devolvería la primera cosa parecida y le daríamos al emparejador una
    /// pista falsa con aire de certeza.
    async fn resolve_recording(&self, track: &Track) -> CoreResult<Option<Resolucion>> {
        let artista = track
            .artista_principal()
            .map(|a| a.name.as_str())
            .unwrap_or_default();
        let titulo = track.title.trim();
        if titulo.is_empty() {
            return Ok(None);
        }

        // Se busca **también** cuando el identificador ya es un vídeo. Suena
        // redundante y no lo es: ahí ya sabemos qué bajar, pero no tenemos la
        // miniatura, y esta es la única llamada que la trae.
        let consulta = if artista.is_empty() {
            titulo.to_owned()
        } else {
            format!("{artista} {titulo}")
        };

        let resp = self
            .cliente
            .buscar(&consulta, Filtro::Canciones)
            .await
            .map_err(|e| caido(&e))?;

        // Se recorren los elementos crudos y no `search_tracks` porque la
        // miniatura vive en el elemento y `Track` no tiene dónde guardarla. Es
        // el dato que venimos a buscar.
        let ahora = Self::ahora();
        let elegido = estanterias(&resp)
            .into_iter()
            .flat_map(|(_, elementos)| elementos)
            .find_map(|e| {
                let candidata = parseo::cancion(e, ahora)?;
                if !es_video(candidata.id.as_str()) || !duracion_compatible(track, &candidata) {
                    return None;
                }
                Some(Resolucion {
                    video_id: candidata.id.into_string(),
                    cover_url: parseo::miniatura_publica(e),
                    album: candidata.album,
                })
            });

        if let Some(r) = &elegido {
            debug!(
                pista = %track.id,
                video = %r.video_id,
                miniatura = r.cover_url.is_some(),
                "resuelta por YouTube Music"
            );
        }
        Ok(elegido)
    }

    async fn search_tracks(&self, query: &str, limit: u8, offset: u16) -> CoreResult<Page<Track>> {
        let resp = self
            .cliente
            .buscar(query, Filtro::Canciones)
            .await
            .map_err(|e| caido(&e))?;

        let ahora = Self::ahora();
        let todas: Vec<Track> = estanterias(&resp)
            .into_iter()
            .flat_map(|(_, elementos)| elementos)
            .filter_map(|e| parseo::cancion(e, ahora))
            .collect();

        // InnerTube pagina con un token de continuación, no con desplazamiento.
        // Se recorta en local: pedir la segunda página exigiría guardar el
        // token entre llamadas, y el puerto no tiene dónde. Veinte resultados
        // por búsqueda son de sobra para elegir una canción.
        let desde = usize::from(offset).min(todas.len());
        let items: Vec<Track> = todas
            .into_iter()
            .skip(desde)
            .take(usize::from(limit))
            .collect();

        debug!(
            query,
            encontradas = items.len(),
            "búsqueda en YouTube Music"
        );
        Ok(Page::new(items, None, None))
    }

    async fn track(&self, id: &TrackId) -> CoreResult<Track> {
        let resp = self
            .cliente
            .reproductor(id.as_str())
            .await
            .map_err(|e| caido(&e))?;

        parseo::pista_de_reproductor(&resp, id, Self::ahora())
            .ok_or_else(|| CoreError::not_found("pista", id.as_str()))
    }

    async fn tracks(&self, ids: &[TrackId]) -> CoreResult<Vec<Track>> {
        // No hay endpoint de lote: el reproductor atiende un vídeo por
        // petición. Se hacen en serie y no en paralelo a propósito, porque
        // lanzar cincuenta peticiones simultáneas a YouTube es la forma más
        // rápida de que empiece a responder con captchas.
        let mut salida = Vec::with_capacity(ids.len());
        for id in ids {
            match self.track(id).await {
                Ok(t) => salida.push(t),
                // Una pista que ya no existe no puede tumbar la carga de las
                // otras cuarenta y nueve.
                Err(e) => debug!(%id, error = %e, "pista no recuperable, se omite"),
            }
        }
        Ok(salida)
    }

    async fn album(&self, id: &AlbumId) -> CoreResult<Album> {
        let resp = self
            .cliente
            .navegar(id.as_str())
            .await
            .map_err(|e| caido(&e))?;

        parseo::album_de_pagina(&resp, id).ok_or_else(|| CoreError::not_found("álbum", id.as_str()))
    }

    async fn album_tracks(&self, id: &AlbumId) -> CoreResult<Vec<Track>> {
        let resp = self
            .cliente
            .navegar(id.as_str())
            .await
            .map_err(|e| caido(&e))?;

        // Las filas de un álbum no repiten artista ni álbum —ya están en la
        // cabecera—, así que hay que leerla para poder devolver pistas
        // completas. Es una sola respuesta: no hay petición de más.
        let cabecera = parseo::album_de_pagina(&resp, id);
        let referencia = AlbumRef {
            id: id.clone(),
            title: cabecera
                .as_ref()
                .map(|a| a.title.clone())
                .unwrap_or_default(),
        };
        let artistas = cabecera.map(|a| a.artists).unwrap_or_default();

        Ok(parseo::pistas_de_album(
            &resp,
            &referencia,
            &artistas,
            Self::ahora(),
        ))
    }

    async fn artist(&self, id: &ArtistId) -> CoreResult<Artist> {
        // Igual que en `artist_top_tracks`: un identificador que no es un canal
        // no existe aquí, y preguntarlo son 400 y una petición gastada.
        if !es_canal(id.as_str()) {
            return Err(CoreError::not_found("artista", id.as_str()));
        }

        let resp = self
            .cliente
            .navegar(id.as_str())
            .await
            .map_err(|e| caido(&e))?;

        parseo::artista_de_pagina(&resp, id)
            .ok_or_else(|| CoreError::not_found("artista", id.as_str()))
    }

    async fn artist_top_tracks(&self, id: &ArtistId) -> CoreResult<Vec<Track>> {
        // No es de aquí: no hay nada que preguntar. Ver `es_canal`.
        if !es_canal(id.as_str()) {
            debug!(artista = %id, "no es un canal de YouTube: no se pregunta");
            return Ok(Vec::new());
        }

        let resp = self
            .cliente
            .navegar(id.as_str())
            .await
            .map_err(|e| caido(&e))?;

        let ahora = Self::ahora();
        Ok(crate::innertube::elementos_de_lista(&resp)
            .into_iter()
            .filter_map(|e| parseo::cancion(e, ahora))
            .collect())
    }

    async fn artist_albums(&self, id: &ArtistId) -> CoreResult<Vec<Album>> {
        if !es_canal(id.as_str()) {
            debug!(artista = %id, "no es un canal de YouTube: no se pregunta");
            return Ok(Vec::new());
        }

        let resp = self
            .cliente
            .navegar(id.as_str())
            .await
            .map_err(|e| caido(&e))?;
        Ok(parseo::albumes_de_carrusel(&resp))
    }

    async fn public_playlist(
        &self,
        url_or_id: &str,
        page_callback: &(dyn Fn(u32, u32) + Send + Sync),
    ) -> CoreResult<PlaylistImport> {
        let id = identificador_de_lista(url_or_id)
            .ok_or_else(|| CoreError::invalid(format!("no parece una lista: '{url_or_id}'")))?;

        let resp = self.cliente.navegar(&id).await.map_err(|e| caido(&e))?;

        let cabecera = buscar_uno(&resp, "musicResponsiveHeaderRenderer")
            .or_else(|| buscar_uno(&resp, "musicDetailHeaderRenderer"));
        let nombre = cabecera
            .and_then(|c| texto_de(c.get("title")))
            .unwrap_or_else(|| "Lista importada".to_owned());

        let ahora = Self::ahora();
        let pistas: Vec<Track> = crate::innertube::elementos_de_lista(&resp)
            .into_iter()
            .filter_map(|e| parseo::cancion(e, ahora))
            .collect();

        let total = u32::try_from(pistas.len()).unwrap_or(u32::MAX);
        // Una sola petición trae la primera tanda; el aviso se emite igual para
        // que quien importe vea progreso desde el principio en vez de un salto
        // de cero a todo.
        page_callback(total, total);

        Ok(PlaylistImport {
            source_id: id,
            name: nombre,
            description: cabecera.and_then(|c| texto_de(c.get("description"))),
            cover_url: cabecera.and_then(parseo::miniatura_publica),
            total,
            tracks: pistas,
        })
    }
}

/// Extrae el identificador de lista de una URL o lo devuelve tal cual.
///
/// Acepta las formas en las que un usuario copia una lista: la URL completa de
/// `music.youtube.com` o de `youtube.com`, y el identificador suelto. El prefijo
/// `VL` es el que espera el endpoint de navegación y no aparece en las URLs, así
/// que se añade.
#[must_use]
pub fn identificador_de_lista(entrada: &str) -> Option<String> {
    let limpio = entrada.trim();

    let bruto = if limpio.contains("://") {
        // `?list=PL...` es donde va en las dos webs.
        limpio
            .split(['?', '&'])
            .find_map(|p| p.strip_prefix("list="))?
    } else {
        limpio
    };

    if bruto.is_empty() {
        return None;
    }
    if bruto.starts_with("VL") {
        return Some(bruto.to_owned());
    }
    Some(format!("VL{bruto}"))
}

#[cfg(test)]
mod tests_identidad {
    use super::*;
    use localify_core::domain::audio::DurationMs;
    use localify_core::domain::ids::TrackId;

    fn pista(id: &str, segundos: u32) -> Track {
        Track {
            id: TrackId::from_trusted(id.to_owned()),
            title: "X".into(),
            album: None,
            artists: Vec::new(),
            duration: DurationMs::from_secs(segundos),
            track_number: None,
            disc_number: None,
            explicit: false,
            isrc: None,
            release_date: None,
            popularity: None,
            added_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn el_identificador_de_una_pista_de_este_catalogo_es_un_video() {
        assert!(es_video("kM0Fpbz0W8U"));
        assert!(es_video("9WfWnkVLwao"));
        assert!(es_video("6Wg1_YOfiM0"), "admite guion bajo");
    }

    #[test]
    fn lo_que_no_es_un_video_no_se_manda_a_descargar() {
        // Un álbum o un canal también viajan como identificador en otros
        // contextos, y mandarlos a yt-dlp bajaría cualquier cosa.
        assert!(!es_video("MPREb_m2xZZHGzRl1"), "álbum");
        assert!(!es_video("UCEPMVbUzImPl4p8k4LkGevA"), "canal");
        assert!(!es_video("3z8h0TU7ReDPLIbEnYhWZb"), "base62 de Spotify");
        assert!(!es_video("0578c31a-4ab4-4181-b05d-1a0a62e49bec"), "MBID");
    }

    #[test]
    fn una_duracion_distinta_descarta_el_candidato() {
        // Sin este filtro, una canción que no esté en YouTube Music devolvería
        // la primera cosa parecida, y le daríamos al emparejador una pista falsa
        // con aire de certeza.
        let original = pista("kM0Fpbz0W8U", 180);
        assert!(duracion_compatible(&original, &pista("otro1234567", 185)));
        assert!(!duracion_compatible(&original, &pista("otro1234567", 600)));
    }

    #[test]
    fn una_duracion_desconocida_no_descarta() {
        // Este catálogo no siempre la trae, y exigirla dejaría fuera
        // coincidencias buenas.
        let original = pista("kM0Fpbz0W8U", 180);
        assert!(duracion_compatible(&original, &pista("otro1234567", 0)));
        assert!(duracion_compatible(
            &pista("kM0Fpbz0W8U", 0),
            &pista("otro1234567", 180)
        ));
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "en un test, un `expect` que falla es el fallo"
)]
mod tests {
    use super::*;

    #[test]
    fn el_identificador_de_lista_se_extrae_de_una_url() {
        assert_eq!(
            identificador_de_lista("https://music.youtube.com/playlist?list=PLabc123")
                .expect("se extrae"),
            "VLPLabc123"
        );
        assert_eq!(
            identificador_de_lista("https://www.youtube.com/watch?v=xyz&list=PLabc123")
                .expect("se extrae"),
            "VLPLabc123"
        );
    }

    #[test]
    fn un_identificador_suelto_vale_igual() {
        assert_eq!(
            identificador_de_lista("PLabc123").expect("vale"),
            "VLPLabc123"
        );
        // Y si ya trae el prefijo, no se duplica.
        assert_eq!(
            identificador_de_lista("VLPLabc123").expect("vale"),
            "VLPLabc123"
        );
    }

    #[test]
    fn una_url_sin_lista_no_es_una_lista() {
        assert!(identificador_de_lista("https://music.youtube.com/watch?v=abc").is_none());
        assert!(identificador_de_lista("").is_none());
    }
}

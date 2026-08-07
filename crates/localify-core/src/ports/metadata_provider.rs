//! Puerto del proveedor de metadatos.
//!
//! Spotify es la implementación de hoy, pero el trait no lo menciona: si
//! Spotify restringe más su API, añadir MusicBrainz o Deezer es escribir otro
//! crate de infraestructura sin tocar ningún servicio (riesgo previsto en el
//! roadmap).

use async_trait::async_trait;

use crate::domain::album::Album;
use crate::domain::artist::Artist;
use crate::domain::ids::{AlbumId, ArtistId, TrackId};
use crate::domain::track::{AlbumRef, Track};
use crate::error::CoreResult;
use crate::events::ProviderStatus;
use crate::page::Page;

/// Playlist remota lista para importar.
#[derive(Debug, Clone, PartialEq)]
pub struct PlaylistImport {
    pub source_id: String,
    pub name: String,
    pub description: Option<String>,
    pub cover_url: Option<String>,
    pub total: u32,
    pub tracks: Vec<Track>,
}

/// Lo que un catálogo sabe de una grabación que le hemos descrito.
///
/// No es una `Track`: es la respuesta a "¿cuál de las tuyas es esta?", y trae
/// solo lo que sirve para bajarla y para enseñarla.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolucion {
    /// Vídeo de YouTube que hay que descargar.
    pub video_id: String,
    /// Miniatura **de la canción**, no de su álbum.
    ///
    /// Son cosas distintas y la diferencia se ve: el álbum es una indirección
    /// que puede apuntar a otra edición —una recopilación, un directo— y acabar
    /// enseñando una carátula que no es la de esta canción. La miniatura viene
    /// en el propio resultado y es la que el catálogo pone junto al título.
    pub cover_url: Option<String>,
    /// El disco al que el catálogo dice que pertenece.
    ///
    /// Viene de la misma respuesta que lo demás, así que es gratis, y hay quien
    /// lo necesita: una playlist importada de Spotify llega **sin álbumes** —su
    /// página de incrustación no los publica— y sin esto sus canciones se
    /// quedarían para siempre sin disco en una biblioteca que sí lo enseña.
    pub album: Option<AlbumRef>,
}

#[async_trait]
pub trait MetadataProvider: Send + Sync + 'static {
    /// Nombre estable para logs y eventos (`"spotify"`).
    fn name(&self) -> &'static str;

    /// Estado actual. Que no esté configurado **no es un error**: es un modo
    /// de operación previsto en el que la app funciona sobre lo local.
    async fn status(&self) -> ProviderStatus;

    async fn search_tracks(&self, query: &str, limit: u8, offset: u16) -> CoreResult<Page<Track>>;

    async fn track(&self, id: &TrackId) -> CoreResult<Track>;

    /// Varias pistas de una vez. La implementación agrupa en lotes del tamaño
    /// que admita el proveedor; los servicios no deben saber ese límite.
    async fn tracks(&self, ids: &[TrackId]) -> CoreResult<Vec<Track>>;

    async fn album(&self, id: &AlbumId) -> CoreResult<Album>;
    async fn album_tracks(&self, id: &AlbumId) -> CoreResult<Vec<Track>>;

    /// Vídeo de YouTube que corresponde a esta grabación, según el catálogo.
    ///
    /// ## Por qué está en este puerto y no en el de YouTube
    ///
    /// Porque quien mejor lo sabe es quien describe la música, y lo sabe de tres
    /// formas distintas según el catálogo:
    ///
    /// 1. **Es su propio identificador.** En YouTube Music, el id de la pista
    ///    *es* el del vídeo. No hay nada que buscar y no cuesta ni una petición.
    /// 2. **Lo tiene guardado.** MusicBrainz relaciona muchas grabaciones con su
    ///    vídeo oficial.
    /// 3. **Lo puede buscar en su catálogo.** Y esto es lo que más cambia: la
    ///    búsqueda de YouTube Music es un catálogo de música curado, mientras
    ///    que `ytsearch` de yt-dlp es YouTube entero —vídeos de letras, covers,
    ///    bucles de una hora—. Con la misma canción, el segundo devuelve basura
    ///    que el primero ni lista.
    ///
    /// Recibe la pista entera y no solo su identificador precisamente por (3):
    /// para buscarla hacen falta el título y el artista.
    ///
    /// ## Devuelve también la miniatura
    ///
    /// Porque la respuesta trae más de lo que se pedía y tirarlo sería absurdo.
    /// Lo importado de una lista de Spotify llega sin nada con lo que pintar una
    /// portada, y el resultado del catálogo trae la de la canción.
    ///
    /// Se probó antes con el **álbum** del resultado, y era peor: el álbum es
    /// una indirección que a veces apunta a otra edición, así que la carátula
    /// que salía no era la de la canción. La miniatura no tiene ese problema
    /// porque no pasa por ningún sitio.
    ///
    /// ## Es una pista, no una orden
    ///
    /// Quien la recibe la mete como un candidato más y la puntúa con el resto,
    /// así que una respuesta equivocada no puede meter una canción errónea en la
    /// biblioteca. Y eso importa: lo descargado no se vuelve a descargar.
    ///
    /// Por defecto `None`, que no es un fallo: significa "no lo sé".
    ///
    /// **Cuidado con ese valor por defecto.** Este puerto lo envuelven dos
    /// delegadores —el conmutador y el combinado— y un método con
    /// implementación por defecto no obliga a delegarlo: olvidarlo compila, no
    /// rompe ningún test y hace que el envoltorio conteste "no lo sé" en nombre
    /// de un catálogo al que nunca preguntó. Ya pasó una vez.
    async fn resolve_recording(&self, _track: &Track) -> CoreResult<Option<Resolucion>> {
        Ok(None)
    }

    async fn artist(&self, id: &ArtistId) -> CoreResult<Artist>;
    async fn artist_top_tracks(&self, id: &ArtistId) -> CoreResult<Vec<Track>>;
    async fn artist_albums(&self, id: &ArtistId) -> CoreResult<Vec<Album>>;

    /// Importa una playlist pública.
    ///
    /// `page_callback` recibe cada página conforme llega, para poder emitir
    /// progreso e ir persistiendo sin esperar a que termine todo. Una playlist
    /// de 1 000 pistas son 10 peticiones: acumularlas en memoria y no dar señal
    /// de vida sería una espera opaca de varios segundos.
    async fn public_playlist(
        &self,
        url_or_id: &str,
        page_callback: &(dyn Fn(u32, u32) + Send + Sync),
    ) -> CoreResult<PlaylistImport>;
}

/// Descarga de imágenes (portadas, fotos de artista).
///
/// Separado de [`MetadataProvider`] porque no depende del proveedor: las URLs
/// son públicas y se descargan igual vengan de donde vengan.
#[async_trait]
pub trait ImageFetcher: Send + Sync + 'static {
    /// Devuelve los bytes de la imagen.
    async fn fetch(&self, url: &str) -> CoreResult<Vec<u8>>;
}

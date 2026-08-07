//! Proveedor de metadatos conmutable.
//!
//! ## Por qué hace falta una indirección
//!
//! Los servicios reciben un `Arc<dyn MetadataProvider>` al construirse y lo
//! guardan. Cambiar de catálogo desde Ajustes tendría que sustituir esa
//! referencia en todos ellos a la vez, o reconstruir medio contenedor de
//! dependencias con la aplicación en marcha.
//!
//! Este tipo lo resuelve siendo **él** el proveedor que reciben todos: por
//! dentro guarda los dos y delega en el que toque. Cambiar de proveedor es
//! entonces escribir un valor, y quien esté a mitad de una búsqueda la termina
//! con el que empezó.
//!
//! ## Spotify puede no estar
//!
//! Sin credenciales, el adaptador de Spotify existe pero no puede responder.
//! Elegirlo en ese estado no es un error del conmutador: se delega igual y es
//! el propio adaptador quien informa de que no está configurado, que es donde
//! vive esa información.

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use async_trait::async_trait;
use localify_core::domain::album::Album;
use localify_core::domain::artist::Artist;
use localify_core::domain::ids::{AlbumId, ArtistId, TrackId};
use localify_core::domain::settings::MetadataProviderKind;
use localify_core::domain::track::Track;
use localify_core::error::CoreResult;
use localify_core::events::ProviderStatus;
use localify_core::page::Page;
use localify_core::ports::metadata_provider::{MetadataProvider, PlaylistImport, Resolucion};
use tracing::info;

/// Códigos de proveedor, como enteros para poder guardarlos en un atómico.
const YTMUSIC: u8 = 0;
const SPOTIFY: u8 = 1;
const MUSICBRAINZ: u8 = 2;
const COMBINADO: u8 = 3;

pub struct ProveedorConmutable {
    ytmusic: Arc<dyn MetadataProvider>,
    spotify: Arc<dyn MetadataProvider>,
    musicbrainz: Arc<dyn MetadataProvider>,
    /// YouTube Music y MusicBrainz a la vez. Es un proveedor más, no un modo:
    /// así el conmutador sigue haciendo una sola cosa —elegir— en lugar de
    /// aprender a combinar.
    combinado: Arc<dyn MetadataProvider>,
    /// Proveedor activo.
    ///
    /// Atómico y no `RwLock` porque se lee en cada llamada al proveedor y se
    /// escribe una vez cada varios meses: un cerrojo aquí sería pagar por una
    /// exclusión que nunca hace falta.
    activo: AtomicU8,
}

impl std::fmt::Debug for ProveedorConmutable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProveedorConmutable")
            .field("activo", &self.tipo())
            .finish_non_exhaustive()
    }
}

impl ProveedorConmutable {
    #[must_use]
    pub fn nuevo(
        ytmusic: Arc<dyn MetadataProvider>,
        spotify: Arc<dyn MetadataProvider>,
        musicbrainz: Arc<dyn MetadataProvider>,
        combinado: Arc<dyn MetadataProvider>,
        inicial: MetadataProviderKind,
    ) -> Self {
        let s = Self {
            ytmusic,
            spotify,
            musicbrainz,
            combinado,
            activo: AtomicU8::new(COMBINADO),
        };
        s.cambiar(inicial);
        s
    }

    /// Cambia el catálogo activo. Tiene efecto en la siguiente llamada.
    pub fn cambiar(&self, tipo: MetadataProviderKind) {
        let codigo = match tipo {
            MetadataProviderKind::Combinado => COMBINADO,
            MetadataProviderKind::YtMusic => YTMUSIC,
            MetadataProviderKind::MusicBrainz => MUSICBRAINZ,
            MetadataProviderKind::Spotify => SPOTIFY,
        };
        if self.activo.swap(codigo, Ordering::Relaxed) != codigo {
            info!(proveedor = tipo.code(), "catálogo de metadatos cambiado");
        }
    }

    #[must_use]
    pub fn tipo(&self) -> MetadataProviderKind {
        match self.activo.load(Ordering::Relaxed) {
            SPOTIFY => MetadataProviderKind::Spotify,
            YTMUSIC => MetadataProviderKind::YtMusic,
            MUSICBRAINZ => MetadataProviderKind::MusicBrainz,
            _ => MetadataProviderKind::Combinado,
        }
    }

    fn actual(&self) -> &Arc<dyn MetadataProvider> {
        match self.activo.load(Ordering::Relaxed) {
            SPOTIFY => &self.spotify,
            YTMUSIC => &self.ytmusic,
            MUSICBRAINZ => &self.musicbrainz,
            _ => &self.combinado,
        }
    }
}

/// Quién sirve una referencia de playlist.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Duenyo {
    Spotify,
    YouTube,
}

/// De quién es esta URL, si se puede saber.
///
/// Mira el dominio y el esquema propio de cada uno. No intenta validar el
/// identificador: de eso ya se encarga el adaptador, que sabe qué formas acepta.
fn duenyo_de(referencia: &str) -> Option<Duenyo> {
    let r = referencia.trim().to_ascii_lowercase();
    if r.contains("spotify.com") || r.starts_with("spotify:") {
        return Some(Duenyo::Spotify);
    }
    if r.contains("youtube.com") || r.contains("youtu.be") {
        return Some(Duenyo::YouTube);
    }
    None
}

#[async_trait]
impl MetadataProvider for ProveedorConmutable {
    fn name(&self) -> &'static str {
        self.actual().name()
    }

    async fn status(&self) -> ProviderStatus {
        self.actual().status().await
    }

    async fn search_tracks(&self, query: &str, limit: u8, offset: u16) -> CoreResult<Page<Track>> {
        self.actual().search_tracks(query, limit, offset).await
    }

    async fn track(&self, id: &TrackId) -> CoreResult<Track> {
        self.actual().track(id).await
    }

    /// Delegar esto **hay que escribirlo**, aunque el puerto traiga un valor por
    /// defecto.
    ///
    /// Es la trampa de los métodos con implementación por defecto en un trait
    /// que alguien envuelve: sin esta línea, el conmutador se quedaba con el
    /// `Ok(None)` de fábrica y la pregunta no llegaba nunca al catálogo. Todo
    /// compilaba, ningún test fallaba, y el síntoma era que las descargas
    /// seguían eligiendo mal sin una sola traza que lo explicara.
    async fn resolve_recording(&self, track: &Track) -> CoreResult<Option<Resolucion>> {
        self.actual().resolve_recording(track).await
    }

    async fn tracks(&self, ids: &[TrackId]) -> CoreResult<Vec<Track>> {
        self.actual().tracks(ids).await
    }

    async fn album(&self, id: &AlbumId) -> CoreResult<Album> {
        self.actual().album(id).await
    }

    async fn album_tracks(&self, id: &AlbumId) -> CoreResult<Vec<Track>> {
        self.actual().album_tracks(id).await
    }

    async fn artist(&self, id: &ArtistId) -> CoreResult<Artist> {
        self.actual().artist(id).await
    }

    async fn artist_top_tracks(&self, id: &ArtistId) -> CoreResult<Vec<Track>> {
        self.actual().artist_top_tracks(id).await
    }

    async fn artist_albums(&self, id: &ArtistId) -> CoreResult<Vec<Album>> {
        self.actual().artist_albums(id).await
    }

    /// Importar va a **quien es dueño de la URL**, no al catálogo activo.
    ///
    /// Es la única llamada del puerto donde el usuario dice explícitamente de
    /// dónde quiere traer algo, y respetarlo es lo obvio. Antes se delegaba en
    /// el catálogo activo, así que pegar un enlace de Spotify teniendo puesto
    /// YouTube Music lo mandaba a YouTube Music, que respondía que eso no es una
    /// lista suya. El comando se llamaba `playlist_import_spotify` y ni siquiera
    /// iba a Spotify.
    ///
    /// Un identificador suelto, sin dominio, sí va al catálogo activo: no hay
    /// forma de saber de quién es y el activo es la mejor apuesta.
    async fn public_playlist(
        &self,
        url_or_id: &str,
        page_callback: &(dyn Fn(u32, u32) + Send + Sync),
    ) -> CoreResult<PlaylistImport> {
        let destino = match duenyo_de(url_or_id) {
            Some(Duenyo::Spotify) => &self.spotify,
            Some(Duenyo::YouTube) => &self.ytmusic,
            None => self.actual(),
        };
        destino.public_playlist(url_or_id, page_callback).await
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "en un test, un `expect` que falla es el fallo"
)]
mod tests {
    use super::*;

    /// Proveedor que solo dice cómo se llama. Basta para comprobar a quién se
    /// está delegando, que es lo único que hace este tipo.
    #[derive(Debug)]
    struct Falso(&'static str);

    #[async_trait]
    impl MetadataProvider for Falso {
        fn name(&self) -> &'static str {
            self.0
        }
        async fn status(&self) -> ProviderStatus {
            ProviderStatus::Ready
        }
        async fn search_tracks(&self, _: &str, _: u8, _: u16) -> CoreResult<Page<Track>> {
            Ok(Page::new(Vec::new(), None, None))
        }
        async fn track(&self, id: &TrackId) -> CoreResult<Track> {
            Err(localify_core::error::CoreError::not_found(
                self.0,
                id.as_str(),
            ))
        }
        async fn tracks(&self, _: &[TrackId]) -> CoreResult<Vec<Track>> {
            Ok(Vec::new())
        }
        async fn album(&self, id: &AlbumId) -> CoreResult<Album> {
            Err(localify_core::error::CoreError::not_found(
                self.0,
                id.as_str(),
            ))
        }
        async fn album_tracks(&self, _: &AlbumId) -> CoreResult<Vec<Track>> {
            Ok(Vec::new())
        }
        async fn artist(&self, id: &ArtistId) -> CoreResult<Artist> {
            Err(localify_core::error::CoreError::not_found(
                self.0,
                id.as_str(),
            ))
        }
        async fn artist_top_tracks(&self, _: &ArtistId) -> CoreResult<Vec<Track>> {
            Ok(Vec::new())
        }
        async fn artist_albums(&self, _: &ArtistId) -> CoreResult<Vec<Album>> {
            Ok(Vec::new())
        }
        async fn public_playlist(
            &self,
            _: &str,
            _: &(dyn Fn(u32, u32) + Send + Sync),
        ) -> CoreResult<PlaylistImport> {
            Err(localify_core::error::CoreError::not_found(self.0, "lista"))
        }
        async fn resolve_recording(&self, _: &Track) -> CoreResult<Option<Resolucion>> {
            // Devuelve su propio nombre como identificador de vídeo, para poder
            // comprobar a quién se delegó.
            Ok(Some(Resolucion {
                video_id: self.0.to_owned(),
                cover_url: None,
                album: None,
            }))
        }
    }

    fn conmutable(inicial: MetadataProviderKind) -> ProveedorConmutable {
        ProveedorConmutable::nuevo(
            Arc::new(Falso("ytmusic")),
            Arc::new(Falso("spotify")),
            Arc::new(Falso("musicbrainz")),
            Arc::new(Falso("combinado")),
            inicial,
        )
    }

    #[test]
    fn arranca_con_el_proveedor_pedido() {
        assert_eq!(conmutable(MetadataProviderKind::YtMusic).name(), "ytmusic");
        assert_eq!(conmutable(MetadataProviderKind::Spotify).name(), "spotify");
        assert_eq!(
            conmutable(MetadataProviderKind::MusicBrainz).name(),
            "musicbrainz"
        );
        assert_eq!(
            conmutable(MetadataProviderKind::Combinado).name(),
            "combinado"
        );
    }

    #[test]
    fn el_valor_por_defecto_es_el_combinado() {
        // Ninguno de los dos que combina pide credenciales, así que puede serlo
        // sin obligar a nadie a configurar nada.
        assert_eq!(
            conmutable(MetadataProviderKind::default()).tipo(),
            MetadataProviderKind::Combinado
        );
    }

    #[test]
    fn cambiar_redirige_las_llamadas_siguientes() {
        let c = conmutable(MetadataProviderKind::YtMusic);
        assert_eq!(c.name(), "ytmusic");

        c.cambiar(MetadataProviderKind::Spotify);
        assert_eq!(c.name(), "spotify");
        assert_eq!(c.tipo(), MetadataProviderKind::Spotify);

        // Y se puede volver: no es un cambio de una sola dirección.
        c.cambiar(MetadataProviderKind::YtMusic);
        assert_eq!(c.name(), "ytmusic");
    }

    #[tokio::test]
    async fn los_metodos_con_valor_por_defecto_tambien_se_delegan() {
        // El fallo que esto caza: `youtube_video_id` trae un `Ok(None)` por
        // defecto en el puerto, así que olvidarse de delegarlo compila, no rompe
        // ningún test y hace que el conmutador conteste "no lo sé" en nombre de
        // un catálogo al que nunca preguntó. El síntoma fue descargas que
        // seguían eligiendo mal, sin una sola traza que lo explicara.
        let c = conmutable(MetadataProviderKind::MusicBrainz);
        let pista = Track {
            id: TrackId::from_trusted("kM0Fpbz0W8U"),
            title: "X".into(),
            album: None,
            artists: Vec::new(),
            duration: localify_core::domain::audio::DurationMs::from_secs(180),
            track_number: None,
            disc_number: None,
            explicit: false,
            isrc: None,
            release_date: None,
            popularity: None,
            added_at: chrono::Utc::now(),
        };

        assert_eq!(
            c.resolve_recording(&pista)
                .await
                .expect("delega")
                .map(|r| r.video_id),
            Some("musicbrainz".to_owned()),
            "no se delegó: se quedó con el valor por defecto del trait"
        );
    }

    #[test]
    fn el_duenyo_se_deduce_del_dominio() {
        assert_eq!(
            duenyo_de("https://open.spotify.com/playlist/00ew3gyVcZCkCJyOW5tSZR?si=abc"),
            Some(Duenyo::Spotify)
        );
        assert_eq!(
            duenyo_de("spotify:playlist:00ew3gyVcZCkCJyOW5tSZR"),
            Some(Duenyo::Spotify)
        );
        assert_eq!(
            duenyo_de("https://music.youtube.com/playlist?list=PLabc"),
            Some(Duenyo::YouTube)
        );
        assert_eq!(
            duenyo_de("https://youtu.be/kM0Fpbz0W8U"),
            Some(Duenyo::YouTube)
        );
        // Un identificador suelto no dice de quién es.
        assert_eq!(duenyo_de("PLabcdefghij"), None);
        assert_eq!(duenyo_de("00ew3gyVcZCkCJyOW5tSZR"), None);
    }

    #[tokio::test]
    async fn importar_va_al_duenyo_de_la_url_y_no_al_catalogo_activo() {
        // El fallo que esto arregla: con YouTube Music activo, pegar un enlace
        // de Spotify lo mandaba a YouTube Music, que respondía que eso no es una
        // lista suya. Quien pega un enlace ya ha dicho de dónde lo quiere.
        let c = conmutable(MetadataProviderKind::YtMusic);
        let e = c
            .public_playlist("https://open.spotify.com/playlist/abc", &|_, _| {})
            .await
            .expect_err("el doble siempre falla");
        assert!(e.to_string().contains("spotify"), "fue a otro sitio: {e}");
    }

    #[tokio::test]
    async fn un_identificador_suelto_va_al_catalogo_activo() {
        let c = conmutable(MetadataProviderKind::MusicBrainz);
        let e = c
            .public_playlist("PLabcdefghij", &|_, _| {})
            .await
            .expect_err("el doble siempre falla");
        assert!(e.to_string().contains("musicbrainz"), "{e}");
    }

    #[tokio::test]
    async fn las_llamadas_asincronas_tambien_se_delegan() {
        // `name()` es síncrono; el resto del puerto no. Si la delegación se
        // hubiera implementado solo para lo fácil, esto lo destaparía.
        let c = conmutable(MetadataProviderKind::Spotify);
        let e = c
            .track(&TrackId::from_trusted("kM0Fpbz0W8U"))
            .await
            .expect_err("el doble siempre falla");
        assert!(
            e.to_string().contains("spotify"),
            "delegó en el proveedor equivocado: {e}"
        );
    }
}

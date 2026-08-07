//! Servicio de metadatos.
//!
//! Orquesta la obtención y persistencia de metadatos, y es la frontera donde
//! "algo de un proveedor" se convierte en "algo canónico de Localify".
//!
//! ## Por qué existe la caducidad
//!
//! Los metadatos de una pista casi nunca cambian, pero "casi nunca" no es
//! "nunca": un álbum se reedita, un artista gana géneros. Se refrescan cuando
//! llevan mucho sin tocarse, en segundo plano, y **nunca en el camino de una
//! consulta del usuario**: si ya están en local, se sirven sin salir a la red.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use localify_core::domain::ids::{AlbumId, ArtistId, TrackId};
use localify_core::error::{CoreError, CoreResult};
use localify_core::events::{DomainEvent, EventPublisher, LibraryScope};
use localify_core::ports::database::{AlbumRepository, ArtistRepository, TrackRepository};
use localify_core::ports::metadata_provider::{ImageFetcher, MetadataProvider};
use localify_core::ports::platform::AppPaths;
use localify_core::ports::services::MetadataService;
use tracing::{debug, warn};

/// Antigüedad a partir de la cual unos metadatos se consideran caducados.
///
/// Treinta días es un compromiso: lo bastante largo para no gastar peticiones
/// en datos que no cambian, y lo bastante corto para que una reedición acabe
/// apareciendo.
const CADUCIDAD_SEGUNDOS: u64 = 30 * 86_400;

pub struct MetadataServiceImpl {
    provider: Arc<dyn MetadataProvider>,
    tracks: Arc<dyn TrackRepository>,
    albums: Arc<dyn AlbumRepository>,
    artists: Arc<dyn ArtistRepository>,
    bus: Arc<dyn EventPublisher>,
    /// Ausente si no hay cliente HTTP: entonces no hay portadas, y la interfaz
    /// enseña su icono sin que nada más cambie.
    imagenes: Option<Arc<dyn ImageFetcher>>,
    paths: Arc<dyn AppPaths>,
    /// Emparejamientos, para la portada de las pistas que no tienen álbum.
    ///
    /// Ausente en los dobles de test y en modo degradado: entonces esas pistas
    /// se quedan con su icono, que es lo que hacían antes.
    matches: Option<Arc<dyn localify_core::ports::database::YoutubeMatchRepository>>,
}

impl std::fmt::Debug for MetadataServiceImpl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MetadataServiceImpl")
            .finish_non_exhaustive()
    }
}

impl MetadataServiceImpl {
    #[must_use]
    pub fn nuevo(
        provider: Arc<dyn MetadataProvider>,
        tracks: Arc<dyn TrackRepository>,
        albums: Arc<dyn AlbumRepository>,
        artists: Arc<dyn ArtistRepository>,
        bus: Arc<dyn EventPublisher>,
        imagenes: Option<Arc<dyn ImageFetcher>>,
        paths: Arc<dyn AppPaths>,
    ) -> Self {
        Self {
            provider,
            tracks,
            albums,
            artists,
            bus,
            imagenes,
            paths,
            matches: None,
        }
    }

    /// Añade el repositorio de emparejamientos.
    ///
    /// Va aparte del constructor porque es opcional y llegó después: meterlo
    /// como parámetro obligaría a que todos los tests pasaran un `None` para una
    /// funcionalidad que no ejercitan.
    #[must_use]
    pub fn con_emparejamientos(
        mut self,
        matches: Arc<dyn localify_core::ports::database::YoutubeMatchRepository>,
    ) -> Self {
        self.matches = Some(matches);
        self
    }

    /// Persiste pistas y, con ellas, los álbumes y artistas que referencian.
    ///
    /// Es el punto por el que entra **todo** lo que llega de un proveedor:
    /// concentrarlo garantiza que la normalización de texto y la denormalización
    /// de `artist_display` se apliquen siempre.
    ///
    /// # Errors
    /// Si falla la escritura.
    /// Una pista que no se pueda guardar no puede llevarse por delante al resto.
    ///
    /// El lote entero va en una sola transacción, que es lo rápido y lo normal.
    /// Pero esa atomicidad no la pide nadie **entre** pistas: lo que tiene que
    /// ser atómico es una pista con su álbum y sus artistas, no veinte
    /// resultados de búsqueda que no se conocen entre sí.
    ///
    /// Esto se escribió después de verlo: un crédito de MusicBrainz con el mismo
    /// artista repetido violaba una clave primaria, la transacción abortaba, y
    /// en la interfaz el catálogo entero parecía no responder. El fallo concreto
    /// está arreglado en su origen; esta red existe para el siguiente, que
    /// vendrá de otro sitio.
    pub async fn persistir(
        &self,
        pistas: &[localify_core::domain::track::Track],
    ) -> CoreResult<()> {
        if pistas.is_empty() {
            return Ok(());
        }

        if let Err(e) = self.tracks.upsert(pistas).await {
            warn!(error = %e, cuantas = pistas.len(), "el lote falló; se guarda pista a pista");

            let mut guardadas = 0_usize;
            for pista in pistas {
                match self.tracks.upsert(std::slice::from_ref(pista)).await {
                    Ok(()) => guardadas += 1,
                    Err(suya) => {
                        debug!(pista = %pista.id, error = %suya, "pista descartada");
                    }
                }
            }

            // Si no se salvó ninguna, el problema no era una pista suelta: es la
            // base de datos, y eso sí tiene que subir.
            if guardadas == 0 {
                return Err(e);
            }
            warn!(guardadas, de = pistas.len(), "lote recuperado parcialmente");
        }

        self.bus.publish(DomainEvent::LibraryChanged {
            scope: LibraryScope::Tracks,
        });
        Ok(())
    }

    /// Completa los géneros de los artistas que aún no los tienen.
    ///
    /// Spotify no devuelve géneros en las respuestas de búsqueda ni de pista:
    /// solo en `/artists`. Y los géneros son la señal principal del motor de
    /// recomendaciones, así que sin este paso las recomendaciones se quedarían
    /// cojas para todo lo que se descubra buscando.
    ///
    /// # Errors
    /// Si falla la consulta o la escritura.
    pub async fn completar_artistas(&self, ids: &[ArtistId]) -> CoreResult<u32> {
        let mut pendientes = Vec::new();
        for id in ids {
            match self.artists.get(id).await? {
                // Sin géneros: o nunca se consultó el detalle, o el artista no
                // tiene ninguno asignado. Se consulta una vez.
                Some(a) if a.genres.is_empty() => pendientes.push(id.clone()),
                None => pendientes.push(id.clone()),
                Some(_) => {}
            }
        }

        if pendientes.is_empty() {
            return Ok(0);
        }

        let mut completados = 0_u32;
        for id in &pendientes {
            match self.provider.artist(id).await {
                Ok(artista) => {
                    self.artists.upsert(std::slice::from_ref(&artista)).await?;
                    completados += 1;
                }
                // Que un artista no se pueda completar no debe abortar el resto:
                // los metadatos básicos ya están y la aplicación funciona.
                Err(e) => debug!(artista = %id, error = %e, "no se pudo completar el artista"),
            }
        }

        Ok(completados)
    }
}

#[async_trait]
impl MetadataService for MetadataServiceImpl {
    async fn ensure_track(&self, id: &TrackId) -> CoreResult<()> {
        // Si ya está en local, no se toca la red. Es la regla que hace que
        // navegar por la biblioteca funcione igual sin conexión.
        if self.tracks.get(id).await?.is_some() {
            return Ok(());
        }
        // Una pista local no tiene equivalente remoto que consultar.
        if id.es_local() {
            return Ok(());
        }

        let pista = self.provider.track(id).await?;
        self.persistir(std::slice::from_ref(&pista)).await
    }

    async fn ensure_album(&self, id: &AlbumId) -> CoreResult<()> {
        if id.es_local() {
            return Ok(());
        }
        if self.albums.get(id).await?.is_some() {
            return Ok(());
        }

        let album = self.provider.album(id).await?;
        self.albums.upsert(std::slice::from_ref(&album)).await?;

        // Las pistas del álbum se traen de una vez: abrir un álbum y descubrir
        // que faltan sus canciones sería inútil.
        let pistas = self.provider.album_tracks(id).await?;
        self.persistir(&pistas).await?;

        self.bus.publish(DomainEvent::LibraryChanged {
            scope: LibraryScope::Albums,
        });
        Ok(())
    }

    async fn ensure_artist(&self, id: &ArtistId) -> CoreResult<()> {
        if id.es_local() {
            return Ok(());
        }
        // A diferencia de pistas y álbumes, aquí no basta con que exista: un
        // artista puede estar en la base de datos como referencia mínima, sin
        // géneros, creado al guardar una pista.
        if let Some(a) = self.artists.get(id).await?
            && !a.genres.is_empty()
        {
            return Ok(());
        }

        let artista = self.provider.artist(id).await?;
        self.artists.upsert(std::slice::from_ref(&artista)).await?;
        self.bus.publish(DomainEvent::LibraryChanged {
            scope: LibraryScope::Artists,
        });
        Ok(())
    }

    /// Garantiza la portada en disco y devuelve su ruta.
    ///
    /// ## Se descarga cuando alguien la mira, no al guardar el álbum
    ///
    /// Una búsqueda trae veinte álbumes y casi ninguno se abre. Bajar sus
    /// portadas al persistirlos serían veinte peticiones por pulsación para
    /// imágenes que nadie va a ver. Así se paga solo lo que se mira, y una vez.
    ///
    /// ## No se reescala
    ///
    /// Se guarda el fichero tal cual llega. Escalar exigiría un decodificador de
    /// imágenes entero como dependencia para ahorrar unos kilobytes por portada
    /// en una aplicación de escritorio; el navegador ya escala al pintar. Los
    /// tamaños de [`CoverSize`] siguen ahí para cuando compense.
    async fn ensure_cover(&self, album: &AlbumId) -> CoreResult<Option<PathBuf>> {
        // La ruta se compone aquí y no en el puerto: `AppPaths` da la carpeta,
        // y el nombre del fichero es cosa de quien lo escribe.
        let destino = self
            .paths
            .covers_dir()
            .join(format!("{}.jpg", album.as_str()));

        // Una portada no cambia. Si ya está, no se vuelve a pedir jamás.
        if tokio::fs::try_exists(&destino).await.unwrap_or(false) {
            return Ok(Some(destino));
        }

        let Some(descargador) = &self.imagenes else {
            return Ok(None);
        };
        // Un álbum guardado a partir de una canción solo tiene identificador y
        // título: la miniatura viaja en la búsqueda de álbumes, no en la de
        // pistas. Cuando falta se pide su ficha, que sí la trae, y se guarda de
        // paso. Solo ocurre la primera vez que alguien mira esa portada.
        let url = match self.albums.get(album).await? {
            Some(f) if f.cover_url.is_some() => f.cover_url,
            _ => match self.provider.album(album).await {
                Ok(completo) => {
                    self.albums.upsert(std::slice::from_ref(&completo)).await?;
                    completo.cover_url
                }
                Err(e) => {
                    debug!(%album, error = %e, "sin ficha del álbum: no hay portada");
                    None
                }
            },
        };
        let Some(url) = url else {
            return Ok(None);
        };

        let Some(bytes) = descargar(descargador.as_ref(), &url).await else {
            return Ok(None);
        };
        guardar_imagen(&destino, &bytes).await?;

        self.albums.set_cover_cached(album, true).await?;
        debug!(%album, bytes = bytes.len(), "portada cacheada");
        Ok(Some(destino))
    }

    async fn ensure_track_thumbnail(&self, track: &TrackId) -> CoreResult<Option<PathBuf>> {
        let Some(pista) = self.tracks.get(track).await? else {
            return Ok(None);
        };

        // Si la canción sale de un disco, su imagen es la del disco. Solo la
        // música que no pertenece a ninguno —singles, bandas sonoras sueltas,
        // subidas de YouTube— necesita una miniatura propia.
        //
        // Se devuelve el fichero del álbum tal cual, sin copiarlo: duplicar la
        // misma imagen por cada pista llenaría el disco de copias idénticas y
        // dejaría doce sitios donde puede quedar desfasada.
        //
        // Esta comprobación va **antes** que la del fichero cacheado a
        // propósito: así una miniatura suelta que dejó una versión anterior no
        // sobrevive a la corrección.
        if let Some(album) = &pista.album
            && let Some(portada) = self.ensure_cover(&album.id).await?
        {
            return Ok(Some(portada));
        }

        let destino = self
            .paths
            .covers_dir()
            .join(format!("track-{}.jpg", track.as_str()));

        if tokio::fs::try_exists(&destino).await.unwrap_or(false) {
            return Ok(Some(destino));
        }

        let Some(descargador) = &self.imagenes else {
            return Ok(None);
        };

        // La miniatura que da el catálogo para **esta canción**. Es cuadrada y
        // es la que se ve junto al título al buscar.
        let mut url = self
            .provider
            .resolve_recording(&pista)
            .await
            .ok()
            .flatten()
            .and_then(|r| r.cover_url);

        // Y si no la da, la del vídeo emparejado. Es 4:3 y se ve peor —la
        // carátula queda pequeña entre bandas—, pero es mejor que un icono.
        if url.is_none()
            && let Some(matches) = &self.matches
            && let Some(candidato) = matches.best_for(track).await?
        {
            // `hqdefault` y no `maxresdefault`: la segunda no existe para todos
            // los vídeos y devuelve 404, mientras que la primera está siempre.
            url = Some(format!(
                "https://i.ytimg.com/vi/{}/hqdefault.jpg",
                candidato.video_id
            ));
        }

        let Some(url) = url else {
            return Ok(None);
        };
        let Some(bytes) = descargar(descargador.as_ref(), &url).await else {
            return Ok(None);
        };
        guardar_imagen(&destino, &bytes).await?;

        debug!(%track, "miniatura de la canción cacheada");
        Ok(Some(destino))
    }

    /// Garantiza la foto del artista en disco y devuelve su ruta.
    ///
    /// ## No hay marca en la base de datos, a diferencia de las portadas
    ///
    /// `albums.cover_cached` existe porque el mosaico de una playlist necesita
    /// saber qué portadas hay **sin tocar el disco**: es una consulta que mira
    /// cientos de álbumes para componer cuatro cuadrados. Con los artistas no
    /// hay ninguna consulta así; el único que pregunta es quien va a servir el
    /// fichero, y para ese la existencia del fichero ya es la respuesta. Una
    /// columna más sería una segunda fuente de verdad que puede desincronizarse
    /// con el disco a cambio de nada.
    async fn ensure_artist_image(&self, artist: &ArtistId) -> CoreResult<Option<PathBuf>> {
        let destino = self
            .paths
            .artists_dir()
            .join(format!("{}.jpg", artist.as_str()));

        if tokio::fs::try_exists(&destino).await.unwrap_or(false) {
            return Ok(Some(destino));
        }

        let Some(descargador) = &self.imagenes else {
            return Ok(None);
        };

        // Igual que con las portadas: un artista guardado como referencia de una
        // canción solo tiene identificador y nombre. La foto viaja en su ficha,
        // así que se pide una vez y se guarda entera —con ella vienen los
        // géneros, que es lo que alimenta las recomendaciones—.
        let url = match self.artists.get(artist).await? {
            Some(a) if a.image_url.is_some() => a.image_url,
            _ => match self.provider.artist(artist).await {
                Ok(completo) => {
                    self.artists.upsert(std::slice::from_ref(&completo)).await?;
                    completo.image_url
                }
                Err(e) => {
                    debug!(%artist, error = %e, "sin ficha del artista: no hay foto");
                    None
                }
            },
        };
        let Some(url) = url else {
            return Ok(None);
        };

        let Some(bytes) = descargar(descargador.as_ref(), &url).await else {
            return Ok(None);
        };
        guardar_imagen(&destino, &bytes).await?;

        debug!(%artist, bytes = bytes.len(), "foto de artista cacheada");
        Ok(Some(destino))
    }

    async fn refresh_stale(&self, limit: u32) -> CoreResult<u32> {
        if !self.provider.status().await.esta_operativo() {
            return Ok(0);
        }

        let caducadas = self.tracks.stale(CADUCIDAD_SEGUNDOS, limit).await?;
        // Las locales no tienen nada que refrescar.
        let remotas: Vec<TrackId> = caducadas.into_iter().filter(|t| !t.es_local()).collect();
        if remotas.is_empty() {
            return Ok(0);
        }

        match self.provider.tracks(&remotas).await {
            Ok(pistas) => {
                let cuantas = u32::try_from(pistas.len()).unwrap_or(0);
                self.persistir(&pistas).await?;
                debug!(cuantas, "metadatos refrescados");
                Ok(cuantas)
            }
            // Refrescar es oportunista: si el proveedor no responde, se
            // reintentará en el siguiente ciclo. No es un fallo que reportar.
            Err(e) => {
                warn!(error = %e, "no se pudieron refrescar los metadatos");
                Ok(0)
            }
        }
    }
}

/// Baja una imagen. `None` si no se pudo, que no es un error de nadie.
///
/// Una imagen que no llega no merece propagar un `Err`: quien la pidió ya sabe
/// pintar su hueco con un icono, y convertirlo en fallo obligaría a cada
/// llamante a decidir otra vez lo mismo.
async fn descargar(descargador: &dyn ImageFetcher, url: &str) -> Option<Vec<u8>> {
    match descargador.fetch(url).await {
        Ok(b) => Some(b),
        Err(e) => {
            debug!(url, error = %e, "no se pudo descargar la imagen");
            None
        }
    }
}

/// Escribe una imagen cacheada con la garantía de "existe ⇒ está entera".
///
/// Temporal y renombrado, como todo lo demás. Una imagen a medio escribir no
/// solo se vería cortada: se quedaría así **para siempre**, porque quien la
/// pide comprueba si el fichero existe y no vuelve a bajarla jamás.
async fn guardar_imagen(destino: &std::path::Path, bytes: &[u8]) -> CoreResult<()> {
    if let Some(padre) = destino.parent() {
        tokio::fs::create_dir_all(padre).await.ok();
    }
    let temporal = destino.with_extension("part");
    tokio::fs::write(&temporal, bytes)
        .await
        .map_err(|e| CoreError::storage(e.to_string()))?;
    tokio::fs::rename(&temporal, destino)
        .await
        .map_err(|e| CoreError::storage(e.to_string()))
}

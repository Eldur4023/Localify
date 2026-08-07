//! El servicio de playlists.
//!
//! ## Reordenar cuesta un `UPDATE`, no `n`
//!
//! La posición de cada entrada es un `f64`, no un índice entero (ADR-009).
//! Mover una pista es calcular el punto medio entre sus dos vecinos nuevos y
//! escribir esa única fila. Con índices enteros habría que renumerar todo lo
//! que hay detrás: arrastrar la primera pista de una playlist de 5 000 al final
//! serían 5 000 escrituras y una transacción de varios segundos.
//!
//! El coste es que los huecos se estrechan al insertar muchas veces en el mismo
//! sitio. Cuando la separación baja del épsilon, se renumera la playlist entera
//! **en segundo plano**: el usuario ya ha visto su cambio aplicado.
//!
//! ## Añadir sí descarga; importar no
//!
//! Meter una canción en una playlist es decir "esta la quiero", así que se deja
//! preparada en disco (ver `preparar`). Es la diferencia entre una playlist que
//! suena en un tren y una que se queda mirando la barra de carga.
//!
//! Importar una de 500 canciones es otra cosa: es traer una lista ajena para
//! verla. Descargarlas todas sería ocupar gigabytes con música que quizá no se
//! escuche nunca. Por eso la importación escribe las entradas directamente en
//! el repositorio y **no pasa por `add_tracks`**, que es donde vive la descarga.
//!
//! ## Las sugerencias son locales
//!
//! Salen de la propia biblioteca por coincidencia de artista, álbum y afinidad
//! (`SimilarityRepository`), resuelto en SQL. No hay servicio de
//! recomendaciones online, y no lo va a haber.

use std::sync::Arc;

use async_trait::async_trait;
use localify_core::domain::audio::DurationMs;
use localify_core::domain::download::Priority;
use localify_core::domain::ids::{PlaylistEntryId, PlaylistId, TrackId};
use localify_core::domain::playlist::{
    Playlist, PlaylistDetail, PlaylistSource, PlaylistSummary, position,
};
use localify_core::domain::track::{Track, TrackRow};
use localify_core::error::{CoreError, CoreResult};
use localify_core::events::{DomainEvent, EventPublisher, PlaylistChangeKind};
use localify_core::page::PageRequest;
use localify_core::ports::database::{PlaylistRepository, SimilarityRepository, TrackRepository};
use localify_core::ports::metadata_provider::MetadataProvider;
use localify_core::ports::platform::{AppPaths, FileSystem};
use localify_core::ports::services::{DownloadService, PlaylistService};
use tracing::{debug, info, warn};
use uuid::Uuid;

/// Entradas por página al leer una playlist.
const PAGINA: u32 = 200;

/// Portadas del mosaico de una playlist. Cuatro es lo que cabe en la rejilla
/// 2×2 de la interfaz.
const PORTADAS_MOSAICO: usize = 4;

/// Nombre máximo. Ni SQLite ni la interfaz lo necesitan, pero un nombre de
/// megabytes pegado desde el portapapeles rompería el diseño de la barra
/// lateral.
const NOMBRE_MAX: usize = 200;

/// Dependencias del servicio.
pub struct Dependencias {
    pub playlists: Arc<dyn PlaylistRepository>,
    pub tracks: Arc<dyn TrackRepository>,
    pub similitud: Arc<dyn SimilarityRepository>,
    pub provider: Arc<dyn MetadataProvider>,
    pub fs: Arc<dyn FileSystem>,
    pub paths: Arc<dyn AppPaths>,
    pub bus: Arc<dyn EventPublisher>,
    /// Descargas, para dejar preparado lo que se añade a una playlist.
    ///
    /// Opcional porque una playlist se maneja igual sin ella: es una mejora,
    /// no un requisito, y los tests no necesitan montar el actor de descargas
    /// para comprobar que reordenar funciona.
    pub descargas: Option<Arc<dyn DownloadService>>,
    /// Para traerse la portada de una playlist importada.
    ///
    /// Opcional por el mismo motivo: sin ella la lista se importa igual y queda
    /// con el mosaico de sus canciones, que es lo que tienen las playlists
    /// creadas a mano.
    pub imagenes: Option<Arc<dyn localify_core::ports::metadata_provider::ImageFetcher>>,
}

impl std::fmt::Debug for Dependencias {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Dependencias").finish_non_exhaustive()
    }
}

pub struct PlaylistServiceImpl {
    deps: Arc<Dependencias>,
}

impl std::fmt::Debug for PlaylistServiceImpl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PlaylistServiceImpl")
            .finish_non_exhaustive()
    }
}

impl PlaylistServiceImpl {
    #[must_use]
    pub fn nuevo(deps: Dependencias) -> Self {
        Self {
            deps: Arc::new(deps),
        }
    }

    fn anunciar(&self, id: &PlaylistId, kind: PlaylistChangeKind) {
        self.deps.bus.publish(DomainEvent::PlaylistChanged {
            playlist_id: *id,
            kind,
        });
    }

    /// Todas las entradas de una playlist, recorriendo sus páginas.
    async fn todas_las_entradas(
        &self,
        id: &PlaylistId,
    ) -> CoreResult<Vec<localify_core::domain::playlist::PlaylistEntry>> {
        let mut salida = Vec::new();
        let mut offset = 0_u32;
        loop {
            let pagina = self
                .deps
                .playlists
                .entries(id, &PageRequest::new(offset, PAGINA))
                .await?;
            if pagina.items.is_empty() {
                break;
            }
            let leidas = u32::try_from(pagina.items.len()).unwrap_or(PAGINA);
            salida.extend(pagina.items);
            offset += leidas;
        }
        Ok(salida)
    }

    /// Deja preparado lo que se acaba de añadir a una playlist.
    ///
    /// ## Por qué añadir implica descargar
    ///
    /// Meter una canción en una playlist es decir "esta la quiero". Que suene
    /// al pulsarla no debería depender de que en ese momento haya red: es justo
    /// el caso —un viaje, un tren— donde la playlist se preparó de antemano
    /// precisamente para eso.
    ///
    /// ## Prioridad de prefetch, y sin esperar
    ///
    /// `Prefetch` y no `Immediate` porque nadie está esperando a oírla ahora
    /// mismo: si compitiera con lo que suena, añadir un disco entero a una
    /// playlist podría cortar la canción en curso.
    ///
    /// Y en una tarea aparte porque el usuario ya vio su canción en la lista.
    /// Añadir cincuenta no puede tardar cincuenta descargas.
    fn preparar(&self, tracks: &[TrackId]) {
        let Some(descargas) = self.deps.descargas.clone() else {
            return;
        };
        let pistas = tracks.to_vec();

        tokio::spawn(async move {
            for t in pistas {
                // Un fallo aquí no se propaga: la canción sigue en la playlist
                // y se descargará al pulsarla. Avisar de cada fallo de red al
                // añadir sería ruido sobre algo que nadie pidió ver.
                if let Err(e) = descargas.ensure(&t, Priority::Prefetch).await {
                    debug!(pista = %t, error = %e, "no se pudo preparar la pista añadida");
                }
            }
        });
    }

    /// Renumera la playlist en segundo plano.
    ///
    /// Se lanza aparte y no se espera: el usuario ya vio su reordenación
    /// aplicada, y hacerle esperar a una renumeración de 5 000 filas por un
    /// detalle interno sería incomprensible desde fuera.
    fn rebalancear_en_segundo_plano(&self, id: &PlaylistId) {
        let repo = Arc::clone(&self.deps.playlists);
        let id = *id;
        tokio::spawn(async move {
            debug!(playlist = %id.as_uuid(), "rebalanceando posiciones");
            if let Err(e) = repo.rebalance(&id).await {
                warn!(error = %e, "el rebalanceo fallo; se reintentara en el proximo movimiento");
            }
        });
    }
}

/// Valida y normaliza un nombre de playlist.
fn nombre_valido(nombre: &str) -> CoreResult<String> {
    let limpio = nombre.trim();
    if limpio.is_empty() {
        return Err(CoreError::invalid("el nombre no puede estar vacio"));
    }
    if limpio.chars().count() > NOMBRE_MAX {
        return Err(CoreError::invalid(format!(
            "el nombre no puede pasar de {NOMBRE_MAX} caracteres"
        )));
    }
    Ok(limpio.to_owned())
}

#[async_trait]
impl PlaylistService for PlaylistServiceImpl {
    async fn list(&self) -> CoreResult<Vec<PlaylistSummary>> {
        self.deps.playlists.list_summaries().await
    }

    async fn create(&self, name: &str) -> CoreResult<PlaylistSummary> {
        let nombre = nombre_valido(name)?;
        let ahora = chrono::Utc::now();
        let playlist = Playlist {
            id: PlaylistId::nuevo(),
            name: nombre.clone(),
            description: None,
            cover_path: None,
            source: PlaylistSource::Local,
            source_id: None,
            created_at: ahora,
            updated_at: ahora,
        };

        self.deps.playlists.create(&playlist).await?;
        self.anunciar(&playlist.id, PlaylistChangeKind::Created);

        Ok(PlaylistSummary {
            id: playlist.id,
            name: nombre,
            track_count: 0,
            cover_albums: Vec::new(),

            has_own_cover: false,
            updated_at: ahora,
            source: PlaylistSource::Local,
        })
    }

    async fn rename(&self, id: &PlaylistId, name: &str) -> CoreResult<()> {
        let nombre = nombre_valido(name)?;
        self.deps.playlists.rename(id, &nombre).await?;
        self.anunciar(id, PlaylistChangeKind::Renamed);
        Ok(())
    }

    async fn set_description(&self, id: &PlaylistId, description: Option<&str>) -> CoreResult<()> {
        // Mismo tope que el nombre no: una descripción es un párrafo, no una
        // etiqueta. Se acota igualmente para que nadie pegue un libro y la ficha
        // deje de poder pintarse.
        const MAXIMO: usize = 500;
        if description.is_some_and(|d| d.chars().count() > MAXIMO) {
            return Err(CoreError::invalid(format!(
                "la descripción no puede pasar de {MAXIMO} caracteres"
            )));
        }

        self.deps.playlists.set_description(id, description).await?;
        self.anunciar(id, PlaylistChangeKind::Renamed);
        Ok(())
    }

    async fn delete(&self, id: &PlaylistId) -> CoreResult<()> {
        self.deps.playlists.delete(id).await?;
        self.anunciar(id, PlaylistChangeKind::Deleted);
        Ok(())
    }

    async fn detail(&self, id: &PlaylistId, page: &PageRequest) -> CoreResult<PlaylistDetail> {
        let playlist = self
            .deps
            .playlists
            .get(id)
            .await?
            .ok_or_else(|| CoreError::not_found("playlist", id.as_uuid().to_string()))?;

        let entries = self.deps.playlists.entries(id, page).await?;

        // La duración total es la de la página, no la de la playlist entera:
        // sumarla exigiría traer las 5 000 entradas. La cabecera muestra la
        // cuenta, que sí es barata.
        let total = entries
            .items
            .iter()
            .map(|e| u64::from(e.track.duration.as_ms()))
            .sum::<u64>();

        let cuantas = entries
            .total
            .and_then(|t| u32::try_from(t).ok())
            .unwrap_or_else(|| u32::try_from(entries.items.len()).unwrap_or(u32::MAX));

        // Los álbumes del mosaico salen de las entradas que ya tenemos a mano.
        // Pedirlos otra vez a la base de datos sería repetir una consulta cuyo
        // resultado está en esta misma función.
        let mut vistos = std::collections::HashSet::new();
        let cover_albums: Vec<_> = entries
            .items
            .iter()
            .filter_map(|e| e.track.album_id.clone())
            .filter(|a| vistos.insert(a.clone()))
            .take(PORTADAS_MOSAICO)
            .collect();

        Ok(PlaylistDetail {
            summary: PlaylistSummary {
                id: playlist.id,
                name: playlist.name,
                track_count: cuantas,
                cover_albums,
                has_own_cover: playlist.cover_path.is_some(),
                updated_at: playlist.updated_at,
                source: playlist.source,
            },
            description: playlist.description,
            total_duration: DurationMs::new(u32::try_from(total).unwrap_or(u32::MAX)),
            entries: entries.items,
        })
    }

    async fn add_tracks(
        &self,
        id: &PlaylistId,
        tracks: &[TrackId],
        at_index: Option<usize>,
    ) -> CoreResult<()> {
        if tracks.is_empty() {
            return Ok(());
        }

        let indice = match at_index {
            Some(i) => i,
            // Sin índice, al final. Se pide el vecino del final en vez de
            // contar las entradas: es una consulta acotada.
            None => usize::MAX,
        };

        let (antes, despues) = self.deps.playlists.neighbors(id, indice).await?;

        // Todas las pistas del lote entran en el mismo hueco, repartidas. Pedir
        // los vecinos una vez por pista serían `n` consultas para una operación
        // que el usuario percibe como una sola.
        #[allow(clippy::cast_precision_loss, reason = "lotes de decenas de pistas")]
        let n = tracks.len() as f64;
        let inicio = position::entre(antes, despues);
        let paso = match despues {
            Some(d) => (d - inicio) / (n + 1.0),
            None => position::PASO,
        };

        let entradas: Vec<(PlaylistEntryId, TrackId, f64)> = tracks
            .iter()
            .enumerate()
            .map(|(i, t)| {
                #[allow(clippy::cast_precision_loss, reason = "lotes de decenas")]
                let offset = i as f64;
                (PlaylistEntryId::nuevo(), t.clone(), inicio + paso * offset)
            })
            .collect();

        self.deps.playlists.add_entries(id, &entradas).await?;
        self.anunciar(id, PlaylistChangeKind::Items);

        self.preparar(tracks);

        if position::necesita_rebalanceo(antes, despues) {
            self.rebalancear_en_segundo_plano(id);
        }
        Ok(())
    }

    async fn remove_entries(&self, id: &PlaylistId, entries: &[PlaylistEntryId]) -> CoreResult<()> {
        if entries.is_empty() {
            return Ok(());
        }
        self.deps.playlists.remove_entries(id, entries).await?;
        self.anunciar(id, PlaylistChangeKind::Items);
        Ok(())
    }

    async fn reorder(
        &self,
        id: &PlaylistId,
        entry: PlaylistEntryId,
        to_index: usize,
    ) -> CoreResult<()> {
        let (antes, despues) = self.deps.playlists.neighbors(id, to_index).await?;
        let posicion = position::entre(antes, despues);

        // Un solo `UPDATE`, sea la playlist de 10 pistas o de 5 000 (ADR-009).
        self.deps
            .playlists
            .set_position(id, entry, posicion)
            .await?;
        self.anunciar(id, PlaylistChangeKind::Items);

        // El hueco se ha partido: si ya era estrecho, se renumera después.
        if position::necesita_rebalanceo(antes, despues) {
            self.rebalancear_en_segundo_plano(id);
        }
        Ok(())
    }

    async fn set_cover(&self, id: &PlaylistId, image: &std::path::Path) -> CoreResult<()> {
        // La imagen se **copia** a la biblioteca. Guardar la ruta original
        // dejaría la portada rota en cuanto el usuario moviera o borrara el
        // fichero, que puede estar en Descargas o en una unidad extraíble.
        let extension = image
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .filter(|e| matches!(e.as_str(), "jpg" | "jpeg" | "png" | "webp"))
            .ok_or_else(|| CoreError::invalid("la portada debe ser jpg, png o webp"))?;

        let relativa =
            std::path::Path::new("covers").join(format!("playlist-{}.{extension}", id.as_uuid()));
        let destino = self.deps.paths.resolve(&relativa);

        if let Some(padre) = destino.parent() {
            self.deps.fs.ensure_dir(padre).await?;
        }
        let bytes = tokio::fs::read(image)
            .await
            .map_err(|e| CoreError::storage(format!("{}: {e}", image.display())))?;
        self.deps.fs.write_synced(&destino, &bytes).await?;

        // La ruta se guarda relativa (ADR-018): cambiar de carpeta de
        // biblioteca no debe romper las portadas.
        let texto = relativa.to_string_lossy().replace('\\', "/");
        self.deps.playlists.set_cover(id, Some(&texto)).await?;
        self.anunciar(id, PlaylistChangeKind::Renamed);
        Ok(())
    }

    async fn clear_cover(&self, id: &PlaylistId) -> CoreResult<()> {
        // El fichero copiado se queda: ocupa unos kilobytes y borrarlo haría
        // que "quitar la portada" fuera irreversible sin avisar. Lo recogerá el
        // mantenimiento cuando toque limpiar `covers/`.
        self.deps.playlists.set_cover(id, None).await?;
        self.anunciar(id, PlaylistChangeKind::Renamed);
        Ok(())
    }

    async fn cover_file(&self, id: &PlaylistId) -> CoreResult<Option<std::path::PathBuf>> {
        let Some(playlist) = self.deps.playlists.get(id).await? else {
            return Ok(None);
        };
        let Some(relativa) = playlist.cover_path else {
            return Ok(None);
        };

        let absoluta = self.deps.paths.resolve(std::path::Path::new(&relativa));
        // Se comprueba que exista: la biblioteca puede haberse movido a mano, y
        // devolver una ruta muerta haría que quien la sirve fallara con un
        // error de disco en vez de con un 404 limpio.
        Ok(self.deps.fs.exists(&absoluta).await.then_some(absoluta))
    }

    async fn import_from_provider(&self, url_or_id: &str) -> CoreResult<Uuid> {
        let import_id = Uuid::now_v7();
        let deps = Arc::clone(&self.deps);
        let referencia = url_or_id.to_owned();

        // Se devuelve al instante: una playlist de 1 000 pistas son 10
        // peticiones a Spotify y varios segundos. El progreso llega por eventos.
        tokio::spawn(async move {
            if let Err(e) = importar(&deps, import_id, &referencia).await {
                warn!(error = %e, "la importacion fallo");
                deps.bus.publish(DomainEvent::Toast {
                    level: localify_core::events::ToastLevel::Error,
                    message_key: e.message_key().to_owned(),
                    params: Vec::new(),
                });
            }
        });

        Ok(import_id)
    }

    async fn suggestions(&self, id: &PlaylistId, limit: u8) -> CoreResult<Vec<TrackRow>> {
        let entradas = self.todas_las_entradas(id).await?;
        if entradas.is_empty() {
            return Ok(Vec::new());
        }

        let dentro: Vec<TrackId> = entradas.into_iter().map(|e| e.track.id).collect();
        let afines = self.deps.similitud.similar_to_set(&dentro, limit).await?;

        // El repositorio ya excluye lo que está dentro y devuelve en orden de
        // afinidad; aquí solo se rehidratan las filas conservando ese orden.
        let ids: Vec<TrackId> = afines.into_iter().map(|(t, _)| t).collect();
        let filas = self.deps.tracks.rows_by_ids(&ids).await?;

        Ok(ids
            .iter()
            .filter_map(|id| filas.iter().find(|f| &f.id == id).cloned())
            .collect())
    }
}

/// Se trae la portada de una lista importada, si la hay y se puede.
///
/// No devuelve error a propósito. Una playlist con sus trece canciones y sin
/// foto es una importación buena; abortarla porque una imagen no se descargó
/// sería tirar lo que sí funcionó. Sin portada queda el mosaico, que es lo que
/// tienen las listas creadas a mano.
async fn traer_portada(deps: &Arc<Dependencias>, id: &PlaylistId, url: Option<&str>) {
    let (Some(url), Some(imagenes)) = (url, deps.imagenes.as_ref()) else {
        return;
    };

    let bytes = match imagenes.fetch(url).await {
        Ok(b) => b,
        Err(e) => {
            debug!(playlist = %id.as_uuid(), error = %e, "sin portada para la lista importada");
            return;
        }
    };

    // Mismo sitio y mismo nombre que la portada que elige el usuario: para la
    // playlist no hay diferencia entre "la traje de Spotify" y "la puse yo", y
    // tener dos caminos distintos daría dos formas de que se desincronicen.
    //
    // La extensión es `jpg` sin mirar: las portadas de Spotify lo son, y el
    // esquema `cover://` decide el tipo por la extensión sin que un error ahí
    // impida pintarla.
    let relativa = std::path::Path::new("covers").join(format!("playlist-{}.jpg", id.as_uuid()));
    let destino = deps.paths.resolve(&relativa);

    if let Some(padre) = destino.parent() {
        let _ = deps.fs.ensure_dir(padre).await;
    }
    if let Err(e) = deps.fs.write_synced(&destino, &bytes).await {
        debug!(playlist = %id.as_uuid(), error = %e, "no se pudo guardar la portada");
        return;
    }

    let texto = relativa.to_string_lossy().replace('\\', "/");
    if let Err(e) = deps.playlists.set_cover(id, Some(&texto)).await {
        debug!(playlist = %id.as_uuid(), error = %e, "no se pudo anotar la portada");
    }
}

/// Rellena el disco de las canciones que llegaron sin él.
///
/// La página de incrustación de Spotify —la única que se puede leer sin
/// credenciales— publica título, artistas y duración, y **nada más**. Sus
/// canciones entraban sin álbum, y eso se nota en todas partes: la columna de
/// álbum vacía, la carátula cayendo a la miniatura del vídeo en vez de a la del
/// disco, y el emparejamiento buscando a ciegas sin saber de qué edición se
/// trata.
///
/// El dato ya está en la respuesta que el catálogo da al resolver la grabación,
/// así que esto no añade ninguna petición que no fuéramos a hacer igualmente al
/// descargar: solo la adelanta al momento en que la lista aparece en pantalla.
///
/// Nunca falla. Una canción sin disco es una canción que se reproduce igual;
/// abortar la importación entera por eso sería tirar lo que sí funcionó.
async fn completar_albumes(deps: &Arc<Dependencias>, pistas: Vec<Track>) -> Vec<Track> {
    let mut completadas = Vec::with_capacity(pistas.len());
    let mut rellenadas = 0_usize;

    for mut pista in pistas {
        if pista.album.is_none()
            && let Ok(Some(r)) = deps.provider.resolve_recording(&pista).await
            && r.album.is_some()
        {
            pista.album = r.album;
            rellenadas += 1;
        }
        completadas.push(pista);
    }

    if rellenadas > 0 {
        debug!(rellenadas, de = completadas.len(), "álbumes completados");
    }
    completadas
}

/// Trae una playlist del proveedor y la persiste.
async fn importar(deps: &Arc<Dependencias>, import_id: Uuid, referencia: &str) -> CoreResult<()> {
    let bus = Arc::clone(&deps.bus);
    let avisar = move |hechas: u32, total: u32| {
        bus.publish(DomainEvent::PlaylistImportProgress {
            import_id,
            done: hechas,
            total,
        });
    };

    let mut importada = deps.provider.public_playlist(referencia, &avisar).await?;
    importada.tracks = completar_albumes(deps, importada.tracks).await;

    // Las pistas se persisten primero: sin ellas en el catálogo, las entradas
    // de la playlist apuntarían a filas que no existen.
    deps.tracks.upsert(&importada.tracks).await?;

    let ahora = chrono::Utc::now();
    let playlist = Playlist {
        id: PlaylistId::nuevo(),
        name: importada.name.clone(),
        description: importada.description.clone(),
        cover_path: None,
        source: PlaylistSource::SpotifyImport,
        source_id: Some(importada.source_id.clone()),
        created_at: ahora,
        updated_at: ahora,
    };
    deps.playlists.create(&playlist).await?;

    // Las posiciones se numeran de golpe desde cero: es una playlist nueva y no
    // hay vecinos con los que negociar huecos.
    #[allow(clippy::cast_precision_loss, reason = "playlists de miles, no de 2^53")]
    let entradas: Vec<(PlaylistEntryId, TrackId, f64)> = importada
        .tracks
        .iter()
        .enumerate()
        .map(|(i, t)| {
            (
                PlaylistEntryId::nuevo(),
                t.id.clone(),
                i as f64 * position::PASO,
            )
        })
        .collect();
    deps.playlists.add_entries(&playlist.id, &entradas).await?;
    traer_portada(deps, &playlist.id, importada.cover_url.as_deref()).await;

    info!(
        playlist = %playlist.id.as_uuid(),
        pistas = entradas.len(),
        "playlist importada"
    );
    deps.bus.publish(DomainEvent::PlaylistImportFinished {
        import_id,
        playlist_id: playlist.id,
    });
    deps.bus.publish(DomainEvent::PlaylistChanged {
        playlist_id: playlist.id,
        kind: PlaylistChangeKind::Created,
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn un_nombre_vacio_se_rechaza() {
        assert!(nombre_valido("").is_err());
        assert!(nombre_valido("   ").is_err());
        assert!(nombre_valido("\t\n").is_err());
    }

    #[test]
    fn el_nombre_se_recorta() {
        assert_eq!(nombre_valido("  Mi lista  ").expect("valido"), "Mi lista");
    }

    #[test]
    fn un_nombre_desmesurado_se_rechaza() {
        // Pegar un texto largo desde el portapapeles no debe romper el diseno
        // de la barra lateral.
        let largo = "a".repeat(NOMBRE_MAX + 1);
        assert!(nombre_valido(&largo).is_err());
        assert!(nombre_valido(&"a".repeat(NOMBRE_MAX)).is_ok());
    }

    #[test]
    fn el_limite_cuenta_caracteres_y_no_bytes() {
        // Con `len()` en bytes, un nombre en japones o con emojis se rechazaria
        // a un tercio de su longitud real.
        let emojis = "🎵".repeat(NOMBRE_MAX);
        assert!(
            nombre_valido(&emojis).is_ok(),
            "200 caracteres deben caber aunque ocupen 800 bytes"
        );
    }
}

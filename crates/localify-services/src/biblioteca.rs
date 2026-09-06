//! El servicio de biblioteca.
//!
//! Consultas sobre el catálogo local —pistas, álbumes, artistas, favoritos,
//! historial— y el **reconciliador**, que es la única parte con lógica de
//! verdad.
//!
//! ## Qué reconcilia el escaneo
//!
//! La base de datos y el disco pueden desincronizarse en las dos direcciones, y
//! las dos ocurren de verdad:
//!
//! - **Fila sin fichero.** El usuario borró la canción desde el explorador, o
//!   movió la carpeta de biblioteca. La fila se elimina y la pista vuelve a
//!   `Absent`: sigue en el catálogo, y volver a darle a play la descarga otra
//!   vez.
//! - **Fichero sin fila.** La base de datos se corrompió y se restauró una
//!   copia vieja, o alguien copió su biblioteca a otro ordenador. El fichero se
//!   **recupera** identificándolo por su nombre y sus etiquetas (ADR-021), sin
//!   volver a descargar nada.
//!
//! El segundo caso es el que justifica el diseño de identidad dual. Sin él,
//! restaurar una copia de seguridad de la base de datos significaría redescargar
//! una biblioteca entera que ya está en disco.
//!
//! ## Por qué el escaneo no bloquea
//!
//! Una biblioteca de 50 000 ficheros tarda minutos. `rescan` devuelve un
//! identificador al instante y el trabajo sigue en una tarea de fondo,
//! publicando progreso. Nunca se ejecuta al arrancar.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use localify_core::domain::album::{AlbumDetail, AlbumFilter, AlbumRow};
use localify_core::domain::artist::{ArtistDetail, ArtistRow};
use localify_core::domain::audio::DurationMs;
use localify_core::domain::ids::{AlbumId, ArtistId, TrackId};
use localify_core::domain::library::{ImportReport, LibraryStats, PlayHistoryEntry, ScanReport};
use localify_core::domain::track::{TrackFilter, TrackRow, TrackSort};
use localify_core::error::{CoreError, CoreResult};
use localify_core::events::{DomainEvent, EventPublisher, LibraryScope};
use localify_core::page::{Page, PageRequest};
use localify_core::ports::database::{
    AlbumRepository, ArtistRepository, AudioFileRepository, FavoriteRepository, HistoryRepository,
    ScanReportRepository, TrackRepository,
};
use localify_core::ports::platform::{AppPaths, FileSystem};
use localify_core::ports::services::LibraryService;
use localify_core::ports::youtube::TagWriter;
use localify_core::text;
use tracing::{debug, info, warn};
use uuid::Uuid;

/// Pistas por página al recorrer la biblioteca durante el escaneo.
const PAGINA_ESCANEO: u32 = 500;

/// Cada cuántos ficheros se publica progreso.
///
/// Un evento por fichero saturaría el puente IPC con 50 000 mensajes para mover
/// una barra que se actualiza sesenta veces por segundo como mucho.
const CADA_CUANTOS_AVISA: u32 = 50;

/// Pistas más escuchadas que se muestran en la ficha de un artista.
const TOP_ARTISTA: u8 = 10;

/// Umbral para considerar una escucha "completa".
///
/// El 90 % es lo que usa Last.fm y lo que distingue haber oído una canción de
/// haberla saltado. Se aplica aquí y no en el reproductor porque es una regla
/// de negocio, no de reproducción.
const FRACCION_COMPLETA: f32 = 0.9;

/// Dependencias del servicio.
pub struct Dependencias {
    pub tracks: Arc<dyn TrackRepository>,
    pub albums: Arc<dyn AlbumRepository>,
    pub artists: Arc<dyn ArtistRepository>,
    pub audio: Arc<dyn AudioFileRepository>,
    pub favoritos: Arc<dyn FavoriteRepository>,
    pub historial: Arc<dyn HistoryRepository>,
    pub informes: Arc<dyn ScanReportRepository>,
    /// La sesión guardada, solo para olvidarla al vaciar la biblioteca.
    pub estado_repo: Arc<dyn localify_core::ports::database::PlayerStateRepository>,
    pub tagger: Arc<dyn TagWriter>,
    pub fs: Arc<dyn FileSystem>,
    pub paths: Arc<dyn AppPaths>,
    pub bus: Arc<dyn EventPublisher>,
}

impl std::fmt::Debug for Dependencias {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Dependencias").finish_non_exhaustive()
    }
}

/// El servicio de biblioteca.
pub struct LibraryServiceImpl {
    deps: Arc<Dependencias>,
    /// Impide dos escaneos a la vez: se pisarían al reconciliar.
    escaneando: Arc<AtomicBool>,
}

impl std::fmt::Debug for LibraryServiceImpl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LibraryServiceImpl")
            .field("escaneando", &self.escaneando.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

impl LibraryServiceImpl {
    #[must_use]
    pub fn nuevo(deps: Dependencias) -> Self {
        Self {
            deps: Arc::new(deps),
            escaneando: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Ejecuta el escaneo de forma síncrona.
    ///
    /// Existe aparte de [`LibraryService::rescan`] para que los tests puedan
    /// esperar el resultado sin sondear.
    ///
    /// # Errors
    /// Si la base de datos falla. Un fichero ilegible **no** es un error: se
    /// cuenta y se sigue, porque abortar el escaneo entero por una canción
    /// corrupta dejaría el resto sin reconciliar.
    pub async fn escanear(&self) -> CoreResult<ScanReport> {
        if self.escaneando.swap(true, Ordering::AcqRel) {
            return Err(CoreError::conflict("ya hay un escaneo en curso"));
        }
        let resultado = reconciliar(&self.deps).await;
        self.escaneando.store(false, Ordering::Release);

        if let Ok(informe) = &resultado {
            self.deps.informes.save(informe).await?;
            self.deps.bus.publish(DomainEvent::LibraryChanged {
                scope: LibraryScope::Tracks,
            });
        }
        resultado
    }
}

/// Recorre disco y base de datos y los pone de acuerdo.
async fn reconciliar(deps: &Arc<Dependencias>) -> CoreResult<ScanReport> {
    let inicio = std::time::Instant::now();
    let scan_id = Uuid::now_v7();

    // ── Paso 1: filas cuyo fichero ya no está ───────────────────────────────
    let mut revisados = 0_u32;
    let mut faltan = 0_u32;
    let mut en_base: std::collections::HashSet<std::path::PathBuf> =
        std::collections::HashSet::new();

    let mut cursor = 0_u32;
    loop {
        let pagina = deps
            .audio
            .list_all(&PageRequest::new(cursor, PAGINA_ESCANEO))
            .await?;
        if pagina.items.is_empty() {
            break;
        }

        for registro in &pagina.items {
            let absoluta = deps.paths.resolve(&registro.rel_path);
            if deps.fs.exists(&absoluta).await {
                en_base.insert(registro.rel_path.clone());
            } else {
                // La pista sigue en el catálogo: solo deja de estar en local.
                // Borrarla del catálogo perdería sus favoritos y su historial.
                deps.audio.delete(&registro.track_id).await?;
                faltan += 1;
                debug!(pista = %registro.track_id.as_str(), "el fichero ya no existe");
            }
            revisados += 1;
            avisar(deps, scan_id, revisados);
        }

        cursor += PAGINA_ESCANEO;
    }

    // ── Paso 2: ficheros sin fila ───────────────────────────────────────────
    let (recuperados, ilegibles, vistos) = recuperar_huerfanos(deps, &en_base, scan_id).await;

    let informe = ScanReport {
        files_scanned: revisados.saturating_add(vistos),
        recovered: recuperados,
        missing: faltan,
        unreadable: ilegibles,
        duration_ms: u64::try_from(inicio.elapsed().as_millis()).unwrap_or(u64::MAX),
    };
    info!(?informe, "escaneo terminado");
    Ok(informe)
}

/// Busca ficheros en `audio/` que la base de datos no conoce y los recupera.
///
/// Devuelve `(recuperados, ilegibles, ficheros vistos)`.
async fn recuperar_huerfanos(
    deps: &Arc<Dependencias>,
    conocidos: &std::collections::HashSet<std::path::PathBuf>,
    scan_id: Uuid,
) -> (u32, u32, u32) {
    let ficheros = listar_audio(&deps.paths.audio_dir());

    let mut recuperados = 0_u32;
    let mut ilegibles = 0_u32;
    let mut vistos = 0_u32;

    for absoluta in ficheros {
        vistos += 1;
        avisar(deps, scan_id, vistos);

        let Ok(relativa) = absoluta.strip_prefix(deps.paths.library_dir()) else {
            continue;
        };
        if conocidos.contains(relativa) {
            continue;
        }

        // Identidad dual (ADR-021): primero la etiqueta, que sobrevive a un
        // renombrado; si no, el nombre del fichero, que sobrevive a que el
        // etiquetado fallara.
        let por_etiqueta = deps
            .tagger
            .read_track_id(&absoluta)
            .await
            .ok()
            .flatten()
            .map(TrackId::from_trusted);

        let Some(id) = por_etiqueta.or_else(|| id_desde_nombre(&absoluta)) else {
            ilegibles += 1;
            debug!(fichero = %absoluta.display(), "sin identidad reconocible");
            continue;
        };

        // Solo se recupera lo que el catálogo ya conoce. Un fichero suelto sin
        // metadatos no puede inventarse título ni artista, y meterlo con el
        // nombre del fichero ensuciaría la biblioteca.
        match deps.tracks.get(&id).await {
            Ok(Some(_)) => match registrar(deps, &id, relativa, &absoluta).await {
                Ok(()) => recuperados += 1,
                Err(e) => {
                    warn!(error = %e, "no se pudo recuperar el fichero");
                    ilegibles += 1;
                }
            },
            Ok(None) => {
                debug!(pista = %id.as_str(), "fichero de una pista desconocida");
                ilegibles += 1;
            }
            Err(e) => {
                warn!(error = %e, "consulta fallida durante el escaneo");
                ilegibles += 1;
            }
        }
    }

    (recuperados, ilegibles, vistos)
}

/// Da de alta un fichero recuperado.
async fn registrar(
    deps: &Arc<Dependencias>,
    id: &TrackId,
    relativa: &std::path::Path,
    absoluta: &std::path::Path,
) -> CoreResult<()> {
    use localify_core::domain::audio::AudioFormat;
    use localify_core::domain::library::{AudioFileRecord, AudioSource};

    let formato = absoluta
        .extension()
        .and_then(|e| e.to_str())
        .and_then(AudioFormat::from_extension)
        .ok_or_else(|| CoreError::invalid("extension no reconocida"))?;

    let bytes = deps.fs.file_size(absoluta).await.unwrap_or(0);

    // La duración se toma del catálogo: medirla exigiría decodificar el fichero
    // entero, y en 50 000 ficheros eso convierte un escaneo de segundos en uno
    // de horas. La verificación fina ya se hizo al descargarlo.
    let duracion = deps
        .tracks
        .get(id)
        .await?
        .map_or(DurationMs::ZERO, |t| t.duration);

    deps.audio
        .insert(&AudioFileRecord {
            track_id: id.clone(),
            rel_path: relativa.to_path_buf(),
            format: formato,
            codec: formato.extension().to_owned(),
            bitrate_kbps: None,
            sample_rate: None,
            channels: None,
            size_bytes: bytes,
            duration: duracion,
            source: AudioSource::Imported,
            youtube_id: None,
            verified_at: chrono::Utc::now(),
        })
        .await
}

/// Identificador deducido del nombre del fichero.
///
/// Los ficheros se guardan como `<TrackId>.<ext>`, así que el nombre **es** la
/// identidad. Es la segunda vía de ADR-021, y la que salva el caso de un
/// fichero cuyo etiquetado falló.
fn id_desde_nombre(ruta: &std::path::Path) -> Option<TrackId> {
    let nombre = ruta.file_stem()?.to_str()?;
    TrackId::parse(nombre).ok()
}

/// Recorre `raiz` en busca de ficheros de audio.
///
/// Iterativo y no recursivo a propósito: la estructura tiene dos niveles, pero
/// un enlace simbólico circular convertiría una recursión en un desbordamiento
/// de pila.
/// Una carpeta ilegible se salta en silencio: un permiso denegado en un
/// subdirectorio no debe abortar el escaneo de los otros 49 999 ficheros.
fn listar_audio(raiz: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut pendientes = vec![raiz.to_path_buf()];
    let mut salida = Vec::new();

    while let Some(dir) = pendientes.pop() {
        let Ok(entradas) = std::fs::read_dir(&dir) else {
            debug!(carpeta = %dir.display(), "carpeta ilegible, se salta");
            continue;
        };
        for entrada in entradas.flatten() {
            let ruta = entrada.path();
            match entrada.file_type() {
                Ok(t) if t.is_dir() => pendientes.push(ruta),
                Ok(t) if t.is_file() && es_audio(&ruta) => salida.push(ruta),
                // Los enlaces simbólicos se ignoran: seguirlos puede salirse de
                // la biblioteca o dar vueltas en círculo.
                _ => {}
            }
        }
    }
    salida
}

fn es_audio(ruta: &std::path::Path) -> bool {
    use localify_core::domain::audio::AudioFormat;
    ruta.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| AudioFormat::from_extension(e).is_some())
}

/// Publica progreso cada tantos ficheros.
/// Borra las miniaturas cacheadas de las pistas.
///
/// ## Por qué al vaciar y no por su cuenta
///
/// Son una caché derivada: se rehacen solas la próxima vez que alguien mire.
/// Pero **el fichero es su propio estado** —existe, luego sirve—, así que si la
/// regla con la que se eligió la imagen cambia, las viejas siguen dando la
/// respuesta antigua para siempre. Vaciar es el momento en que el usuario dice
/// "quiero esto otra vez desde cero", y ahí entra.
///
/// Solo `track-*.jpg`. Las portadas de álbum se rehacen igual pero no estorban,
/// y las de playlist **las eligió el usuario**: borrarlas sería perder algo que
/// no se puede recuperar solo.
async fn borrar_miniaturas(deps: &Arc<Dependencias>) {
    let dir = deps.paths.covers_dir();
    let Ok(mut entradas) = tokio::fs::read_dir(&dir).await else {
        return;
    };

    let mut borradas = 0_u32;
    while let Ok(Some(entrada)) = entradas.next_entry().await {
        let ruta = entrada.path();
        let es_miniatura = ruta
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with("track-"));
        if es_miniatura && tokio::fs::remove_file(&ruta).await.is_ok() {
            borradas += 1;
        }
    }
    if borradas > 0 {
        debug!(borradas, "miniaturas de pista borradas");
    }
}

fn avisar(deps: &Arc<Dependencias>, scan_id: Uuid, hechos: u32) {
    if hechos.is_multiple_of(CADA_CUANTOS_AVISA) {
        deps.bus.publish(DomainEvent::ScanProgress {
            scan_id,
            done: hechos,
            // El total no se conoce sin recorrer disco y base de datos enteros
            // primero, y hacerlo duplicaría el trabajo. La interfaz muestra un
            // contador, no una barra con porcentaje.
            total: 0,
        });
    }
}

#[async_trait]
impl LibraryService for LibraryServiceImpl {
    async fn tracks(
        &self,
        filter: &TrackFilter,
        sort: TrackSort,
        page: &PageRequest,
    ) -> CoreResult<Page<TrackRow>> {
        self.deps.tracks.list_rows(filter, sort, page).await
    }

    async fn albums(&self, filter: &AlbumFilter, page: &PageRequest) -> CoreResult<Page<AlbumRow>> {
        self.deps.albums.list_rows(filter, page).await
    }

    async fn artists(&self, page: &PageRequest) -> CoreResult<Page<ArtistRow>> {
        self.deps.artists.list_rows(page).await
    }

    async fn album_detail(&self, id: &AlbumId) -> CoreResult<AlbumDetail> {
        let album = self
            .deps
            .albums
            .get(id)
            .await?
            .ok_or_else(|| CoreError::not_found("album", id.as_str()))?;
        let tracks = self.deps.albums.tracks_of(id).await?;

        let total = tracks
            .iter()
            .map(|t| u64::from(t.duration.as_ms()))
            .sum::<u64>();
        let locales = tracks.iter().filter(|t| t.availability.es_local()).count();

        Ok(AlbumDetail {
            album,
            total_duration: DurationMs::new(u32::try_from(total).unwrap_or(u32::MAX)),
            local_count: u16::try_from(locales).unwrap_or(u16::MAX),
            tracks,
        })
    }

    async fn artist_detail(&self, id: &ArtistId) -> CoreResult<ArtistDetail> {
        let artist = self
            .deps
            .artists
            .get(id)
            .await?
            .ok_or_else(|| CoreError::not_found("artist", id.as_str()))?;

        let top_tracks = self.deps.artists.top_tracks_of(id, TOP_ARTISTA).await?;
        let albums = self.deps.artists.albums_of(id).await?;
        let locales = top_tracks
            .iter()
            .filter(|t| t.availability.es_local())
            .count();

        Ok(ArtistDetail {
            artist,
            top_tracks,
            albums,
            local_track_count: u32::try_from(locales).unwrap_or(u32::MAX),
        })
    }

    async fn set_favorite(&self, id: &TrackId, enabled: bool) -> CoreResult<()> {
        self.deps.favoritos.set(id, enabled).await?;
        self.deps.bus.publish(DomainEvent::LibraryChanged {
            scope: LibraryScope::Favorites,
        });
        Ok(())
    }

    async fn favorites(&self, page: &PageRequest) -> CoreResult<Page<TrackRow>> {
        self.deps.favoritos.list(page).await
    }

    async fn record_play(&self, id: &TrackId, ms_played: u32, completed: bool) -> CoreResult<()> {
        // Se ignoran las escuchas de menos de un segundo: pasar por una pista
        // saltando no es haberla escuchado, y contarlo envenenaría las
        // recomendaciones.
        if ms_played < 1000 {
            return Ok(());
        }

        self.deps
            .historial
            .record(&PlayHistoryEntry {
                track_id: id.clone(),
                played_at: chrono::Utc::now(),
                ms_played,
                completed,
                context: None,
            })
            .await?;

        self.deps.bus.publish(DomainEvent::TrackFinished {
            track_id: id.clone(),
            completed,
            ms_played,
        });
        Ok(())
    }

    async fn recent(&self, limit: u16) -> CoreResult<Vec<TrackRow>> {
        self.deps.historial.recent_tracks(limit).await
    }

    async fn stats(&self) -> CoreResult<LibraryStats> {
        self.deps.tracks.stats().await
    }

    async fn delete_download(&self, id: &TrackId) -> CoreResult<()> {
        let Some(registro) = self.deps.audio.get(id).await? else {
            // No había nada que borrar. No es un error: el usuario pidió que no
            // estuviera, y no está.
            return Ok(());
        };

        // El fichero primero y el registro después. Al revés, un fallo al
        // borrar el fichero dejaría un huérfano que el catálogo ya no conoce y
        // que nadie limpiaría nunca.
        let absoluta = self.deps.paths.resolve(&registro.rel_path);
        if let Err(e) = self.deps.fs.remove_file(&absoluta).await {
            // Que el fichero no esté no impide seguir: puede haberlo borrado el
            // usuario por fuera, y quedarse con el registro sería peor.
            debug!(pista = %id, error = %e, "no se pudo borrar el fichero");
        }
        self.deps.audio.delete(id).await?;

        self.deps.bus.publish(DomainEvent::AvailabilityChanged {
            track_id: id.clone(),
            availability: localify_core::domain::availability::Availability::Absent,
        });
        info!(pista = %id, "descarga borrada");
        Ok(())
    }

    async fn wipe_downloads(&self) -> CoreResult<u32> {
        let mut borradas = 0_u32;

        // Siempre la primera página: cada vuelta borra lo que lee, así que el
        // desplazamiento se quedaría saltando lo que queda. Es el error clásico
        // de paginar mientras se borra.
        loop {
            let pagina = self
                .deps
                .audio
                .list_all(&PageRequest::new(0, PAGINA_ESCANEO))
                .await?;
            if pagina.items.is_empty() {
                break;
            }

            for registro in &pagina.items {
                let absoluta = self.deps.paths.resolve(&registro.rel_path);
                if let Err(e) = self.deps.fs.remove_file(&absoluta).await {
                    debug!(pista = %registro.track_id, error = %e, "no se pudo borrar el fichero");
                }
                self.deps.audio.delete(&registro.track_id).await?;
                borradas += 1;
            }
        }

        borrar_miniaturas(&self.deps).await;

        // El historial se va con las descargas. No es un adorno: es lo **único**
        // que alimenta Inicio, así que dejarlo intacto deja esa pantalla llena de
        // canciones que ya no están, en secciones que se titulan "sigue
        // escuchando" y no llevan a ninguna parte.
        //
        // Los favoritos y las playlists sí se quedan: son decisiones que el
        // usuario tomó una por una. El historial no lo decidió nadie, se
        // acumuló.
        match self.deps.historial.clear().await {
            Ok(escuchas) => debug!(escuchas, "historial vaciado"),
            Err(e) => warn!(error = %e, "no se pudo vaciar el historial"),
        }
        if let Err(e) = self.deps.estado_repo.clear().await {
            warn!(error = %e, "no se pudo olvidar la sesión guardada");
        }

        // Un evento por pista serían miles: se avisa una vez de que cambió la
        // biblioteca y cada vista se refresca entera, que es lo que hace de
        // todas formas tras un cambio de este tamaño.
        self.deps.bus.publish(DomainEvent::LibraryChanged {
            scope: LibraryScope::Tracks,
        });
        info!(borradas, "biblioteca vaciada de descargas");
        Ok(borradas)
    }

    async fn rescan(&self) -> CoreResult<Uuid> {
        if self.escaneando.load(Ordering::Acquire) {
            return Err(CoreError::conflict("ya hay un escaneo en curso"));
        }
        let scan_id = Uuid::now_v7();

        // Se devuelve al instante: una biblioteca de 50 000 ficheros tarda
        // minutos, y bloquear el comando IPC congelaría la interfaz.
        let deps = Arc::clone(&self.deps);
        let escaneando = Arc::clone(&self.escaneando);
        tokio::spawn(async move {
            if escaneando.swap(true, Ordering::AcqRel) {
                return;
            }
            match reconciliar(&deps).await {
                Ok(informe) => {
                    if let Err(e) = deps.informes.save(&informe).await {
                        warn!(error = %e, "no se pudo guardar el informe del escaneo");
                    }
                    deps.bus.publish(DomainEvent::LibraryChanged {
                        scope: LibraryScope::Tracks,
                    });
                }
                Err(e) => warn!(error = %e, "el escaneo fallo"),
            }
            escaneando.store(false, Ordering::Release);
        });

        Ok(scan_id)
    }

    async fn last_scan_report(&self) -> CoreResult<Option<ScanReport>> {
        self.deps.informes.last().await
    }

    async fn delete_track(&self, id: &TrackId) -> CoreResult<()> {
        // El fichero primero: si el borrado de la fila fallara después de
        // borrar el fichero no pasaría nada (la pista quedaría `Absent`, como
        // si nunca se hubiera descargado), pero al revés dejaría un fichero
        // huérfano que ningún catálogo referencia ya.
        if let Some(registro) = self.deps.audio.get(id).await? {
            let absoluta = self.deps.paths.resolve(&registro.rel_path);
            if let Err(e) = self.deps.fs.remove_file(&absoluta).await {
                debug!(pista = %id, error = %e, "no se pudo borrar el fichero");
            }
        }

        self.deps.tracks.delete(id).await?;
        self.deps.bus.publish(DomainEvent::LibraryChanged {
            scope: LibraryScope::Tracks,
        });
        info!(pista = %id, "pista borrada del catálogo");
        Ok(())
    }

    async fn import_files(&self, paths: Vec<std::path::PathBuf>) -> CoreResult<ImportReport> {
        let informe = importar(&self.deps, paths).await;

        if informe.imported > 0 {
            self.deps.bus.publish(DomainEvent::LibraryChanged {
                scope: LibraryScope::Tracks,
            });
        }
        info!(?informe, "importación de ficheros propios terminada");
        Ok(informe)
    }
}

/// Importa una selección manual de ficheros. Nunca aborta el lote entero por
/// un fichero que falle: se cuenta y se sigue con el siguiente.
async fn importar(deps: &Arc<Dependencias>, rutas: Vec<std::path::PathBuf>) -> ImportReport {
    let mut informe = ImportReport {
        files_selected: u32::try_from(rutas.len()).unwrap_or(u32::MAX),
        ..ImportReport::default()
    };

    for absoluta in rutas {
        match importar_uno(deps, &absoluta).await {
            Ok(()) => informe.imported += 1,
            Err(e) => {
                debug!(fichero = %absoluta.display(), error = %e, "no se pudo importar");
                informe.skipped_unreadable += 1;
            }
        }
    }

    informe
}

/// Da de alta un único fichero propio del usuario.
///
/// A diferencia de [`registrar`], que recupera un fichero de una pista que el
/// catálogo **ya conoce**, aquí la pista es nueva: sus metadatos salen de las
/// etiquetas del propio fichero, no de ningún proveedor. El fichero se copia
/// —nunca se mueve ni se toca el original— al mismo esquema de rutas que usan
/// las descargas, para que la identidad por nombre de fichero (ADR-021) siga
/// funcionando en un `rescan` posterior.
async fn importar_uno(deps: &Arc<Dependencias>, absoluta: &std::path::Path) -> CoreResult<()> {
    use localify_core::domain::album::{Album, AlbumType, CoverSet};
    use localify_core::domain::audio::AudioFormat;
    use localify_core::domain::track::{AlbumRef, ArtistRef, Track};

    let formato = absoluta
        .extension()
        .and_then(|e| e.to_str())
        .and_then(AudioFormat::from_extension)
        .ok_or_else(|| CoreError::invalid("extensión no reconocida"))?;

    let tags = deps.tagger.read_generic_tags(absoluta).await?;

    // `tracks.duration_ms` lleva un `CHECK (duration_ms > 0)`: caer a cero
    // violaría la restricción con un error de SQLite en lugar de un fallo
    // legible. Un fichero cuya duración no se pueda medir no es un fichero
    // de audio utilizable, así que se cuenta como ilegible y no se importa.
    let duracion = tags
        .duration
        .filter(|d| !d.is_zero())
        .ok_or_else(|| CoreError::invalid("no se pudo determinar la duración del audio"))?;

    let titulo = tags.title.filter(|t| !t.trim().is_empty()).unwrap_or_else(|| {
        absoluta
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("Pista sin título")
            .to_owned()
    });

    let artistas: Vec<ArtistRef> = match tags.artist.filter(|a| !a.trim().is_empty()) {
        Some(nombre) => vec![ArtistRef {
            id: ArtistId::nuevo_local(),
            name: nombre,
        }],
        None => Vec::new(),
    };

    // Álbum: solo si la etiqueta lo trae, y reutilizando uno ya existente con
    // el mismo título y artista en vez de mintar uno sintético por pista. Sin
    // esto, importar varias pistas del mismo álbum lo fragmentaría en tantos
    // álbumes de una sola canción como pistas se importen.
    let album_nuevo = tags.album.filter(|a| !a.trim().is_empty()).map(|titulo_album| {
        let normalizado = text::normalize(&titulo_album);
        (titulo_album, normalizado)
    });

    let album_existente = if let (Some((_, titulo_norm)), Some(principal)) =
        (&album_nuevo, artistas.first())
    {
        deps.albums
            .find_by_title_and_artist(titulo_norm, &text::normalize(&principal.name))
            .await?
    } else {
        None
    };

    let album_ref = match (&album_nuevo, &album_existente) {
        (Some((titulo_album, _)), Some(id)) => Some(AlbumRef {
            id: id.clone(),
            title: titulo_album.clone(),
        }),
        (Some((titulo_album, _)), None) => Some(AlbumRef {
            id: AlbumId::nuevo_local(),
            title: titulo_album.clone(),
        }),
        (None, _) => None,
    };

    let track_id = TrackId::nuevo_local();
    let track = Track {
        id: track_id.clone(),
        title: titulo,
        album: album_ref.clone(),
        artists: artistas.clone(),
        duration: duracion,
        track_number: tags.track_number,
        disc_number: None,
        explicit: false,
        isrc: None,
        release_date: None,
        popularity: None,
        added_at: chrono::Utc::now(),
    };

    // El fichero se copia **antes** de escribir nada en la base de datos: si la
    // copia falla, no debe quedar un catálogo apuntando a un audio que no
    // existe.
    let relativa = deps.paths.audio_rel_path(track_id.as_str(), formato.extension());
    let destino = deps.paths.resolve(&relativa);
    if let Some(padre) = destino.parent() {
        deps.fs.ensure_dir(padre).await?;
    }
    deps.fs.copy_file(absoluta, &destino).await?;

    deps.tracks.upsert(std::slice::from_ref(&track)).await?;

    if let (Some(album_ref), None) = (&album_ref, &album_existente) {
        deps.albums
            .upsert(&[Album {
                id: album_ref.id.clone(),
                title: album_ref.title.clone(),
                artists: artistas,
                album_type: AlbumType::Album,
                release_date: None,
                total_tracks: None,
                cover_url: None,
                covers: CoverSet::default(),
                label: None,
            }])
            .await?;
    }

    registrar(deps, &track_id, &relativa, &destino).await
}

/// Decide si una escucha cuenta como completa.
///
/// Va aquí y no en el reproductor porque es una regla de negocio —qué cuenta
/// como "haber escuchado algo"— y no de reproducción.
#[must_use]
pub fn escucha_completa(ms_played: u32, duracion: DurationMs) -> bool {
    if duracion.is_zero() {
        return false;
    }
    #[allow(clippy::cast_precision_loss, reason = "duraciones de minutos")]
    let fraccion = ms_played as f32 / duracion.as_ms() as f32;
    fraccion >= FRACCION_COMPLETA
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn una_escucha_casi_entera_cuenta_como_completa() {
        let d = DurationMs::from_secs(200);
        assert!(escucha_completa(190_000, d));
        assert!(escucha_completa(200_000, d));
    }

    #[test]
    fn saltar_una_cancion_no_cuenta_como_escucharla() {
        // Es la senal negativa del motor de recomendaciones: contarla como
        // positiva haria que saltar una cancion la recomendase mas.
        let d = DurationMs::from_secs(200);
        assert!(!escucha_completa(5_000, d));
        assert!(!escucha_completa(100_000, d));
    }

    #[test]
    fn una_duracion_desconocida_nunca_cuenta_como_completa() {
        // Dividir por cero daria `inf`, que es `>= 0.9`: todas las pistas sin
        // duracion contarian como escuchadas enteras.
        assert!(!escucha_completa(60_000, DurationMs::ZERO));
    }

    #[test]
    fn el_nombre_del_fichero_identifica_la_pista() {
        let ruta = std::path::Path::new("C:/lib/audio/3z/3z8h0TU7ReDPLIbEnYhWZb.opus");
        assert_eq!(
            id_desde_nombre(ruta).map(|i| i.as_str().to_owned()),
            Some("3z8h0TU7ReDPLIbEnYhWZb".to_owned())
        );
    }

    #[test]
    fn un_nombre_que_no_es_un_identificador_se_descarta() {
        // Un fichero que el usuario copio a mano no debe entrar como si fuera
        // una pista del catalogo.
        for nombre in [
            "C:/lib/audio/cancion favorita.mp3",
            "C:/lib/audio/01 - Intro.flac",
            "C:/lib/audio/.opus",
        ] {
            assert!(
                id_desde_nombre(std::path::Path::new(nombre)).is_none(),
                "'{nombre}' no deberia identificar ninguna pista"
            );
        }
    }

    #[test]
    fn solo_se_recorren_extensiones_de_audio() {
        assert!(es_audio(std::path::Path::new("a.opus")));
        assert!(es_audio(std::path::Path::new("a.FLAC")));
        assert!(!es_audio(std::path::Path::new("portada.jpg")));
        assert!(!es_audio(std::path::Path::new("a.part")));
        assert!(!es_audio(std::path::Path::new("sin-extension")));
    }
}

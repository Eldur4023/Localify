//! Contenedor de inyección de dependencias.
//!
//! **Este es el único punto del programa donde se nombran tipos concretos de
//! infraestructura.** Todo lo demás trabaja contra `Arc<dyn Trait>`.
//!
//! La consecuencia práctica se ve al sustituir un servicio provisional por el
//! definitivo: cambia una línea aquí, y ni los comandos ni el frontend se
//! enteran. Si algún día hay que tocar `commands/` para cambiar de
//! implementación, la arquitectura se ha roto.

use std::sync::Arc;

use localify_core::domain::settings::CookiesVigentes;
use localify_core::events::EventPublisher;
use localify_core::ports::database::MaintenanceRepository;
use localify_core::ports::services::{
    DownloadService, LibraryService, LyricsService, MetadataService, NotificationService,
    PlaybackService, PlaylistService, QueueService, RecommendationService, SearchService,
    SettingsService,
};
use localify_services::inerte::{AvisosPorBus, SinAudio, SinBiblioteca, SinLetras};

use crate::bridge::EventBus;

/// Estado compartido de la aplicación.
///
/// Se registra en Tauri con `manage` y los comandos lo reciben como
/// `State<'_, AppContext>`. Clonarlo es barato: todo son `Arc`.
#[derive(Clone)]
pub struct AppContext {
    pub library: Arc<dyn LibraryService>,
    pub search: Arc<dyn SearchService>,
    pub playback: Arc<dyn PlaybackService>,
    pub queue: Arc<dyn QueueService>,
    pub downloads: Arc<dyn DownloadService>,
    pub playlists: Arc<dyn PlaylistService>,
    pub recommendations: Arc<dyn RecommendationService>,
    pub lyrics: Arc<dyn LyricsService>,
    pub settings: Arc<dyn SettingsService>,
    pub metadata: Arc<dyn MetadataService>,
    pub notifications: Arc<dyn NotificationService>,
    /// Mantenimiento de la base de datos.
    ///
    /// Es lo único aquí que no es un servicio de dominio: no lo usa ningún
    /// comando, solo la tarea de fondo del arranque. Va en el contexto porque
    /// es el único sitio donde el `Pool` está a mano.
    ///
    /// `None` en modo degradado: sin base de datos no hay nada que mantener.
    pub mantenimiento: Option<Arc<dyn MaintenanceRepository>>,
    /// Credenciales y cola de Last.fm.
    ///
    /// No es un servicio de dominio y por eso no está detrás de un `dyn Trait`:
    /// es una integración opcional que solo tocan sus propios comandos y su
    /// tarea de fondo. Ponerle un puerto sería inventar una abstracción para un
    /// único implementador.
    ///
    /// `None` en modo degradado: la cola de scrobbles vive en la base de datos.
    pub lastfm: Option<Arc<localify_integrations::GestorLastfm>>,
    /// De donde Discord saca la carátula de lo que suena.
    ///
    /// Está aquí por el mismo motivo que `mantenimiento`: no lo usa ningún
    /// comando, solo una tarea de fondo, y este es el único sitio donde el
    /// `Pool` y el proveedor están a mano. `None` en modo degradado.
    pub para_discord: Option<PiezasDeDiscord>,
    /// Las dos comprobaciones de Ajustes que tocan a yt-dlp directamente.
    pub diagnostico: Arc<Diagnostico>,
    pub bus: EventBus,
}

/// Comprobar las cookies y poner yt-dlp al día.
///
/// No son servicios de dominio y por eso no van detrás de un puerto: las dos son
/// «ejecuta el binario y cuenta qué pasó», sin ninguna decisión de negocio
/// detrás. Inventarles un trait sería una abstracción con un solo implementador
/// que además no abstrae nada.
pub struct Diagnostico {
    ejecutor: Option<Arc<dyn localify_ytdlp::proceso::Ejecutor>>,
    cookies: Arc<CookiesVigentes>,
    binarios: std::path::PathBuf,
}

impl std::fmt::Debug for Diagnostico {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Diagnostico").finish_non_exhaustive()
    }
}

/// El vídeo con el que se comprueban las cookies.
///
/// «Me at the zoo», el primero que se subió a YouTube. Se elige por ser el que
/// menos probabilidades tiene de desaparecer, y no se descarga: solo se pide su
/// ficha, que es la misma operación con la que falla el emparejador.
const VIDEO_DE_PRUEBA: &str = "https://www.youtube.com/watch?v=jNQXAC9IVRw";

impl Diagnostico {
    fn real(
        infra: &Infraestructura,
        ejecutor: Arc<dyn localify_ytdlp::proceso::Ejecutor>,
        cookies: Arc<CookiesVigentes>,
    ) -> Arc<Self> {
        Arc::new(Self {
            ejecutor: Some(ejecutor),
            cookies,
            binarios: infra.paths.binaries_dir(),
        })
    }

    /// Sin base de datos no hay descargas que arreglar, pero la pantalla de
    /// Ajustes sigue abierta y sus botones tienen que decir algo sensato en vez
    /// de reventar.
    fn inerte() -> Arc<Self> {
        Arc::new(Self {
            ejecutor: None,
            cookies: Arc::default(),
            binarios: std::path::PathBuf::new(),
        })
    }

    /// Ejecuta una extracción real con las cookies configuradas.
    ///
    /// Lo que de verdad comprueba es que **el almacén de cookies se puede
    /// leer**, que es donde falla casi siempre: en Windows, Chrome y sus
    /// derivados lo cifran con App-Bound Encryption desde la versión 127, un
    /// perfil abierto puede estar bloqueado, y un `cookies.txt` exportado hace
    /// meses tiene la sesión caducada. Si el muro de «confirma que no eres un
    /// bot» no aparece hoy en este vídeo, esto pasará aunque las cookies no
    /// sirvan para otro: es una comprobación de que el mecanismo funciona, no
    /// una garantía sobre el catálogo entero.
    pub async fn probar_cookies(&self) -> crate::dto::settings::PruebaDto {
        let Some(ejecutor) = &self.ejecutor else {
            return crate::dto::settings::PruebaDto::fallo(
                "settings.cookies_no_ytdlp",
                String::new(),
            );
        };

        let mut args = vec![
            VIDEO_DE_PRUEBA.to_owned(),
            "--simulate".to_owned(),
            "--no-warnings".to_owned(),
            "--no-playlist".to_owned(),
            "--print".to_owned(),
            "%(id)s".to_owned(),
        ];
        args.extend(localify_ytdlp::args_de_cookies(&self.cookies.leer()));

        match ejecutor.ejecutar("yt-dlp", &args).await {
            Ok(s) if s.es_ok() => crate::dto::settings::PruebaDto::ok("settings.cookies_ok"),
            // El detalle es la salida cruda de yt-dlp, sin traducir: es la única
            // parte que dice *qué* ha fallado, y traducirla sería inventarse un
            // catálogo de mensajes de otro programa (ADR-012 manda las claves,
            // no los diagnósticos ajenos).
            Ok(s) => crate::dto::settings::PruebaDto::fallo(
                "settings.cookies_error",
                ultima_linea_util(&s.stderr),
            ),
            Err(e) => {
                crate::dto::settings::PruebaDto::fallo("settings.cookies_error", e.to_string())
            }
        }
    }

    /// Pone yt-dlp al día y cuenta el resultado.
    pub async fn actualizar_ytdlp(&self) -> crate::dto::settings::PruebaDto {
        use localify_platform::Actualizacion;

        let locator = localify_platform::SidecarLocator::new(self.binarios.clone());
        match localify_platform::actualizar_yt_dlp(&locator, TOPE_ACTUALIZACION).await {
            Actualizacion::AlDia(v) => {
                crate::dto::settings::PruebaDto::detalle("settings.ytdlp_al_dia", v)
            }
            Actualizacion::Actualizado { ahora, .. } => {
                crate::dto::settings::PruebaDto::detalle("settings.ytdlp_actualizado", ahora)
            }
            Actualizacion::NoEsNuestro => {
                crate::dto::settings::PruebaDto::fallo("settings.ytdlp_ajeno", String::new())
            }
            Actualizacion::NoSePudo(motivo) => {
                crate::dto::settings::PruebaDto::fallo("settings.ytdlp_error", motivo)
            }
        }
    }
}

/// Cuánto se espera a que yt-dlp compruebe su versión.
///
/// Un minuto: comprobar tarda un segundo, pero si hay versión nueva se descarga
/// el binario entero, que son unos diecisiete megas.
pub const TOPE_ACTUALIZACION: std::time::Duration = std::time::Duration::from_secs(60);

/// La última línea de un error de yt-dlp que dice algo.
///
/// Su `stderr` trae la traza de Python por delante cuando algo revienta de
/// verdad, y lo accionable siempre está al final.
fn ultima_linea_util(stderr: &str) -> String {
    stderr
        .lines()
        .map(str::trim)
        .rfind(|l| !l.is_empty())
        .unwrap_or("")
        .to_owned()
}

/// Lo que la tarea de Discord necesita y ningún comando usa.
///
/// Van juntas en una estructura y no como tres campos sueltos porque se piden
/// las tres o ninguna: sin base de datos no hay biblioteca que anunciar, y la
/// tarea no arranca.
#[derive(Clone)]
#[expect(
    missing_debug_implementations,
    reason = "son tres `Arc<dyn Trait>`; un `Debug` aquí solo imprimiría el nombre"
)]
pub struct PiezasDeDiscord {
    pub albums: Arc<dyn localify_core::ports::database::AlbumRepository>,
    pub tracks: Arc<dyn localify_core::ports::database::TrackRepository>,
    pub provider: Arc<dyn localify_core::ports::metadata_provider::MetadataProvider>,
}

impl std::fmt::Debug for AppContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppContext")
            .field("bus", &self.bus)
            .finish_non_exhaustive()
    }
}

/// Piezas de infraestructura ya construidas, para cablear el contexto.
pub struct Infraestructura {
    pub pool: localify_db::Pool,
    pub secretos: Arc<dyn localify_core::ports::platform::SecretStore>,
    pub paths: Arc<dyn localify_core::ports::platform::AppPaths>,
}

impl std::fmt::Debug for Infraestructura {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Infraestructura").finish_non_exhaustive()
    }
}

impl AppContext {
    /// Cablea la aplicación con la persistencia y el proveedor reales.
    ///
    /// # Errors
    /// Si el transporte HTTP no se puede construir.
    #[allow(
        clippy::too_many_lines,
        reason = "es una lista de cableado: lo que tiene sentido propio ya está extraído \
                  —`Repositorios`, `metadatos`, `abrir_motor`, `descargas`…— y partirla más \
                  solo esconde el orden en el que se construyen las cosas, que es justo lo \
                  que hay que poder leer de un tirón"
    )]
    pub async fn real(
        bus: EventBus,
        infra: Infraestructura,
    ) -> localify_core::error::CoreResult<Self> {
        let publicador: Arc<dyn localify_core::events::EventPublisher> = Arc::new(bus.clone());

        // ── Proveedores de metadatos ────────────────────────────────────────
        let (spotify, conmutador) = proveedores(&infra).await?;
        let provider: Arc<dyn localify_core::ports::metadata_provider::MetadataProvider> =
            Arc::clone(&conmutador) as _;

        // ── Repositorios ────────────────────────────────────────────────────
        let Repositorios {
            tracks,
            albums,
            artists,
            search: search_repo,
        } = Repositorios::sobre(&infra);

        // ── Servicios reales ────────────────────────────────────────────────
        let imagenes = descargador_de_imagenes();
        let metadata = metadatos(
            &infra,
            &provider,
            &tracks,
            &albums,
            artists,
            &publicador,
            imagenes.clone(),
        );
        let search = Arc::new(localify_services::SearchServiceImpl::nuevo(
            search_repo,
            Arc::clone(&tracks),
            Arc::clone(&provider),
            Arc::clone(&metadata),
            Arc::clone(&publicador),
        ));

        let (motor, eventos_audio) = abrir_motor();

        // Compartido entre ajustes (escribe) y reproducción (lee al encadenar).
        let crossfade = Arc::new(std::sync::atomic::AtomicU32::new(0));
        // Lo mismo con las cookies: las escribe Ajustes y las lee yt-dlp en cada
        // búsqueda y cada descarga.
        let cookies: Arc<CookiesVigentes> = Arc::default();

        // El ejecutor se crea aquí y no dentro de `descargas` porque lo comparten
        // dos consumidores: la descarga y la comprobación de cookies de Ajustes.
        let ejecutor_ytdlp: Arc<dyn localify_ytdlp::proceso::Ejecutor> = Arc::new(
            localify_ytdlp::proceso::EjecutorReal::nuevo(infra.paths.binaries_dir()),
        );

        // ── Descargas, cola y reproducción ──────────────────────────────────
        let downloads = descargas(
            &infra,
            &tracks,
            &publicador,
            &provider,
            &cookies,
            &ejecutor_ytdlp,
        )
        .await;
        let queue = localify_services::QueueActor::nuevo(localify_services::DependenciasCola {
            tracks: Arc::clone(&tracks),
            bus: Arc::clone(&publicador),
        });
        let playback = reproduccion(
            &infra,
            &tracks,
            &downloads,
            &queue,
            &publicador,
            motor.clone(),
            eventos_audio,
            Arc::clone(&crossfade),
        )
        .await;
        let library = biblioteca(&infra, &tracks, &publicador);
        let playlists = listas(
            &infra,
            &tracks,
            &provider,
            imagenes.as_ref(),
            &publicador,
            &downloads,
        );
        let recommendations = recomendaciones(&infra, &tracks, &provider);

        let settings = ajustes(
            &infra,
            &publicador,
            motor,
            crossfade,
            Arc::clone(&cookies),
            &spotify,
            &conmutador,
        )
        .await;
        let lyrics = letras(&infra, &tracks);
        let lastfm = scrobbler(&infra, &tracks, &settings);

        Ok(Self {
            library,
            search,
            playback,
            queue: Arc::new(queue),
            downloads,
            playlists,
            recommendations,
            lyrics,
            settings,
            metadata,
            notifications: Arc::new(AvisosPorBus(Arc::clone(&publicador))),
            mantenimiento: Some(mantenimiento(&infra)),
            lastfm: Some(lastfm),
            para_discord: Some(PiezasDeDiscord {
                albums,
                tracks: Arc::clone(&tracks),
                provider: Arc::clone(&provider),
            }),
            diagnostico: Diagnostico::real(&infra, ejecutor_ytdlp, cookies),
            bus,
        })
    }

    /// Cablea la aplicación **sin biblioteca**, para cuando la base de datos no
    /// abre o su esquema no es utilizable.
    ///
    /// ## Por qué se arranca igual
    ///
    /// Cerrarse dejaría al usuario delante de una ventana que desaparece, sin
    /// saber qué ha pasado ni dónde está su biblioteca. Abriendo, la pantalla de
    /// Ajustes sigue accesible —es donde está la ruta— y cada operación dice por
    /// qué no puede hacerse.
    ///
    /// ## Por qué ya no hay datos de ejemplo
    ///
    /// Aquí se cableaban trece servicios sobre un catálogo inventado, y el
    /// resultado era que abrir la aplicación con la base de datos rota enseñaba
    /// una biblioteca de Queen y Radiohead como si fuera la del usuario. Nada en
    /// la pantalla decía que no lo era. Un error se entiende; una biblioteca
    /// ajena, no.
    #[must_use]
    pub fn sin_biblioteca(bus: EventBus, ruta: std::path::PathBuf) -> Self {
        let publicador: Arc<dyn EventPublisher> = Arc::new(bus.clone());
        let inerte = Arc::new(SinBiblioteca::nuevo(ruta));

        Self {
            library: Arc::clone(&inerte) as _,
            search: Arc::clone(&inerte) as _,
            playback: Arc::new(SinAudio),
            queue: Arc::clone(&inerte) as _,
            downloads: Arc::clone(&inerte) as _,
            playlists: Arc::clone(&inerte) as _,
            recommendations: Arc::clone(&inerte) as _,
            lyrics: Arc::new(SinLetras),
            settings: Arc::clone(&inerte) as _,
            metadata: inerte as _,
            notifications: Arc::new(AvisosPorBus(publicador)),
            mantenimiento: None,
            lastfm: None,
            para_discord: None,
            diagnostico: Diagnostico::inerte(),
            bus,
        }
    }
}

/// Los repositorios que `real` reparte entre varios servicios.
///
/// Van juntos porque se construyen igual —todos sobre el mismo `Pool`— y porque
/// los cuatro los comparte más de un servicio. Los que usa uno solo se crean
/// dentro de su propia función de cableado, donde se leen al lado de quien los
/// necesita.
struct Repositorios {
    tracks: Arc<dyn localify_core::ports::database::TrackRepository>,
    albums: Arc<dyn localify_core::ports::database::AlbumRepository>,
    artists: Arc<localify_db::repositories::SqliteArtistRepository>,
    search: Arc<localify_db::repositories::SqliteSearchRepository>,
}

impl Repositorios {
    fn sobre(infra: &Infraestructura) -> Self {
        use localify_db::repositories::{
            SqliteAlbumRepository, SqliteArtistRepository, SqliteSearchRepository,
            SqliteTrackRepository,
        };

        Self {
            tracks: Arc::new(SqliteTrackRepository::new(infra.pool.clone())),
            albums: Arc::new(SqliteAlbumRepository::new(infra.pool.clone())),
            artists: Arc::new(SqliteArtistRepository::new(infra.pool.clone())),
            search: Arc::new(SqliteSearchRepository::new(infra.pool.clone())),
        }
    }
}

/// Cablea el servicio de metadatos.
///
/// Se saca de `real` porque su construcción tiene una vuelta de tuerca que
/// merece explicación propia, no porque sean muchas líneas.
#[allow(
    clippy::too_many_arguments,
    reason = "es cableado: cada argumento es una dependencia distinta"
)]
fn metadatos(
    infra: &Infraestructura,
    provider: &Arc<dyn localify_core::ports::metadata_provider::MetadataProvider>,
    tracks: &Arc<dyn localify_core::ports::database::TrackRepository>,
    albums: &Arc<dyn localify_core::ports::database::AlbumRepository>,
    artists: Arc<localify_db::repositories::SqliteArtistRepository>,
    publicador: &Arc<dyn EventPublisher>,
    imagenes: Option<Arc<dyn localify_core::ports::metadata_provider::ImageFetcher>>,
) -> Arc<localify_services::MetadataServiceImpl> {
    Arc::new(
        localify_services::MetadataServiceImpl::nuevo(
            Arc::clone(provider),
            Arc::clone(tracks),
            Arc::clone(albums),
            artists,
            Arc::clone(publicador),
            imagenes,
            Arc::clone(&infra.paths),
        )
        // Para la portada de las pistas sin álbum: sale de la miniatura del
        // vídeo que se emparejó.
        .con_emparejamientos(Arc::new(
            localify_db::repositories::SqliteYoutubeMatchRepository::new(infra.pool.clone()),
        )),
    )
}

/// Last.fm: la cola vive en la base de datos y las credenciales —clave, secreto
/// y sesión— en el almacén del sistema.
fn scrobbler(
    infra: &Infraestructura,
    tracks: &Arc<dyn localify_core::ports::database::TrackRepository>,
    settings: &Arc<dyn SettingsService>,
) -> Arc<localify_integrations::GestorLastfm> {
    Arc::new(localify_integrations::GestorLastfm::nuevo(
        localify_integrations::DependenciasLastfm {
            cola: Arc::new(localify_db::repositories::SqliteScrobbleRepository::new(
                infra.pool.clone(),
            )),
            tracks: Arc::clone(tracks),
            secretos: Arc::clone(&infra.secretos),
            ajustes: Arc::clone(settings),
        },
    ))
}

/// Construye los dos catálogos y el conmutador que los une.
///
/// Devuelve también el adaptador de Spotify concreto porque la importación de
/// playlists lo usa directamente: es una función suya, no del puerto.
///
/// # Errors
/// Si el transporte HTTP de Spotify no se puede construir.
async fn proveedores(
    infra: &Infraestructura,
) -> localify_core::error::CoreResult<(
    Arc<localify_spotify::provider::SpotifyProvider>,
    Arc<localify_services::proveedor::ProveedorConmutable>,
)> {
    let transporte = Arc::new(
        localify_spotify::TransporteHttp::nuevo()
            .map_err(|e| localify_core::error::CoreError::internal(Box::new(e)))?,
    );
    let spotify = Arc::new(localify_spotify::provider::SpotifyProvider::nuevo(
        transporte,
    ));
    spotify
        .set_credenciales(crate::credenciales::cargar(&infra.secretos).await)
        .await;

    // YouTube Music no necesita nada: ni clave, ni cuenta, ni configuración. Si
    // su cliente HTTP no se pudiera construir, se cae a Spotify, que al menos
    // sabe decir que no está configurado.
    //
    // ## Por qué `en`/`US` y no el idioma de la interfaz
    //
    // Ese par no es una preferencia de idioma: es lo que InnerTube usa para
    // **regionalizar los resultados**, y el efecto es enorme. Buscando "judas"
    // con `es`/`ES` no aparece la de Lady Gaga ni entre las veinte primeras —la
    // lista se llena de canciones en español que se llaman igual—; con `en`/`US`
    // es la primera. Comprobado con `--example explorar -- judas 20 en US`.
    //
    // El precio es el simétrico: el repertorio local queda peor servido. Se
    // acepta porque quien escribe el nombre de una canción concreta busca **esa**
    // canción, no lo que más suena en su país, y porque lo segundo tiene arreglo
    // —escribir el nombre del artista— y lo primero no.
    let ytmusic: Arc<dyn localify_core::ports::metadata_provider::MetadataProvider> =
        match localify_ytmusic::YtMusicProvider::nuevo("en", "US") {
            Ok(p) => Arc::new(p),
            Err(e) => {
                tracing::warn!(error = %e, "sin cliente para YouTube Music");
                Arc::clone(&spotify) as _
            }
        };

    // MusicBrainz tampoco pide nada. Si su cliente no se puede construir se cae
    // a YouTube Music: entonces el "combinado" combina uno con él mismo, que es
    // exactamente lo que la aplicación hacía antes de existir este catálogo.
    let musicbrainz: Arc<dyn localify_core::ports::metadata_provider::MetadataProvider> =
        match localify_musicbrainz::MusicBrainzProvider::nuevo() {
            Ok(p) => Arc::new(p),
            Err(e) => {
                tracing::warn!(error = %e, "sin cliente para MusicBrainz");
                Arc::clone(&ytmusic)
            }
        };

    // El combinado es un proveedor más, no un modo del conmutador: así el
    // conmutador sigue sabiendo solo elegir.
    let combinado: Arc<dyn localify_core::ports::metadata_provider::MetadataProvider> =
        Arc::new(localify_services::ProveedorCombinado::nuevo(
            Arc::clone(&ytmusic),
            Arc::clone(&musicbrainz),
        ));

    // El conmutador **es** el proveedor que ven todos los servicios: así,
    // cambiar de catálogo en Ajustes es escribir un valor y no reconstruir medio
    // contenedor de dependencias con la aplicación en marcha.
    let conmutador = Arc::new(localify_services::proveedor::ProveedorConmutable::nuevo(
        ytmusic,
        Arc::clone(&spotify) as _,
        musicbrainz,
        combinado,
        // El valor de verdad lo fija el servicio de ajustes al cargarse; aquí
        // solo hace falta uno para arrancar.
        localify_core::domain::settings::MetadataProviderKind::default(),
    ));

    Ok((spotify, conmutador))
}

/// Cablea el mantenimiento de la base de datos.
///
/// No es un servicio de dominio y ningún comando lo usa: lo consume la tarea de
/// fondo del arranque, que purga los metadatos sueltos de las búsquedas y
/// recorta el WAL.
fn mantenimiento(infra: &Infraestructura) -> Arc<dyn MaintenanceRepository> {
    Arc::new(localify_db::repositories::SqliteMaintenanceRepository::new(
        infra.pool.clone(),
    ))
}

/// Cablea la configuración persistente.
///
/// El motor de audio entra por parámetro porque también lo usa la reproducción:
/// abrirlo dos veces daría dos flujos WASAPI compitiendo por el mismo
/// dispositivo.
async fn ajustes(
    infra: &Infraestructura,
    publicador: &Arc<dyn EventPublisher>,
    motor: Option<Arc<dyn localify_core::ports::audio_engine::AudioEngine>>,
    crossfade: Arc<std::sync::atomic::AtomicU32>,
    cookies: Arc<CookiesVigentes>,
    spotify: &Arc<localify_spotify::provider::SpotifyProvider>,
    conmutador: &Arc<localify_services::proveedor::ProveedorConmutable>,
) -> Arc<dyn SettingsService> {
    Arc::new(
        localify_services::ajustes::SettingsServiceImpl::cargar(
            localify_services::ajustes::Dependencias {
                repo: Arc::new(localify_db::repositories::SqliteSettingsRepository::new(
                    infra.pool.clone(),
                )),
                secretos: Arc::clone(&infra.secretos),
                eventos: Arc::clone(publicador),
                paths: Arc::clone(&infra.paths),
                fs: Arc::new(localify_platform::RealFileSystem::new()),
                audio: motor,
                crossfade,
                cookies,
                locale: Arc::new(localify_platform::SystemLocale::new()),
                proveedor: Some(Arc::clone(conmutador)),
                spotify: Some(Arc::clone(spotify) as _),
            },
        )
        .await,
    )
}

/// Cablea las letras contra LRCLIB.
///
/// Si el cliente HTTP no se puede construir, la integración queda inerte y la
/// interfaz no muestra el panel. Es opcional por definición: no puede impedir
/// que la aplicación arranque.
fn letras(
    infra: &Infraestructura,
    tracks: &Arc<dyn localify_core::ports::database::TrackRepository>,
) -> Arc<dyn LyricsService> {
    match localify_integrations::LyricsServiceImpl::cliente() {
        Ok(http) => Arc::new(localify_integrations::LyricsServiceImpl::nuevo(
            localify_integrations::DependenciasLetras {
                repo: Arc::new(localify_db::repositories::SqliteLyricsRepository::new(
                    infra.pool.clone(),
                )),
                tracks: Arc::clone(tracks),
                http,
            },
        )),
        Err(e) => {
            tracing::warn!(error = %e, "sin cliente HTTP: no habrá letras");
            Arc::new(SinLetras)
        }
    }
}

/// Cablea el servicio de descargas con yt-dlp y FFmpeg.
///
/// Un solo `Ejecutor` para los tres binarios: salen de la misma carpeta y
/// comparten la política de lanzarse sin ventana de consola.
/// El `provider` entra por un motivo estrecho: preguntarle si ya sabe qué vídeo
/// de YouTube es cada pista. MusicBrainz lo sabe, y entonces no hay nada que
/// emparejar.
async fn descargas(
    infra: &Infraestructura,
    tracks: &Arc<dyn localify_core::ports::database::TrackRepository>,
    publicador: &Arc<dyn EventPublisher>,
    provider: &Arc<dyn localify_core::ports::metadata_provider::MetadataProvider>,
    cookies: &Arc<CookiesVigentes>,
    ejecutor: &Arc<dyn localify_ytdlp::proceso::Ejecutor>,
) -> Arc<localify_services::DownloadActor> {
    use localify_db::repositories::{
        SqliteAudioFileRepository, SqliteDownloadJobRepository, SqliteYoutubeMatchRepository,
    };

    let ejecutor = Arc::clone(ejecutor);
    let cliente = Arc::new(localify_ytdlp::ClienteYtDlp::nuevo(
        Arc::clone(&ejecutor),
        Arc::clone(cookies),
    ));

    let actor = Arc::new(localify_services::DownloadActor::arrancar(
        localify_services::DependenciasDescarga {
            matcher: Arc::new(localify_ytdlp::MatcherYtDlp::nuevo(Arc::clone(&cliente))),
            provider: Arc::clone(provider),
            downloader: Arc::new(localify_ytdlp::DescargadorYtDlp::nuevo(cliente, ejecutor)),
            tagger: Arc::new(localify_ytdlp::EtiquetadorLofty::nuevo()),
            tracks: Arc::clone(tracks),
            audio: Arc::new(SqliteAudioFileRepository::new(infra.pool.clone())),
            jobs: Arc::new(SqliteDownloadJobRepository::new(infra.pool.clone())),
            matches: Arc::new(SqliteYoutubeMatchRepository::new(infra.pool.clone())),
            fs: Arc::new(localify_platform::RealFileSystem::new()),
            paths: Arc::clone(&infra.paths),
            bus: Arc::clone(publicador),
            formato: localify_core::domain::settings::FormatPreference::default(),
            backoff: localify_services::BACKOFF_POR_DEFECTO.to_vec(),
        },
    ));

    // Lo que quedó a medias en la sesión anterior se descarta aquí, antes de
    // que nada pueda confundir un `.part` con biblioteca.
    match actor.limpiar_interrumpidos().await {
        Ok(0) => {}
        Ok(n) => tracing::info!(trabajos = n, "descargas interrumpidas descartadas"),
        Err(e) => tracing::warn!(error = %e, "no se pudo purgar .tmp/"),
    }
    actor
}

/// Cablea el servicio de biblioteca.
///
/// El etiquetador es el mismo que usa la descarga: el escaneo lo necesita para
/// identificar un fichero huérfano por su etiqueta (ADR-021), que es la vía que
/// sobrevive a un renombrado.
fn biblioteca(
    infra: &Infraestructura,
    tracks: &Arc<dyn localify_core::ports::database::TrackRepository>,
    publicador: &Arc<dyn EventPublisher>,
) -> Arc<dyn LibraryService> {
    use localify_db::repositories::{
        SqliteAlbumRepository, SqliteArtistRepository, SqliteAudioFileRepository,
        SqliteFavoriteRepository, SqliteHistoryRepository, SqlitePlayerStateRepository,
        SqliteScanReportRepository,
    };

    Arc::new(localify_services::LibraryServiceImpl::nuevo(
        localify_services::DependenciasBiblioteca {
            tracks: Arc::clone(tracks),
            albums: Arc::new(SqliteAlbumRepository::new(infra.pool.clone())),
            artists: Arc::new(SqliteArtistRepository::new(infra.pool.clone())),
            audio: Arc::new(SqliteAudioFileRepository::new(infra.pool.clone())),
            favoritos: Arc::new(SqliteFavoriteRepository::new(infra.pool.clone())),
            historial: Arc::new(SqliteHistoryRepository::new(infra.pool.clone())),
            informes: Arc::new(SqliteScanReportRepository::new(infra.pool.clone())),
            estado_repo: Arc::new(SqlitePlayerStateRepository::new(infra.pool.clone())),
            tagger: Arc::new(localify_ytdlp::EtiquetadorLofty::nuevo()),
            fs: Arc::new(localify_platform::RealFileSystem::new()),
            paths: Arc::clone(&infra.paths),
            bus: Arc::clone(publicador),
        },
    ))
}

/// Cablea el servicio de playlists.
///
/// El proveedor entra aquí porque importar una playlist pública es suyo. Que
/// no haya credenciales no impide nada: las playlists locales funcionan igual y
/// la importación devuelve un error accionable desde Ajustes.
/// Cablea las playlists.
///
/// Recibe el **conmutador**, no el adaptador de Spotify. Lo usa para una sola
/// cosa —importar una lista pública— y ahí el destino lo decide la URL que pegue
/// el usuario, no el catálogo activo: con Spotify concreto, un enlace de YouTube
/// Music no se podía importar de ninguna forma.
fn listas(
    infra: &Infraestructura,
    tracks: &Arc<dyn localify_core::ports::database::TrackRepository>,
    provider: &Arc<dyn localify_core::ports::metadata_provider::MetadataProvider>,
    imagenes: Option<&Arc<dyn localify_core::ports::metadata_provider::ImageFetcher>>,
    publicador: &Arc<dyn EventPublisher>,
    descargas: &Arc<localify_services::DownloadActor>,
) -> Arc<dyn PlaylistService> {
    use localify_db::repositories::{SqlitePlaylistRepository, SqliteSimilarityRepository};

    Arc::new(localify_services::PlaylistServiceImpl::nuevo(
        localify_services::DependenciasPlaylists {
            playlists: Arc::new(SqlitePlaylistRepository::new(infra.pool.clone())),
            tracks: Arc::clone(tracks),
            similitud: Arc::new(SqliteSimilarityRepository::new(infra.pool.clone())),
            provider: Arc::clone(provider),
            fs: Arc::new(localify_platform::RealFileSystem::new()),
            paths: Arc::clone(&infra.paths),
            bus: Arc::clone(publicador),
            descargas: Some(Arc::clone(descargas) as _),
            imagenes: imagenes.map(Arc::clone),
        },
    ))
}

/// El cliente HTTP de las imágenes, o nada.
///
/// Quedarse sin él no impide arrancar: se pierden las portadas y las fotos de
/// artista, y todo lo demás sigue igual.
fn descargador_de_imagenes()
-> Option<Arc<dyn localify_core::ports::metadata_provider::ImageFetcher>> {
    match localify_integrations::DescargadorDeImagenes::nuevo() {
        Ok(d) => Some(Arc::new(d)),
        Err(e) => {
            tracing::warn!(error = %e, "sin cliente HTTP: no habrá portadas");
            None
        }
    }
}

/// Abre el motor de audio, o se queda sin él.
///
/// Se abre aquí y no dentro de `reproduccion` porque lo comparten dos
/// consumidores: el actor de reproducción y el servicio de ajustes, que es quien
/// le fija el ecualizador y el dispositivo de salida.
///
/// **Sin dispositivo de audio la aplicación no se cae.** Un portátil con la
/// tarjeta deshabilitada debe poder abrir su biblioteca, buscar y ordenar
/// playlists; lo único que no hará es sonar.
type MotorYEventos = (
    Option<Arc<dyn localify_core::ports::audio_engine::AudioEngine>>,
    Option<localify_audio::engine::ReceptorEventos>,
);

fn abrir_motor() -> MotorYEventos {
    match localify_audio::engine::MotorAudio::arrancar() {
        Ok((m, e)) => {
            let motor: Arc<dyn localify_core::ports::audio_engine::AudioEngine> = Arc::new(m);
            (Some(motor), Some(e))
        }
        Err(e) => {
            tracing::warn!(error = %e, "sin dispositivo de audio: la reproduccion queda inactiva");
            (None, None)
        }
    }
}

/// Cablea las recomendaciones.
///
/// El criterio sale entero del historial y del catálogo local; el proveedor solo
/// se usa para preguntar "¿qué tienes de este artista?" con una semilla que
/// hemos elegido nosotros. Ver la cabecera de `recomendaciones.rs`.
fn recomendaciones(
    infra: &Infraestructura,
    tracks: &Arc<dyn localify_core::ports::database::TrackRepository>,
    provider: &Arc<dyn localify_core::ports::metadata_provider::MetadataProvider>,
) -> Arc<dyn RecommendationService> {
    use localify_db::repositories::{
        SqliteArtistRepository, SqliteCacheRepository, SqliteFavoriteRepository,
        SqliteHistoryRepository, SqlitePlaylistRepository, SqliteSimilarityRepository,
    };

    Arc::new(localify_services::RecommendationServiceImpl::nuevo(
        localify_services::DependenciasRecomendaciones {
            tracks: Arc::clone(tracks),
            artistas: Arc::new(SqliteArtistRepository::new(infra.pool.clone())),
            historial: Arc::new(SqliteHistoryRepository::new(infra.pool.clone())),
            favoritos: Arc::new(SqliteFavoriteRepository::new(infra.pool.clone())),
            playlists: Arc::new(SqlitePlaylistRepository::new(infra.pool.clone())),
            similitud: Arc::new(SqliteSimilarityRepository::new(infra.pool.clone())),
            provider: Arc::clone(provider),
            cache: Arc::new(SqliteCacheRepository::new(infra.pool.clone())),
        },
    ))
}

/// Cablea el reproductor sobre el motor de audio.
///
/// **Sin dispositivo de audio la aplicación no se cae.** Arranca sin
/// reproductor y todo lo demás —biblioteca, búsqueda, descargas— sigue
/// funcionando: un portátil con la tarjeta deshabilitada no debería impedir
/// organizar la música.
#[allow(
    clippy::too_many_arguments,
    reason = "es cableado: cada argumento es una dependencia distinta y agruparlas en una struct solo movería la lista de sitio"
)]
async fn reproduccion(
    infra: &Infraestructura,
    tracks: &Arc<dyn localify_core::ports::database::TrackRepository>,
    downloads: &Arc<localify_services::DownloadActor>,
    queue: &localify_services::QueueActor,
    publicador: &Arc<dyn EventPublisher>,
    motor: Option<Arc<dyn localify_core::ports::audio_engine::AudioEngine>>,
    eventos: Option<localify_audio::engine::ReceptorEventos>,
    crossfade: Arc<std::sync::atomic::AtomicU32>,
) -> Arc<dyn PlaybackService> {
    use localify_db::repositories::{
        SqliteAlbumRepository, SqliteFavoriteRepository, SqliteHistoryRepository,
        SqlitePlayerStateRepository, SqlitePlaylistRepository,
    };

    let (Some(motor), Some(eventos)) = (motor, eventos) else {
        return Arc::new(SinAudio);
    };

    let actor =
        localify_services::PlaybackActor::arrancar(localify_services::DependenciasReproduccion {
            motor,
            cola: queue.clone(),
            descargas: Arc::clone(downloads) as Arc<dyn DownloadService>,
            tracks: Arc::clone(tracks),
            albums: Arc::new(SqliteAlbumRepository::new(infra.pool.clone())),
            playlists: Arc::new(SqlitePlaylistRepository::new(infra.pool.clone())),
            favoritos: Arc::new(SqliteFavoriteRepository::new(infra.pool.clone())),
            historial: Arc::new(SqliteHistoryRepository::new(infra.pool.clone())),
            estado_repo: Arc::new(SqlitePlayerStateRepository::new(infra.pool.clone())),
            bus: Arc::clone(publicador),
            crossfade,
        });
    localify_services::conectar_eventos(&actor, Box::new(eventos));

    match actor.restaurar().await {
        Ok(true) => tracing::info!("sesion anterior restaurada"),
        Ok(false) => {}
        Err(e) => tracing::warn!(error = %e, "no se pudo restaurar la sesion"),
    }
    Arc::new(actor)
}

#[cfg(test)]
mod tests {
    use localify_core::domain::queue::PlaybackContext;
    use localify_core::domain::track::TrackFilter;
    use localify_core::page::PageRequest;

    use super::*;

    fn contexto() -> AppContext {
        AppContext::sin_biblioteca(EventBus::new(), std::path::PathBuf::from(r"D:\Musica"))
    }

    #[tokio::test]
    async fn sin_base_de_datos_la_biblioteca_falla_en_vez_de_inventar() {
        // Antes había aquí trece servicios sobre un catálogo de ejemplo, y este
        // mismo test exigía lo contrario: que la lista **no** viniera vacía,
        // "porque el frontend necesita algo que pintar". La consecuencia era que
        // abrir la aplicación con la base de datos rota enseñaba una biblioteca
        // de Queen y Radiohead como si fuera la del usuario.
        let ctx = contexto();
        let error = ctx
            .library
            .tracks(
                &TrackFilter::default(),
                localify_core::domain::track::TrackSort::TitleAsc,
                &PageRequest::new(0, 50),
            )
            .await
            .expect_err("no hay biblioteca que listar");

        assert_eq!(error.code(), "STORAGE");
    }

    #[tokio::test]
    async fn sin_base_de_datos_los_ajustes_siguen_diciendo_donde_esta_la_carpeta() {
        // Es lo único que se responde de verdad, y a propósito: es el dato que
        // el usuario necesita para ir a mirar qué le ha pasado a su música.
        let ctx = contexto();
        let ajustes = ctx.settings.get().await;
        assert_eq!(ajustes.library_path, std::path::PathBuf::from(r"D:\Musica"));
    }

    #[tokio::test]
    async fn sin_tarjeta_de_sonido_reproducir_falla_en_vez_de_fingir() {
        // Fingía: marcaba la canción como sonando y dejaba la barra a cero. El
        // usuario la veía puesta en el reproductor y no oía nada, sin ninguna
        // explicación en pantalla.
        let ctx = contexto();
        let error = ctx
            .playback
            .play_track(
                &localify_core::domain::ids::TrackId::nuevo_local(),
                PlaybackContext::Single,
            )
            .await
            .expect_err("no hay dispositivo de audio");

        assert_eq!(error.code(), "AUDIO");
    }

    #[tokio::test]
    async fn sin_tarjeta_de_sonido_el_reproductor_se_pinta_parado() {
        // El estado sí se responde: un error aquí dejaría la barra inferior rota
        // en vez de simplemente quieta.
        let ctx = contexto();
        let estado = ctx.playback.state().await;
        assert_eq!(
            estado.status,
            localify_core::domain::queue::PlayStatus::Stopped
        );
        assert!(estado.track.is_none());
    }
}

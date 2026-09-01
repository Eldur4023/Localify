//! Panel multimedia del sistema y teclas de medios.
//!
//! Es lo que hace que Localify aparezca en el panel que sale al pulsar
//! subir/bajar volumen en Windows, con su portada y sus botones, y que las
//! teclas de reproducción del teclado funcionen con la aplicación en segundo
//! plano.
//!
//! ## Por qué el interop y no la API de UWP
//!
//! `SystemMediaTransportControls::GetForCurrentView()` es la forma documentada
//! en casi todos los ejemplos, y **no funciona aquí**: exige una
//! `CoreApplicationView`, que solo existe dentro de una aplicación UWP. Una
//! aplicación de escritorio tiene una ventana Win32, no una vista.
//!
//! La vía correcta es `ISystemMediaTransportControlsInterop::GetForWindow`, que
//! ata los controles a un `HWND`. A cambio hay que activar la factoría de WinRT
//! a mano, que es lo que hace [`obtener_controles`].
//!
//! ## Sobre el hilo
//!
//! Los controles se atan a la ventana, y sus eventos llegan por COM al hilo que
//! la posee. En Tauri ese es el hilo principal, que ya bombea mensajes, así que
//! los `callbacks` llegan solos sin montar otro bucle.
//!
//! ## Linux: MPRIS
//!
//! `mod linux` implementa `org.mpris.MediaPlayer2` sobre D-Bus con
//! `mpris-server`. Vive al lado de `mod win` sin tocar una línea de negocio:
//! el resto de la aplicación solo conoce [`SystemMediaIntegration`].
//!
//! ## Fuera de Windows y Linux
//!
//! [`SinIntegracion`] no hace nada y la aplicación funciona igual.

// La usan `Manejador`, de SMTC y de MPRIS.
#[cfg(any(windows, target_os = "linux"))]
use std::sync::{Arc, Mutex};

use localify_core::domain::audio::DurationMs;
use localify_core::domain::queue::PlayStatus;
use localify_core::error::CoreResult;
use localify_core::ports::platform::{MediaCommand, NowPlaying, SystemMediaIntegration};

/// Implementación que no hace nada.
///
/// Es la de las plataformas sin panel multimedia, y también la de respaldo si
/// Windows rechaza los controles: **la reproducción nunca depende de esto**.
#[derive(Debug, Default)]
pub struct SinIntegracion;

#[async_trait::async_trait]
impl SystemMediaIntegration for SinIntegracion {
    async fn set_now_playing(&self, _info: &NowPlaying) -> CoreResult<()> {
        Ok(())
    }
    async fn set_status(&self, _status: PlayStatus) -> CoreResult<()> {
        Ok(())
    }
    async fn set_position(&self, _position: DurationMs, _duration: DurationMs) -> CoreResult<()> {
        Ok(())
    }
    async fn clear(&self) -> CoreResult<()> {
        Ok(())
    }
    fn set_command_handler(&self, _handler: Box<dyn Fn(MediaCommand) + Send + Sync>) {}
}

/// El receptor de las órdenes del sistema, compartido con el callback nativo
/// (COM en Windows, la tarea de D-Bus en Linux).
///
/// Solo en Windows y Linux: son los dos sistemas con panel multimedia. Fuera
/// de ahí la integración es [`SinIntegracion`], que no recibe órdenes de
/// nadie.
#[cfg(any(windows, target_os = "linux"))]
type Manejador = Arc<Mutex<Option<Box<dyn Fn(MediaCommand) + Send + Sync>>>>;

#[cfg(windows)]
mod win {
    use super::{Manejador, MediaCommand, NowPlaying, PlayStatus, SystemMediaIntegration};

    use std::sync::{Arc, Mutex};

    use localify_core::domain::audio::DurationMs;
    use localify_core::error::{CoreError, CoreResult};
    use tracing::{debug, warn};
    use windows::Foundation::TimeSpan;
    use windows::Media::{
        MediaPlaybackStatus, MediaPlaybackType, SystemMediaTransportControls,
        SystemMediaTransportControlsButton, SystemMediaTransportControlsButtonPressedEventArgs,
        SystemMediaTransportControlsTimelineProperties,
    };
    use windows::Storage::Streams::RandomAccessStreamReference;
    use windows::Win32::Foundation::HWND;
    use windows::Win32::System::WinRT::ISystemMediaTransportControlsInterop;
    use windows::core::HSTRING;

    /// Un `TimeSpan` cuenta en unidades de 100 ns.
    const POR_MILISEGUNDO: i64 = 10_000;

    /// Los controles multimedia de Windows.
    ///
    /// El puntero de COM no es `Send`, pero sí es seguro llamarlo desde otros
    /// hilos: `SystemMediaTransportControls` está marcado como agile
    /// (`IAgileObject`), así que WinRT hace el marshalling por su cuenta. Se
    /// declara explícitamente porque el compilador no puede saberlo.
    pub struct ControlesWindows {
        controles: SystemMediaTransportControls,
        manejador: Manejador,
    }

    // SAFETY: `SystemMediaTransportControls` implementa `IAgileObject`, lo que
    // significa que el propio WinRT garantiza el acceso desde cualquier
    // apartamento sin marshalling manual. Es la misma garantía en la que se
    // apoyan las implementaciones de C++/WinRT que actualizan el panel desde un
    // hilo de trabajo.
    unsafe impl Send for ControlesWindows {}
    // SAFETY: ídem; las llamadas son atómicas desde el punto de vista de COM.
    unsafe impl Sync for ControlesWindows {}

    impl std::fmt::Debug for ControlesWindows {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("ControlesWindows").finish_non_exhaustive()
        }
    }

    impl ControlesWindows {
        /// Ata los controles a una ventana.
        ///
        /// # Errors
        /// Si Windows no concede los controles. No es un fallo crítico: quien
        /// llama debe caer a [`super::SinIntegracion`] y seguir.
        pub fn nuevo(hwnd: isize) -> CoreResult<Self> {
            let controles = obtener_controles(hwnd)?;

            // Sin esto, el panel aparece pero con los botones apagados. Los
            // errores se ignoran uno a uno a propósito: que Windows rechace
            // habilitar "anterior" no es motivo para quedarse sin panel.
            let _ = controles.SetIsEnabled(true);
            let _ = controles.SetIsPlayEnabled(true);
            let _ = controles.SetIsPauseEnabled(true);
            let _ = controles.SetIsNextEnabled(true);
            let _ = controles.SetIsPreviousEnabled(true);

            let manejador: Manejador = Arc::new(Mutex::new(None));
            enganchar_botones(&controles, &manejador)?;

            Ok(Self {
                controles,
                manejador,
            })
        }
    }

    /// Activa la factoría de WinRT y pide los controles de esta ventana.
    fn obtener_controles(hwnd: isize) -> CoreResult<SystemMediaTransportControls> {
        // SAFETY: `factory` devuelve una interfaz válida o un error; `GetForWindow`
        // solo necesita que el HWND sea el de una ventana viva, cosa que
        // garantiza quien llama (se obtiene de la ventana ya creada por Tauri).
        unsafe {
            let interop: ISystemMediaTransportControlsInterop = windows::core::factory::<
                SystemMediaTransportControls,
                ISystemMediaTransportControlsInterop,
            >()
            .map_err(|e| CoreError::internal(format!("SMTC no disponible: {e}")))?;

            interop
                .GetForWindow::<SystemMediaTransportControls>(HWND(hwnd as *mut core::ffi::c_void))
                .map_err(|e| CoreError::internal(format!("SMTC rechazo la ventana: {e}")))
        }
    }

    /// Traduce los botones del panel a órdenes del dominio.
    fn enganchar_botones(
        controles: &SystemMediaTransportControls,
        manejador: &Manejador,
    ) -> CoreResult<()> {
        let destino = Arc::clone(manejador);

        let escuchador = windows::Foundation::TypedEventHandler::<
            SystemMediaTransportControls,
            SystemMediaTransportControlsButtonPressedEventArgs,
        >::new(move |_, args| {
            let Some(args) = args.as_ref() else {
                return Ok(());
            };
            let orden = match args.Button()? {
                SystemMediaTransportControlsButton::Play => MediaCommand::Play,
                SystemMediaTransportControlsButton::Pause => MediaCommand::Pause,
                SystemMediaTransportControlsButton::Stop => MediaCommand::Stop,
                SystemMediaTransportControlsButton::Next => MediaCommand::Next,
                SystemMediaTransportControlsButton::Previous => MediaCommand::Previous,
                otro => {
                    debug!(?otro, "boton multimedia sin uso");
                    return Ok(());
                }
            };

            // El manejador corre en un hilo de COM: debe ser breve y no
            // bloquear. El de arriba solo encola en un canal.
            if let Ok(g) = destino.lock()
                && let Some(f) = g.as_ref()
            {
                f(orden);
            }
            Ok(())
        });

        controles.ButtonPressed(&escuchador).map_err(|e| {
            CoreError::internal(format!("no se pudieron enganchar los botones: {e}"))
        })?;
        Ok(())
    }

    #[async_trait::async_trait]
    impl SystemMediaIntegration for ControlesWindows {
        async fn set_now_playing(&self, info: &NowPlaying) -> CoreResult<()> {
            let actualizador = self
                .controles
                .DisplayUpdater()
                .map_err(|e| CoreError::internal(e.to_string()))?;

            actualizador
                .SetType(MediaPlaybackType::Music)
                .map_err(|e| CoreError::internal(e.to_string()))?;

            let musica = actualizador
                .MusicProperties()
                .map_err(|e| CoreError::internal(e.to_string()))?;
            let _ = musica.SetTitle(&HSTRING::from(&info.title));
            let _ = musica.SetArtist(&HSTRING::from(&info.artist));
            if let Some(album) = &info.album {
                let _ = musica.SetAlbumTitle(&HSTRING::from(album));
            }

            // La portada tiene que ser un fichero: el panel del sistema no
            // acepta una URL ni un buffer en memoria.
            match &info.cover_path {
                Some(ruta) => {
                    let uri = windows::Foundation::Uri::CreateUri(&HSTRING::from(format!(
                        "file:///{}",
                        ruta.display().to_string().replace('\\', "/")
                    )));
                    if let Ok(uri) = uri
                        && let Ok(flujo) = RandomAccessStreamReference::CreateFromUri(&uri)
                    {
                        let _ = actualizador.SetThumbnail(&flujo);
                    }
                }
                None => {
                    let _ = actualizador.SetThumbnail(None);
                }
            }

            actualizador
                .Update()
                .map_err(|e| CoreError::internal(e.to_string()))?;
            Ok(())
        }

        async fn set_status(&self, status: PlayStatus) -> CoreResult<()> {
            let valor = match status {
                PlayStatus::Playing => MediaPlaybackStatus::Playing,
                PlayStatus::Paused => MediaPlaybackStatus::Paused,
                // "Cargando" es un estado del reproductor, no del panel: para
                // Windows sigue siendo una reproducción en curso, y parpadear
                // entre pausa y play en cada buffer quedaría fatal.
                PlayStatus::Buffering => MediaPlaybackStatus::Changing,
                PlayStatus::Stopped => MediaPlaybackStatus::Stopped,
            };
            self.controles
                .SetPlaybackStatus(valor)
                .map_err(|e| CoreError::internal(e.to_string()))?;
            Ok(())
        }

        async fn set_position(&self, position: DurationMs, duration: DurationMs) -> CoreResult<()> {
            let props = SystemMediaTransportControlsTimelineProperties::new()
                .map_err(|e| CoreError::internal(e.to_string()))?;

            let ts = |ms: u32| TimeSpan {
                Duration: i64::from(ms) * POR_MILISEGUNDO,
            };
            let _ = props.SetStartTime(ts(0));
            let _ = props.SetMinSeekTime(ts(0));
            let _ = props.SetPosition(ts(position.as_ms()));
            let _ = props.SetMaxSeekTime(ts(duration.as_ms()));
            let _ = props.SetEndTime(ts(duration.as_ms()));

            self.controles
                .UpdateTimelineProperties(&props)
                .map_err(|e| CoreError::internal(e.to_string()))?;
            Ok(())
        }

        async fn clear(&self) -> CoreResult<()> {
            if let Ok(u) = self.controles.DisplayUpdater() {
                let _ = u.ClearAll();
                let _ = u.Update();
            }
            let _ = self
                .controles
                .SetPlaybackStatus(MediaPlaybackStatus::Stopped);
            Ok(())
        }

        fn set_command_handler(&self, handler: Box<dyn Fn(MediaCommand) + Send + Sync>) {
            match self.manejador.lock() {
                Ok(mut g) => *g = Some(handler),
                Err(e) => warn!(error = %e, "no se pudo instalar el manejador multimedia"),
            }
        }
    }
}

#[cfg(windows)]
pub use win::ControlesWindows;

#[cfg(target_os = "linux")]
mod linux {
    use super::{Manejador, MediaCommand, NowPlaying, PlayStatus, SystemMediaIntegration};

    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};

    use localify_core::domain::audio::DurationMs;
    use localify_core::error::{CoreError, CoreResult};
    use mpris_server::{
        LoopStatus, Metadata, PlaybackRate, PlaybackStatus, PlayerInterface, Property,
        RootInterface, Server, Signal, Time, TrackId, Volume, zbus,
    };
    use tracing::warn;

    /// Identifica a Localify ante los clientes de MPRIS.
    const IDENTIDAD: &str = "Localify";
    /// Basename del `.desktop` que instala el paquete (ADR de empaquetado
    /// Linux): lo que un cliente usa para encontrar el icono.
    const ENTRADA_ESCRITORIO: &str = "localify";

    /// Lo que está sonando ahora mismo, tal y como lo dejó la última llamada a
    /// [`SystemMediaIntegration::set_now_playing`].
    #[derive(Debug, Clone)]
    struct Pista {
        titulo: String,
        artista: String,
        album: Option<String>,
        cover_path: Option<PathBuf>,
        duracion_ms: u32,
        /// Identificador sintético: `NowPlaying` no trae uno propio, y MPRIS
        /// exige un `TrackId` estable para poder rechazar un `SetPosition`
        /// que llega tarde y ya no habla de la pista actual.
        id: u64,
    }

    /// Lo que la interfaz D-Bus necesita leer y escribir, compartido entre el
    /// [`Server`] y el [`IntegracionMpris`] que lo expone al resto de la
    /// aplicación.
    struct Estado {
        pista: Mutex<Option<Pista>>,
        posicion_ms: AtomicU32,
        reproduccion: Mutex<PlaybackStatus>,
        siguiente_id: AtomicU64,
        manejador: Manejador,
    }

    impl std::fmt::Debug for Estado {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("Estado").finish_non_exhaustive()
        }
    }

    impl Estado {
        fn nuevo() -> Self {
            Self {
                pista: Mutex::new(None),
                posicion_ms: AtomicU32::new(0),
                reproduccion: Mutex::new(PlaybackStatus::Stopped),
                siguiente_id: AtomicU64::new(0),
                manejador: Arc::new(Mutex::new(None)),
            }
        }

        fn track_id_de(id: u64) -> TrackId {
            TrackId::try_from(format!("/org/localify/Track/{id}")).unwrap_or(TrackId::NO_TRACK)
        }

        /// Metadatos MPRIS de la pista actual, o los de "no hay pista".
        fn metadata(&self) -> Metadata {
            let pista = self
                .pista
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some(p) = pista.as_ref() else {
                return Metadata::builder().trackid(TrackId::NO_TRACK).build();
            };

            let mut b = Metadata::builder()
                .trackid(Self::track_id_de(p.id))
                .title(p.titulo.clone())
                .artist(vec![p.artista.clone()])
                .length(Time::from_millis(i64::from(p.duracion_ms)));
            if let Some(album) = &p.album {
                b = b.album(album.clone());
            }
            if let Some(ruta) = &p.cover_path {
                // Igual que en Windows: el panel necesita un fichero, no un
                // buffer en memoria. A diferencia de Windows, aquí el propio
                // URI de fichero es lo que MPRIS espera, sin pasar por
                // ninguna API de flujos.
                b = b.art_url(format!("file://{}", ruta.display()));
            }
            b.build()
        }

        /// Envía la orden al manejador instalado, si lo hay. Nunca bloquea el
        /// bus de D-Bus más de lo que tarda encolar.
        fn enviar(&self, orden: MediaCommand) {
            if let Ok(g) = self.manejador.lock()
                && let Some(f) = g.as_ref()
            {
                f(orden);
            }
        }
    }

    /// La implementación de las dos interfaces de MPRIS. Solo lee y escribe
    /// [`Estado`]; no habla con el reproductor directamente (ADR-008: el
    /// manejador es lo único que cruza esa frontera).
    #[derive(Debug)]
    struct Implementacion {
        estado: Arc<Estado>,
    }

    // La firma de cada método la dicta el trait de `mpris-server`, que exige
    // `async fn` (es la interfaz de D-Bus, no una elección nuestra). La
    // mayoría no necesita esperar nada: son datos que Localify no expone por
    // este panel (fullscreen, listas de reproducción) o valores fijos.
    #[allow(clippy::unused_async_trait_impl)]
    impl RootInterface for Implementacion {
        async fn raise(&self) -> zbus::fdo::Result<()> {
            Ok(())
        }

        async fn quit(&self) -> zbus::fdo::Result<()> {
            Ok(())
        }

        async fn can_quit(&self) -> zbus::fdo::Result<bool> {
            Ok(false)
        }

        async fn fullscreen(&self) -> zbus::fdo::Result<bool> {
            Ok(false)
        }

        async fn set_fullscreen(&self, _fullscreen: bool) -> zbus::Result<()> {
            Ok(())
        }

        async fn can_set_fullscreen(&self) -> zbus::fdo::Result<bool> {
            Ok(false)
        }

        async fn can_raise(&self) -> zbus::fdo::Result<bool> {
            Ok(false)
        }

        async fn has_track_list(&self) -> zbus::fdo::Result<bool> {
            Ok(false)
        }

        async fn identity(&self) -> zbus::fdo::Result<String> {
            Ok(IDENTIDAD.to_owned())
        }

        async fn desktop_entry(&self) -> zbus::fdo::Result<String> {
            Ok(ENTRADA_ESCRITORIO.to_owned())
        }

        async fn supported_uri_schemes(&self) -> zbus::fdo::Result<Vec<String>> {
            Ok(Vec::new())
        }

        async fn supported_mime_types(&self) -> zbus::fdo::Result<Vec<String>> {
            Ok(Vec::new())
        }
    }

    // Mismo motivo que en `RootInterface`: la firma la impone el trait, y
    // aquí casi todo es leer un `Mutex`/`Atomic` o encolar en el manejador,
    // sin nada que esperar de verdad.
    #[allow(clippy::unused_async_trait_impl)]
    impl PlayerInterface for Implementacion {
        async fn next(&self) -> zbus::fdo::Result<()> {
            self.estado.enviar(MediaCommand::Next);
            Ok(())
        }

        async fn previous(&self) -> zbus::fdo::Result<()> {
            self.estado.enviar(MediaCommand::Previous);
            Ok(())
        }

        async fn pause(&self) -> zbus::fdo::Result<()> {
            self.estado.enviar(MediaCommand::Pause);
            Ok(())
        }

        async fn play_pause(&self) -> zbus::fdo::Result<()> {
            self.estado.enviar(MediaCommand::Toggle);
            Ok(())
        }

        async fn stop(&self) -> zbus::fdo::Result<()> {
            self.estado.enviar(MediaCommand::Stop);
            Ok(())
        }

        async fn play(&self) -> zbus::fdo::Result<()> {
            self.estado.enviar(MediaCommand::Play);
            Ok(())
        }

        async fn seek(&self, offset: Time) -> zbus::fdo::Result<()> {
            let actual = i64::from(self.estado.posicion_ms.load(Ordering::Relaxed));
            let duracion = self
                .estado
                .pista
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_ref()
                .map_or(0, |p| i64::from(p.duracion_ms));

            let destino = actual + offset.as_millis();
            if destino >= duracion {
                // "acts like a call to Next" (especificación de MPRIS).
                self.estado.enviar(MediaCommand::Next);
            } else {
                let posicion = u32::try_from(destino.max(0)).unwrap_or(0);
                self.estado.enviar(MediaCommand::Seek {
                    position_ms: posicion,
                });
            }
            Ok(())
        }

        async fn set_position(&self, track_id: TrackId, position: Time) -> zbus::fdo::Result<()> {
            let vigente = self
                .estado
                .pista
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_ref()
                .map(|p| Estado::track_id_de(p.id));
            // Un `SetPosition` que ya no habla de la pista actual llega
            // tarde: se ignora en vez de mover una pista que el usuario ya
            // dejó atrás.
            if vigente != Some(track_id) {
                return Ok(());
            }
            let ms = u32::try_from(position.as_millis().max(0)).unwrap_or(0);
            self.estado.enviar(MediaCommand::Seek { position_ms: ms });
            Ok(())
        }

        async fn open_uri(&self, _uri: String) -> zbus::fdo::Result<()> {
            Err(zbus::fdo::Error::NotSupported(
                "Localify no acepta URIs externas".to_owned(),
            ))
        }

        async fn playback_status(&self) -> zbus::fdo::Result<PlaybackStatus> {
            Ok(*self
                .estado
                .reproduccion
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner))
        }

        async fn loop_status(&self) -> zbus::fdo::Result<LoopStatus> {
            // Localify no expone el modo de repetición a las integraciones
            // del sistema (tampoco lo hace SMTC): es un ajuste de la propia
            // interfaz, no un control de panel externo.
            Ok(LoopStatus::None)
        }

        async fn set_loop_status(&self, _loop_status: LoopStatus) -> zbus::Result<()> {
            Ok(())
        }

        async fn rate(&self) -> zbus::fdo::Result<PlaybackRate> {
            Ok(1.0)
        }

        async fn set_rate(&self, _rate: PlaybackRate) -> zbus::Result<()> {
            Ok(())
        }

        async fn shuffle(&self) -> zbus::fdo::Result<bool> {
            Ok(false)
        }

        async fn set_shuffle(&self, _shuffle: bool) -> zbus::Result<()> {
            Ok(())
        }

        async fn metadata(&self) -> zbus::fdo::Result<Metadata> {
            Ok(self.estado.metadata())
        }

        async fn volume(&self) -> zbus::fdo::Result<Volume> {
            // Igual que el modo de repetición: el volumen del sistema no
            // gobierna el de Localify, así que no hay nada real que leer.
            Ok(1.0)
        }

        async fn set_volume(&self, _volume: Volume) -> zbus::Result<()> {
            Ok(())
        }

        async fn position(&self) -> zbus::fdo::Result<Time> {
            Ok(Time::from_millis(i64::from(
                self.estado.posicion_ms.load(Ordering::Relaxed),
            )))
        }

        async fn minimum_rate(&self) -> zbus::fdo::Result<PlaybackRate> {
            Ok(1.0)
        }

        async fn maximum_rate(&self) -> zbus::fdo::Result<PlaybackRate> {
            Ok(1.0)
        }

        async fn can_go_next(&self) -> zbus::fdo::Result<bool> {
            Ok(true)
        }

        async fn can_go_previous(&self) -> zbus::fdo::Result<bool> {
            Ok(true)
        }

        async fn can_play(&self) -> zbus::fdo::Result<bool> {
            Ok(true)
        }

        async fn can_pause(&self) -> zbus::fdo::Result<bool> {
            Ok(true)
        }

        async fn can_seek(&self) -> zbus::fdo::Result<bool> {
            Ok(true)
        }

        async fn can_control(&self) -> zbus::fdo::Result<bool> {
            Ok(true)
        }
    }

    /// La integración MPRIS completa: el servidor D-Bus más el estado que
    /// alimenta sus respuestas.
    #[derive(Debug)]
    pub struct IntegracionMpris {
        estado: Arc<Estado>,
        servidor: Server<Implementacion>,
    }

    impl IntegracionMpris {
        /// Publica el nombre de bus `org.mpris.MediaPlayer2.localify.instance{pid}`.
        ///
        /// El sufijo lleva el PID porque la especificación exige un
        /// identificador único por instancia: dos Localify abiertos a la vez
        /// (dos usuarios, o una sesión de depuración junto a la real) no
        /// pueden competir por el mismo nombre.
        ///
        /// # Errors
        /// Si D-Bus no está disponible o rechaza el nombre. No es un fallo
        /// crítico: quien llama debe caer a [`super::SinIntegracion`] y
        /// seguir.
        pub async fn nuevo() -> CoreResult<Self> {
            let estado = Arc::new(Estado::nuevo());
            let servidor = Server::new(
                &format!("localify.instance{}", std::process::id()),
                Implementacion {
                    estado: Arc::clone(&estado),
                },
            )
            .await
            .map_err(|e| CoreError::internal(format!("MPRIS no disponible: {e}")))?;

            Ok(Self { estado, servidor })
        }
    }

    #[async_trait::async_trait]
    impl SystemMediaIntegration for IntegracionMpris {
        async fn set_now_playing(&self, info: &NowPlaying) -> CoreResult<()> {
            let id = self.estado.siguiente_id.fetch_add(1, Ordering::Relaxed);
            {
                let mut pista = self
                    .estado
                    .pista
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                *pista = Some(Pista {
                    titulo: info.title.clone(),
                    artista: info.artist.clone(),
                    album: info.album.clone(),
                    cover_path: info.cover_path.clone(),
                    duracion_ms: info.duration.as_ms(),
                    id,
                });
            }
            self.estado.posicion_ms.store(0, Ordering::Relaxed);

            self.servidor
                .properties_changed([Property::Metadata(self.estado.metadata())])
                .await
                .map_err(|e| CoreError::internal(e.to_string()))?;
            Ok(())
        }

        async fn set_status(&self, status: PlayStatus) -> CoreResult<()> {
            let valor = match status {
                PlayStatus::Playing => PlaybackStatus::Playing,
                PlayStatus::Paused => PlaybackStatus::Paused,
                // MPRIS no distingue "cargando": para el cliente sigue siendo
                // una reproducción en curso (mismo criterio que SMTC).
                PlayStatus::Buffering => PlaybackStatus::Playing,
                PlayStatus::Stopped => PlaybackStatus::Stopped,
            };
            *self
                .estado
                .reproduccion
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = valor;

            self.servidor
                .properties_changed([Property::PlaybackStatus(valor)])
                .await
                .map_err(|e| CoreError::internal(e.to_string()))?;
            Ok(())
        }

        async fn set_position(&self, position: DurationMs, duration: DurationMs) -> CoreResult<()> {
            self.estado
                .posicion_ms
                .store(position.as_ms(), Ordering::Relaxed);
            if let Some(p) = self
                .estado
                .pista
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_mut()
            {
                p.duracion_ms = duration.as_ms();
            }

            self.servidor
                .emit(Signal::Seeked {
                    position: Time::from_millis(i64::from(position.as_ms())),
                })
                .await
                .map_err(|e| CoreError::internal(e.to_string()))?;
            Ok(())
        }

        async fn clear(&self) -> CoreResult<()> {
            *self
                .estado
                .pista
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
            self.estado.posicion_ms.store(0, Ordering::Relaxed);
            *self
                .estado
                .reproduccion
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = PlaybackStatus::Stopped;

            self.servidor
                .properties_changed([
                    Property::Metadata(self.estado.metadata()),
                    Property::PlaybackStatus(PlaybackStatus::Stopped),
                ])
                .await
                .map_err(|e| CoreError::internal(e.to_string()))?;
            Ok(())
        }

        fn set_command_handler(&self, handler: Box<dyn Fn(MediaCommand) + Send + Sync>) {
            match self.estado.manejador.lock() {
                Ok(mut g) => *g = Some(handler),
                Err(e) => warn!(error = %e, "no se pudo instalar el manejador multimedia (MPRIS)"),
            }
        }
    }
}

#[cfg(target_os = "linux")]
pub use linux::IntegracionMpris;

/// Construye la integración multimedia para esta ventana.
///
/// Devuelve [`SinIntegracion`] si el sistema no la concede. **No falla**: que
/// el panel del sistema no aparezca no es motivo para que la música no
/// suene.
///
/// `hwnd` solo lo usa Windows; en Linux, MPRIS no se ata a ninguna ventana.
#[must_use]
pub async fn integracion(hwnd: isize) -> std::sync::Arc<dyn SystemMediaIntegration> {
    #[cfg(windows)]
    {
        match win::ControlesWindows::nuevo(hwnd) {
            Ok(c) => return std::sync::Arc::new(c),
            Err(e) => {
                tracing::warn!(error = %e, "sin panel multimedia del sistema");
            }
        }
    }
    #[cfg(not(windows))]
    let _ = hwnd;

    #[cfg(target_os = "linux")]
    {
        match linux::IntegracionMpris::nuevo().await {
            Ok(i) => return std::sync::Arc::new(i),
            Err(e) => {
                tracing::warn!(error = %e, "sin MPRIS: panel multimedia no disponible");
            }
        }
    }

    std::sync::Arc::new(SinIntegracion)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn la_implementacion_nula_acepta_todo_sin_fallar() {
        // Es la que corre en cualquier plataforma sin panel multimedia. Si
        // fallara, la reproduccion dependeria de una integracion opcional.
        let s = SinIntegracion;
        s.set_status(PlayStatus::Playing).await.expect("estado");
        s.set_position(DurationMs::new(1000), DurationMs::new(2000))
            .await
            .expect("posicion");
        s.clear().await.expect("limpia");
        s.set_command_handler(Box::new(|_| {}));

        s.set_now_playing(&NowPlaying {
            title: "Bohemian Rhapsody".into(),
            artist: "Queen".into(),
            album: None,
            duration: DurationMs::from_secs(354),
            cover_path: None,
        })
        .await
        .expect("metadatos");
    }

    #[tokio::test]
    async fn una_ventana_invalida_no_tumba_la_aplicacion() {
        // Un HWND que no existe debe degradar a la implementacion nula, no
        // entrar en panico: es exactamente lo que pasaria si la ventana se
        // cerrara entre que se pide y se ata.
        let _ = integracion(0).await;
    }
}

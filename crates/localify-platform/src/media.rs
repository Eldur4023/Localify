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
//! ## Fuera de Windows
//!
//! [`SinIntegracion`] no hace nada y la aplicación funciona igual. Portar a
//! Linux es escribir MPRIS aquí al lado, sin tocar una línea de negocio.

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

/// El receptor de las órdenes del sistema, compartido con el callback de COM.
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

/// Construye la integración multimedia para esta ventana.
///
/// Devuelve [`SinIntegracion`] si el sistema no la concede. **No falla**: que
/// el panel de Windows no aparezca no es motivo para que la música no suene.
#[must_use]
pub fn integracion(hwnd: isize) -> std::sync::Arc<dyn SystemMediaIntegration> {
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

    #[test]
    fn una_ventana_invalida_no_tumba_la_aplicacion() {
        // Un HWND que no existe debe degradar a la implementacion nula, no
        // entrar en panico: es exactamente lo que pasaria si la ventana se
        // cerrara entre que se pide y se ata.
        let _ = integracion(0);
    }
}

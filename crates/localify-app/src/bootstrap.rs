//! Arranque por etapas.
//!
//! El objetivo es "ventana visible cuanto antes". Nada que pueda esperar
//! bloquea la aparición de la interfaz: los sidecars, la purga de temporales y
//! el mantenimiento de la base de datos se lanzan como tareas de fondo una vez
//! la ventana ya está en pantalla.
//!
//! ## `--headless`
//!
//! Cerrar la ventana **nunca** mata el proceso: la reproducción, MPRIS, SMTC
//! y la API de control siguen vivos sin ella (ver el manejo de
//! `RunEvent::ExitRequested` al final de [`run`]). `--headless` solo decide
//! si esa ventana se crea al arrancar; `--quit` y un lanzamiento sin
//! argumentos, dirigidos a una instancia que ya está en marcha, son las dos
//! formas de hablarle desde fuera (ver [`avisar_a_la_instancia_en_marcha`]).

use std::sync::Arc;

use localify_core::events::EventPublisher;
use localify_core::ports::platform::AppPaths;
use localify_platform::{LocalifyPaths, adquirir_instancia};
use tracing::{debug, error, info, warn};

/// Los dos únicos flags que entiende la línea de órdenes.
///
/// Nada de una dependencia para dos banderas: `--headless` arranca sin
/// ventana, `--quit` le pide a la instancia que ya corre que cierre de
/// verdad. Cualquier otro argumento se ignora en silencio.
struct Argumentos {
    headless: bool,
    quit: bool,
}

impl Argumentos {
    fn desde_entorno() -> Self {
        let mut headless = false;
        let mut quit = false;
        for arg in std::env::args().skip(1) {
            match arg.as_str() {
                "--headless" => headless = true,
                "--quit" => quit = true,
                _ => {}
            }
        }
        Self { headless, quit }
    }
}

/// Arranca la aplicación.
#[allow(
    clippy::too_many_lines,
    reason = "cablea una docena de tareas de fondo independientes; partirla oscurecería el orden de arranque, no lo aclararía"
)]
pub fn run() {
    let inicio = std::time::Instant::now();
    let args = Argumentos::desde_entorno();

    let paths = match LocalifyPaths::detectar() {
        Ok(p) => p,
        Err(e) => {
            // Único uso justificado de `eprintln!` en todo el proyecto: sin
            // rutas no hay carpeta de logs, así que `tracing` todavía no
            // existe. Es esto o fallar en silencio.
            #[allow(clippy::print_stderr, reason = "el logging aún no está inicializado")]
            {
                eprintln!("Localify no pudo determinar sus carpetas de datos: {e}");
            }
            std::process::exit(1);
        }
    };

    let _log_guard = crate::logging::init(&paths.logs_dir());
    info!(version = env!("CARGO_PKG_VERSION"), "iniciando Localify");

    if let Err(e) = paths.crear_estructura() {
        error!(error = %e, "no se pudo crear la estructura de carpetas");
        std::process::exit(1);
    }

    // Dos instancias compartiendo base de datos y dispositivo de audio no es un
    // escenario a soportar. Si ya hay una, esta invocación no arranca nada
    // propio: le pasa el aviso y termina.
    let Some(_instancia) = adquirir_o_avisar(&args) else {
        return;
    };

    info!(
        config = %paths.config_dir().display(),
        library = %paths.library_dir().display(),
        "rutas resueltas"
    );

    let bus = crate::bridge::EventBus::new();
    let contexto = construir_contexto(&paths, &bus);

    let playback = Arc::clone(&contexto.playback);
    let metadata_smtc = Arc::clone(&contexto.metadata);
    let contexto_portadas = contexto.clone();
    let mantenimiento = contexto.mantenimiento.clone();
    let ajustes_integraciones = Arc::clone(&contexto.settings);
    let playback_discord = Arc::clone(&contexto.playback);
    let metadata_discord = Arc::clone(&contexto.metadata);
    let piezas_discord = contexto.para_discord.clone();
    let binarios_ytdlp = paths.binaries_dir();
    let actualizacion_disponible = Arc::clone(&contexto.actualizacion_disponible);
    let eventos_actualizacion: Arc<dyn EventPublisher> = Arc::new(bus.clone());
    let playback_control_api = Arc::clone(&contexto.playback);
    let headless = args.headless;

    let builder = crate::registrar_comandos!(tauri::Builder::default())
        // Las portadas se sirven por su propio esquema en vez de por el
        // protocolo `asset:`, que obligaría a mandar rutas de disco al frontend
        // y a configurar un ámbito de ficheros. Aquí el frontend solo conoce el
        // identificador del álbum: la ruta no cruza el puente.
        //
        // La descarga es perezosa: la primera vez que alguien mira una portada
        // se baja y se cachea; las siguientes salen del disco. Una búsqueda trae
        // veinte álbumes y casi ninguno se abre, así que bajarlas todas al
        // guardarlas sería tráfico para imágenes que nadie va a ver.
        .register_asynchronous_uri_scheme_protocol("cover", move |_app, peticion, responder| {
            let ctx = contexto_portadas.clone();
            tauri::async_runtime::spawn(async move {
                responder.respond(portada(&ctx, peticion.uri().path()).await);
            });
        })
        // Solo para el selector de carpetas de Ajustes, y solo desde Rust: el
        // frontend no importa nada del plugin (ver `settings_pick_folder`).
        .plugin(tauri_plugin_dialog::init())
        .manage(contexto);

    builder
        .setup(move |app| {
            // El puente traduce eventos del dominio y los emite al WebView.
            crate::bridge::arrancar(app.handle().clone(), &bus);

            // `--headless`: no se crea ninguna ventana. `mostrar_ventana`
            // sabe crearla bajo demanda —desde `/window/show`— con la misma
            // configuración declarativa de `tauri.conf.json`, así que no
            // arrancar aquí no es un caso especial, es simplemente no llamar
            // a la misma función todavía.
            if !headless && let Err(e) = mostrar_ventana(app.handle()) {
                error!(error = %e, "no se pudo crear la ventana principal");
            }

            // Arranca siempre, con o sin ventana: en Linux, MPRIS no depende
            // de ninguna; en Windows, sin HWND —`--headless`, o la ventana
            // se cerró antes de llegar aquí, que no puede pasar pero da
            // igual— `integracion_multimedia` cae sola a `SinIntegracion`.
            // Antes esto se saltaba entero sin ventana en Windows, y con
            // ello se perdía SMTC para el resto de la sesión aunque
            // `/window/show` la creara después.
            let hwnd = hwnd_principal(app);
            tauri::async_runtime::spawn(crate::multimedia::arrancar(
                hwnd.unwrap_or(0),
                playback,
                metadata_smtc,
                bus.clone(),
            ));

            abrir_devtools(app);

            // Única señal de que Localify sigue vivo sin ventana: cerrarla ya
            // no mata el proceso (ver `RunEvent::ExitRequested` más abajo), y
            // sin esto no habría forma de saberlo ni de recuperarlo salvo por
            // `localify --quit`. Si el icono no se puede crear —sistema sin
            // bandeja, o sin `libayatana-appindicator3` en Linux— se avisa y
            // se sigue: no es motivo para no arrancar.
            if let Err(e) = arrancar_bandeja(app) {
                warn!(error = %e, "sin icono de bandeja");
            }

            info!(ms = inicio.elapsed().as_millis(), "ventana lista");

            if let Some(repo) = mantenimiento {
                tauri::async_runtime::spawn(mantener(repo));
            }

            // yt-dlp se pone al día en paralelo, sin bloquear nada.
            //
            // Se rompe cuando YouTube cambia, cada pocas semanas, y hasta ahora
            // nadie lo actualizaba: el binario se quedaba con la versión del día
            // que se instaló. Meses después, las descargas empiezan a fallar sin
            // motivo aparente, que es exactamente el síntoma que había.
            //
            // Va en una tarea propia y no en el arranque porque comprobar tarda
            // un segundo y descargar puede tardar veinte, y ninguna de las dos
            // cosas debe retrasar la ventana. Si una descarga ocurre mientras
            // tanto, usa el binario viejo: reemplazarlo no afecta a un proceso
            // que ya está en marcha.
            tauri::async_runtime::spawn(poner_al_dia(binarios_ytdlp));

            // Aviso de nuevas versiones. Una comprobación por arranque, igual
            // que yt-dlp: comprobar tarda un segundo y no vale la pena
            // mantener una tarea despierta para volver a preguntar algo que
            // no cambia en el rato que dura una sesión.
            tauri::async_runtime::spawn(comprobar_actualizaciones(
                eventos_actualizacion,
                actualizacion_disponible,
            ));

            // Se engancha al bus y no se le vuelve a hablar. Arranca aunque esté
            // desactivada: comprueba su ajuste al recibir un evento, así que
            // encenderla surte efecto en la siguiente canción y no al reiniciar.
            // La alternativa —arrancar y parar la tarea al tocar el
            // interruptor— sería estado que mantener a cambio de ahorrar una
            // tarea dormida.
            //
            // Sin catálogo no arranca: en modo degradado no hay biblioteca que
            // anunciar, así que la tarea solo serviría para dormir.
            if let Some(piezas) = piezas_discord {
                tauri::async_runtime::spawn(localify_integrations::discord::atender(
                    localify_integrations::DependenciasDiscord {
                        playback: playback_discord,
                        ajustes: ajustes_integraciones,
                        albums: piezas.albums,
                        metadata: metadata_discord,
                        tracks: piezas.tracks,
                        provider: piezas.provider,
                    },
                    bus.subscribe(),
                ));
            }

            // API de control local para procesos externos (scripts, mandos
            // físicos, atajos personalizados): pausar, reanudar, saltar de
            // pista y leer el estado por HTTP en 127.0.0.1. Igual que las
            // demás integraciones, si no puede arrancar —el puerto ya está
            // en uso— se avisa y la reproducción sigue igual. También es
            // quien atiende `/window/show` y `/app/quit`: por eso necesita el
            // `AppHandle`, no solo el reproductor.
            tauri::async_runtime::spawn(crate::control_api::arrancar(
                playback_control_api,
                app.handle().clone(),
            ));

            Ok(())
        })
        .build(tauri::generate_context!())
        .unwrap_or_else(|e| {
            error!(error = %e, "fallo irrecuperable de Tauri");
            std::process::exit(1);
        })
        .run(|_app_handle, event| {
            // Cerrar la última ventana dispara esto con `code: None`. Sin
            // interceptarlo, Tauri sale del proceso entero —adiós motor de
            // audio, MPRIS y la API de control— por el gesto de cerrar un
            // WebView. `code` viene `Some(_)` solo cuando alguien pidió salir
            // de verdad con `AppHandle::exit` (nuestro `/app/quit`), y ahí sí
            // se deja seguir.
            if let tauri::RunEvent::ExitRequested { code, api, .. } = event
                && code.is_none()
            {
                api.prevent_exit();
            }
        });
}

/// Manda un `POST` sin cuerpo a la API de control de la instancia que ya
/// está corriendo, y no espera nada más que la conexión.
///
/// Deliberadamente mínimo: esta invocación del binario no llega a construir
/// ningún runtime asíncrono —se detiene nada más comprobar el bloqueo de
/// instancia única—, así que tirar de `reqwest` aquí sería levantar toda una
/// pila async para una petición de una línea. Un `TcpStream` a mano hace lo
/// mismo con menos.
/// Adquiere el bloqueo de instancia única, o atiende lo que corresponda si ya
/// hay una corriendo.
///
/// `None` significa que esta invocación ya terminó su trabajo —avisó a la
/// instancia existente, o `--quit` no tenía nada que cerrar— y `run` debe
/// volver sin construir nada más.
fn adquirir_o_avisar(args: &Argumentos) -> Option<localify_platform::InstanceGuard> {
    match adquirir_instancia() {
        Ok(guardia) => {
            // Sin ninguna instancia en marcha, `--quit` no tiene nada que
            // cerrar.
            if args.quit {
                info!("--quit sin ninguna instancia en marcha: nada que hacer");
                return None;
            }
            Some(guardia)
        }
        Err(e) => {
            // `--headless` combinado con una instancia existente no pide
            // nada —no tiene sentido abrir una ventana que nadie pidió
            // ver—, así que no manda nada.
            warn!(error = %e, "ya hay otra instancia en ejecución");
            if !args.headless {
                avisar_a_la_instancia_en_marcha(if args.quit {
                    "/app/quit"
                } else {
                    "/window/show"
                });
            }
            None
        }
    }
}

fn avisar_a_la_instancia_en_marcha(ruta: &str) {
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::time::Duration;

    let intento = (|| -> std::io::Result<()> {
        let mut conexion = TcpStream::connect(("127.0.0.1", crate::control_api::PUERTO))?;
        conexion.set_read_timeout(Some(Duration::from_secs(3)))?;
        write!(
            conexion,
            "POST {ruta} HTTP/1.1\r\n\
             Host: 127.0.0.1\r\n\
             Content-Length: 0\r\n\
             Connection: close\r\n\r\n"
        )?;
        // No interesa la respuesta, solo que la petición salga; leer hasta
        // el cierre es lo que garantiza que el servidor la procesó antes de
        // que este proceso termine.
        let mut respuesta = Vec::new();
        conexion.read_to_end(&mut respuesta)?;
        Ok(())
    })();

    match intento {
        Ok(()) => info!(ruta, "aviso enviado a la instancia en marcha"),
        Err(e) => warn!(error = %e, ruta, "no se pudo avisar a la instancia en marcha"),
    }
}

/// Muestra la ventana principal, creándola si hace falta.
///
/// Cubre dos casos con el mismo código: el arranque normal (la ventana no
/// existe todavía) y `/window/show` sobre una instancia que arrancó en
/// `--headless` o que ya la había cerrado. La configuración —tamaño, tema,
/// color de fondo— sale de `tauri.conf.json`, la misma que usaría Tauri si
/// no le hubiéramos puesto `"create": false` para poder decidir esto a mano.
///
/// # Errors
/// Si Tauri no puede construir o enfocar la ventana.
pub(crate) fn mostrar_ventana(app: &tauri::AppHandle) -> tauri::Result<()> {
    use tauri::Manager;

    if let Some(ventana) = app.get_webview_window("main") {
        ventana.show()?;
        ventana.set_focus()?;
        return Ok(());
    }

    let Some(config) = app.config().app.windows.first() else {
        return Ok(());
    };
    let ventana = tauri::WebviewWindowBuilder::from_config(app, config)?.build()?;
    ventana.set_focus()?;
    Ok(())
}

/// Crea el icono de la bandeja del sistema.
///
/// Dos entradas y nada más: "Mostrar" llama a lo mismo que `/window/show`, y
/// "Salir" a lo mismo que `/app/quit`. En Windows, el clic izquierdo directo
/// sobre el icono también muestra la ventana —el menú es para el derecho—;
/// en Linux esa distinción no existe (`TrayIconEvent` no se emite ahí, según
/// la propia documentación de Tauri), así que el menú es el único camino, y
/// se llega a él con cualquier botón.
///
/// # Errors
/// Si Tauri no puede crear el icono o el menú.
fn arrancar_bandeja(app: &tauri::App) -> tauri::Result<()> {
    use tauri::menu::{Menu, MenuItem};
    use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

    let mostrar = MenuItem::with_id(app, "mostrar", "Mostrar Localify", true, None::<&str>)?;
    let salir = MenuItem::with_id(app, "salir", "Salir", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&mostrar, &salir])?;

    let mut icono = TrayIconBuilder::new()
        .menu(&menu)
        .tooltip("Localify")
        .on_menu_event(|app, event| match event.id().0.as_str() {
            "mostrar" => {
                if let Err(e) = mostrar_ventana(app) {
                    warn!(error = %e, "no se pudo mostrar la ventana desde la bandeja");
                }
            }
            "salir" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
                && let Err(e) = mostrar_ventana(tray.app_handle())
            {
                warn!(error = %e, "no se pudo mostrar la ventana desde la bandeja");
            }
        });

    if let Some(icono_app) = app.default_window_icon() {
        icono = icono.icon(icono_app.clone());
    }

    icono.build(app)?;
    Ok(())
}

/// Espera antes del primer repaso de la base de datos.
///
/// La ventana acaba de aparecer y lo primero que hace el usuario es buscar algo.
/// Un `DELETE` sobre `tracks` en ese instante compite por el mismo escritor y se
/// nota; medio minuto después, no.
const RETRASO_MANTENIMIENTO: std::time::Duration = std::time::Duration::from_secs(30);

/// Cada cuánto se repite con la aplicación en marcha.
const INTERVALO_MANTENIMIENTO: std::time::Duration = std::time::Duration::from_secs(30 * 60);

/// Días que sobrevive un resultado de búsqueda que nadie llegó a tocar.
///
/// Buscar guarda en el catálogo cada resultado, y sin esto la biblioteca crece
/// sola con canciones que el usuario solo vio de pasada. Una semana es margen de
/// sobra para volver a algo que se buscó y no se abrió; lo que se escuchó,
/// descargó, marcó o encoló no entra en la purga por definición.
const DIAS_HUERFANAS: u16 = 7;

/// Repasa la base de datos en segundo plano mientras la aplicación vive.
///
/// Tres tareas, todas prescindibles: si alguna falla se registra y se sigue. Un
/// fallo de mantenimiento no puede tumbar una sesión de reproducción.
async fn mantener(repo: Arc<dyn localify_core::ports::database::MaintenanceRepository>) {
    tokio::time::sleep(RETRASO_MANTENIMIENTO).await;

    loop {
        match repo.purge_orphans(DIAS_HUERFANAS).await {
            Ok(0) => {}
            Ok(n) => info!(pistas = n, "metadatos sueltos purgados"),
            Err(e) => warn!(error = %e, "no se pudo purgar el catálogo"),
        }

        if let Err(e) = repo.optimize().await {
            warn!(error = %e, "no se pudo optimizar la base de datos");
        }

        // El WAL crece con cada guardado de posición —uno cada cinco segundos—
        // y solo se recorta al integrarse. Se comprueba antes de forzarlo
        // porque un checkpoint bloquea a los escritores, y hacerlo cada media
        // hora para recuperar unos kilobytes sale más caro que dejarlo estar.
        let wal = repo.wal_bytes();
        if wal > localify_core::ports::database::WAL_MAXIMO_BYTES {
            info!(bytes = wal, "integrando el WAL");
            if let Err(e) = repo.checkpoint_wal().await {
                warn!(error = %e, "no se pudo integrar el WAL");
            }
        }

        tokio::time::sleep(INTERVALO_MANTENIMIENTO).await;
    }
}

/// Sirve la portada de un álbum, descargándola si hace falta.
///
/// Un fallo se responde con **404 y cuerpo vacío**, nunca con un error del
/// protocolo: la etiqueta `<img>` del frontend ya sabe caer a su icono, y un
/// error de esquema dejaría la petición colgada en la consola del WebView.
async fn portada(ctx: &crate::context::AppContext, ruta: &str) -> tauri::http::Response<Vec<u8>> {
    use localify_core::domain::ids::{AlbumId, ArtistId, PlaylistId};
    use tauri::http::{Response, StatusCode};

    let vacia = |estado: StatusCode| {
        Response::builder()
            .status(estado)
            .body(Vec::new())
            .unwrap_or_else(|_| Response::new(Vec::new()))
    };

    // Tres formas: `/MPREb_m2xZZHGzRl1` para un álbum, `/playlist/<uuid>` para
    // la imagen que el usuario eligió y `/artist/<id>` para la foto de un
    // artista. El prefijo evita tener que adivinar de qué tipo es un
    // identificador por su forma —álbumes y artistas comparten alfabeto y
    // longitud, así que no hay forma de distinguirlos mirándolos—.
    let camino = ruta.trim_start_matches('/');

    let fichero = if let Some(uuid) = camino.strip_prefix("playlist/") {
        let Ok(id) = PlaylistId::parse(uuid) else {
            return vacia(StatusCode::BAD_REQUEST);
        };
        match ctx.playlists.cover_file(&id).await {
            Ok(Some(f)) => f,
            _ => return vacia(StatusCode::NOT_FOUND),
        }
    } else if let Some(bruto) = camino.strip_prefix("track/") {
        // Pistas sin álbum: la portada sale de la miniatura del vídeo que se
        // emparejó. Es el caso de lo importado de una lista pública, que llega
        // sin disco.
        let Ok(id) = localify_core::domain::ids::TrackId::parse(bruto) else {
            return vacia(StatusCode::BAD_REQUEST);
        };
        match ctx.metadata.ensure_track_thumbnail(&id).await {
            Ok(Some(f)) => f,
            _ => return vacia(StatusCode::NOT_FOUND),
        }
    } else if let Some(bruto) = camino.strip_prefix("artist/") {
        let Ok(id) = ArtistId::parse(bruto) else {
            return vacia(StatusCode::BAD_REQUEST);
        };
        match ctx.metadata.ensure_artist_image(&id).await {
            Ok(Some(f)) => f,
            _ => return vacia(StatusCode::NOT_FOUND),
        }
    } else {
        let Ok(album) = AlbumId::parse(camino) else {
            return vacia(StatusCode::BAD_REQUEST);
        };
        match ctx.metadata.ensure_cover(&album).await {
            Ok(Some(f)) => f,
            _ => return vacia(StatusCode::NOT_FOUND),
        }
    };

    let Ok(bytes) = tokio::fs::read(&fichero).await else {
        return vacia(StatusCode::NOT_FOUND);
    };

    // Las de álbum siempre son JPEG; la del usuario puede ser lo que él eligió.
    // Un tipo equivocado no impide que Chromium la pinte, pero sí ensucia la
    // consola con avisos en cada carga.
    let tipo = match fichero.extension().and_then(|e| e.to_str()) {
        Some("png") => "image/png",
        Some("webp") => "image/webp",
        _ => "image/jpeg",
    };

    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", tipo)
        // El contenido de una URL de portada no cambia: la del usuario lleva la
        // marca de tiempo de la playlist, así que cambiarla cambia la URL.
        .header("Cache-Control", "public, max-age=31536000, immutable")
        .body(bytes)
        .unwrap_or_else(|_| Response::new(Vec::new()))
}

/// Abre las herramientas de desarrollo si `LOCALIFY_DEVTOOLS` está puesta.
///
/// Sin bundler ni servidor de desarrollo (ADR-019), el inspector del WebView es
/// la única forma de ver por qué el CSS hace lo que hace. Va tras una variable
/// de entorno y no tras `debug_assertions` a secas porque abrirlo en cada
/// arranque de depuración roba el foco y tapa media ventana.
///
/// En release la función no existe: no hay manera de activarlo por accidente.
#[cfg(debug_assertions)]
fn abrir_devtools(app: &tauri::App) {
    use tauri::Manager;

    if std::env::var_os("LOCALIFY_DEVTOOLS").is_none() {
        return;
    }
    if let Some(ventana) = app.get_webview_window("main") {
        ventana.open_devtools();
    }
}

#[cfg(not(debug_assertions))]
#[allow(
    clippy::missing_const_for_fn,
    reason = "la variante de depuracion no puede serlo"
)]
fn abrir_devtools(_app: &tauri::App) {}

/// Identificador nativo de la ventana principal.
///
/// El panel multimedia de Windows se ata a un `HWND`; sin él no hay
/// integración posible. MPRIS, en Linux, no se ata a ninguna ventana, así que
/// fuera de Windows esto siempre es `None` y el arranque del panel no
/// depende de él.
fn hwnd_principal(app: &tauri::App) -> Option<isize> {
    #[cfg(windows)]
    {
        use tauri::Manager;
        let ventana = app.get_webview_window("main")?;
        ventana.hwnd().ok().map(|h| h.0 as isize)
    }
    #[cfg(not(windows))]
    {
        let _ = app;
        None
    }
}

/// Abre la persistencia y cablea el contexto.
///
/// **Nunca falla el arranque.** Si la base de datos no se puede abrir o las
/// migraciones fallan, se arranca sin biblioteca: la ventana se abre, Ajustes
/// sigue accesible —es donde está la ruta de la carpeta— y cada operación dice
/// por qué no puede hacerse. Cerrarse dejaría al usuario delante de una ventana
/// que desaparece, sin saber dónde ha quedado su música.
fn construir_contexto(
    paths: &LocalifyPaths,
    bus: &crate::bridge::EventBus,
) -> crate::context::AppContext {
    let resultado = tauri::async_runtime::block_on(async {
        let pool = localify_db::Pool::abrir(&paths.database_path())?;

        let estado = localify_db::ejecutar_migraciones(&pool).await?;
        if !estado.es_utilizable() {
            warn!(?estado, "el esquema no es utilizable");
            return Err(localify_core::error::CoreError::storage(format!(
                "esquema inutilizable: {estado:?}"
            )));
        }
        info!(?estado, "base de datos lista");

        let secretos: Arc<dyn localify_core::ports::platform::SecretStore> = Arc::new(
            localify_platform::AlmacenDeSecretos::new(paths.config_dir()),
        );

        crate::context::AppContext::real(
            bus.clone(),
            crate::context::Infraestructura {
                pool,
                secretos,
                paths: Arc::new(paths.clone()) as Arc<dyn AppPaths>,
            },
        )
        .await
    });

    match resultado {
        Ok(ctx) => ctx,
        Err(e) => {
            error!(error = %e, "arranque sin biblioteca: la base de datos no abre");
            crate::context::AppContext::sin_biblioteca(bus.clone(), paths.library_dir().to_owned())
        }
    }
}

/// Comprueba la versión de yt-dlp y la actualiza si hace falta.
///
/// No devuelve nada y no puede fallar hacia fuera: quedarse sin actualizar es
/// molesto, pero impedir el arranque por ello sería mucho peor. Todo lo que pasa
/// va al log, que es donde se mira cuando las descargas empiezan a fallar.
async fn poner_al_dia(binarios: std::path::PathBuf) {
    use localify_platform::Actualizacion;

    let locator = localify_platform::SidecarLocator::new(binarios);
    match localify_platform::actualizar_yt_dlp(&locator, crate::context::TOPE_ACTUALIZACION).await {
        Actualizacion::AlDia(v) => info!(version = %v, "yt-dlp al día"),
        Actualizacion::Actualizado { antes, ahora } => {
            info!(%antes, %ahora, "yt-dlp actualizado");
        }
        Actualizacion::NoEsNuestro => {
            debug!("yt-dlp viene del PATH del sistema: no se toca");
        }
        // Sin red, sin permisos o con GitHub limitando: se sigue con la versión
        // que haya, que es lo que se hacía siempre hasta ahora.
        Actualizacion::NoSePudo(motivo) => warn!(%motivo, "no se pudo actualizar yt-dlp"),
    }
}

/// Comprueba una vez si hay una versión de Localify más nueva publicada.
///
/// Sin cliente HTTP no hay comprobación posible —pasa, por ejemplo, sin
/// entropía para TLS— y no es un motivo para que el arranque falle: se
/// registra y se sigue sin avisar de nada.
async fn comprobar_actualizaciones(
    eventos: Arc<dyn EventPublisher>,
    url_pendiente: Arc<std::sync::Mutex<Option<String>>>,
) {
    let Ok(http) = localify_integrations::autoupdate::cliente() else {
        warn!("sin cliente HTTP: no se comprobaron actualizaciones");
        return;
    };
    localify_integrations::autoupdate::vigilar(http, eventos, move |url| {
        if let Ok(mut g) = url_pendiente.lock() {
            *g = Some(url);
        }
    })
    .await;
}

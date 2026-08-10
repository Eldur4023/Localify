//! Arranque por etapas.
//!
//! El objetivo es "ventana visible cuanto antes". Nada que pueda esperar
//! bloquea la aparición de la interfaz: los sidecars, la purga de temporales y
//! el mantenimiento de la base de datos se lanzan como tareas de fondo una vez
//! la ventana ya está en pantalla.

use std::sync::Arc;

use localify_core::ports::platform::AppPaths;
use localify_platform::{LocalifyPaths, adquirir_instancia};
use tracing::{error, info, warn};

/// Arranca la aplicación.
pub fn run() {
    let inicio = std::time::Instant::now();

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
    // escenario a soportar. Se rechaza antes de tocar nada.
    let _instancia = match adquirir_instancia() {
        Ok(g) => g,
        Err(e) => {
            warn!(error = %e, "ya hay otra instancia en ejecución; saliendo");
            return;
        }
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
    let lastfm = contexto.lastfm.clone();
    let ajustes_integraciones = Arc::clone(&contexto.settings);
    let playback_discord = Arc::clone(&contexto.playback);
    let metadata_discord = Arc::clone(&contexto.metadata);
    let piezas_discord = contexto.para_discord.clone();

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

            // El panel multimedia se ata a la ventana, así que solo se puede
            // pedir una vez existe. Si el sistema no lo concede, la integración
            // se queda inerte y la reproducción funciona igual.
            if let Some(hwnd) = hwnd_principal(app) {
                crate::multimedia::arrancar(hwnd, playback, metadata_smtc, &bus);
            } else {
                warn!("sin ventana principal: no hay panel multimedia");
            }

            abrir_devtools(app);

            info!(ms = inicio.elapsed().as_millis(), "ventana lista");

            if let Some(repo) = mantenimiento {
                tauri::async_runtime::spawn(mantener(repo));
            }

            // Las dos integraciones se enganchan al bus y no se les vuelve a
            // hablar. Arrancan aunque estén desactivadas: cada una comprueba su
            // ajuste al recibir un evento, así que encenderlas surte efecto en
            // la siguiente canción y no al reiniciar. La alternativa —arrancar y
            // parar tareas al tocar el interruptor— sería estado que mantener a
            // cambio de ahorrar una tarea dormida.
            if let Some(gestor) = lastfm {
                tauri::async_runtime::spawn(localify_integrations::lastfm::atender(
                    gestor,
                    bus.subscribe(),
                ));
            }
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

            Ok(())
        })
        .run(tauri::generate_context!())
        .unwrap_or_else(|e| {
            error!(error = %e, "fallo irrecuperable de Tauri");
            std::process::exit(1);
        });
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
/// integración posible. Fuera de Windows no existe el concepto y se devuelve
/// `None`, que la integración traduce a "no hacer nada".
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
/// migraciones fallan, se arranca en modo degradado con datos en memoria: la
/// interfaz sigue siendo utilizable y el usuario puede ver qué pasó y exportar
/// su base de datos. Cerrarse le dejaría sin forma de recuperar su biblioteca.
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

        let secretos: Arc<dyn localify_core::ports::platform::SecretStore> =
            Arc::new(localify_platform::DpapiSecretStore::new(paths.config_dir()));

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
            error!(error = %e, "arranque en modo degradado: sin persistencia");
            crate::context::AppContext::en_memoria(bus.clone())
        }
    }
}

//! Comandos de configuración.

use tauri::State;

use crate::context::AppContext;
use crate::dto::common::ApiError;
use crate::dto::settings::{
    AudioDeviceDto, EqProfileDto, EqProfileInputDto, ProviderStatusDto, PruebaDto, SettingsDto,
    SettingsPatchDto,
};

type Resultado<T> = Result<T, ApiError>;

/// Versión del contrato de la API.
///
/// El frontend la comprueba al arrancar. Hoy es trivial, pero cuando existan
/// clientes externos será lo que impida que uno antiguo hable con un backend
/// incompatible y falle de formas incomprensibles.
pub const VERSION_API: &str = "1.0.0";

#[tauri::command]
pub async fn api_version() -> Resultado<String> {
    Ok(VERSION_API.to_owned())
}

#[tauri::command]
pub async fn settings_get(ctx: State<'_, AppContext>) -> Resultado<SettingsDto> {
    Ok(ctx.settings.get().await.into())
}

/// Aplica un cambio parcial.
///
/// La validación entera vive en el dominio y ocurre **antes** de escribir nada:
/// un patch inválido devuelve error sin dejar la configuración a medias.
#[tauri::command]
pub async fn settings_patch(
    ctx: State<'_, AppContext>,
    patch: SettingsPatchDto,
) -> Resultado<SettingsDto> {
    let patch = patch.try_into()?;
    Ok(ctx.settings.patch(patch).await?.into())
}

#[tauri::command]
pub async fn settings_audio_devices(ctx: State<'_, AppContext>) -> Resultado<Vec<AudioDeviceDto>> {
    Ok(ctx
        .settings
        .audio_devices()
        .await?
        .into_iter()
        .map(Into::into)
        .collect())
}

#[tauri::command]
pub async fn settings_eq_profiles(ctx: State<'_, AppContext>) -> Resultado<Vec<EqProfileDto>> {
    Ok(ctx
        .settings
        .eq_profiles()
        .await?
        .into_iter()
        .map(Into::into)
        .collect())
}

/// Guarda las credenciales de aplicación de Spotify.
///
/// El usuario **no inicia sesión**: no hay cuenta de Spotify implicada. El
/// secreto va al almacén del sistema y no vuelve a salir de ahí.
#[tauri::command]
pub async fn settings_set_spotify_credentials(
    ctx: State<'_, AppContext>,
    client_id: String,
    client_secret: String,
) -> Resultado<ProviderStatusDto> {
    if client_id.trim().is_empty() || client_secret.trim().is_empty() {
        return Err(localify_core::error::CoreError::invalid(
            "las credenciales no pueden estar vacías",
        )
        .into());
    }
    Ok(ctx
        .settings
        .set_spotify_credentials(client_id.trim(), client_secret.trim())
        .await?
        .into())
}

#[tauri::command]
pub async fn settings_test_spotify(ctx: State<'_, AppContext>) -> Resultado<ProviderStatusDto> {
    Ok(ctx.settings.test_spotify().await?.into())
}

/// Abre en el navegador una de las páginas que hacen falta para configurar algo.
///
/// ## Destinos cerrados, no una URL cualquiera
///
/// La alternativa obvia sería un `settings_open_url(url)`, y sería un error: el
/// frontend acabaría pudiendo mandar cualquier cosa al manejador de protocolos
/// del sistema. Aquí llega un nombre de una lista corta y la URL la pone Rust,
/// así que no hay nada que un fallo —o una inyección en la interfaz— pueda
/// convertir en "abre esto otro".
#[tauri::command]
pub async fn settings_open_external(destino: String) -> Resultado<()> {
    let url = match destino.as_str() {
        "discord_apps" => "https://discord.com/developers/applications",
        otro => {
            return Err(localify_core::error::CoreError::invalid(format!(
                "destino desconocido: {otro}"
            ))
            .into());
        }
    };
    localify_platform::navegador::abrir(url)?;
    Ok(())
}

/// Aplica un ecualizador sin guardarlo.
///
/// Es lo que se llama mientras se arrastra un deslizador. Guardar en cada
/// movimiento serían decenas de transacciones por segundo para un valor que el
/// usuario todavía está eligiendo.
#[tauri::command]
pub async fn settings_preview_eq(
    ctx: State<'_, AppContext>,
    profile: EqProfileInputDto,
) -> Resultado<()> {
    ctx.settings.preview_eq(&profile.try_into()?).await?;
    Ok(())
}

/// Abre el selector de carpetas del sistema.
///
/// El diálogo lo abre Rust y no el frontend a propósito. Es coherente con la
/// regla del proyecto —la lógica vive en Rust y la interfaz pinta y manda
/// comandos— y evita depender del paquete npm del plugin, que obligaría a
/// meter Node y un empaquetador donde no los hay (ADR-019).
///
/// Devuelve `None` si el usuario cancela. Cancelar no es un error.
#[tauri::command]
pub async fn settings_pick_folder(app: tauri::AppHandle) -> Resultado<Option<String>> {
    use tauri_plugin_dialog::DialogExt;

    // El diálogo nativo es bloqueante y no se puede esperar desde el hilo del
    // comando sin colgar la interfaz: se pide con callback y se espera al
    // canal, que sí es asíncrono.
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog().file().pick_folder(move |ruta| {
        let _ = tx.send(ruta);
    });

    let elegida = rx.await.map_err(|_| {
        localify_core::error::CoreError::internal("el selector de carpetas se cerró sin responder")
    })?;

    Ok(elegida.map(|r| r.to_string()))
}

/// Cambia la carpeta de la biblioteca.
///
/// Devuelve **inmediatamente** el identificador de la operación: con
/// `move_existing`, copiar decenas de gigabytes no cabe en el tiempo de un
/// comando IPC. El avance llega por `libraryMoveProgress` y el final por
/// `libraryPathChanged`.
#[tauri::command]
pub async fn settings_change_library_path(
    ctx: State<'_, AppContext>,
    path: String,
    move_existing: bool,
) -> Resultado<String> {
    if path.trim().is_empty() {
        return Err(localify_core::error::CoreError::invalid("la ruta está vacía").into());
    }
    Ok(ctx
        .settings
        .change_library_path(std::path::Path::new(path.trim()), move_existing)
        .await?
        .to_string())
}

/// Selector nativo para el fichero de cookies.
///
/// Filtrado a `.txt` porque el formato Netscape que yt-dlp lee es texto plano,
/// y es lo que exportan las extensiones de navegador que la gente usa para
/// esto.
#[tauri::command]
pub async fn settings_pick_cookies(app: tauri::AppHandle) -> Resultado<Option<String>> {
    use tauri_plugin_dialog::DialogExt;

    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .add_filter("cookies.txt", &["txt"])
        .pick_file(move |ruta| {
            let _ = tx.send(ruta);
        });

    let elegida = rx.await.map_err(|_| {
        localify_core::error::CoreError::internal("el selector de ficheros se cerró sin responder")
    })?;

    Ok(elegida.map(|r| r.to_string()))
}

/// Comprueba que las cookies configuradas sirven de verdad.
///
/// ## Por qué hace falta un botón
///
/// Elegir un navegador en un desplegable no garantiza nada: en Windows, Chrome y
/// sus derivados cifran las cookies con App-Bound Encryption desde la versión
/// 127 y yt-dlp no siempre puede descifrarlas; un perfil puede estar bloqueado
/// porque el navegador está abierto; y un `cookies.txt` exportado hace meses
/// tiene la sesión caducada.
///
/// Sin esta comprobación, el usuario configura algo, cierra Ajustes y se entera
/// de que no funcionaba tres canciones después, cuando el fallo ya no se parece
/// a lo que tocó.
///
/// Se pide la ficha de un vídeo real sin descargar nada. Es la misma operación
/// que hace el emparejador, así que si esto pasa, la descarga también.
#[tauri::command]
pub async fn settings_test_cookies(ctx: State<'_, AppContext>) -> Resultado<PruebaDto> {
    Ok(ctx.diagnostico.probar_cookies().await)
}

/// Fuerza una comprobación de versión de yt-dlp.
///
/// Ya se hace sola al arrancar; esto existe para cuando las descargas empiezan a
/// fallar **ahora** y esperar al siguiente arranque no es una respuesta.
#[tauri::command]
pub async fn settings_update_ytdlp(ctx: State<'_, AppContext>) -> Resultado<PruebaDto> {
    Ok(ctx.diagnostico.actualizar_ytdlp().await)
}

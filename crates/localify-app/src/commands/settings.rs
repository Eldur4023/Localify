//! Comandos de configuración.

use tauri::State;

use crate::context::AppContext;
use crate::dto::common::ApiError;
use crate::dto::settings::{
    AudioDeviceDto, EqProfileDto, EqProfileInputDto, LastfmAuthDto, ProviderStatusDto, SettingsDto,
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
        "lastfm_api" => "https://www.last.fm/api/account/create",
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

// ── Last.fm ─────────────────────────────────────────────────────────────────
//
// La autenticación es de tres pasos y **tiene que serlo**: Last.fm no acepta
// usuario y contraseña desde una aplicación de escritorio, sino un token que el
// usuario autoriza en su navegador. Se parte en dos comandos porque entre uno y
// otro hace falta que una persona haga algo fuera de Localify, y no hay forma
// de esperar a eso dentro de una llamada IPC.

/// El gestor, o un error legible si la aplicación arrancó sin base de datos.
fn gestor_lastfm(
    ctx: &AppContext,
) -> Resultado<std::sync::Arc<localify_integrations::GestorLastfm>> {
    ctx.lastfm.clone().ok_or_else(|| {
        localify_core::error::CoreError::internal("Last.fm no está disponible en modo degradado")
            .into()
    })
}

/// Guarda la clave de API de Last.fm y su secreto.
///
/// Cambiarlas desconecta la sesión anterior: estaba firmada con el secreto
/// viejo y dejarla puesta daría un error en cada envío.
#[tauri::command]
pub async fn settings_set_lastfm_credentials(
    ctx: State<'_, AppContext>,
    api_key: String,
    api_secret: String,
) -> Resultado<SettingsDto> {
    if api_key.trim().is_empty() || api_secret.trim().is_empty() {
        return Err(localify_core::error::CoreError::invalid(
            "las credenciales no pueden estar vacías",
        )
        .into());
    }
    gestor_lastfm(&ctx)?
        .guardar_credenciales(api_key.trim(), api_secret.trim())
        .await?;
    // Sin sesión: guardar credenciales no conecta a nadie, y la pantalla tiene
    // que reflejarlo en vez de quedarse con el usuario anterior.
    Ok(ctx.settings.set_lastfm_session(None).await?.into())
}

/// Primer paso: abre la página de autorización en el navegador del usuario.
///
/// La abre **Rust**, no el frontend. `window.open` dentro del WebView llevaría
/// la página de inicio de sesión de Last.fm a una ventana sin barra de
/// direcciones, que es justo la forma que tiene una suplantación.
///
/// Devuelve también la URL —por si el navegador no se abre, para poder
/// copiarla— y el token, que hace falta para el segundo paso. El token no es un
/// secreto: caduca en una hora y solo sirve para esta autorización.
#[tauri::command]
pub async fn settings_lastfm_begin_auth(ctx: State<'_, AppContext>) -> Resultado<LastfmAuthDto> {
    let (token, url) = gestor_lastfm(&ctx)?.iniciar_autenticacion().await?;
    localify_platform::navegador::abrir(&url)?;
    Ok(LastfmAuthDto { token, url })
}

/// Segundo paso: canjea el token ya autorizado por una sesión permanente.
#[tauri::command]
pub async fn settings_lastfm_complete_auth(
    ctx: State<'_, AppContext>,
    token: String,
) -> Resultado<SettingsDto> {
    let usuario = gestor_lastfm(&ctx)?.completar_autenticacion(&token).await?;
    Ok(ctx.settings.set_lastfm_session(Some(usuario)).await?.into())
}

/// Olvida la sesión. La cola de pendientes **no se toca**.
///
/// Desconectarse no es tirar lo escuchado: si el usuario vuelve a conectar, esos
/// scrobbles salen. Borrarlos aquí sería castigar un cambio de opinión.
#[tauri::command]
pub async fn settings_lastfm_disconnect(ctx: State<'_, AppContext>) -> Resultado<SettingsDto> {
    gestor_lastfm(&ctx)?.desconectar().await?;
    Ok(ctx.settings.set_lastfm_session(None).await?.into())
}

/// Cuántas escuchas esperan a poder enviarse, y un intento de vaciarlas.
///
/// Se aprovecha para empujar la cola: quien abre esta pantalla suele estar
/// preguntándose justo eso, y esperar cinco minutos al siguiente ciclo sería
/// una espera sin motivo.
#[tauri::command]
pub async fn settings_lastfm_pending(ctx: State<'_, AppContext>) -> Resultado<u64> {
    let gestor = gestor_lastfm(&ctx)?;
    gestor.vaciar_cola().await;
    Ok(gestor.pendientes().await?)
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

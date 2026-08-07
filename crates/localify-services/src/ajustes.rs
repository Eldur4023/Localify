//! Configuración persistente.
//!
//! ## Instantánea en memoria, escritura inmediata
//!
//! `get()` está declarado como barato en el puerto porque lo llama la interfaz
//! en cada montaje de Ajustes y lo consultan varios servicios. Se sirve de un
//! `RwLock<Settings>` que se llena al arrancar; cada `patch` escribe primero en
//! la base de datos y solo entonces actualiza la copia. Al revés —memoria
//! primero— una escritura fallida dejaría a la aplicación creyendo un ajuste
//! que no sobrevive al reinicio.
//!
//! ## Una fila por sección, no un blob único
//!
//! La tabla `settings` es clave/valor y aquí se guarda un JSON por sección
//! (`audio`, `download`, `ui`…). Podría ser un solo documento, pero las
//! escrituras ya son por sección —lo dice `SettingsSection`, que viaja en el
//! evento de cambio— y, sobre todo, un valor que deje de parsearse se lleva por
//! delante únicamente su sección. Con un blob único, un campo corrupto tira la
//! configuración entera.
//!
//! Por eso la carga es tolerante: una sección ilegible se sustituye por su
//! valor por defecto y se avisa en el log. Perder el ecualizador es molesto;
//! no poder abrir la aplicación, mucho peor.
//!
//! ## El secreto de Spotify no vive aquí
//!
//! `SpotifySettings` solo dice si hay credenciales y cuál es el `client_id`.
//! El `client_secret` va al almacén del sistema (DPAPI) a través de
//! [`SecretStore`] y no vuelve a salir: ni a la base de datos, ni al puente
//! IPC, ni a los logs.

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use localify_core::domain::audio::{AudioDevice, EqProfile};
use localify_core::domain::settings::{
    AudioSettings, DownloadSettings, IntegrationSettings, Language, MetadataProviderKind, Settings,
    SettingsPatch, SettingsSection, SpotifySettings, UiSettings,
};
use localify_core::error::{CoreError, CoreResult};
use localify_core::events::{DomainEvent, EventPublisher, ProviderStatus};
use localify_core::ports::audio_engine::AudioEngine;
use localify_core::ports::database::SettingsRepository;
use localify_core::ports::metadata_provider::MetadataProvider;
use localify_core::ports::platform::{AppPaths, FileSystem, LocaleProvider, SecretStore};
use localify_core::ports::services::SettingsService;
use serde::Serialize;
use serde::de::DeserializeOwned;
use tracing::{debug, info, warn};
use uuid::Uuid;

/// Claves de la tabla `settings`. Constantes y no cadenas sueltas: una errata
/// en una de ellas no daría error, solo silencio y un ajuste que no persiste.
const K_IDIOMA: &str = "language";
const K_PROVEEDOR: &str = "metadataProvider";
const K_BIBLIOTECA: &str = "libraryPath";
const K_AUDIO: &str = "audio";
const K_DESCARGAS: &str = "download";
const K_INTEGRACIONES: &str = "integrations";
const K_UI: &str = "ui";

use localify_core::ports::platform::claves::{
    LASTFM_API_KEY, LASTFM_API_SECRET, LASTFM_SESION, SPOTIFY_ID as S_SPOTIFY_ID,
    SPOTIFY_SECRETO as S_SPOTIFY_SECRETO,
};

/// Margen sobre el tamaño de la biblioteca al comprobar espacio para migrar.
///
/// La copia necesita tanto como ocupa el origen; el 10 % extra cubre los
/// temporales de la copia y evita dejar el volumen destino a cero, que es una
/// forma bastante eficaz de romper otras cosas del sistema.
const MARGEN_ESPACIO: f64 = 1.1;

pub struct Dependencias {
    pub repo: Arc<dyn SettingsRepository>,
    pub secretos: Arc<dyn SecretStore>,
    pub eventos: Arc<dyn EventPublisher>,
    pub paths: Arc<dyn AppPaths>,
    pub fs: Arc<dyn FileSystem>,
    /// Ausente cuando la máquina no tiene salida de audio. No es un caso
    /// degradado que haya que disimular con un motor falso: sin dispositivo no
    /// hay ecualizador que aplicar, y decirlo con un `Option` evita inventar
    /// una implementación vacía que solo existiría para no escribir esto.
    pub audio: Option<Arc<dyn AudioEngine>>,
    /// Crossfade vigente, en milisegundos.
    ///
    /// Lo escribe este servicio y lo lee el actor de reproducción al encadenar
    /// pistas. Va por atómico y no por consulta porque se lee en el camino
    /// crítico de cada cambio de canción, donde una llamada `async` a otro
    /// actor añadiría una espera para leer un `u32`.
    pub crossfade: Arc<std::sync::atomic::AtomicU32>,
    pub locale: Arc<dyn LocaleProvider>,
    /// Conmutador del catálogo de metadatos.
    ///
    /// Se le avisa al cambiar el ajuste, que es lo que hace que el cambio
    /// tenga efecto sin reiniciar. Ausente en los tests, donde no hay
    /// proveedores que conmutar.
    pub proveedor: Option<Arc<crate::proveedor::ProveedorConmutable>>,
    /// Ausente cuando no hay credenciales: comprobar Spotify sin proveedor no
    /// es un error, es "no configurado".
    pub spotify: Option<Arc<dyn MetadataProvider>>,
}

impl std::fmt::Debug for Dependencias {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Dependencias").finish_non_exhaustive()
    }
}

pub struct SettingsServiceImpl {
    deps: Arc<Dependencias>,
    /// Instantánea vigente. `RwLock` de `std` y no de tokio: nunca se espera
    /// nada con el cerrojo tomado, y con el de `std` intentarlo no compila.
    actual: RwLock<Settings>,
}

impl std::fmt::Debug for SettingsServiceImpl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SettingsServiceImpl")
            .finish_non_exhaustive()
    }
}

impl SettingsServiceImpl {
    /// Carga la configuración de la base de datos.
    ///
    /// **Nunca falla.** Si la base de datos no responde o un valor está
    /// corrupto, se arranca con los valores por defecto: la alternativa es una
    /// aplicación que no abre por un ajuste mal escrito.
    pub async fn cargar(deps: Dependencias) -> Self {
        let deps = Arc::new(deps);
        let mut settings = Settings::por_defecto_en(deps.paths.library_dir().to_path_buf());

        // El idioma del primer arranque sale del sistema, no de una constante:
        // abrir en inglés a alguien cuyo Windows está en español es un mal
        // recibimiento evitable.
        settings.language = Language::from_locale(&deps.locale.system_locale());

        match deps.repo.get_all().await {
            Ok(filas) => aplicar_filas(&mut settings, &filas),
            Err(e) => {
                warn!(error = %e, "no se pudo leer la configuración; se usan los valores por defecto");
            }
        }

        settings.spotify = leer_spotify(&deps.secretos).await;
        // Lo mismo que con Spotify: el estado de conexión no se persiste, se
        // deduce del almacén del sistema en cada arranque. Así no puede quedarse
        // diciendo "conectado" después de que alguien borre la sesión por fuera.
        settings.integrations.lastfm_connected = hay_sesion_lastfm(&deps.secretos).await;

        let servicio = Self {
            deps,
            actual: RwLock::new(settings),
        };
        servicio.aplicar_a_audio();
        // También al arrancar: el conmutador nace en su valor por defecto y sin
        // esto, el proveedor elegido no se respetaría hasta volver a tocarlo.
        if let Some(conmutador) = &servicio.deps.proveedor {
            conmutador.cambiar(servicio.instantanea().metadata_provider);
        }
        servicio
    }

    /// Vuelca al audio lo que dependa de él.
    ///
    /// Se llama al cargar y tras cada cambio de la sección. Son tres destinos
    /// distintos: el ecualizador y el dispositivo van al motor, y el crossfade
    /// al atómico que lee el actor de reproducción.
    fn aplicar_a_audio(&self) {
        let (perfil, dispositivo, crossfade) = {
            let s = self
                .actual
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            (
                s.audio.eq_profile.clone(),
                s.audio.output_device_id.clone(),
                s.audio.crossfade_ms,
            )
        };

        self.deps
            .crossfade
            .store(crossfade, std::sync::atomic::Ordering::Relaxed);

        let Some(motor) = &self.deps.audio else {
            return;
        };

        motor.set_equalizer(&perfil);

        // Un dispositivo que ya no existe —unos cascos desenchufados desde la
        // última sesión— no puede impedir que suene nada: se avisa y se sigue
        // con el predeterminado del sistema.
        if let Err(e) = motor.set_device(dispositivo.as_deref()) {
            warn!(error = %e, dispositivo = ?dispositivo, "dispositivo de salida no disponible; se usa el del sistema");
            if let Err(e) = motor.set_device(None) {
                warn!(error = %e, "tampoco hay dispositivo predeterminado");
            }
        }
    }

    /// Persiste una sección y devuelve si hubo que escribir.
    async fn guardar<T: Serialize>(&self, clave: &str, valor: &T) -> CoreResult<()> {
        let json = serde_json::to_string(valor)
            .map_err(|e| CoreError::internal(format!("no se pudo serializar '{clave}': {e}")))?;
        self.deps.repo.set_raw(clave, &json).await
    }

    fn instantanea(&self) -> Settings {
        self.actual
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

/// Aplica sobre `settings` lo que se haya podido leer de la base de datos.
fn aplicar_filas(settings: &mut Settings, filas: &[(String, String)]) {
    for (clave, valor) in filas {
        match clave.as_str() {
            K_PROVEEDOR => {
                if let Some(p) = MetadataProviderKind::from_code(valor.trim_matches(char::from(34)))
                {
                    settings.metadata_provider = p;
                } else {
                    warn!(
                        valor,
                        "proveedor guardado desconocido; se conserva el actual"
                    );
                }
            }
            K_IDIOMA => {
                if let Some(l) = Language::from_code(valor.trim_matches('"')) {
                    settings.language = l;
                } else {
                    warn!(
                        valor,
                        "idioma guardado desconocido; se conserva el detectado"
                    );
                }
            }
            K_BIBLIOTECA => {
                if let Some(ruta) = leer::<PathBuf>(clave, valor) {
                    settings.library_path = ruta;
                }
            }
            K_AUDIO => {
                if let Some(a) = leer::<AudioSettings>(clave, valor) {
                    settings.audio = a;
                }
            }
            K_DESCARGAS => {
                if let Some(d) = leer::<DownloadSettings>(clave, valor) {
                    settings.download = d;
                }
            }
            K_INTEGRACIONES => {
                if let Some(i) = leer::<IntegrationSettings>(clave, valor) {
                    settings.integrations = i;
                }
            }
            K_UI => {
                if let Some(u) = leer::<UiSettings>(clave, valor) {
                    settings.ui = u;
                }
            }
            // Otras claves de la tabla `settings` pertenecen a otros
            // subsistemas (estado del reproductor, marcas de mantenimiento).
            // Que estén ahí no es un problema de esta función.
            _ => {}
        }
    }
}

/// Deserializa una sección; `None` si está corrupta, avisando en el log.
fn leer<T: DeserializeOwned>(clave: &str, valor: &str) -> Option<T> {
    match serde_json::from_str(valor) {
        Ok(v) => Some(v),
        Err(e) => {
            warn!(clave, error = %e, "sección de configuración ilegible; se usa el valor por defecto");
            None
        }
    }
}

/// Estado visible de Spotify: si hay credenciales y cuál es el identificador.
async fn leer_spotify(secretos: &Arc<dyn SecretStore>) -> SpotifySettings {
    let id = secretos.get(S_SPOTIFY_ID).await.ok().flatten();
    let hay_secreto = secretos
        .get(S_SPOTIFY_SECRETO)
        .await
        .ok()
        .flatten()
        .is_some_and(|s| !s.is_empty());

    SpotifySettings {
        configured: hay_secreto && id.as_ref().is_some_and(|i| !i.is_empty()),
        client_id: id.filter(|i| !i.is_empty()),
    }
}

/// Si hay clave de API, secreto **y** sesión: las tres hacen falta para
/// scrobblear, y con dos de tres la interfaz estaría prometiendo algo que no va
/// a pasar.
async fn hay_sesion_lastfm(secretos: &Arc<dyn SecretStore>) -> bool {
    for clave in [LASTFM_API_KEY, LASTFM_API_SECRET, LASTFM_SESION] {
        let puesta = secretos
            .get(clave)
            .await
            .ok()
            .flatten()
            .is_some_and(|v| !v.is_empty());
        if !puesta {
            return false;
        }
    }
    true
}

#[async_trait]
impl SettingsService for SettingsServiceImpl {
    async fn get(&self) -> Settings {
        self.instantanea()
    }

    async fn patch(&self, patch: SettingsPatch) -> CoreResult<Settings> {
        // Se valida **todo** antes de escribir **nada**: un patch inválido no
        // puede dejar media configuración aplicada.
        patch.validar()?;
        let secciones = patch.secciones();
        if secciones.is_empty() {
            return Ok(self.instantanea());
        }

        // Escritura primero, memoria después. Si la base de datos falla, la
        // instantánea sigue reflejando lo que hay guardado en vez de un ajuste
        // que se perdería al reiniciar.
        if let Some(l) = patch.language {
            self.guardar(K_IDIOMA, &l.code()).await?;
        }
        if let Some(p) = patch.metadata_provider {
            self.guardar(K_PROVEEDOR, &p.code()).await?;
        }
        if let Some(a) = &patch.audio {
            self.guardar(K_AUDIO, a).await?;
        }
        if let Some(d) = &patch.download {
            self.guardar(K_DESCARGAS, d).await?;
        }
        if let Some(i) = &patch.integrations {
            self.guardar(K_INTEGRACIONES, i).await?;
        }
        if let Some(u) = &patch.ui {
            self.guardar(K_UI, u).await?;
        }

        let actualizado = {
            let mut s = self
                .actual
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(l) = patch.language {
                s.language = l;
            }
            if let Some(p) = patch.metadata_provider {
                s.metadata_provider = p;
            }
            if let Some(a) = patch.audio {
                s.audio = a;
            }
            if let Some(d) = patch.download {
                s.download = d;
            }
            if let Some(i) = patch.integrations {
                // El patch viene del frontend y no puede decidir si hay sesión:
                // se conserva lo que sabe el servicio. Sin esto, tocar el
                // interruptor de Discord dejaría Last.fm "desconectado" en la
                // pantalla hasta reiniciar.
                let conectado = s.integrations.lastfm_connected;
                s.integrations = i;
                s.integrations.lastfm_connected = conectado;
            }
            if let Some(u) = patch.ui {
                s.ui = u;
            }
            s.clone()
        };

        if secciones.contains(&SettingsSection::Audio) {
            self.aplicar_a_audio();
        }
        if secciones.contains(&SettingsSection::Provider)
            && let Some(conmutador) = &self.deps.proveedor
        {
            conmutador.cambiar(actualizado.metadata_provider);
        }

        self.deps.eventos.publish(DomainEvent::SettingsChanged {
            sections: secciones,
        });

        Ok(actualizado)
    }

    async fn set_spotify_credentials(
        &self,
        client_id: &str,
        client_secret: &str,
    ) -> CoreResult<ProviderStatus> {
        self.deps.secretos.set(S_SPOTIFY_ID, client_id).await?;
        self.deps
            .secretos
            .set(S_SPOTIFY_SECRETO, client_secret)
            .await?;

        {
            let mut s = self
                .actual
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            s.spotify = SpotifySettings {
                configured: true,
                client_id: Some(client_id.to_owned()),
            };
        }

        self.deps.eventos.publish(DomainEvent::SettingsChanged {
            sections: vec![SettingsSection::Spotify],
        });

        // Las credenciales nuevas no llegan al proveedor ya construido: se
        // inyecta al arrancar (ver `credenciales.rs`). Se dice claramente en
        // vez de devolver `Ready` y que el usuario descubra por su cuenta que
        // hace falta reiniciar.
        let estado = match &self.deps.spotify {
            Some(p) => p.status().await,
            None => ProviderStatus::Unavailable {
                reason_key: "provider.restart_required".into(),
            },
        };

        self.deps
            .eventos
            .publish(DomainEvent::ProviderStatusChanged {
                provider: "spotify".into(),
                status: estado.clone(),
            });

        info!("credenciales de Spotify guardadas en el almacén del sistema");
        Ok(estado)
    }

    async fn set_lastfm_session(&self, user: Option<String>) -> CoreResult<Settings> {
        let conectado = hay_sesion_lastfm(&self.deps.secretos).await;

        let seccion = {
            let mut s = self
                .actual
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            s.integrations.lastfm_user = user;
            s.integrations.lastfm_connected = conectado;
            s.integrations.clone()
        };

        // El nombre sí se persiste: es lo que la pantalla enseña al abrir, antes
        // de que nadie haya hablado con Last.fm. `lastfm_connected` no viaja al
        // JSON —lleva `serde(skip)`— así que esto guarda solo lo que debe.
        self.guardar(K_INTEGRACIONES, &seccion).await?;

        self.deps.eventos.publish(DomainEvent::SettingsChanged {
            sections: vec![SettingsSection::Integrations],
        });

        Ok(self.instantanea())
    }

    async fn test_spotify(&self) -> CoreResult<ProviderStatus> {
        let estado = match &self.deps.spotify {
            Some(p) => p.status().await,
            None => ProviderStatus::NotConfigured,
        };
        self.deps
            .eventos
            .publish(DomainEvent::ProviderStatusChanged {
                provider: "spotify".into(),
                status: estado.clone(),
            });
        Ok(estado)
    }

    async fn change_library_path(&self, path: &Path, move_existing: bool) -> CoreResult<Uuid> {
        let origen = self.instantanea().library_path;
        let destino = path.to_path_buf();

        validar_destino(&self.deps, &origen, &destino, move_existing).await?;

        let id = Uuid::new_v4();
        let deps = Arc::clone(&self.deps);

        if !move_existing {
            // Sin migración no hay nada en segundo plano: se cambia el ajuste y
            // el reconciliador de la Fase 8 se encargará de lo que encuentre en
            // la carpeta nueva.
            self.guardar(K_BIBLIOTECA, &destino).await?;
            let mut s = self
                .actual
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            s.library_path.clone_from(&destino);
            drop(s);

            deps.eventos.publish(DomainEvent::LibraryPathChanged {
                path: destino.display().to_string(),
            });
            return Ok(id);
        }

        // Con migración, la respuesta es inmediata y el trabajo va detrás: 50 GB
        // de copia no caben en el tiempo de un comando IPC.
        let repo = Arc::clone(&deps.repo);
        tokio::spawn(async move {
            match migrar(&deps, id, &origen, &destino).await {
                Ok(copiados) => {
                    // El ajuste se cambia **después** de copiar y **antes** de
                    // borrar: ver `migrar` para por qué ese orden es el único
                    // que sobrevive a una interrupción.
                    let json = serde_json::to_string(&destino).unwrap_or_default();
                    if let Err(e) = repo.set_raw(K_BIBLIOTECA, &json).await {
                        warn!(error = %e, "la migración copió pero no se pudo fijar la carpeta nueva");
                        aviso(&deps, "settings.move_failed");
                        return;
                    }
                    limpiar_origen(&deps, &origen, &copiados).await;

                    deps.eventos.publish(DomainEvent::LibraryPathChanged {
                        path: destino.display().to_string(),
                    });
                    info!(ficheros = copiados.len(), destino = %destino.display(), "biblioteca migrada");
                }
                Err(e) => {
                    warn!(error = %e, "migración de biblioteca fallida");
                    aviso(&deps, "settings.move_failed");
                }
            }
        });

        Ok(id)
    }

    async fn audio_devices(&self) -> CoreResult<Vec<AudioDevice>> {
        // Sin motor la lista está vacía, no es un error: la interfaz muestra
        // solo "predeterminado del sistema" y no hay nada que elegir.
        Ok(self
            .deps
            .audio
            .as_ref()
            .map(|m| m.devices())
            .unwrap_or_default())
    }

    async fn preview_eq(&self, profile: &EqProfile) -> CoreResult<()> {
        // Se valida igual que si fuera a guardarse: un perfil con ganancias
        // absurdas haría trabajar al limitador de forma constante, y "es solo
        // una previsualización" no lo hace menos audible.
        EqProfile::new(
            profile.id.clone(),
            profile.name_key.clone(),
            profile.gains_db,
        )?;

        if let Some(motor) = &self.deps.audio {
            motor.set_equalizer(profile);
        }
        Ok(())
    }

    async fn eq_profiles(&self) -> CoreResult<Vec<EqProfile>> {
        let mut perfiles = EqProfile::predefinidos();

        // El perfil personalizado del usuario se añade al final si difiere de
        // los de fábrica: si no, la lista mostraría dos entradas idénticas.
        let propio = self.instantanea().audio.eq_profile;
        if !perfiles.iter().any(|p| p.id == propio.id) {
            perfiles.push(propio);
        }
        Ok(perfiles)
    }
}

/// Comprueba que el destino sirve **antes** de tocar nada.
async fn validar_destino(
    deps: &Dependencias,
    origen: &Path,
    destino: &Path,
    con_migracion: bool,
) -> CoreResult<()> {
    if destino == origen {
        return Err(CoreError::invalid(
            "la carpeta de destino es la misma que la actual",
        ));
    }

    // Una carpeta dentro de la actual convertiría la copia en un bucle: cada
    // fichero copiado aparecería como origen nuevo del recorrido.
    if destino.starts_with(origen) {
        return Err(CoreError::invalid(
            "la carpeta de destino no puede estar dentro de la actual",
        ));
    }

    deps.fs.ensure_dir(destino).await?;
    if !deps.fs.is_writable(destino).await {
        return Err(CoreError::storage(format!(
            "no se puede escribir en '{}'",
            destino.display()
        )));
    }

    if con_migracion {
        let necesario = tamano_total(origen);
        #[allow(
            clippy::cast_precision_loss,
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "una biblioteca no llega a los 2^53 bytes y el margen es aproximado"
        )]
        let con_margen = (necesario as f64 * MARGEN_ESPACIO) as u64;
        let libre = deps.fs.available_space(destino).await?;

        // Se comprueba antes de empezar y no sobre la marcha: quedarse sin
        // espacio a mitad de una copia de 50 GB deja el destino lleno de
        // basura y al usuario esperando para nada.
        if libre < con_margen {
            return Err(CoreError::storage(format!(
                "hacen falta {con_margen} bytes en '{}' y solo hay {libre}",
                destino.display()
            )));
        }
    }

    Ok(())
}

/// Copia la biblioteca entera al destino. Devuelve las rutas relativas copiadas.
///
/// ## Por qué copiar y no mover
///
/// Mover fichero a fichero es más rápido y no necesita espacio extra, pero si
/// el proceso muere a mitad la biblioteca queda partida entre dos carpetas y
/// **ninguna de las dos está completa**. Copiando, el orden es:
///
/// 1. copiar todo (el origen sigue intacto y el ajuste sigue apuntando a él),
/// 2. cambiar el ajuste (el destino ya está completo),
/// 3. borrar el origen.
///
/// Cortar en (1) deja la biblioteca vieja entera y unos ficheros sueltos en el
/// destino. Cortar en (3) deja la nueva entera y unos ficheros sueltos en el
/// origen. En ningún punto hay un estado en el que falte una canción.
async fn migrar(
    deps: &Dependencias,
    id: Uuid,
    origen: &Path,
    destino: &Path,
) -> CoreResult<Vec<PathBuf>> {
    let ficheros = listar(origen);
    let total = u32::try_from(ficheros.len()).unwrap_or(u32::MAX);
    let mut copiados = Vec::with_capacity(ficheros.len());

    for (i, absoluta) in ficheros.iter().enumerate() {
        let Ok(relativa) = absoluta.strip_prefix(origen) else {
            continue;
        };
        deps.fs.copy_file(absoluta, &destino.join(relativa)).await?;
        copiados.push(relativa.to_path_buf());

        let hechos = u32::try_from(i + 1).unwrap_or(u32::MAX);
        deps.eventos.publish(DomainEvent::LibraryMoveProgress {
            move_id: id,
            done: hechos,
            total,
        });
    }

    Ok(copiados)
}

/// Borra del origen lo que ya está copiado y verificado.
///
/// Un borrado que falle no se propaga: en este punto la biblioteca nueva ya es
/// la buena y el ajuste ya apunta a ella. Lo que quede atrás ocupa espacio,
/// que es un problema menor y visible, no una pérdida de datos.
async fn limpiar_origen(deps: &Dependencias, origen: &Path, copiados: &[PathBuf]) {
    let mut fallos = 0_u32;
    for relativa in copiados {
        if let Err(e) = deps.fs.remove_file(&origen.join(relativa)).await {
            fallos += 1;
            debug!(fichero = %relativa.display(), error = %e, "no se pudo borrar el original");
        }
    }
    if fallos > 0 {
        warn!(
            fallos,
            carpeta = %origen.display(),
            "quedan originales sin borrar; ocupan espacio pero la biblioteca nueva está completa"
        );
    }
}

/// Recorrido iterativo de la carpeta. Sin recursión: una biblioteca con enlaces
/// simbólicos circulares desbordaría la pila.
fn listar(raiz: &Path) -> Vec<PathBuf> {
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
                // Los enlaces no se siguen: copiar a través de uno sacaría
                // ficheros de fuera de la biblioteca.
                Ok(t) if t.is_symlink() => {}
                Ok(t) if t.is_dir() => pendientes.push(ruta),
                Ok(t) if t.is_file() => salida.push(ruta),
                _ => {}
            }
        }
    }
    salida
}

/// Bytes que ocupa una carpeta.
fn tamano_total(raiz: &Path) -> u64 {
    listar(raiz)
        .iter()
        .filter_map(|f| std::fs::metadata(f).ok())
        .map(|m| m.len())
        .sum()
}

/// Aviso discreto en la interfaz.
fn aviso(deps: &Dependencias, clave: &str) {
    deps.eventos.publish(DomainEvent::Toast {
        level: localify_core::events::ToastLevel::Error,
        message_key: clave.to_owned(),
        params: Vec::new(),
    });
}

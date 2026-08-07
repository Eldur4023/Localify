//! Scrobbling a Last.fm.
//!
//! ## Consumidor del bus, nunca dependencia
//!
//! Nadie llama a este módulo para reproducir. Se suscribe a los eventos y actúa
//! por su cuenta, así que si Last.fm no responde —o el usuario lo desactiva— la
//! música sigue sonando exactamente igual y ningún otro servicio se entera.
//!
//! ## La cola es la funcionalidad, no un detalle de implementación
//!
//! Escuchar música sin conexión es lo normal en un portátil. Todo lo que se
//! escucha se encola primero **en la base de datos** y se envía después; enviar
//! directo y encolar solo al fallar perdería el caso que importa —cerrar la
//! aplicación antes de recuperar la red— y es el error clásico de este tipo de
//! integración.
//!
//! ## Qué se pide al usuario y por qué
//!
//! Una clave de API de Last.fm, su secreto y una autorización. Es el mismo
//! peaje que Spotify y por el mismo motivo: incrustar unas credenciales en un
//! binario GPL no las esconde de nadie, solo las convierte en credenciales
//! compartidas por todos los usuarios que cualquiera puede sacar del ejecutable
//! y que Last.fm revocaría en cuanto una se usara mal.
//!
//! Ni el secreto ni la clave de sesión tocan la base de datos: van al almacén
//! del sistema, como el `client_secret` de Spotify.

pub mod api;

use std::sync::Arc;

use chrono::Utc;
use localify_core::domain::ids::TrackId;
use localify_core::domain::scrobble::merece_scrobble;
use localify_core::error::{CoreError, CoreResult};
use localify_core::events::DomainEvent;
use localify_core::ports::database::{ScrobbleRepository, TrackRepository};
use localify_core::ports::platform::SecretStore;
use localify_core::ports::services::SettingsService;
use tokio::sync::broadcast;
use tracing::{debug, info, warn};

pub use api::{ClienteLastfm, Escucha, FalloLastfm, ResultadoLastfm, Sesion};

/// Claves en el almacén del sistema. Ninguna de las tres pasa por SQLite.
///
/// Vienen de `core` porque el servicio de ajustes lee la de sesión para saber
/// si hay conexión: dos copias del literal se separarían el día que alguien
/// renombre una.
use localify_core::ports::platform::claves::{
    LASTFM_API_KEY as S_API_KEY, LASTFM_API_SECRET as S_API_SECRET, LASTFM_SESION as S_SESION,
};

/// Cada cuánto se reintenta la cola pendiente.
///
/// Cinco minutos es un compromiso: lo bastante frecuente para que recuperar la
/// red se note enseguida, lo bastante espaciado para no golpear un servicio que
/// está caído. Los scrobbles nuevos no esperan a este reloj —se intentan al
/// encolarlos—, así que esto solo gobierna el rescate de lo atascado.
const REINTENTO: std::time::Duration = std::time::Duration::from_secs(300);

/// Días tras los cuales Last.fm rechaza una escucha. Es su límite, no nuestro.
const CADUCIDAD_DIAS: u16 = 14;

pub struct Dependencias {
    pub cola: Arc<dyn ScrobbleRepository>,
    pub tracks: Arc<dyn TrackRepository>,
    pub secretos: Arc<dyn SecretStore>,
    pub ajustes: Arc<dyn SettingsService>,
}

impl std::fmt::Debug for Dependencias {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Dependencias").finish_non_exhaustive()
    }
}

/// Punto único de acceso a Last.fm: credenciales, autenticación y entrega.
pub struct GestorLastfm {
    deps: Dependencias,
    /// Impide que dos vaciados corran a la vez.
    ///
    /// Sin esto, el reloj de reintento y una canción que acaba de terminar
    /// pueden entrar juntos, leer la misma página de pendientes y enviarla dos
    /// veces: el perfil del usuario acabaría con escuchas duplicadas y no habría
    /// forma de saber de dónde salieron.
    vaciando: tokio::sync::Mutex<()>,
}

impl std::fmt::Debug for GestorLastfm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GestorLastfm").finish_non_exhaustive()
    }
}

impl GestorLastfm {
    #[must_use]
    pub fn nuevo(deps: Dependencias) -> Self {
        Self {
            deps,
            vaciando: tokio::sync::Mutex::new(()),
        }
    }

    /// Guarda la clave de API y su secreto.
    ///
    /// Cambiarlas invalida la sesión: está firmada con el secreto anterior y
    /// dejarla puesta daría un error 9 en cada envío hasta que alguien se diera
    /// cuenta. Se borra aquí, que es donde se sabe.
    ///
    /// # Errors
    /// Si el almacén del sistema falla.
    pub async fn guardar_credenciales(&self, api_key: &str, api_secret: &str) -> CoreResult<()> {
        self.deps.secretos.set(S_API_KEY, api_key).await?;
        self.deps.secretos.set(S_API_SECRET, api_secret).await?;
        self.deps.secretos.delete(S_SESION).await?;
        Ok(())
    }

    /// Si hay clave y secreto guardados.
    pub async fn hay_credenciales(&self) -> bool {
        self.cliente().await.is_some()
    }

    /// Si además hay sesión: entonces se puede scrobblear.
    pub async fn esta_conectado(&self) -> bool {
        self.sesion().await.is_some()
    }

    async fn secreto(&self, clave: &str) -> Option<String> {
        self.deps
            .secretos
            .get(clave)
            .await
            .ok()
            .flatten()
            .filter(|v| !v.is_empty())
    }

    async fn sesion(&self) -> Option<String> {
        self.secreto(S_SESION).await
    }

    /// Construye un cliente con las credenciales guardadas.
    async fn cliente(&self) -> Option<ClienteLastfm> {
        let key = self.secreto(S_API_KEY).await?;
        let secret = self.secreto(S_API_SECRET).await?;
        match ClienteLastfm::nuevo(key, secret) {
            Ok(c) => Some(c),
            Err(e) => {
                warn!(error = %e, "no se pudo construir el cliente de Last.fm");
                None
            }
        }
    }

    /// Primer paso: pide un token y devuelve la URL que el usuario debe abrir.
    ///
    /// # Errors
    /// Si no hay credenciales o Last.fm no responde.
    pub async fn iniciar_autenticacion(&self) -> CoreResult<(String, String)> {
        let cliente = self
            .cliente()
            .await
            .ok_or_else(|| CoreError::invalid("faltan las credenciales de Last.fm"))?;
        let token = cliente
            .pedir_token()
            .await
            .map_err(|e| CoreError::provider_unavailable("lastfm", Box::new(e)))?;
        let url = cliente.url_de_autorizacion(&token);
        Ok((token, url))
    }

    /// Segundo paso: canjea el token ya autorizado. Devuelve el usuario.
    ///
    /// # Errors
    /// Si el usuario todavía no ha autorizado el token, o si falla la llamada.
    pub async fn completar_autenticacion(&self, token: &str) -> CoreResult<String> {
        let cliente = self
            .cliente()
            .await
            .ok_or_else(|| CoreError::invalid("faltan las credenciales de Last.fm"))?;
        let sesion = cliente
            .obtener_sesion(token)
            .await
            .map_err(|e| CoreError::provider_unavailable("lastfm", Box::new(e)))?;

        self.deps.secretos.set(S_SESION, &sesion.clave).await?;
        info!(usuario = %sesion.usuario, "sesión de Last.fm establecida");
        Ok(sesion.usuario)
    }

    /// Olvida la sesión. Las credenciales de API se quedan.
    ///
    /// # Errors
    /// Si el almacén del sistema falla.
    pub async fn desconectar(&self) -> CoreResult<()> {
        self.deps.secretos.delete(S_SESION).await
    }

    /// Cuántas escuchas esperan a poder enviarse.
    ///
    /// # Errors
    /// Si falla la consulta.
    pub async fn pendientes(&self) -> CoreResult<u64> {
        self.deps.cola.count().await
    }

    /// Si el usuario tiene el scrobbling encendido.
    async fn activo(&self) -> bool {
        self.deps.ajustes.get().await.integrations.lastfm_enabled
    }

    /// Traduce una pista del catálogo a lo que entiende Last.fm.
    ///
    /// Solo el artista principal: Last.fm casa por nombre contra su propio
    /// catálogo, y "Linkin Park, Jay-Z" no casa con nada. El artista invitado se
    /// pierde, que es preferible a que se pierda el scrobble entero.
    async fn escucha_de(&self, id: &TrackId, comienzo: chrono::DateTime<Utc>) -> Option<Escucha> {
        let pista = self.deps.tracks.get(id).await.ok().flatten()?;
        let artista = pista.artists.first()?.name.clone();
        Some(Escucha {
            artista,
            titulo: pista.title,
            album: pista.album.map(|a| a.title),
            duracion_s: Some(pista.duration.as_ms() / 1000),
            comienzo,
        })
    }

    /// Anuncia lo que está sonando.
    ///
    /// No se encola ni se reintenta a propósito: cuando el reintento saliera, la
    /// canción ya sería otra y estaríamos anunciando lo que ya no suena.
    async fn anunciar(&self, id: &TrackId) {
        if !self.activo().await {
            return;
        }
        let (Some(cliente), Some(sesion)) = (self.cliente().await, self.sesion().await) else {
            return;
        };
        let Some(escucha) = self.escucha_de(id, Utc::now()).await else {
            return;
        };
        if let Err(e) = cliente.ahora_suena(&sesion, &escucha).await {
            debug!(error = %e, "no se pudo anunciar la pista en Last.fm");
        }
    }

    /// Encola una escucha terminada, si cumple la regla de Last.fm.
    async fn encolar(&self, id: &TrackId, ms_played: u32) {
        if !self.activo().await {
            return;
        }
        let Ok(Some(pista)) = self.deps.tracks.get(id).await else {
            return;
        };
        if !merece_scrobble(ms_played, pista.duration) {
            return;
        }

        // La API quiere cuándo **empezó** a sonar. Restar lo escuchado es una
        // aproximación —una pausa larga la desplaza— y es la mejor disponible
        // sin guardar el instante de inicio en el evento. El error queda dentro
        // de la propia escucha, así que el orden de la línea temporal se
        // mantiene, que es lo que Last.fm usa.
        let comienzo = Utc::now() - chrono::Duration::milliseconds(i64::from(ms_played));

        if let Err(e) = self.deps.cola.enqueue(id, comienzo).await {
            warn!(error = %e, "no se pudo encolar el scrobble");
            return;
        }
        self.vaciar_cola().await;
    }

    /// Intenta entregar lo que haya pendiente.
    ///
    /// Silencioso a propósito: se llama al terminar cada canción y cada cinco
    /// minutos. Un fallo de red aquí no es noticia, es el estado normal de una
    /// aplicación de escritorio que a veces no tiene conexión.
    pub async fn vaciar_cola(&self) {
        // Si ya hay uno vaciando, este sobra: la cola que leería es la misma.
        let Ok(_guardia) = self.vaciando.try_lock() else {
            return;
        };
        if !self.activo().await {
            return;
        }

        // Lo que Last.fm ya no aceptaría se tira antes de intentarlo: reintentar
        // algo de hace un mes es gastar peticiones en un rechazo seguro.
        match self.deps.cola.purge_older_than(CADUCIDAD_DIAS).await {
            Ok(n) if n > 0 => info!(n, "scrobbles descartados por antigüedad"),
            Ok(_) => {}
            Err(e) => warn!(error = %e, "no se pudo purgar la cola de scrobbles"),
        }

        let (Some(cliente), Some(sesion)) = (self.cliente().await, self.sesion().await) else {
            return;
        };

        let pendientes = match self.deps.cola.pending(api::LOTE_MAXIMO_U16).await {
            Ok(p) if !p.is_empty() => p,
            Ok(_) => return,
            Err(e) => {
                warn!(error = %e, "no se pudo leer la cola de scrobbles");
                return;
            }
        };

        // Una fila cuya pista ya no existe no se puede describir, así que no se
        // puede enviar. Se saca de la cola en vez de bloquear el lote para
        // siempre.
        let mut ids = Vec::with_capacity(pendientes.len());
        let mut escuchas = Vec::with_capacity(pendientes.len());
        let mut huerfanas = Vec::new();
        for p in pendientes {
            match self.escucha_de(&p.track_id, p.started_at).await {
                Some(e) => {
                    ids.push(p.id);
                    escuchas.push(e);
                }
                None => huerfanas.push(p.id),
            }
        }
        if !huerfanas.is_empty() {
            debug!(n = huerfanas.len(), "scrobbles sin pista: se descartan");
            let _ = self.deps.cola.remove(&huerfanas).await;
        }
        if escuchas.is_empty() {
            return;
        }

        let respuesta = cliente.scrobblear(&sesion, &escuchas).await;
        match desenlace(&respuesta, ids) {
            Desenlace::Sacar(ids) => {
                info!(n = ids.len(), "scrobbles resueltos");
                if let Err(e) = self.deps.cola.remove(&ids).await {
                    warn!(error = %e, "no se pudo vaciar la cola tras entregar");
                }
            }
            Desenlace::Aplazar(ids, motivo) => {
                debug!(motivo, n = ids.len(), "scrobbles aplazados");
                let _ = self.deps.cola.mark_failed(&ids, &motivo).await;
            }
            Desenlace::Esperar => {
                warn!("la sesión de Last.fm ya no vale; los scrobbles esperan");
            }
        }
    }
}

/// Qué hacer con las filas enviadas, según lo que respondiera Last.fm.
#[derive(Debug, PartialEq, Eq)]
enum Desenlace {
    /// Fuera de la cola: entregadas, o rechazadas de forma irreversible.
    Sacar(Vec<i64>),
    /// Se quedan y se cuenta el intento.
    Aplazar(Vec<i64>, String),
    /// Se quedan **sin** contar intento: no ha fallado el envío, falta permiso.
    Esperar,
}

/// Decide el desenlace. Separado del envío para poder comprobarlo sin red.
///
/// Es la regla que sostiene la promesa del módulo, y es fácil de escribir al
/// revés: borrar al fallar pierde escuchas, y guardar siempre atasca la cola con
/// filas que Last.fm nunca va a aceptar.
fn desenlace(respuesta: &ResultadoLastfm<Vec<usize>>, ids: Vec<i64>) -> Desenlace {
    match respuesta {
        // Entregadas e ignoradas salen igual: las ignoradas no van a entrar
        // nunca por más veces que se manden.
        Ok(_) => Desenlace::Sacar(ids),
        Err(FalloLastfm::Temporal(motivo)) => Desenlace::Aplazar(ids, motivo.clone()),
        Err(FalloLastfm::Definitivo(motivo)) => {
            warn!(motivo, "Last.fm rechazó el lote sin remedio");
            Desenlace::Sacar(ids)
        }
        // La cola se queda entera: en cuanto el usuario vuelva a autorizar, sale
        // todo. Borrarla sería castigarle por algo que no depende de él, y
        // contar el intento tampoco: no ha fallado nada suyo.
        Err(FalloLastfm::SesionInvalida) => Desenlace::Esperar,
    }
}

/// El bucle del scrobbler. **No se lanza sola**: la spawnea quien la llama.
///
/// Devolver la tarea en vez de arrancarla no es ceremonia. Este crate no sabe
/// —ni debe— en qué runtime corre la aplicación: el arranque de Localify ocurre
/// en el `setup` de Tauri, que está fuera del runtime asíncrono, y un
/// `tokio::spawn` ahí entra en pánico con "there is no reactor running" antes de
/// que la ventana llegue a aparecer.
pub async fn atender(gestor: Arc<GestorLastfm>, mut eventos: broadcast::Receiver<DomainEvent>) {
    let mut reloj = tokio::time::interval(REINTENTO);
    // El primer tick de un `interval` es inmediato: aprovecha para rescatar lo
    // que quedó pendiente de la sesión anterior.
    loop {
        tokio::select! {
            _ = reloj.tick() => gestor.vaciar_cola().await,
            recibido = eventos.recv() => match recibido {
                Ok(DomainEvent::TrackChanged { track_id, .. }) => {
                    gestor.anunciar(&track_id).await;
                }
                Ok(DomainEvent::TrackFinished { track_id, ms_played, .. }) => {
                    gestor.encolar(&track_id, ms_played).await;
                }
                Ok(_) => {}
                // Retrasarse aquí no rompe nada: lo que se pierde son anuncios
                // de "ahora suena", y los scrobbles que importan están en la
                // base de datos.
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    debug!(perdidos = n, "el scrobbler se retrasó");
                }
                Err(broadcast::error::RecvError::Closed) => break,
            },
        }
    }
    debug!("scrobbler terminado");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quedarse_sin_red_no_pierde_ningun_scrobble() {
        // Es la promesa entera del módulo. Escrito al revés —sacar la fila al
        // fallar— la aplicación funcionaría igual salvo el día que importa.
        let fallo = Err(FalloLastfm::Temporal("sin conexión".into()));
        assert_eq!(
            desenlace(&fallo, vec![1, 2]),
            Desenlace::Aplazar(vec![1, 2], "sin conexión".into())
        );
    }

    #[test]
    fn lo_que_lastfm_rechaza_sin_remedio_sale_de_la_cola() {
        // Guardarlo sería atascar la cola: cada vaciado leería primero estas
        // filas, se llevaría el rechazo y las siguientes no saldrían nunca.
        let fallo = Err(FalloLastfm::Definitivo("error 6".into()));
        assert_eq!(desenlace(&fallo, vec![7]), Desenlace::Sacar(vec![7]));
    }

    #[test]
    fn una_sesion_caducada_no_gasta_intentos() {
        // No ha fallado el envío: falta permiso. Contar el intento haría que la
        // cola pareciera envenenada cuando lo único que hace falta es volver a
        // autorizar.
        let fallo = Err(FalloLastfm::SesionInvalida);
        assert_eq!(desenlace(&fallo, vec![1]), Desenlace::Esperar);
    }

    #[test]
    fn lo_ignorado_sale_junto_con_lo_entregado() {
        // Last.fm acepta el lote y marca alguna escucha como ignorada. Esas no
        // van a entrar nunca, así que reintentarlas sería un bucle silencioso.
        let respuesta: ResultadoLastfm<Vec<usize>> = Ok(vec![1]);
        assert_eq!(
            desenlace(&respuesta, vec![10, 11, 12]),
            Desenlace::Sacar(vec![10, 11, 12])
        );
    }
}

//! Discord Rich Presence.
//!
//! Enseña en el perfil de Discord qué está sonando. Como el scrobbler, es un
//! consumidor del bus: nadie depende de él y si Discord no está abierto no pasa
//! absolutamente nada.
//!
//! ## El límite de frecuencia no se salta bajando el ritmo de los eventos
//!
//! Discord acepta pocas actualizaciones por minuto y descarta las que pasen del
//! límite. La tentación es ignorar los cambios que llegan demasiado seguidos,
//! pero entonces saltar cinco canciones rápido dejaría el perfil anunciando la
//! primera para siempre. Aquí se guarda **lo último que se quiere publicar** y
//! se envía en cuanto la ventana se abre: se pierden los estados intermedios,
//! que es justo lo que sobra, y nunca el final.
//!
//! ## Hace falta un identificador de aplicación
//!
//! Discord asocia la actividad a una aplicación registrada, y ese identificador
//! es el nombre que aparece en el perfil. No se puede incrustar uno: sería el
//! de quien compiló, y todos los usuarios aparecerían bajo la misma aplicación
//! ajena. Sin identificador configurado, la integración simplemente no arranca.

pub mod ipc;

use std::sync::Arc;
use std::time::{Duration, Instant};

use localify_core::domain::ids::AlbumId;
use localify_core::domain::queue::PlayStatus;
use localify_core::events::DomainEvent;
use localify_core::ports::database::AlbumRepository;
use localify_core::ports::services::{MetadataService, PlaybackService, SettingsService};
use tokio::sync::broadcast;
use tracing::{debug, warn};

use ipc::{ConexionDiscord, ESPERA_INICIAL, Respuesta, siguiente_espera};

/// Mínimo entre dos publicaciones. Ver la cabecera del módulo.
const VENTANA: Duration = Duration::from_secs(15);

/// Actividad publicada. Se compara para no reenviar lo mismo.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Actividad {
    titulo: String,
    artista: String,
    album: Option<String>,
    /// URL pública de la carátula.
    ///
    /// Es la del proveedor —`i.scdn.co`, `lh3.googleusercontent.com`—, no la
    /// nuestra. Tiene que serlo: quien va a descargar esa imagen es el cliente
    /// de Discord, en su propio proceso, y `cover://` solo existe dentro del
    /// WebView de Localify. La caché en disco tampoco vale, porque el campo
    /// admite una URL, no una ruta.
    portada: Option<String>,
    /// Segundos desde época en que empezó a sonar, para la barra de progreso.
    comienzo: i64,
    fin: i64,
}

impl Actividad {
    /// Segunda línea del perfil: artista y, si se sabe, álbum.
    ///
    /// El álbum va aquí y no solo en el globo de la carátula. El globo hay que
    /// buscarlo con el ratón, y además solo existe si la imagen llega: si el
    /// proveedor no dio portada, esta línea es lo único que queda.
    ///
    /// El separador es un punto medio y no un guion: los títulos de álbum llevan
    /// guiones con frecuencia y "Artista - Disco - Edición" no deja ver dónde
    /// acaba uno y empieza el otro.
    fn segunda_linea(&self) -> String {
        match &self.album {
            Some(a) => format!("{} · {a}", self.artista),
            None => self.artista.clone(),
        }
    }

    /// El bloque de imagen, o `None` si no hay carátula que enseñar.
    ///
    /// Se omite entero en vez de mandarlo vacío: con `assets` presente y sin
    /// `large_image`, Discord reserva el hueco y pinta su interrogante, que es
    /// peor que no reservarlo.
    fn assets(&self) -> Option<serde_json::Value> {
        let url = self.portada.as_ref()?;
        Some(serde_json::json!({
            "large_image": url,
            // Globo al pasar por encima. Es información repetida —el álbum ya
            // está en la segunda línea— y aquí no estorba: solo aparece si
            // alguien apunta a la carátula.
            "large_text": self.album.clone().unwrap_or_else(|| self.titulo.clone()),
        }))
    }

    fn a_json(&self) -> serde_json::Value {
        let mut actividad = self.campos_fijos();

        // La clave se **añade solo si hay imagen**. Escribirla siempre y dejarla
        // a `null` cuando no la hay parece equivalente y no lo es: Discord
        // responde `4000 — "assets" must be an object` y tira la actividad
        // entera. El perfil se queda vacío por una canción sin carátula, no por
        // la carátula.
        if let Some(assets) = self.assets()
            && let Some(mapa) = actividad.as_object_mut()
        {
            mapa.insert("assets".into(), assets);
        }
        actividad
    }

    fn campos_fijos(&self) -> serde_json::Value {
        serde_json::json!({
            // 2 es "Listening": el perfil dice "Escuchando" en vez de "Jugando".
            // Un cliente antiguo que no lo entienda cae a "Jugando", que es feo
            // pero no rompe nada.
            //
            // Lo que va **después** de "Escuchando" es el nombre de la
            // aplicación registrada en Discord, y no se puede mandar desde aquí:
            // `SET_ACTIVITY` no acepta un campo `name`. Se cambia renombrando la
            // aplicación en el portal.
            "type": 2,
            "details": self.titulo,
            "state": self.segunda_linea(),
            "timestamps": { "start": self.comienzo, "end": self.fin },
        })
    }
}

pub struct Dependencias {
    pub playback: Arc<dyn PlaybackService>,
    pub ajustes: Arc<dyn SettingsService>,
    /// De donde sale la URL de la carátula.
    pub albums: Arc<dyn AlbumRepository>,
    /// Se usa solo para forzar que esa URL exista.
    ///
    /// Un álbum guardado a partir de una canción suelta no trae portada: la
    /// miniatura viaja en la búsqueda de álbumes, no en la de pistas.
    /// `ensure_cover` pide la ficha y la persiste de paso, así que llamarlo es
    /// lo que hace que la carátula aparezca también al reproducir algo que se
    /// encontró buscando canciones.
    pub metadata: Arc<dyn MetadataService>,
}

impl std::fmt::Debug for Dependencias {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Dependencias").finish_non_exhaustive()
    }
}

/// Lo que se quiere ver en Discord, según el reproductor.
///
/// `None` es "nada": ni pausado ni parado se anuncian. Un perfil que dice
/// "escuchando" algo que lleva media hora en pausa es peor que uno vacío.
async fn deseada(deps: &Dependencias) -> Option<Actividad> {
    let estado = deps.playback.state().await;
    if estado.status != PlayStatus::Playing {
        return None;
    }
    let pista = estado.track?;

    let (posicion, _) = deps.playback.position();
    let ahora = chrono::Utc::now().timestamp();
    let comienzo = ahora - i64::from(posicion.as_ms() / 1000);

    Some(Actividad {
        titulo: pista.title,
        artista: pista.artist_display,
        album: pista.album_title,
        portada: portada_de(deps, pista.album_id.as_ref()).await,
        comienzo,
        fin: comienzo + i64::from(pista.duration.as_ms() / 1000),
    })
}

/// URL pública de la carátula de un álbum.
///
/// Ocurre una vez por canción y fuera de cualquier camino crítico, así que puede
/// permitirse pedir la ficha al proveedor si hace falta. Cualquier fallo devuelve
/// `None`: quedarse sin imagen no puede impedir que se publique lo demás.
async fn portada_de(deps: &Dependencias, album: Option<&AlbumId>) -> Option<String> {
    let album = album?;
    // Barato cuando ya está cacheada —comprueba que el fichero existe y vuelve—,
    // y es lo que rellena `cover_url` cuando el álbum entró como referencia de
    // una canción suelta.
    let _ = deps.metadata.ensure_cover(album).await;
    deps.albums.get(album).await.ok()??.cover_url
}

/// El bucle de Discord. **No se lanza sola**: la spawnea quien la llama, por el
/// mismo motivo que en el scrobbler —el arranque de Localify ocurre fuera del
/// runtime asíncrono—.
pub async fn atender(deps: Dependencias, mut eventos: broadcast::Receiver<DomainEvent>) {
    {
        let mut conexion: Option<ConexionDiscord> = None;
        let mut espera = ESPERA_INICIAL;
        let mut reintentar_en = Instant::now();

        let mut deseado: Option<Actividad> = None;
        let mut publicado: Option<Actividad> = None;
        // Una ventana en el pasado: la primera publicación no espera. En el
        // arranque de la máquina el reloj puede no llevar quince segundos
        // encendido, así que `checked_sub` puede no dar nada; entonces vale
        // `now`, y lo único que pasa es que la primera actualización espera.
        let mut ultimo_envio = Instant::now()
            .checked_sub(VENTANA)
            .unwrap_or_else(Instant::now);

        loop {
            // Sin nada que publicar, el plazo es lejano: la tarea se queda
            // dormida esperando eventos en vez de despertarse cada segundo.
            let hay_trabajo = deseado != publicado;
            let plazo = if hay_trabajo {
                (ultimo_envio + VENTANA).max(reintentar_en)
            } else {
                Instant::now() + Duration::from_secs(3600)
            };

            tokio::select! {
                recibido = eventos.recv() => match recibido {
                    Ok(DomainEvent::TrackChanged { .. } | DomainEvent::PlayStatusChanged { .. }) => {
                        deseado = if activo(&deps).await { deseada(&deps).await } else { None };
                    }
                    Ok(_) => {}
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        debug!(perdidos = n, "Discord se retrasó");
                        // Perder eventos aquí importa poco, pero el estado
                        // publicado puede haberse quedado viejo: se recalcula.
                        deseado = if activo(&deps).await { deseada(&deps).await } else { None };
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                },
                () = tokio::time::sleep_until(plazo.into()), if hay_trabajo => {
                    let Some(client_id) = client_id(&deps).await else {
                        // Desactivado o sin identificador: se suelta la conexión
                        // y se deja de intentar hasta que vuelva a activarse.
                        conexion = None;
                        publicado.clone_from(&deseado);
                        continue;
                    };

                    if conexion.is_none() {
                        conexion = ConexionDiscord::conectar(&client_id).await;
                        if conexion.is_none() {
                            // Discord cerrado: se espera más cada vez y se
                            // vuelve a la escucha de eventos.
                            espera = siguiente_espera(espera);
                            reintentar_en = Instant::now() + espera;
                            continue;
                        }
                        espera = ESPERA_INICIAL;
                    }

                    let Some(c) = conexion.as_mut() else { continue };
                    match c.publicar(deseado.as_ref().map(Actividad::a_json)).await {
                        Ok(Respuesta::Aceptada) => {
                            publicado.clone_from(&deseado);
                            ultimo_envio = Instant::now();
                        }
                        Ok(Respuesta::Rechazada(motivo)) => {
                            // La tubería está bien; lo que está mal es lo que se
                            // mandó. Reconectar no arregla nada y reintentarlo
                            // daría el mismo error para siempre, así que se
                            // apunta como publicado —la siguiente canción trae
                            // otra actividad y tendrá su oportunidad— y se deja
                            // dicho en el log, que es lo que faltaba.
                            warn!(%motivo, "Discord rechazó la actividad");
                            publicado.clone_from(&deseado);
                            ultimo_envio = Instant::now();
                        }
                        Err(e) => {
                            debug!(error = %e, "se perdió la conexión con Discord");
                            // La tubería queda inservible tras un error de
                            // escritura: se tira y se reconecta desde cero.
                            conexion = None;
                            espera = siguiente_espera(espera);
                            reintentar_en = Instant::now() + espera;
                        }
                    }
                }
            }
        }
        debug!("integración de Discord terminada");
    }
}

async fn activo(deps: &Dependencias) -> bool {
    let i = deps.ajustes.get().await.integrations;
    i.discord_enabled && i.discord_client_id.is_some()
}

async fn client_id(deps: &Dependencias) -> Option<String> {
    let i = deps.ajustes.get().await.integrations;
    if !i.discord_enabled {
        return None;
    }
    i.discord_client_id.filter(|c| !c.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn actividad() -> Actividad {
        Actividad {
            titulo: "Faint".into(),
            artista: "Linkin Park".into(),
            album: Some("Meteora".into()),
            portada: Some("https://i.scdn.co/image/abc".into()),
            comienzo: 1_700_000_000,
            fin: 1_700_000_162,
        }
    }

    #[test]
    fn la_actividad_lleva_tipo_escuchando() {
        let json = actividad().a_json();
        assert_eq!(json["type"], 2);
        assert_eq!(json["details"], "Faint");
        assert_eq!(json["timestamps"]["end"], 1_700_000_162_i64);
    }

    #[test]
    fn el_album_va_en_la_segunda_linea_ademas_del_globo() {
        // Estuvo **solo** en `assets.large_text`, que es el globo de una imagen:
        // sin imagen no lo veía nadie. El síntoma de ese fallo es que el JSON
        // parece correcto y en pantalla no aparece nada.
        let json = actividad().a_json();
        assert_eq!(json["state"], "Linkin Park · Meteora");
        assert_eq!(json["assets"]["large_text"], "Meteora");
    }

    #[test]
    fn sin_album_la_segunda_linea_es_solo_el_artista() {
        let suelta = Actividad {
            album: None,
            ..actividad()
        };
        assert_eq!(suelta.a_json()["state"], "Linkin Park");
    }

    #[test]
    fn la_caratula_viaja_como_url_publica() {
        // Tiene que ser la del proveedor: quien la descarga es el cliente de
        // Discord, en otro proceso, donde `cover://` no existe.
        let json = actividad().a_json();
        assert_eq!(json["assets"]["large_image"], "https://i.scdn.co/image/abc");
    }

    #[test]
    fn sin_caratula_la_clave_de_imagen_no_existe() {
        // No basta con que `assets` valga `null`: Discord contesta
        // `4000 — "assets" must be an object` y **descarta la actividad
        // entera**, así que una canción sin carátula vaciaba el perfil en vez
        // de enseñarse sin imagen. La clave tiene que no estar.
        //
        // El test anterior comprobaba `is_null()`, que era cierto con el fallo
        // vivo: `null` y "ausente" se leen igual desde `serde_json` si se
        // pregunta por el valor en vez de por la clave.
        let sin = Actividad {
            portada: None,
            ..actividad()
        };
        let json = sin.a_json();
        let mapa = json.as_object().expect("la actividad es un objeto");
        assert!(
            !mapa.contains_key("assets"),
            "la clave `assets` no debe existir sin carátula, ni siquiera a null"
        );
        assert_eq!(json["details"], "Faint", "lo demás se sigue publicando");
    }

    #[test]
    fn un_marco_de_error_no_es_una_publicacion_correcta() {
        // Discord rechaza con un marco normal, no con un fallo de escritura.
        // Tomarlo por bueno es lo que dejó la integración muda sin una sola
        // línea en el log.
        let error = serde_json::json!({
            "cmd": "SET_ACTIVITY",
            "evt": "ERROR",
            "data": { "code": 4000, "message": "\"assets\" must be an object" },
        });
        assert_eq!(
            Respuesta::de(&error),
            Respuesta::Rechazada("\"assets\" must be an object".into())
        );

        let bien = serde_json::json!({ "cmd": "SET_ACTIVITY", "evt": null });
        assert_eq!(Respuesta::de(&bien), Respuesta::Aceptada);
    }

    #[test]
    fn dos_actividades_iguales_no_se_reenvian() {
        // Es lo que impide gastar la ventana de publicación en repetir lo mismo:
        // el bucle compara `deseado` con `publicado` antes de hablar con Discord.
        assert_eq!(actividad(), actividad());
        let otra = Actividad {
            titulo: "Numb".into(),
            ..actividad()
        };
        assert_ne!(actividad(), otra);
    }
}

//! Cliente de la API 2.0 de Last.fm.
//!
//! Cubre lo justo para scrobblear: autenticarse, decir qué suena y entregar
//! escuchas. Nada de leer el perfil del usuario ni sus recomendaciones: eso es
//! justo lo que Localify hace por su cuenta con el historial local.
//!
//! ## La firma
//!
//! Cada llamada autenticada lleva un `api_sig`: se ordenan los parámetros por
//! nombre, se concatenan `nombre` y `valor` sin separadores, se pega el secreto
//! al final y se pasa por MD5. `format` y `callback` quedan fuera —lo dice la
//! documentación— y es un detalle que no perdona: incluir `format` devuelve un
//! error 13 sin más explicación.
//!
//! ## Fallos que se reintentan y fallos que no
//!
//! Es la distinción que sostiene la cola. Quedarse sin red o que Last.fm
//! responda un 503 significa "vuelve luego" y la escucha se queda esperando.
//! Que rechace la firma o diga que la pista no existe significa que reintentar
//! va a dar exactamente el mismo resultado hasta el fin de los tiempos, y
//! guardarla sería llenar la cola de basura. Tratar todo como reintentable
//! parece más seguro y es peor: una fila envenenada bloquea las siguientes cada
//! vez que se intenta vaciar la cola.

use std::collections::BTreeMap;
use std::time::Duration;

use chrono::{DateTime, Utc};
use md5::{Digest, Md5};
use serde::Deserialize;
use tracing::debug;

/// Punto de entrada de la API.
const BASE: &str = "https://ws.audioscrobbler.com/2.0/";

/// Dónde manda el usuario a autorizar la aplicación.
const AUTORIZACION: &str = "https://www.last.fm/api/auth/";

/// Tope por petición. El scrobbling ocurre de fondo: si tarda más, se reintenta
/// luego y nadie se entera.
const TIMEOUT: Duration = Duration::from_secs(10);

/// Máximo de escuchas por lote que acepta `track.scrobble`.
pub const LOTE_MAXIMO: usize = 50;

/// El mismo tope, para pedir esa cantidad a la cola sin convertir en cada uso.
pub const LOTE_MAXIMO_U16: u16 = 50;

const _: () = assert!(LOTE_MAXIMO_U16 as usize == LOTE_MAXIMO);

/// Códigos de error de Last.fm que sí merecen reintento.
///
/// 8 (fallo de operación), 11 y 16 (servicio no disponible) y 29 (límite de
/// peticiones) describen un mal momento, no una petición mal hecha.
const REINTENTABLES: &[u16] = &[8, 11, 16, 29];

/// Código de "sesión inválida". Es el único que exige intervención del usuario.
const SESION_INVALIDA: u16 = 9;

#[derive(Debug)]
pub enum FalloLastfm {
    /// Vuelve a intentarlo: no hay red, o el servicio está de capa caída.
    Temporal(String),
    /// No lo intentes más: la petición está mal o la pista no le vale.
    Definitivo(String),
    /// La sesión ya no sirve. Hay que volver a autorizar la aplicación.
    SesionInvalida,
}

impl std::fmt::Display for FalloLastfm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Temporal(m) => write!(f, "temporal: {m}"),
            Self::Definitivo(m) => write!(f, "definitivo: {m}"),
            Self::SesionInvalida => write!(f, "la sesión de Last.fm ya no es válida"),
        }
    }
}

impl std::error::Error for FalloLastfm {}

pub type ResultadoLastfm<T> = Result<T, FalloLastfm>;

/// Una escucha lista para enviar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Escucha {
    pub artista: String,
    pub titulo: String,
    pub album: Option<String>,
    pub duracion_s: Option<u32>,
    /// Cuándo **empezó** a sonar.
    pub comienzo: DateTime<Utc>,
}

/// La sesión tal y como la envuelve la respuesta de `auth.getSession`.
#[derive(Deserialize)]
struct Envoltorio {
    session: DatosDeSesion,
}

#[derive(Deserialize)]
struct DatosDeSesion {
    name: String,
    key: String,
}

/// Sesión concedida por el usuario.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sesion {
    pub usuario: String,
    /// No caduca, y por eso no toca la base de datos: va al almacén del sistema.
    pub clave: String,
}

pub struct ClienteLastfm {
    http: reqwest::Client,
    api_key: String,
    api_secret: String,
}

impl std::fmt::Debug for ClienteLastfm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Sin el secreto ni la clave: este `Debug` puede acabar en un log.
        f.debug_struct("ClienteLastfm").finish_non_exhaustive()
    }
}

impl ClienteLastfm {
    /// # Errors
    /// Si el cliente HTTP no se puede construir.
    pub fn nuevo(api_key: String, api_secret: String) -> Result<Self, reqwest::Error> {
        Ok(Self {
            http: reqwest::Client::builder().timeout(TIMEOUT).build()?,
            api_key,
            api_secret,
        })
    }

    /// Firma un conjunto de parámetros.
    ///
    /// El `BTreeMap` no es casualidad: la firma exige **orden alfabético** por
    /// nombre y un mapa ordenado lo garantiza por construcción, en vez de
    /// depender de que quien llame recuerde ordenar.
    fn firmar(&self, params: &BTreeMap<String, String>) -> String {
        let mut base = String::new();
        for (clave, valor) in params {
            // `format` y `callback` quedan fuera de la firma por especificación.
            if clave == "format" || clave == "callback" {
                continue;
            }
            base.push_str(clave);
            base.push_str(valor);
        }
        base.push_str(&self.api_secret);

        let resumen = Md5::digest(base.as_bytes());
        // Hexadecimal en minúsculas: en mayúsculas Last.fm devuelve error 13.
        resumen.iter().fold(String::new(), |mut s, b| {
            use std::fmt::Write as _;
            let _ = write!(s, "{b:02x}");
            s
        })
    }

    /// Lanza una llamada firmada y devuelve el cuerpo ya interpretado.
    async fn llamar(
        &self,
        metodo: &str,
        mut params: BTreeMap<String, String>,
        post: bool,
    ) -> ResultadoLastfm<serde_json::Value> {
        params.insert("method".into(), metodo.into());
        params.insert("api_key".into(), self.api_key.clone());
        let firma = self.firmar(&params);
        params.insert("api_sig".into(), firma);
        // Después de firmar: `format` no entra en la firma pero sí en la
        // petición, que es lo que hace que la respuesta venga en JSON.
        params.insert("format".into(), "json".into());

        let peticion = if post {
            self.http.post(BASE).form(&params)
        } else {
            self.http.get(BASE).query(&params)
        };

        let respuesta = peticion
            .send()
            .await
            // Un fallo de transporte es siempre temporal: sin red, con el wifi
            // a medias, con el servicio caído. Nunca hay que descartar por esto.
            .map_err(|e| FalloLastfm::Temporal(e.to_string()))?;

        let estado = respuesta.status();
        let cuerpo: serde_json::Value = respuesta.json().await.map_err(|e| {
            if estado.is_server_error() {
                FalloLastfm::Temporal(format!("{estado}: {e}"))
            } else {
                FalloLastfm::Definitivo(format!("respuesta ilegible ({estado}): {e}"))
            }
        })?;

        // Last.fm responde 200 con un cuerpo de error, así que el estado HTTP no
        // basta para saber si salió bien.
        if let Some(codigo) = cuerpo.get("error").and_then(serde_json::Value::as_u64) {
            let codigo = u16::try_from(codigo).unwrap_or(u16::MAX);
            let mensaje = cuerpo
                .get("message")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("sin mensaje")
                .to_owned();
            return Err(match codigo {
                SESION_INVALIDA => FalloLastfm::SesionInvalida,
                c if REINTENTABLES.contains(&c) => FalloLastfm::Temporal(mensaje),
                _ => FalloLastfm::Definitivo(format!("error {codigo}: {mensaje}")),
            });
        }

        Ok(cuerpo)
    }

    /// Primer paso de la autenticación: un token que aún no vale para nada.
    ///
    /// # Errors
    /// Si la petición falla.
    pub async fn pedir_token(&self) -> ResultadoLastfm<String> {
        let cuerpo = self.llamar("auth.getToken", BTreeMap::new(), false).await?;
        cuerpo
            .get("token")
            .and_then(serde_json::Value::as_str)
            .map(ToOwned::to_owned)
            .ok_or_else(|| FalloLastfm::Definitivo("la respuesta no traía token".into()))
    }

    /// La página que el usuario tiene que abrir para autorizar.
    #[must_use]
    pub fn url_de_autorizacion(&self, token: &str) -> String {
        format!("{AUTORIZACION}?api_key={}&token={token}", self.api_key)
    }

    /// Último paso: canjea el token autorizado por una sesión permanente.
    ///
    /// # Errors
    /// Si el usuario aún no ha autorizado el token, o si la petición falla.
    pub async fn obtener_sesion(&self, token: &str) -> ResultadoLastfm<Sesion> {
        let mut params = BTreeMap::new();
        params.insert("token".to_owned(), token.to_owned());
        let cuerpo = self.llamar("auth.getSession", params, false).await?;

        let datos: Envoltorio = serde_json::from_value(cuerpo)
            .map_err(|e| FalloLastfm::Definitivo(format!("sesión ilegible: {e}")))?;
        Ok(Sesion {
            usuario: datos.session.name,
            clave: datos.session.key,
        })
    }

    /// Dice qué está sonando ahora mismo.
    ///
    /// No se encola ni se reintenta: para cuando el reintento saliera, la
    /// canción ya sería otra y estaríamos anunciando el pasado.
    ///
    /// # Errors
    /// Si la petición falla.
    pub async fn ahora_suena(&self, sesion: &str, escucha: &Escucha) -> ResultadoLastfm<()> {
        let mut params = campos_de_pista(escucha);
        params.insert("sk".to_owned(), sesion.to_owned());
        self.llamar("track.updateNowPlaying", params, true).await?;
        Ok(())
    }

    /// Entrega un lote de escuchas.
    ///
    /// Devuelve los índices —**dentro de `escuchas`**— que Last.fm rechazó de
    /// forma definitiva. Un lote puede salir bien en conjunto y traer alguna
    /// pista ignorada dentro; devolverlas permite sacarlas de la cola en vez de
    /// reintentarlas para siempre.
    ///
    /// # Errors
    /// Si la petición falla entera.
    pub async fn scrobblear(
        &self,
        sesion: &str,
        escuchas: &[Escucha],
    ) -> ResultadoLastfm<Vec<usize>> {
        if escuchas.is_empty() {
            return Ok(Vec::new());
        }
        if escuchas.len() > LOTE_MAXIMO {
            return Err(FalloLastfm::Definitivo(format!(
                "un lote no puede pasar de {LOTE_MAXIMO} escuchas"
            )));
        }

        let mut params = BTreeMap::new();
        for (i, e) in escuchas.iter().enumerate() {
            for (clave, valor) in campos_de_pista(e) {
                params.insert(format!("{clave}[{i}]"), valor);
            }
            params.insert(
                format!("timestamp[{i}]"),
                e.comienzo.timestamp().to_string(),
            );
        }
        params.insert("sk".to_owned(), sesion.to_owned());

        let cuerpo = self.llamar("track.scrobble", params, true).await?;
        Ok(ignoradas(&cuerpo, escuchas.len()))
    }
}

/// Los campos que describen una pista, comunes a `nowPlaying` y a `scrobble`.
fn campos_de_pista(e: &Escucha) -> BTreeMap<String, String> {
    let mut params = BTreeMap::new();
    params.insert("artist".to_owned(), e.artista.clone());
    params.insert("track".to_owned(), e.titulo.clone());
    if let Some(album) = &e.album {
        params.insert("album".to_owned(), album.clone());
    }
    if let Some(d) = e.duracion_s {
        params.insert("duration".to_owned(), d.to_string());
    }
    params
}

/// Índices que Last.fm marcó como ignorados dentro de un lote aceptado.
///
/// La respuesta cambia de forma según el número de escuchas: con una sola,
/// `scrobbles.scrobble` es un objeto; con varias, un array. Es el clásico de las
/// APIs que serializan XML a JSON, y no contemplarlo hace que el caso más común
/// —una escucha suelta— se lea siempre como "nada ignorado".
fn ignoradas(cuerpo: &serde_json::Value, cuantas: usize) -> Vec<usize> {
    let Some(nodo) = cuerpo.pointer("/scrobbles/scrobble") else {
        return Vec::new();
    };
    let items: Vec<&serde_json::Value> = match nodo {
        serde_json::Value::Array(v) => v.iter().collect(),
        otro => vec![otro],
    };

    items
        .iter()
        .take(cuantas)
        .enumerate()
        .filter_map(|(i, item)| {
            let codigo = item
                .pointer("/ignoredMessage/code")
                .and_then(serde_json::Value::as_str)?;
            // "0" es "no se ignoró". Cualquier otro código es un motivo por el
            // que esta escucha concreta no va a entrar nunca.
            if codigo == "0" {
                return None;
            }
            let motivo = item
                .pointer("/ignoredMessage/#text")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            debug!(indice = i, codigo, motivo, "Last.fm ignoró una escucha");
            Some(i)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cliente() -> ClienteLastfm {
        ClienteLastfm::nuevo("clave".into(), "secreto".into()).expect("construye")
    }

    #[test]
    fn la_firma_es_el_md5_del_orden_alfabetico_mas_el_secreto() {
        // Comprobado a mano: md5("api_keyclavemethodauth.getTokensecreto").
        let mut params = BTreeMap::new();
        params.insert("method".to_owned(), "auth.getToken".to_owned());
        params.insert("api_key".to_owned(), "clave".to_owned());

        let esperado = {
            use std::fmt::Write as _;
            Md5::digest(b"api_keyclavemethodauth.getTokensecreto")
                .iter()
                .fold(String::new(), |mut s, b| {
                    let _ = write!(s, "{b:02x}");
                    s
                })
        };
        assert_eq!(cliente().firmar(&params), esperado);
    }

    #[test]
    fn format_queda_fuera_de_la_firma() {
        // Incluirlo devuelve un error 13 ("firma inválida") sin más pistas, y es
        // de los fallos más difíciles de ver leyendo el código.
        let mut sin = BTreeMap::new();
        sin.insert("method".to_owned(), "x".to_owned());

        let mut con = sin.clone();
        con.insert("format".to_owned(), "json".to_owned());

        assert_eq!(cliente().firmar(&sin), cliente().firmar(&con));
    }

    #[test]
    fn una_escucha_suelta_devuelve_un_objeto_no_un_array() {
        // Con una sola escucha la API no envuelve en array. Sin contemplarlo, el
        // caso más común de todos se leería siempre como "nada ignorado".
        // Delimitador de dos almohadillas: el propio JSON de Last.fm contiene
        // `"#text"`, que cerraría un `r#"…"#` a mitad de la cadena.
        let cuerpo: serde_json::Value = serde_json::from_str(
            r##"{"scrobbles":{"scrobble":{"ignoredMessage":{"code":"1","#text":"artista filtrado"}}}}"##,
        )
        .expect("json");
        assert_eq!(ignoradas(&cuerpo, 1), vec![0]);
    }

    #[test]
    fn el_codigo_cero_significa_aceptada() {
        let cuerpo: serde_json::Value = serde_json::from_str(
            r##"{"scrobbles":{"scrobble":[
                 {"ignoredMessage":{"code":"0","#text":""}},
                 {"ignoredMessage":{"code":"3","#text":"marca de tiempo muy antigua"}}
               ]}}"##,
        )
        .expect("json");
        assert_eq!(ignoradas(&cuerpo, 2), vec![1]);
    }

    #[test]
    fn una_pista_sin_album_no_manda_el_campo() {
        // Mandar `album=""` hace que Last.fm guarde el scrobble con un álbum
        // vacío en vez de dejar que él lo resuelva por su catálogo.
        let e = Escucha {
            artista: "Linkin Park".into(),
            titulo: "Faint".into(),
            album: None,
            duracion_s: None,
            comienzo: Utc::now(),
        };
        let campos = campos_de_pista(&e);
        assert!(!campos.contains_key("album"));
        assert!(!campos.contains_key("duration"));
    }
}

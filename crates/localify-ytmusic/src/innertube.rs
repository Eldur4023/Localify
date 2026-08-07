//! Cliente de la API InnerTube de YouTube Music.
//!
//! ## Qué es esto
//!
//! `music.youtube.com` no habla con su servidor por una API pública sino por
//! `youtubei/v1`, un endpoint JSON que espera un objeto `context` describiendo
//! qué cliente eres. No pide clave ni sesión. Es lo que usa la propia web, y es
//! lo que usan todos los clientes alternativos.
//!
//! ## No está documentada, y eso tiene consecuencias
//!
//! No hay contrato: los nombres de los campos pueden cambiar sin aviso. Por eso
//! aquí **nada se deserializa a structs con `#[derive(Deserialize)]`**. Se
//! navega el JSON como `serde_json::Value` y cada acceso puede fallar
//! devolviendo `None`.
//!
//! Con structs, un campo que desaparezca rompe la deserialización entera y la
//! búsqueda deja de funcionar del todo. Navegando, lo que se pierde es ese
//! campo: una canción sin álbum sigue siendo una canción que se puede
//! reproducir. Es la diferencia entre degradarse y caerse.
//!
//! ## La forma de la respuesta
//!
//! ```text
//! contents
//!   tabbedSearchResultsRenderer.tabs[0].tabRenderer.content
//!     sectionListRenderer.contents[]
//!       musicShelfRenderer            ← una por tipo de resultado
//!         contents[]
//!           musicResponsiveListItemRenderer
//!             flexColumns[]           ← título, subtítulo, ...
//!             playlistItemData.videoId
//! ```

use serde_json::{Value, json};

/// Endpoint de búsqueda.
const URL_BUSQUEDA: &str = "https://music.youtube.com/youtubei/v1/search";

/// Endpoint de navegación: álbumes, artistas y listas por identificador.
const URL_BROWSE: &str = "https://music.youtube.com/youtubei/v1/browse";

/// Endpoint del reproductor. Es la vía para los datos de un vídeo suelto.
const URL_PLAYER: &str = "https://music.youtube.com/youtubei/v1/player";

/// Cliente que dice ser la web de YouTube Music.
///
/// `WEB_REMIX` es el nombre interno de esa web. Decir otro devuelve resultados
/// de YouTube a secas, sin la estructura de artista y álbum que es justamente
/// la razón de usar esta API.
const CLIENTE: &str = "WEB_REMIX";

/// Versión declarada del cliente.
///
/// No tiene que ser la última: el servidor acepta versiones anteriores. Fijarla
/// evita que una actualización de YouTube cambie el formato de la respuesta sin
/// que nos enteremos.
const VERSION_CLIENTE: &str = "1.20240101.01.00";

/// Filtros de búsqueda.
///
/// Son cadenas opacas: van codificadas en protobuf y base64 dentro del propio
/// parámetro. No se pueden construir, solo copiar de lo que envía la web, y por
/// eso están aquí como constantes con nombre en vez de esparcidas por el código.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Filtro {
    Canciones,
    Albumes,
    Artistas,
    ListasDeReproduccion,
}

impl Filtro {
    #[must_use]
    pub const fn params(self) -> &'static str {
        match self {
            Self::Canciones => "EgWKAQIIAWoMEA4QChADEAQQCRAF",
            Self::Albumes => "EgWKAQIYAWoMEA4QChADEAQQCRAF",
            Self::Artistas => "EgWKAQIgAWoMEA4QChADEAQQCRAF",
            Self::ListasDeReproduccion => "EgWKAQIoAWoMEA4QChADEAQQCRAF",
        }
    }
}

/// Cliente HTTP contra InnerTube.
#[derive(Debug, Clone)]
pub struct ClienteInnerTube {
    http: reqwest::Client,
    idioma: String,
    pais: String,
}

impl ClienteInnerTube {
    /// # Errors
    /// Si el cliente HTTP no se puede construir.
    pub fn nuevo(idioma: &str, pais: &str) -> Result<Self, reqwest::Error> {
        Ok(Self {
            http: reqwest::Client::builder()
                // Sin un agente de navegador la API responde, pero conviene ser
                // reconocible y coherente con el `context` que se envía.
                .user_agent(
                    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                     (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36",
                )
                .timeout(std::time::Duration::from_secs(10))
                .build()?,
            idioma: idioma.to_owned(),
            pais: pais.to_owned(),
        })
    }

    /// Lanza una búsqueda y devuelve el JSON crudo.
    ///
    /// # Errors
    /// Si la petición falla o la respuesta no es JSON.
    pub async fn buscar(&self, consulta: &str, filtro: Filtro) -> Result<Value, reqwest::Error> {
        let cuerpo = json!({
            "context": {
                "client": {
                    "clientName": CLIENTE,
                    "clientVersion": VERSION_CLIENTE,
                    "hl": self.idioma,
                    "gl": self.pais,
                }
            },
            "query": consulta,
            "params": filtro.params(),
        });

        self.enviar(URL_BUSQUEDA, cuerpo).await
    }

    /// Navega a un álbum, artista o lista por su identificador.
    ///
    /// # Errors
    /// Si la petición falla o la respuesta no es JSON.
    pub async fn navegar(&self, browse_id: &str) -> Result<Value, reqwest::Error> {
        let cuerpo = json!({
            "context": { "client": self.cliente_json() },
            "browseId": browse_id,
        });
        self.enviar(URL_BROWSE, cuerpo).await
    }

    /// Datos de un vídeo suelto.
    ///
    /// Es la única vía para una pista de la que solo se conoce el `videoId`,
    /// que es el caso al releer del catálogo algo guardado hace meses.
    ///
    /// # Errors
    /// Si la petición falla o la respuesta no es JSON.
    pub async fn reproductor(&self, video_id: &str) -> Result<Value, reqwest::Error> {
        let cuerpo = json!({
            "context": { "client": self.cliente_json() },
            "videoId": video_id,
        });
        self.enviar(URL_PLAYER, cuerpo).await
    }

    fn cliente_json(&self) -> Value {
        json!({
            "clientName": CLIENTE,
            "clientVersion": VERSION_CLIENTE,
            "hl": self.idioma,
            "gl": self.pais,
        })
    }

    async fn enviar(&self, url: &str, cuerpo: Value) -> Result<Value, reqwest::Error> {
        self.http
            .post(url)
            .json(&cuerpo)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Navegación del JSON
// ─────────────────────────────────────────────────────────────────────────────

/// Todos los nodos que llevan la clave `nombre`, en cualquier profundidad.
///
/// ## Por qué se busca en vez de indexar por ruta
///
/// La ruta canónica de una respuesta de navegación cambia según el diseño que
/// sirva YouTube: la misma consulta llega a veces bajo
/// `singleColumnBrowseResultsRenderer` y otras bajo
/// `twoColumnBrowseResultsRenderer`. Una ruta fija funciona hasta el día del
/// cambio y entonces devuelve vacío sin más.
///
/// Recorrer el árbol cuesta unos milisegundos sobre respuestas de menos de un
/// megabyte y es indiferente a cómo esté envuelto lo que se busca.
#[must_use]
pub fn buscar_todos<'a>(raiz: &'a Value, nombre: &str) -> Vec<&'a Value> {
    let mut salida = Vec::new();
    let mut pila = vec![raiz];

    while let Some(nodo) = pila.pop() {
        match nodo {
            Value::Object(mapa) => {
                for (clave, valor) in mapa {
                    if clave == nombre {
                        salida.push(valor);
                    }
                    pila.push(valor);
                }
            }
            Value::Array(v) => pila.extend(v.iter()),
            _ => {}
        }
    }
    salida
}

/// El primer nodo con la clave `nombre`, o `None`.
#[must_use]
pub fn buscar_uno<'a>(raiz: &'a Value, nombre: &str) -> Option<&'a Value> {
    buscar_todos(raiz, nombre).into_iter().next()
}

/// Elementos de lista de una respuesta de navegación.
///
/// Vale igual para el listado de pistas de un álbum y para las mejores
/// canciones de un artista: los dos usan `musicShelfRenderer`.
#[must_use]
pub fn elementos_de_lista(raiz: &Value) -> Vec<&Value> {
    buscar_todos(raiz, "musicResponsiveListItemRenderer")
}

/// Elementos de carrusel: la discografía de un artista, por ejemplo.
#[must_use]
pub fn elementos_de_carrusel(raiz: &Value) -> Vec<&Value> {
    buscar_todos(raiz, "musicTwoRowItemRenderer")
}

/// Duración de una fila de álbum.
///
/// En el listado de un álbum la duración no va en las columnas flexibles sino
/// en una fija, que es una estructura distinta. Buscarla donde va en la
/// búsqueda devolvería siempre `None` y todas las pistas del álbum quedarían a
/// cero.
#[must_use]
pub fn duracion_de_columna_fija(elemento: &Value) -> Option<u32> {
    let texto = texto_de(
        elemento
            .get("fixedColumns")?
            .as_array()?
            .first()?
            .get("musicResponsiveListItemFixedColumnRenderer")?
            .get("text"),
    )?;
    duracion_ms(&texto)
}

/// Estanterías de resultados de una respuesta de búsqueda.
///
/// Devuelve pares (título de la estantería, elementos). El título viene
/// traducido al idioma pedido, así que **no se usa para decidir nada**: solo
/// sirve para diagnóstico. Lo que determina el tipo de resultado es el filtro
/// que se envió.
#[must_use]
pub fn estanterias(respuesta: &Value) -> Vec<(String, Vec<&Value>)> {
    let secciones = respuesta
        .pointer("/contents/tabbedSearchResultsRenderer/tabs/0/tabRenderer/content/sectionListRenderer/contents")
        .and_then(Value::as_array);

    let Some(secciones) = secciones else {
        return Vec::new();
    };

    secciones
        .iter()
        .filter_map(|s| {
            let estanteria = s.get("musicShelfRenderer")?;
            let titulo = texto_de(estanteria.get("title")).unwrap_or_default();
            let elementos = estanteria
                .get("contents")?
                .as_array()?
                .iter()
                .filter_map(|c| c.get("musicResponsiveListItemRenderer"))
                .collect();
            Some((titulo, elementos))
        })
        .collect()
}

/// Texto de un nodo `{ runs: [{ text }] }`, concatenando todos los tramos.
#[must_use]
pub fn texto_de(nodo: Option<&Value>) -> Option<String> {
    let runs = nodo?.get("runs")?.as_array()?;
    let s: String = runs
        .iter()
        .filter_map(|r| r.get("text")?.as_str())
        .collect();
    if s.is_empty() { None } else { Some(s) }
}

/// Textos de la columna `n` de un elemento.
///
/// Las columnas son posicionales: la 0 es el título y la 1 el subtítulo con
/// artista, álbum y duración separados por `•`. No hay nombres de campo que
/// consultar, así que la posición es lo único que hay.
#[must_use]
pub fn columna(elemento: &Value, n: usize) -> Option<String> {
    texto_de(
        elemento
            .get("flexColumns")?
            .as_array()?
            .get(n)?
            .get("musicResponsiveListItemFlexColumnRenderer")?
            .get("text"),
    )
}

/// Tramos de la columna `n` con su `browseId`/`videoId` si lo llevan.
///
/// El subtítulo trae los nombres **y** los enlaces: el artista lleva el
/// `browseId` de su página y el álbum el suyo. Es la única forma de obtener
/// identificadores estables de artista y álbum, porque por texto habría que
/// adivinar dónde acaba uno y empieza el otro.
#[must_use]
pub fn tramos(elemento: &Value, n: usize) -> Vec<(String, Option<String>)> {
    let runs = elemento
        .get("flexColumns")
        .and_then(Value::as_array)
        .and_then(|c| c.get(n))
        .and_then(|c| c.get("musicResponsiveListItemFlexColumnRenderer"))
        .and_then(|c| c.get("text"))
        .and_then(|t| t.get("runs"))
        .and_then(Value::as_array);

    let Some(runs) = runs else {
        return Vec::new();
    };

    runs.iter()
        .filter_map(|r| {
            let texto = r.get("text")?.as_str()?.to_owned();
            let id = r
                .pointer("/navigationEndpoint/browseEndpoint/browseId")
                .or_else(|| r.pointer("/navigationEndpoint/watchEndpoint/videoId"))
                .and_then(Value::as_str)
                .map(str::to_owned);
            Some((texto, id))
        })
        .collect()
}

/// Identificador de vídeo de un elemento de canción.
#[must_use]
pub fn video_id(elemento: &Value) -> Option<String> {
    elemento
        .pointer("/playlistItemData/videoId")
        .or_else(|| {
            elemento.pointer(
                "/overlay/musicItemThumbnailOverlayRenderer/content/musicPlayButtonRenderer/playNavigationEndpoint/watchEndpoint/videoId",
            )
        })
        .and_then(Value::as_str)
        .map(str::to_owned)
}

/// Identificador de navegación (álbum, artista, lista).
#[must_use]
pub fn browse_id(elemento: &Value) -> Option<String> {
    elemento
        .pointer("/navigationEndpoint/browseEndpoint/browseId")
        .and_then(Value::as_str)
        .map(str::to_owned)
}

/// Convierte `"5:29"` o `"1:02:03"` en milisegundos.
///
/// **Exige al menos unos dos puntos.** Sin esa condición, un número suelto se
/// interpretaría como segundos, y el subtítulo de un álbum trae precisamente
/// eso: el año. "2018" se convertiría en una duración de treinta y tres
/// minutos y el álbum se quedaría sin fecha, en silencio.
#[must_use]
pub fn duracion_ms(texto: &str) -> Option<u32> {
    let partes: Vec<&str> = texto.trim().split(':').collect();
    if partes.len() < 2 || partes.len() > 3 {
        return None;
    }
    let mut total = 0_u32;
    for p in &partes {
        total = total.checked_mul(60)?.checked_add(p.parse::<u32>().ok()?)?;
    }
    total.checked_mul(1_000)
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "en un test, un `expect` que falla es el fallo"
)]
mod tests {
    use super::*;

    #[test]
    fn las_duraciones_se_convierten() {
        assert_eq!(duracion_ms("5:29"), Some(329_000));
        assert_eq!(duracion_ms("1:02:03"), Some(3_723_000));
        assert_eq!(duracion_ms("0:45"), Some(45_000));
    }

    #[test]
    fn una_duracion_que_no_lo_es_no_se_inventa() {
        // El subtítulo trae textos que no son duraciones ("1,5 M
        // reproducciones"); confundirlos con una daría canciones de horas.
        assert_eq!(duracion_ms("1,5 M reproducciones"), None);
        assert_eq!(duracion_ms(""), None);
        assert_eq!(duracion_ms("1:2:3:4"), None);
    }

    #[test]
    fn un_numero_suelto_no_es_una_duracion() {
        // El año de un álbum viene así. Sin esta comprobación, "2018" se
        // convertía en 33 minutos y el álbum perdía la fecha sin que nada
        // fallara.
        assert_eq!(duracion_ms("2018"), None);
        assert_eq!(duracion_ms("304"), None);
    }

    #[test]
    fn el_texto_de_varios_tramos_se_concatena() {
        let v = json!({ "runs": [{ "text": "Bohemian " }, { "text": "Rhapsody" }] });
        assert_eq!(texto_de(Some(&v)).expect("hay texto"), "Bohemian Rhapsody");
    }

    #[test]
    fn un_nodo_sin_la_forma_esperada_devuelve_none_en_vez_de_romper() {
        // Es la razón de navegar el JSON en vez de deserializarlo: un campo que
        // cambie de nombre se pierde solo, sin tumbar la búsqueda entera.
        assert!(texto_de(Some(&json!({ "otra_cosa": 1 }))).is_none());
        assert!(texto_de(None).is_none());
        assert!(columna(&json!({}), 0).is_none());
        assert!(video_id(&json!({})).is_none());
        assert!(estanterias(&json!({ "contents": 42 })).is_empty());
    }
}

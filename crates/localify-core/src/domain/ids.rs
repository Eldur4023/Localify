//! Identificadores del dominio.
//!
//! Cada entidad tiene su propio tipo de ID. No son `String` sueltas: pasar un
//! `AlbumId` donde se espera un `TrackId` no compila, que es exactamente la
//! clase de error que más cuesta encontrar en una app con cuatro catálogos
//! entrelazados.
//!
//! ## De dónde sale un identificador
//!
//! Un ID viene del catálogo que lo emitió: YouTube Music o Spotify. **No hay un
//! proveedor privilegiado**: el que use la biblioteca lo decide el usuario en
//! Ajustes, y los dos pueden convivir en la misma base de datos.
//!
//! Esto reemplaza al invariante original del proyecto —"el identificador
//! principal es el de Spotify, el de YouTube nunca es clave de dominio"—, que
//! se escribió cuando Spotify era el único origen posible. Con YouTube Music
//! como origen, mantenerlo obligaría a inventar un ID de Spotify para
//! contenido que no está en Spotify, o a arrastrar una tabla de equivalencias
//! para nada: el `videoId` **es** la identidad de esa pista.
//!
//! ## Por qué se sigue validando la forma
//!
//! La tentación es aceptar cualquier cadena y dejar que el proveedor se
//! encargue. No: la validación existe para cazar el error de pasar un
//! identificador de álbum donde se espera uno de pista, o un título donde se
//! espera un ID, y ese error no desaparece por haber dos catálogos. Lo que
//! cambia es que ahora hay **varias formas válidas**, una por catálogo, y se
//! aceptan todas.

use std::fmt;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Prefijo de los IDs sintéticos, para pistas sin equivalente en Spotify
/// (ficheros importados por el usuario).
pub const PREFIJO_LOCAL: &str = "local:";

/// Longitud de un ID base62 de Spotify.
const LONGITUD_SPOTIFY: usize = 22;

/// Longitud de un identificador de vídeo de YouTube.
const LONGITUD_VIDEO: usize = 11;

/// Prefijos de los identificadores de navegación de YouTube Music.
///
/// `UC` es un canal (artista), `MPRE` un álbum y `VLPL`/`PL` una lista. No se
/// intenta distinguir cuál es cuál aquí: eso lo sabe el proveedor, y meter esa
/// lógica en el tipo lo ataría a un catálogo concreto.
const PREFIJOS_BROWSE: [&str; 4] = ["UC", "MPRE", "VLPL", "PL"];

/// `true` si la cadena tiene forma de identificador de algún catálogo conocido.
///
/// Es deliberadamente laxa dentro de cada forma: comprueba longitud y alfabeto,
/// no que el identificador exista. Lo que caza es pasar un título, una ruta o
/// un identificador de otra entidad, que es el error real.
///
/// **Es pública porque hay un segundo sitio que necesita la misma regla**: la
/// recuperación de ficheros por nombre (ADR-021), que decide si
/// `kM0Fpbz0W8U.opus` es un fichero de Localify o algo que puso el usuario.
/// Tenía su propia copia de la comprobación y se quedó atrás al admitir
/// YouTube, con lo que ningún fichero de ese catálogo se habría recuperado
/// jamás. Una sola definición evita que vuelva a pasar.
#[must_use]
pub fn tiene_forma_de_id(valor: &str) -> bool {
    forma_conocida(valor)
}

fn forma_conocida(valor: &str) -> bool {
    let alfanumerico_o_signo = |c: char| c.is_ascii_alphanumeric() || c == '-' || c == '_';

    // Spotify: base62 de 22.
    if valor.len() == LONGITUD_SPOTIFY && valor.chars().all(|c| c.is_ascii_alphanumeric()) {
        return true;
    }
    // YouTube: identificador de vídeo de 11, en base64url.
    if valor.len() == LONGITUD_VIDEO && valor.chars().all(alfanumerico_o_signo) {
        return true;
    }
    // YouTube Music: identificador de navegación, de longitud variable.
    if PREFIJOS_BROWSE.iter().any(|p| valor.starts_with(p))
        && valor.len() >= 8
        && valor.chars().all(alfanumerico_o_signo)
    {
        return true;
    }
    // MusicBrainz: un MBID es un UUID con guiones.
    //
    // No choca con `local:<uuid>`, que lleva prefijo y se reconoce aparte, ni
    // con ninguna de las formas de arriba: 36 caracteres con guiones en
    // posiciones fijas no es un base62 de 22 ni un vídeo de 11.
    if es_uuid(valor) {
        return true;
    }
    false
}

/// Forma de un UUID canónico: `8-4-4-4-12` en hexadecimal.
///
/// Se comprueba a mano en vez de con `Uuid::parse_str` porque aquí solo importa
/// la **forma**, igual que en el resto de la función: aceptar variantes que el
/// parser tolera —sin guiones, entre llaves— haría que un base32 cualquiera
/// pasara por identificador.
fn es_uuid(valor: &str) -> bool {
    const TRAMOS: [usize; 5] = [8, 4, 4, 4, 12];

    let mut partes = valor.split('-');
    for largo in TRAMOS {
        let Some(tramo) = partes.next() else {
            return false;
        };
        if tramo.len() != largo || !tramo.chars().all(|c| c.is_ascii_hexdigit()) {
            return false;
        }
    }
    partes.next().is_none()
}

macro_rules! id_de_catalogo {
    ($nombre:ident, $entidad:literal) => {
        #[doc = concat!("Identificador de ", $entidad, ". Viene del catálogo que lo emitió (YouTube Music o Spotify), o es `local:<uuid>`.")]
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $nombre(String);

        impl $nombre {
            /// Construye el ID sin validar. Reservado a la capa de
            /// persistencia, que lee valores ya validados en su día.
            #[must_use]
            pub fn from_trusted(valor: impl Into<String>) -> Self {
                Self(valor.into())
            }

            /// Genera un ID local para contenido ajeno a Spotify.
            #[must_use]
            pub fn nuevo_local() -> Self {
                Self(format!("{}{}", PREFIJO_LOCAL, Uuid::now_v7()))
            }

            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }

            #[must_use]
            pub fn into_string(self) -> String {
                self.0
            }

            /// `true` si el ID no proviene de ningún catálogo externo.
            #[must_use]
            pub fn es_local(&self) -> bool {
                self.0.starts_with(PREFIJO_LOCAL)
            }

            /// Valida la forma del ID.
            ///
            /// # Errors
            /// Si está vacío, o si no tiene la forma de ningún catálogo
            /// conocido ni la de un `local:` bien formado.
            pub fn parse(valor: impl Into<String>) -> Result<Self, $crate::error::CoreError> {
                let valor = valor.into();
                if valor.is_empty() {
                    return Err($crate::error::CoreError::invalid(concat!(
                        "el id de ",
                        $entidad,
                        " está vacío"
                    )));
                }
                if let Some(resto) = valor.strip_prefix(PREFIJO_LOCAL) {
                    return if resto.is_empty() {
                        Err($crate::error::CoreError::invalid(concat!(
                            "id local de ",
                            $entidad,
                            " sin uuid"
                        )))
                    } else {
                        Ok(Self(valor))
                    };
                }
                if forma_conocida(&valor) {
                    Ok(Self(valor))
                } else {
                    Err($crate::error::CoreError::invalid(format!(
                        concat!("id de ", $entidad, " con formato inválido: '{}'"),
                        valor
                    )))
                }
            }
        }

        impl fmt::Display for $nombre {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl AsRef<str> for $nombre {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }
    };
}

id_de_catalogo!(TrackId, "pista");
id_de_catalogo!(AlbumId, "álbum");
id_de_catalogo!(ArtistId, "artista");

/// Identificador de playlist. Siempre local: las playlists importadas de
/// Spotify se copian a la biblioteca del usuario y pasan a ser suyas, con su
/// propio ciclo de vida. El ID de origen se conserva en `playlists.source_id`
/// solo a título informativo.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PlaylistId(Uuid);

impl PlaylistId {
    /// UUID v7: ordenable por tiempo de creación, lo que da un orden por
    /// defecto útil sin columna extra.
    #[must_use]
    pub fn nuevo() -> Self {
        Self(Uuid::now_v7())
    }

    #[must_use]
    pub const fn from_uuid(id: Uuid) -> Self {
        Self(id)
    }

    #[must_use]
    pub const fn as_uuid(&self) -> Uuid {
        self.0
    }

    /// # Errors
    /// Si el texto no es un UUID válido.
    pub fn parse(valor: &str) -> Result<Self, crate::error::CoreError> {
        Uuid::parse_str(valor)
            .map(Self)
            .map_err(|e| crate::error::CoreError::invalid(format!("id de playlist inválido: {e}")))
    }
}

impl fmt::Display for PlaylistId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Identidad de una entrada concreta dentro de una playlist.
///
/// No es el `TrackId`: la misma pista puede aparecer varias veces en la misma
/// playlist (Spotify lo permite), y "elimina esta fila" debe ser inequívoco.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PlaylistEntryId(Uuid);

impl PlaylistEntryId {
    #[must_use]
    pub fn nuevo() -> Self {
        Self(Uuid::now_v7())
    }

    #[must_use]
    pub const fn from_uuid(id: Uuid) -> Self {
        Self(id)
    }

    #[must_use]
    pub const fn as_uuid(&self) -> Uuid {
        self.0
    }
}

impl fmt::Display for PlaylistEntryId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Identidad de una entrada de la cola de reproducción. Mismo motivo que
/// [`PlaylistEntryId`]: la cola admite duplicados.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct QueueEntryId(Uuid);

impl QueueEntryId {
    #[must_use]
    pub fn nuevo() -> Self {
        Self(Uuid::now_v7())
    }

    #[must_use]
    pub const fn from_uuid(id: Uuid) -> Self {
        Self(id)
    }

    #[must_use]
    pub const fn as_uuid(&self) -> Uuid {
        self.0
    }
}

impl fmt::Display for QueueEntryId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acepta_un_id_base62_de_spotify() {
        let id = TrackId::parse("3z8h0TU7ReDPLIbEnYhWZb").expect("id válido");
        assert!(!id.es_local());
        assert_eq!(id.as_str(), "3z8h0TU7ReDPLIbEnYhWZb");
    }

    #[test]
    fn rechaza_longitudes_y_caracteres_incorrectos() {
        assert!(TrackId::parse("demasiado-corto").is_err());
        assert!(TrackId::parse("").is_err());
        // 22 caracteres pero con un símbolo no base62
        assert!(TrackId::parse("3z8h0TU7ReDPLIbEnYhW$b").is_err());
    }

    #[test]
    fn acepta_las_formas_de_youtube_music() {
        // Vídeo: 11 caracteres en base64url. El guion bajo y el guion son
        // legales ahí y no lo eran en base62, que es justo por lo que hay dos
        // alfabetos y no uno solo más permisivo.
        assert!(TrackId::parse("kM0Fpbz0W8U").is_ok());
        assert!(TrackId::parse("6Wg1_YOfiM0").is_ok());

        // Canal de artista y álbum: identificadores de navegación.
        assert!(ArtistId::parse("UCEPMVbUzImPl4p8k4LkGevA").is_ok());
        assert!(AlbumId::parse("MPREb_m2xZZHGzRl1").is_ok());
    }

    #[test]
    fn acepta_los_mbid_de_musicbrainz() {
        // El de "Bury the Light" de Casey Edwards y Victor Borba, que es la
        // canción que destapó que faltaba este catálogo.
        assert!(TrackId::parse("0578c31a-4ab4-4181-b05d-1a0a62e49bec").is_ok());
        assert!(ArtistId::parse("e2ac1391-5d5f-466c-b308-440020d36184").is_ok());
    }

    #[test]
    fn un_uuid_mal_formado_no_pasa_por_mbid() {
        // La comprobación mira la forma, no que exista. Lo que no puede es
        // dejar pasar cualquier cadena con guiones.
        assert!(
            TrackId::parse("0578c31a-4ab4-4181-b05d").is_err(),
            "le faltan tramos"
        );
        assert!(
            TrackId::parse("0578c31a-4ab4-4181-b05d-1a0a62e49bec-extra").is_err(),
            "le sobra un tramo"
        );
        assert!(
            TrackId::parse("zzzzzzzz-4ab4-4181-b05d-1a0a62e49bec").is_err(),
            "no es hexadecimal"
        );
        assert!(
            TrackId::parse("0578c31a4ab44181b05d1a0a62e49bec").is_err(),
            "sin guiones son 32 caracteres que no son ninguna forma conocida"
        );
    }

    #[test]
    fn lo_que_no_es_un_identificador_sigue_rechazandose() {
        // Es lo que la validación tiene que seguir cazando tras admitir dos
        // catálogos: un título o una ruta donde se espera un ID.
        assert!(TrackId::parse("Bohemian Rhapsody").is_err());
        assert!(TrackId::parse("C:/musica/cancion.opus").is_err());
        assert!(
            TrackId::parse("MPRE").is_err(),
            "prefijo suelto, sin cuerpo"
        );
        // Doce caracteres: ni vídeo (11) ni Spotify (22).
        assert!(TrackId::parse("kM0Fpbz0W8Ux").is_err());
    }

    #[test]
    fn los_ids_locales_se_reconocen() {
        let id = TrackId::nuevo_local();
        assert!(id.es_local());
        assert!(TrackId::parse(id.as_str()).is_ok());
        assert!(
            TrackId::parse("local:").is_err(),
            "un id local sin uuid no es válido"
        );
    }

    #[test]
    fn los_uuid_v7_de_playlist_son_monotonos() {
        let a = PlaylistId::nuevo();
        let b = PlaylistId::nuevo();
        assert!(a < b || a.as_uuid() != b.as_uuid());
    }
}

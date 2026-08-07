//! Interpretación de identificadores, URIs y URLs de Spotify.
//!
//! El usuario pega lo que tiene a mano: un enlace copiado del navegador, un URI
//! del botón "Compartir", o el identificador suelto. Aceptar las tres formas es
//! la diferencia entre que importar una playlist funcione a la primera o exija
//! explicar un formato.

use crate::error::{SpotifyError, SpotifyResult};

/// Tipo de recurso.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tipo {
    Track,
    Album,
    Artist,
    Playlist,
}

impl Tipo {
    #[must_use]
    pub const fn como_str(self) -> &'static str {
        match self {
            Self::Track => "track",
            Self::Album => "album",
            Self::Artist => "artist",
            Self::Playlist => "playlist",
        }
    }

    fn desde_str(s: &str) -> Option<Self> {
        match s {
            "track" => Some(Self::Track),
            "album" => Some(Self::Album),
            "artist" => Some(Self::Artist),
            "playlist" => Some(Self::Playlist),
            _ => None,
        }
    }
}

/// Un identificador de Spotify ya extraído.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Referencia {
    pub tipo: Tipo,
    pub id: String,
}

/// Longitud de un identificador base62 de Spotify.
const LONGITUD: usize = 22;

fn es_id_valido(s: &str) -> bool {
    s.len() == LONGITUD && s.chars().all(|c| c.is_ascii_alphanumeric())
}

/// Extrae la referencia de cualquiera de las formas admitidas.
///
/// Acepta:
/// - identificador suelto: `37i9dQZF1DXcBWIGoYBM5M`
/// - URI: `spotify:playlist:37i9dQZF1DXcBWIGoYBM5M`
/// - URL: `https://open.spotify.com/playlist/37i9...?si=abc`
/// - URL con idioma: `https://open.spotify.com/intl-es/track/3z8h...`
///
/// # Errors
/// Si no se reconoce ninguna forma válida, o si el tipo no es el esperado.
pub fn extraer(entrada: &str, esperado: Tipo) -> SpotifyResult<Referencia> {
    let entrada = entrada.trim();
    if entrada.is_empty() {
        return Err(SpotifyError::Invalido("la entrada está vacía".into()));
    }

    // Identificador suelto.
    if es_id_valido(entrada) {
        return Ok(Referencia {
            tipo: esperado,
            id: entrada.to_owned(),
        });
    }

    // URI: spotify:tipo:id
    if let Some(resto) = entrada.strip_prefix("spotify:") {
        let mut partes = resto.split(':');
        let tipo = partes.next().and_then(Tipo::desde_str);
        let id = partes.next().unwrap_or_default();
        return validar(tipo, id, esperado, entrada);
    }

    // URL de open.spotify.com. El segmento de tipo es el penúltimo, lo que
    // absorbe de paso los prefijos de idioma (`/intl-es/`).
    if entrada.contains("open.spotify.com") {
        let sin_query = entrada.split(['?', '#']).next().unwrap_or(entrada);
        let segmentos: Vec<&str> = sin_query
            .trim_end_matches('/')
            .split('/')
            .filter(|s| !s.is_empty())
            .collect();

        if segmentos.len() >= 2 {
            let tipo = Tipo::desde_str(segmentos[segmentos.len() - 2]);
            let id = segmentos[segmentos.len() - 1];
            return validar(tipo, id, esperado, entrada);
        }
    }

    Err(SpotifyError::Invalido(format!(
        "no se reconoce como {} de Spotify: '{entrada}'",
        esperado.como_str()
    )))
}

fn validar(
    tipo: Option<Tipo>,
    id: &str,
    esperado: Tipo,
    entrada: &str,
) -> SpotifyResult<Referencia> {
    let Some(tipo) = tipo else {
        return Err(SpotifyError::Invalido(format!(
            "tipo de recurso no reconocido en '{entrada}'"
        )));
    };
    if tipo != esperado {
        return Err(SpotifyError::Invalido(format!(
            "se esperaba un {} y llegó un {}",
            esperado.como_str(),
            tipo.como_str()
        )));
    }
    if !es_id_valido(id) {
        return Err(SpotifyError::Invalido(format!(
            "identificador con formato inválido en '{entrada}'"
        )));
    }
    Ok(Referencia {
        tipo,
        id: id.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "37i9dQZF1DXcBWIGoYBM5M";

    #[test]
    fn acepta_un_identificador_suelto() {
        let r = extraer(ID, Tipo::Playlist).expect("válido");
        assert_eq!(r.id, ID);
        assert_eq!(r.tipo, Tipo::Playlist);
    }

    #[test]
    fn acepta_un_uri() {
        let r = extraer(&format!("spotify:playlist:{ID}"), Tipo::Playlist).expect("válido");
        assert_eq!(r.id, ID);
    }

    #[test]
    fn acepta_una_url_con_parametros() {
        // Es exactamente lo que copia el botón "Compartir".
        let url = format!("https://open.spotify.com/playlist/{ID}?si=abc123&pt=xyz");
        assert_eq!(extraer(&url, Tipo::Playlist).expect("válido").id, ID);
    }

    #[test]
    fn acepta_una_url_con_prefijo_de_idioma() {
        let url = format!("https://open.spotify.com/intl-es/track/{ID}");
        assert_eq!(extraer(&url, Tipo::Track).expect("válido").id, ID);
    }

    #[test]
    fn acepta_una_url_con_ancla_y_barra_final() {
        let url = format!("https://open.spotify.com/album/{ID}/#seccion");
        assert_eq!(extraer(&url, Tipo::Album).expect("válido").id, ID);
    }

    #[test]
    fn ignora_los_espacios_alrededor() {
        let entrada = format!("  spotify:artist:{ID}  ");
        assert_eq!(extraer(&entrada, Tipo::Artist).expect("válido").id, ID);
    }

    #[test]
    fn rechaza_un_tipo_distinto_del_esperado() {
        let error = extraer(&format!("spotify:album:{ID}"), Tipo::Playlist)
            .expect_err("los tipos no coinciden");
        assert!(error.to_string().contains("se esperaba"), "{error}");
    }

    #[test]
    fn rechaza_identificadores_mal_formados() {
        for entrada in [
            "",
            "   ",
            "demasiado-corto",
            "spotify:playlist:corto",
            "https://open.spotify.com/playlist/corto",
            "https://ejemplo.com/playlist/37i9dQZF1DXcBWIGoYBM5M",
        ] {
            assert!(
                extraer(entrada, Tipo::Playlist).is_err(),
                "'{entrada}' no debería aceptarse"
            );
        }
    }

    #[test]
    fn un_id_con_simbolos_no_pasa_por_valido() {
        // 22 caracteres pero con uno fuera de base62.
        assert!(extraer("37i9dQZF1DXcBWIGoYBM5$", Tipo::Playlist).is_err());
    }
}

//! Proveedor que pregunta a dos catálogos a la vez.
//!
//! ## Por qué no basta con elegir uno
//!
//! YouTube Music y MusicBrainz no compiten, se completan. El primero conoce lo
//! que hay **subido**: remezclas, versiones de canal, cosas que no existen fuera
//! de YouTube. El segundo conoce lo **publicado**: ediciones, bandas sonoras,
//! ISRC, duraciones exactas.
//!
//! El caso que obligó a escribir esto: buscar "casey edwards bury the light" en
//! YouTube Music devuelve veinte resultados y **ninguno** es la canción —solo
//! covers y remezclas—, porque la original no está subida como canción. En
//! MusicBrainz sale la primera. Con un desplegable de catálogos, el usuario
//! tendría que acertar cuál usar *antes* de buscar, y cuál es el correcto
//! depende de la canción concreta.
//!
//! ## Cómo se mezclan los resultados
//!
//! Alternando, empezando por YouTube Music. Ordenar por alguna puntuación común
//! sería inventarse una: uno mide relevancia de texto sobre doce millones de
//! grabaciones y el otro reproducciones de vídeos, y esos números no se
//! comparan. Alternar no necesita inventar nada y garantiza lo que importa: que
//! lo mejor de cada catálogo esté arriba.
//!
//! Empieza YouTube Music para no cambiar lo que ya funcionaba: para la mayoría
//! de búsquedas su primer resultado sigue siendo el primero de todos, y el de
//! MusicBrainz pasa a estar segundo en lugar de no estar.
//!
//! ## Los duplicados no se filtran aquí
//!
//! Una canción que está en los dos catálogos sale dos veces, con dos
//! identificadores distintos. Filtrarlo aquí exigiría decidir cuál de los dos es
//! "la buena" sin saber qué va a hacer el usuario con ella, y equivocarse
//! escondería la única que tiene ISRC —o la única descargable—.
//!
//! No hace falta: el servicio de búsqueda ya agrupa por título canónico y
//! artista, así que las dos caen en la misma fila y solo una se ve. La otra
//! queda desplegable, que es donde debe estar algo que puede que sí quieras.
//!
//! ## Las consultas por identificador no se duplican
//!
//! Pedir una pista concreta va a **un** catálogo, el que emitió ese
//! identificador, y se sabe por su forma: un MBID es un UUID y nada más lo es.
//! Preguntar a los dos sería gastar una petición garantizada a fallar.

use std::sync::Arc;

use async_trait::async_trait;
use localify_core::domain::album::Album;
use localify_core::domain::artist::Artist;
use localify_core::domain::ids::{AlbumId, ArtistId, TrackId};
use localify_core::domain::track::Track;
use localify_core::error::CoreResult;
use localify_core::events::ProviderStatus;
use localify_core::page::Page;
use localify_core::ports::metadata_provider::{MetadataProvider, PlaylistImport, Resolucion};
use tracing::debug;

pub const NOMBRE: &str = "combinado";

pub struct ProveedorCombinado {
    /// El que manda en el orden y el que atiende lo que no es un MBID.
    principal: Arc<dyn MetadataProvider>,
    /// MusicBrainz.
    editado: Arc<dyn MetadataProvider>,
}

impl std::fmt::Debug for ProveedorCombinado {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProveedorCombinado").finish_non_exhaustive()
    }
}

impl ProveedorCombinado {
    #[must_use]
    pub fn nuevo(principal: Arc<dyn MetadataProvider>, editado: Arc<dyn MetadataProvider>) -> Self {
        Self { principal, editado }
    }

    /// A quién le toca un identificador, según su forma.
    fn para(&self, id: &str) -> &Arc<dyn MetadataProvider> {
        if es_mbid(id) {
            &self.editado
        } else {
            &self.principal
        }
    }
}

/// `true` si la cadena tiene forma de MBID: un UUID canónico.
///
/// Es la misma regla que valida los identificadores en `core`, aplicada aquí
/// para enrutar. No se comparte función porque allí responde "¿es *algún*
/// identificador?" y aquí "¿es de *este* catálogo?": son dos preguntas
/// distintas que hoy coinciden en la forma y mañana pueden no hacerlo.
fn es_mbid(valor: &str) -> bool {
    const TRAMOS: [usize; 5] = [8, 4, 4, 4, 12];
    let mut partes = valor.split('-');
    for largo in TRAMOS {
        match partes.next() {
            Some(t) if t.len() == largo && t.chars().all(|c| c.is_ascii_hexdigit()) => {}
            _ => return false,
        }
    }
    partes.next().is_none()
}

/// Mezcla dos listas alternando, empezando por la primera.
///
/// Cuando una se acaba, el resto de la otra va detrás: quedarse corto porque el
/// otro catálogo tuviera menos resultados sería tirar respuestas buenas.
fn alternar<T>(a: Vec<T>, b: Vec<T>) -> Vec<T> {
    let mut salida = Vec::with_capacity(a.len() + b.len());
    let (mut ia, mut ib) = (a.into_iter(), b.into_iter());
    loop {
        let (x, y) = (ia.next(), ib.next());
        if x.is_none() && y.is_none() {
            break;
        }
        salida.extend(x);
        salida.extend(y);
    }
    salida
}

#[async_trait]
impl MetadataProvider for ProveedorCombinado {
    fn name(&self) -> &'static str {
        NOMBRE
    }

    async fn status(&self) -> ProviderStatus {
        // Basta con que uno responda. Ninguno de los dos pide credenciales, así
        // que en la práctica esto solo es `Unavailable` sin red.
        let (a, b) = tokio::join!(self.principal.status(), self.editado.status());
        if a.esta_operativo() { a } else { b }
    }

    async fn search_tracks(&self, query: &str, limit: u8, offset: u16) -> CoreResult<Page<Track>> {
        // En paralelo: son dos servicios distintos y encadenarlas sumaría sus
        // latencias por nada. MusicBrainz ya se frena solo a una petición por
        // segundo dentro de su cliente.
        let (uno, otro) = tokio::join!(
            self.principal.search_tracks(query, limit, offset),
            self.editado.search_tracks(query, limit, offset),
        );

        // Que un catálogo falle no puede dejar la búsqueda sin resultados: el
        // otro sigue sirviendo, que es media razón de tener dos.
        let principales = uno.unwrap_or_else(|e| {
            debug!(error = %e, "el catálogo principal no respondió");
            Page::new(Vec::new(), None, None)
        });
        let editados = otro.unwrap_or_else(|e| {
            debug!(error = %e, "MusicBrainz no respondió");
            Page::new(Vec::new(), None, None)
        });

        Ok(Page::new(
            alternar(principales.items, editados.items),
            // Los totales de los dos no se suman: contarían dos veces lo que
            // está en ambos, y ninguno de los dos número significa gran cosa
            // para quien mira una lista de veinte.
            None,
            None,
        ))
    }

    async fn track(&self, id: &TrackId) -> CoreResult<Track> {
        self.para(id.as_str()).track(id).await
    }

    /// Primero el dueño del identificador; si no lo sabe, que lo busque el otro.
    ///
    /// El dueño responde con lo que **tiene guardado**, que es lo más fiable.
    /// Pero la mayoría de las grabaciones no tienen esa relación, y ahí la
    /// búsqueda de YouTube Music sigue siendo mucho mejor que dejar que el
    /// emparejador rastree YouTube entero: es un catálogo de música y no lista
    /// vídeos de letras ni bucles de una hora.
    ///
    /// Es justo lo que le pasaba a lo importado de una lista de Spotify: sin
    /// álbum, sin ISRC y con identificadores que no son de YouTube, el
    /// emparejador se quedaba solo con el texto.
    async fn resolve_recording(&self, track: &Track) -> CoreResult<Option<Resolucion>> {
        let duenyo = self.para(track.id.as_str());
        if let Ok(Some(resuelta)) = duenyo.resolve_recording(track).await {
            return Ok(Some(resuelta));
        }

        // `principal` es YouTube Music. Si el dueño ya era él, esta segunda
        // llamada no aporta nada y se ahorra.
        if std::ptr::eq(
            std::ptr::from_ref::<dyn MetadataProvider>(duenyo.as_ref()).cast::<()>(),
            std::ptr::from_ref::<dyn MetadataProvider>(self.principal.as_ref()).cast::<()>(),
        ) {
            return Ok(None);
        }
        self.principal.resolve_recording(track).await
    }

    async fn tracks(&self, ids: &[TrackId]) -> CoreResult<Vec<Track>> {
        // Se reparten por forma y se piden en dos lotes, no de una en una: cada
        // catálogo agrupa como sabe y esa es justamente la razón de que el
        // puerto tenga este método.
        let (mbid, resto): (Vec<TrackId>, Vec<TrackId>) =
            ids.iter().cloned().partition(|i| es_mbid(i.as_str()));

        let (a, b) = tokio::join!(self.principal.tracks(&resto), self.editado.tracks(&mbid));
        let mut salida = a.unwrap_or_default();
        salida.extend(b.unwrap_or_default());
        Ok(salida)
    }

    async fn album(&self, id: &AlbumId) -> CoreResult<Album> {
        self.para(id.as_str()).album(id).await
    }

    async fn album_tracks(&self, id: &AlbumId) -> CoreResult<Vec<Track>> {
        self.para(id.as_str()).album_tracks(id).await
    }

    async fn artist(&self, id: &ArtistId) -> CoreResult<Artist> {
        self.para(id.as_str()).artist(id).await
    }

    async fn artist_top_tracks(&self, id: &ArtistId) -> CoreResult<Vec<Track>> {
        self.para(id.as_str()).artist_top_tracks(id).await
    }

    async fn artist_albums(&self, id: &ArtistId) -> CoreResult<Vec<Album>> {
        self.para(id.as_str()).artist_albums(id).await
    }

    async fn public_playlist(
        &self,
        url_or_id: &str,
        page_callback: &(dyn Fn(u32, u32) + Send + Sync),
    ) -> CoreResult<PlaylistImport> {
        // MusicBrainz no tiene playlists, así que aquí no hay nada que combinar.
        self.principal
            .public_playlist(url_or_id, page_callback)
            .await
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "en un test, un `expect` que falla es el fallo"
)]
mod tests {
    use super::*;

    #[test]
    fn alternar_empieza_por_el_primero() {
        assert_eq!(
            alternar(vec![1, 3, 5], vec![2, 4, 6]),
            vec![1, 2, 3, 4, 5, 6]
        );
    }

    #[test]
    fn alternar_no_pierde_la_cola_de_la_lista_mas_larga() {
        // Si un catálogo devuelve menos, el resto del otro tiene que salir: eran
        // resultados buenos y quedarse corto por eso sería absurdo.
        assert_eq!(alternar(vec![1], vec![2, 4, 6]), vec![1, 2, 4, 6]);
        assert_eq!(alternar(vec![1, 3, 5], vec![2]), vec![1, 2, 3, 5]);
    }

    #[test]
    fn alternar_con_una_vacia_devuelve_la_otra_entera() {
        // Es el caso de "un catálogo no respondió", que tiene que seguir siendo
        // una búsqueda útil y no media.
        assert_eq!(alternar(Vec::<i32>::new(), vec![2, 4]), vec![2, 4]);
        assert_eq!(alternar(vec![1, 3], Vec::<i32>::new()), vec![1, 3]);
    }

    #[test]
    fn solo_un_uuid_canonico_se_enruta_a_musicbrainz() {
        // El de "Bury the Light", que es el que destapó todo esto.
        assert!(es_mbid("0578c31a-4ab4-4181-b05d-1a0a62e49bec"));
        // Y las formas de los otros catálogos, que no pueden confundirse.
        assert!(!es_mbid("kM0Fpbz0W8U"), "vídeo de YouTube");
        assert!(!es_mbid("3z8h0TU7ReDPLIbEnYhWZb"), "base62 de Spotify");
        assert!(!es_mbid("MPREb_m2xZZHGzRl1"), "álbum de YouTube Music");
        assert!(
            !es_mbid("local:0198a1b2-c3d4-7890-abcd-ef0123456789"),
            "id local"
        );
        assert!(!es_mbid("0578c31a-4ab4-4181-b05d"), "uuid incompleto");
    }
}

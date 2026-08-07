//! Implementación de [`MetadataProvider`] sobre la Web API de Spotify.

use std::sync::Arc;

use async_trait::async_trait;
use localify_core::domain::album::Album;
use localify_core::domain::artist::Artist;
use localify_core::domain::ids::{AlbumId, ArtistId, TrackId};
use localify_core::domain::track::Track;
use localify_core::error::CoreResult;
use localify_core::events::ProviderStatus;
use localify_core::page::Page;
use localify_core::ports::metadata_provider::{MetadataProvider, PlaylistImport};

use crate::auth::Credenciales;
use crate::client::ClienteSpotify;
use crate::error::{PROVEEDOR, SpotifyError, SpotifyResult};
use crate::models::{
    AlbumCompleto, ArtistaCrudo, ArtistasRespuesta, BusquedaRespuesta, Paginado, PistaCruda,
    PistasRespuesta, PlaylistCruda, TopTracksRespuesta,
};
use crate::transporte::Transporte;
use crate::{mapper, uri};

/// Máximo de identificadores por petición en los endpoints de lote.
///
/// Lo fija Spotify. Pedir cincuenta pistas de una vez en lugar de una a una es
/// la diferencia entre una petición y cincuenta al abrir una playlist.
const LOTE_PISTAS: usize = 50;
const LOTE_ARTISTAS: usize = 50;

/// Elementos por página al recorrer una playlist. También lo fija Spotify.
const PAGINA_PLAYLIST: u32 = 100;

/// Mercado con el que se consultan los catálogos.
///
/// Sin él, Spotify devuelve pistas sin datos de disponibilidad y algunas
/// respuestas quedan incompletas. `from_token` no sirve con credenciales de
/// aplicación (no hay usuario), así que se fija uno amplio.
const MERCADO: &str = "ES";

pub struct SpotifyProvider {
    cliente: Arc<ClienteSpotify>,
}

impl std::fmt::Debug for SpotifyProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SpotifyProvider").finish_non_exhaustive()
    }
}

impl SpotifyProvider {
    #[must_use]
    pub fn nuevo(transporte: Arc<dyn Transporte>) -> Self {
        Self {
            cliente: Arc::new(ClienteSpotify::nuevo(transporte)),
        }
    }

    /// Establece o borra las credenciales de aplicación.
    pub async fn set_credenciales(&self, credenciales: Option<Credenciales>) {
        self.cliente.set_credenciales(credenciales).await;
    }

    /// Comprueba las credenciales con una petición mínima.
    pub async fn comprobar(&self) -> ProviderStatus {
        if !self.cliente.hay_credenciales().await {
            return ProviderStatus::NotConfigured;
        }
        // Una búsqueda con un límite de uno es la petición más barata que
        // ejercita el token de verdad.
        match self
            .cliente
            .get::<BusquedaRespuesta>("/search?q=a&type=track&limit=1")
            .await
        {
            Ok(_) => ProviderStatus::Ready,
            Err(SpotifyError::SinCredenciales | SpotifyError::CredencialesInvalidas) => {
                ProviderStatus::NotConfigured
            }
            Err(e) => ProviderStatus::Unavailable {
                reason_key: clave_de(&e),
            },
        }
    }

    /// Pide pistas en lotes del tamaño que admite Spotify.
    async fn pistas_en_lotes(&self, ids: &[TrackId]) -> SpotifyResult<Vec<Track>> {
        let mut resultado = Vec::with_capacity(ids.len());

        for lote in ids.chunks(LOTE_PISTAS) {
            let lista = lote
                .iter()
                .map(TrackId::as_str)
                .collect::<Vec<_>>()
                .join(",");
            let respuesta: PistasRespuesta = self
                .cliente
                .get(&format!("/tracks?ids={lista}&market={MERCADO}"))
                .await?;

            // Los nulos corresponden a ids que Spotify no reconoce. Se saltan:
            // pedir cincuenta y recibir cuarenta y nueve es normal.
            resultado.extend(respuesta.tracks.iter().flatten().filter_map(mapper::pista));
        }

        Ok(resultado)
    }
}

/// Clave i18n del motivo por el que el proveedor no está disponible.
fn clave_de(e: &SpotifyError) -> String {
    match e {
        SpotifyError::LimiteAlcanzado { .. } => "provider.rate_limited",
        SpotifyError::Red(_) => "provider.network",
        SpotifyError::NoEncontrado(_) => "provider.not_found",
        SpotifyError::Respuesta(_) => "provider.bad_response",
        _ => "provider.unavailable",
    }
    .to_owned()
}

/// Codifica un valor para una cadena de consulta.
///
/// Se hace a mano porque es lo único que hace falta de un crate de URLs, y una
/// búsqueda con un `&` o un `#` sin escapar rompería la petición entera.
fn escapar(valor: &str) -> String {
    use std::fmt::Write as _;

    let mut salida = String::with_capacity(valor.len() * 3);
    for byte in valor.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                salida.push(*byte as char);
            }
            b' ' => salida.push('+'),
            otro => {
                // Escribir en el `String` evita una asignación por byte
                // escapado. `write!` sobre `String` no puede fallar.
                let _ = write!(salida, "%{otro:02X}");
            }
        }
    }
    salida
}

#[async_trait]
impl MetadataProvider for SpotifyProvider {
    fn name(&self) -> &'static str {
        PROVEEDOR
    }

    async fn status(&self) -> ProviderStatus {
        if self.cliente.hay_credenciales().await {
            ProviderStatus::Ready
        } else {
            ProviderStatus::NotConfigured
        }
    }

    async fn search_tracks(&self, query: &str, limit: u8, offset: u16) -> CoreResult<Page<Track>> {
        if query.trim().is_empty() {
            return Ok(Page::empty());
        }

        let limite = limit.clamp(1, 50);
        let ruta = format!(
            "/search?q={}&type=track&limit={limite}&offset={offset}&market={MERCADO}",
            escapar(query)
        );

        let respuesta: BusquedaRespuesta = self.cliente.get(&ruta).await?;
        let Some(pagina) = respuesta.tracks else {
            return Ok(Page::empty());
        };

        let total = pagina.total.map(u64::from);
        let items = mapper::pistas(&pagina.items);
        Ok(Page::new(items, total, None))
    }

    async fn track(&self, id: &TrackId) -> CoreResult<Track> {
        let cruda: PistaCruda = self
            .cliente
            .get(&format!("/tracks/{}?market={MERCADO}", id.as_str()))
            .await?;

        mapper::pista(&cruda)
            .ok_or_else(|| SpotifyError::Respuesta("la pista no trae id".into()).into())
    }

    async fn tracks(&self, ids: &[TrackId]) -> CoreResult<Vec<Track>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        Ok(self.pistas_en_lotes(ids).await?)
    }

    async fn album(&self, id: &AlbumId) -> CoreResult<Album> {
        let completo: AlbumCompleto = self
            .cliente
            .get(&format!("/albums/{}?market={MERCADO}", id.as_str()))
            .await?;

        mapper::album_completo(&completo)
            .ok_or_else(|| SpotifyError::Respuesta("el álbum no trae id".into()).into())
    }

    async fn album_tracks(&self, id: &AlbumId) -> CoreResult<Vec<Track>> {
        // Se pide el álbum completo, no `/albums/{id}/tracks`: las pistas de
        // ese endpoint no incluyen el álbum al que pertenecen, y habría que
        // consultarlo aparte para poder construir la entidad.
        let completo: AlbumCompleto = self
            .cliente
            .get(&format!("/albums/{}?market={MERCADO}", id.as_str()))
            .await?;

        let Some(pistas) = &completo.tracks else {
            return Ok(Vec::new());
        };

        Ok(pistas
            .items
            .iter()
            .filter_map(|p| mapper::pista_de_album(p, &completo.simple))
            .collect())
    }

    async fn artist(&self, id: &ArtistId) -> CoreResult<Artist> {
        let crudo: ArtistaCrudo = self
            .cliente
            .get(&format!("/artists/{}", id.as_str()))
            .await?;

        mapper::artista(&crudo)
            .ok_or_else(|| SpotifyError::Respuesta("el artista no trae id".into()).into())
    }

    async fn artist_top_tracks(&self, id: &ArtistId) -> CoreResult<Vec<Track>> {
        let respuesta: TopTracksRespuesta = self
            .cliente
            .get(&format!(
                "/artists/{}/top-tracks?market={MERCADO}",
                id.as_str()
            ))
            .await?;
        Ok(mapper::pistas(&respuesta.tracks))
    }

    async fn artist_albums(&self, id: &ArtistId) -> CoreResult<Vec<Album>> {
        let pagina: Paginado<crate::models::AlbumSimple> = self
            .cliente
            .get(&format!(
                "/artists/{}/albums?include_groups=album,single&limit=50&market={MERCADO}",
                id.as_str()
            ))
            .await?;
        Ok(pagina.items.iter().filter_map(mapper::album).collect())
    }

    async fn public_playlist(
        &self,
        url_or_id: &str,
        page_callback: &(dyn Fn(u32, u32) + Send + Sync),
    ) -> CoreResult<PlaylistImport> {
        let referencia = uri::extraer(url_or_id, uri::Tipo::Playlist)?;

        // Sin credenciales se lee la página de incrustación, que es pública.
        //
        // Traerse una lista que alguien te ha pasado no debería obligar a
        // registrar una aplicación en el panel de Spotify: es un peaje que tiene
        // sentido para usar Spotify como catálogo y ninguno para esto. Con
        // credenciales se usa la API, que además da la descripción y no tiene
        // tope de canciones.
        if !self.cliente.hay_credenciales().await {
            return crate::publica::leer(self.cliente.transporte(), &referencia.id, page_callback)
                .await;
        }

        let cruda: PlaylistCruda = self
            .cliente
            .get(&format!("/playlists/{}?market={MERCADO}", referencia.id))
            .await?;

        let total = cruda.tracks.as_ref().and_then(|t| t.total).unwrap_or(0);
        let mut pistas = Vec::with_capacity(total as usize);

        // La primera página viene dentro del propio recurso: aprovecharla evita
        // una petición redundante.
        if let Some(primera) = &cruda.tracks {
            pistas.extend(
                primera
                    .items
                    .iter()
                    .filter_map(|e| e.track.as_ref())
                    .filter_map(mapper::pista),
            );
        }
        page_callback(u32::try_from(pistas.len()).unwrap_or(0), total);

        let mut offset = PAGINA_PLAYLIST;
        while (offset as usize) < total as usize {
            let pagina: Paginado<crate::models::EntradaPlaylist> = self
                .cliente
                .get(&format!(
                    "/playlists/{}/tracks?offset={offset}&limit={PAGINA_PLAYLIST}&market={MERCADO}",
                    referencia.id
                ))
                .await?;

            if pagina.items.is_empty() {
                break;
            }

            pistas.extend(
                pagina
                    .items
                    .iter()
                    .filter_map(|e| e.track.as_ref())
                    .filter_map(mapper::pista),
            );
            page_callback(u32::try_from(pistas.len()).unwrap_or(0), total);
            offset += PAGINA_PLAYLIST;
        }

        Ok(PlaylistImport {
            source_id: referencia.id,
            name: cruda.name,
            description: cruda.description,
            cover_url: crate::models::imagen_mayor(&cruda.images).map(str::to_owned),
            total,
            tracks: pistas,
        })
    }
}

/// Consulta artistas en lotes. Va aparte del trait porque solo lo necesita el
/// servicio de metadatos al rellenar géneros.
impl SpotifyProvider {
    /// # Errors
    /// El error de la petición.
    pub async fn artists_batch(&self, ids: &[ArtistId]) -> CoreResult<Vec<Artist>> {
        let mut resultado = Vec::with_capacity(ids.len());

        for lote in ids.chunks(LOTE_ARTISTAS) {
            let lista = lote
                .iter()
                .map(ArtistId::as_str)
                .collect::<Vec<_>>()
                .join(",");
            let respuesta: ArtistasRespuesta =
                self.cliente.get(&format!("/artists?ids={lista}")).await?;
            resultado.extend(
                respuesta
                    .artists
                    .iter()
                    .flatten()
                    .filter_map(mapper::artista),
            );
        }

        Ok(resultado)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transporte::falso::{Peticion, TransporteFalso};

    fn token() -> String {
        r#"{"access_token":"tok","token_type":"Bearer","expires_in":3600}"#.to_owned()
    }

    async fn proveedor(t: Arc<TransporteFalso>) -> SpotifyProvider {
        let p = SpotifyProvider::nuevo(t);
        p.set_credenciales(Some(Credenciales {
            client_id: "id".into(),
            client_secret: "secreto".into(),
        }))
        .await;
        p
    }

    fn urls(t: &TransporteFalso) -> Vec<String> {
        t.registradas()
            .into_iter()
            .filter_map(|p| match p {
                Peticion::Get { url, .. } => Some(url),
                Peticion::PostForm { .. } => None,
            })
            .collect()
    }

    #[test]
    fn el_escapado_protege_la_cadena_de_consulta() {
        assert_eq!(escapar("bohemian rhapsody"), "bohemian+rhapsody");
        assert_eq!(escapar("AC/DC"), "AC%2FDC");
        assert_eq!(escapar("a&b=c"), "a%26b%3Dc");
        assert_eq!(escapar("#hash"), "%23hash");
        // No ASCII: debe ir percent-encoded en UTF-8.
        assert_eq!(escapar("Björk"), "Bj%C3%B6rk");
    }

    #[tokio::test]
    async fn sin_credenciales_el_estado_es_no_configurado() {
        let p = SpotifyProvider::nuevo(Arc::new(TransporteFalso::nuevo()));
        assert_eq!(p.status().await, ProviderStatus::NotConfigured);
    }

    #[tokio::test]
    async fn una_busqueda_vacia_no_sale_a_la_red() {
        let t = Arc::new(TransporteFalso::nuevo());
        let p = proveedor(t.clone()).await;

        let pagina = p.search_tracks("   ", 20, 0).await.expect("no falla");
        assert!(pagina.is_empty());
        assert_eq!(
            t.cuantas(),
            0,
            "una consulta en blanco no merece una petición"
        );
    }

    #[tokio::test]
    async fn la_busqueda_traduce_los_resultados() {
        let json = r#"{"tracks":{"items":[
            {"id":"3z8h0TU7ReDPLIbEnYhWZb","name":"Under Pressure","duration_ms":248000,
             "artists":[{"id":"1dfeR4HaWDbWqFHLkxsg1d","name":"Queen"}],
             "album":{"id":"1GbtB4zTqAsyfZEsm1RZfx","name":"Hot Space",
                      "release_date":"1982-05-21","release_date_precision":"day"},
             "external_ids":{"isrc":"GBUM71029604"}}
        ],"total":1}}"#;

        let t = Arc::new(TransporteFalso::nuevo().con_json(&token()).con_json(json));
        let p = proveedor(t.clone()).await;

        let pagina = p
            .search_tracks("under pressure", 20, 0)
            .await
            .expect("busca");
        assert_eq!(pagina.items.len(), 1);
        assert_eq!(pagina.items[0].title, "Under Pressure");
        assert_eq!(pagina.items[0].isrc.as_deref(), Some("GBUM71029604"));
        assert_eq!(pagina.total, Some(1));

        let url = &urls(&t)[0];
        assert!(url.contains("q=under+pressure"), "{url}");
        assert!(url.contains("type=track"), "{url}");
        assert!(
            url.contains("market="),
            "el mercado es necesario para datos completos"
        );
    }

    #[tokio::test]
    async fn el_limite_de_busqueda_se_acota_al_maximo_de_spotify() {
        let t = Arc::new(
            TransporteFalso::nuevo()
                .con_json(&token())
                .con_json(r#"{"tracks":{"items":[],"total":0}}"#),
        );
        let p = proveedor(t.clone()).await;
        p.search_tracks("x", 200, 0).await.expect("busca");

        assert!(
            urls(&t)[0].contains("limit=50"),
            "Spotify rechaza límites mayores"
        );
    }

    #[tokio::test]
    async fn las_pistas_se_piden_en_lotes_de_cincuenta() {
        let vacio = r#"{"tracks":[]}"#;
        let t = Arc::new(
            TransporteFalso::nuevo()
                .con_json(&token())
                .con_json(vacio)
                .con_json(vacio)
                .con_json(vacio),
        );
        let p = proveedor(t.clone()).await;

        let ids: Vec<TrackId> = (0..120)
            .map(|i| TrackId::from_trusted(format!("id{i:019}")))
            .collect();
        p.tracks(&ids).await.expect("consulta");

        assert_eq!(
            urls(&t).len(),
            3,
            "120 pistas deben salir en tres peticiones, no en 120"
        );
    }

    #[tokio::test]
    async fn una_respuesta_con_pistas_desconocidas_devuelve_las_demas() {
        let json = r#"{"tracks":[
            null,
            {"id":"3z8h0TU7ReDPLIbEnYhWZb","name":"Under Pressure","duration_ms":248000},
            null
        ]}"#;
        let t = Arc::new(TransporteFalso::nuevo().con_json(&token()).con_json(json));
        let p = proveedor(t).await;

        let pistas = p
            .tracks(&[
                TrackId::from_trusted("a".repeat(22)),
                TrackId::from_trusted("3z8h0TU7ReDPLIbEnYhWZb"),
                TrackId::from_trusted("b".repeat(22)),
            ])
            .await
            .expect("consulta");

        assert_eq!(pistas.len(), 1, "pedir tres y recibir una es normal");
    }

    #[tokio::test]
    async fn las_pistas_de_un_album_heredan_su_album() {
        let json = r#"{
            "id":"1GbtB4zTqAsyfZEsm1RZfx","name":"Hot Space","album_type":"album",
            "release_date":"1982-05-21","release_date_precision":"day","label":"EMI",
            "tracks":{"items":[
                {"id":"3z8h0TU7ReDPLIbEnYhWZb","name":"Under Pressure","duration_ms":248000,
                 "track_number":11,"disc_number":1,
                 "artists":[{"id":"1dfeR4HaWDbWqFHLkxsg1d","name":"Queen"}]}
            ]}
        }"#;
        let t = Arc::new(TransporteFalso::nuevo().con_json(&token()).con_json(json));
        let p = proveedor(t).await;

        let pistas = p
            .album_tracks(&AlbumId::from_trusted("1GbtB4zTqAsyfZEsm1RZfx"))
            .await
            .expect("consulta");

        assert_eq!(pistas.len(), 1);
        assert_eq!(
            pistas[0].album.as_ref().map(|a| a.title.as_str()),
            Some("Hot Space"),
            "las pistas de /albums/{{id}}/tracks no traen álbum: por eso se pide el completo"
        );
        assert_eq!(
            pistas[0].release_date,
            chrono::NaiveDate::from_ymd_opt(1982, 5, 21)
        );
    }

    #[tokio::test]
    async fn importar_una_playlist_recorre_todas_sus_paginas() {
        // 150 pistas: la primera página viene en el recurso y falta una más.
        let primera: String = format!(
            r#"{{"id":"37i9dQZF1DXcBWIGoYBM5M","name":"Mix","tracks":{{"items":[{}],"total":150}}}}"#,
            (0..100)
                .map(|i| format!(
                    r#"{{"track":{{"id":"{i:022}","name":"P{i}","duration_ms":1000}}}}"#
                ))
                .collect::<Vec<_>>()
                .join(",")
        );
        let segunda: String = format!(
            r#"{{"items":[{}],"total":150}}"#,
            (100..150)
                .map(|i| format!(
                    r#"{{"track":{{"id":"{i:022}","name":"P{i}","duration_ms":1000}}}}"#
                ))
                .collect::<Vec<_>>()
                .join(",")
        );

        let t = Arc::new(
            TransporteFalso::nuevo()
                .con_json(&token())
                .con_json(&primera)
                .con_json(&segunda),
        );
        let p = proveedor(t.clone()).await;

        let progreso = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let capturado = std::sync::Arc::clone(&progreso);
        let callback = move |hechas: u32, total: u32| {
            if let Ok(mut v) = capturado.lock() {
                v.push((hechas, total));
            }
        };

        let importada = p
            .public_playlist(
                "https://open.spotify.com/playlist/37i9dQZF1DXcBWIGoYBM5M",
                &callback,
            )
            .await
            .expect("importa");

        assert_eq!(importada.name, "Mix");
        assert_eq!(importada.total, 150);
        assert_eq!(importada.tracks.len(), 150);

        let avisos = progreso.lock().expect("lock").clone();
        assert_eq!(
            avisos,
            vec![(100, 150), (150, 150)],
            "debe informarse por página"
        );
    }

    #[tokio::test]
    async fn importar_salta_las_entradas_sin_pista() {
        // Ocurre con pistas retiradas del catálogo o ficheros locales.
        let json = r#"{"id":"37i9dQZF1DXcBWIGoYBM5M","name":"Con huecos","tracks":{"items":[
            {"track":null},
            {"track":{"id":"3z8h0TU7ReDPLIbEnYhWZb","name":"Buena","duration_ms":1000}},
            {"track":null}
        ],"total":3}}"#;

        let t = Arc::new(TransporteFalso::nuevo().con_json(&token()).con_json(json));
        let p = proveedor(t).await;

        let importada = p
            .public_playlist("37i9dQZF1DXcBWIGoYBM5M", &|_, _| {})
            .await
            .expect("importa");

        assert_eq!(importada.tracks.len(), 1);
        assert_eq!(importada.total, 3, "el total declarado se conserva");
    }

    #[tokio::test]
    async fn importar_algo_que_no_es_una_playlist_falla_antes_de_salir_a_la_red() {
        let t = Arc::new(TransporteFalso::nuevo());
        let p = proveedor(t.clone()).await;

        assert!(
            p.public_playlist("https://ejemplo.com/lista", &|_, _| {})
                .await
                .is_err()
        );
        assert_eq!(t.cuantas(), 0);
    }

    #[tokio::test]
    async fn un_album_inaccesible_da_no_encontrado() {
        let t = Arc::new(
            TransporteFalso::nuevo()
                .con_json(&token())
                .con_estado(404, None),
        );
        let p = proveedor(t).await;

        let error = p
            .album(&AlbumId::from_trusted("1GbtB4zTqAsyfZEsm1RZfx"))
            .await
            .expect_err("debe fallar");
        assert_eq!(error.code(), "NOT_FOUND");
    }
}

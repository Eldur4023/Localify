//! Recomendaciones.
//!
//! ## Qué significa "generarlas localmente"
//!
//! Que **el criterio es nuestro**: quién te gusta, qué géneros pones, qué hay en
//! tus playlists y qué llevas meses sin escuchar. No que las canciones tengan
//! que estar ya en la máquina.
//!
//! Lo que el proyecto descarta es delegar la decisión en un tercero —mandarle a
//! alguien tu historial y pintar la lista que devuelva—. Eso no pasa aquí: no
//! hay servicio de recomendaciones, no hay cuenta que crear y no sale de la
//! máquina nada sobre lo que escuchas.
//!
//! Preguntarle al catálogo "¿qué tiene este artista?" o "¿qué hay de este
//! género?" es otra cosa: es la misma consulta que hace el buscador, con una
//! palabra que hemos elegido nosotros a partir de tu historial. El catálogo
//! contesta un listado; la selección la hacemos aquí.
//!
//! Sin eso, "recomiéndame algo" solo podía responder con lo que ya habías
//! buscado tú, que es una forma cara de no recomendar nada.
//!
//! ## Por qué las secciones se omiten en vez de rellenarse
//!
//! Una biblioteca recién creada no tiene historial. La tentación es rellenar
//! Inicio con pistas al azar para que no se vea vacío; el resultado es una
//! pantalla que finge conocerte y acierta menos que no decir nada.
//!
//! Aquí una sección sin datos suficientes **no aparece**. Inicio crece conforme
//! la biblioteca da información real, y lo que muestra siempre significa algo.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use localify_core::domain::ids::{ArtistId, PlaylistId, TrackId};
use localify_core::domain::track::{Track, TrackRow};
use localify_core::error::CoreResult;
use localify_core::page::PageRequest;
use localify_core::ports::database::{
    ArtistRepository, CacheRepository, FavoriteRepository, HistoryRepository, PlaylistRepository,
    SimilarityRepository, TrackRepository,
};
use localify_core::ports::metadata_provider::MetadataProvider;
use localify_core::ports::services::{HomeItems, HomeSection, RecommendationService};
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

/// Elementos por sección de Inicio. Una fila de tarjetas, como Spotify.
const POR_SECCION: u8 = 12;

/// Mínimo para que una sección merezca aparecer.
///
/// Con menos de cuatro, la fila queda coja y da la sensación de que la
/// aplicación no tiene nada que ofrecer. Es mejor omitirla.
const MINIMO_SECCION: usize = 4;

/// Días que se miran para "lo que más escuchas".
const VENTANA_DIAS: u16 = 30;

/// Días sin escuchar para considerar que algo se puede redescubrir.
const DIAS_OLVIDO: u16 = 90;

/// Cuántos artistas del historial se usan como semilla para el catálogo.
///
/// Cinco: con menos la fila sale monotemática, y con más las peticiones se
/// acumulan sin que aporten variedad —el sexto artista más escuchado ya casi no
/// distingue el gusto de nadie—.
const SEMILLAS: u8 = 5;

/// Dónde se guarda la tanda de candidatos del catálogo.
const CACHE_NS: &str = "home";
const CACHE_CLAVE: &str = "del_catalogo";

/// Cuánto vale una tanda antes de volver a pedirla.
///
/// Seis horas. Más corto convertiría abrir Inicio en tráfico constante para una
/// fila que apenas cambia; más largo la dejaría enseñando lo mismo durante días.
const FRESCURA: Duration = Duration::from_secs(6 * 3600);

/// Cuánto se conserva la tanda en disco.
///
/// Muy por encima de [`FRESCURA`] a propósito: la caducidad de verdad se
/// comprueba mirando la marca de tiempo de dentro, y lo viejo se sigue
/// enseñando mientras se busca lo nuevo. Si expirase a las seis horas, la fila
/// desaparecería de Inicio cada vez que toca refrescar.
const CONSERVACION: u64 = 30 * 24 * 3600;

/// Una tanda de candidatos traída del catálogo.
#[derive(Serialize, Deserialize)]
struct Tanda {
    /// Segundos desde época en que se pidió. Ver [`CONSERVACION`].
    generada: i64,
    ids: Vec<String>,
}

/// Dependencias del servicio.
pub struct Dependencias {
    pub tracks: Arc<dyn TrackRepository>,
    pub artistas: Arc<dyn ArtistRepository>,
    pub historial: Arc<dyn HistoryRepository>,
    pub favoritos: Arc<dyn FavoriteRepository>,
    pub playlists: Arc<dyn PlaylistRepository>,
    pub similitud: Arc<dyn SimilarityRepository>,
    /// El mismo catálogo del que salen las búsquedas.
    pub provider: Arc<dyn MetadataProvider>,
    /// Para no pedirle al catálogo lo mismo cada vez que se abre Inicio.
    pub cache: Arc<dyn CacheRepository>,
}

impl std::fmt::Debug for Dependencias {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Dependencias").finish_non_exhaustive()
    }
}

pub struct RecommendationServiceImpl {
    deps: Arc<Dependencias>,
}

impl std::fmt::Debug for RecommendationServiceImpl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RecommendationServiceImpl")
            .finish_non_exhaustive()
    }
}

impl RecommendationServiceImpl {
    #[must_use]
    pub fn nuevo(deps: Dependencias) -> Self {
        Self {
            deps: Arc::new(deps),
        }
    }

    /// Rehidrata identificadores conservando el orden de afinidad.
    ///
    /// El repositorio devuelve en orden de puntuación; `rows_by_ids` no
    /// garantiza ninguno. Sin este paso, las mejores recomendaciones acabarían
    /// repartidas al azar por la fila.
    async fn en_orden(&self, ids: &[TrackId]) -> CoreResult<Vec<TrackRow>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let filas = self.deps.tracks.rows_by_ids(ids).await?;
        Ok(ids
            .iter()
            .filter_map(|id| filas.iter().find(|f| &f.id == id).cloned())
            .collect())
    }
}

/// Pide candidatos al catálogo, los persiste y guarda la tanda.
///
/// Devuelve cuántos quedaron. Cero es un resultado válido: sin historial no hay
/// semilla, y con un catálogo que no responde tampoco pasa nada —la fila no
/// aparece y las demás secciones siguen igual—.
///
/// ## De dónde salen las semillas
///
/// De tus artistas más escuchados y de los géneros de esos artistas. Las dos son
/// preguntas que ya sabemos hacer —son las mismas del buscador— y lo que decide
/// **qué** se pregunta sale entero del historial local.
async fn candidatos(deps: &Arc<Dependencias>) -> CoreResult<usize> {
    let semilla = deps.historial.top_artists(VENTANA_DIAS, SEMILLAS).await?;
    if semilla.is_empty() {
        return Ok(0);
    }

    // Por artista y no todo junto: así se puede intercalar después. Doce
    // canciones del mismo grupo no es una recomendación, es su discografía.
    let mut por_semilla: Vec<Vec<Track>> = Vec::new();
    for artista in &semilla {
        por_semilla.push(del_artista(deps, &artista.id).await);
    }
    for genero in generos_de(deps, &semilla).await {
        por_semilla.push(del_genero(deps, &genero).await);
    }

    let escogidas = intercalar(por_semilla, POR_SECCION as usize * 3);

    // Se quedan fuera las que ya has puesto alguna vez: es lo que separa esta
    // fila de las de historial, que están tres secciones más abajo.
    let mut nuevas = Vec::new();
    for pista in escogidas {
        if deps.historial.play_count(&pista.id).await.unwrap_or(0) == 0 {
            nuevas.push(pista);
        }
        if nuevas.len() >= POR_SECCION as usize {
            break;
        }
    }
    if nuevas.is_empty() {
        return Ok(0);
    }

    // Persistir es obligatorio, no una optimización: la fila enseña `TrackRow`
    // leídas de la base de datos, y reproducir una pista necesita que exista.
    // Sin esto la sección saldría siempre vacía por mucho que el catálogo
    // contestara.
    deps.tracks.upsert(&nuevas).await?;

    let tanda = Tanda {
        generada: chrono::Utc::now().timestamp(),
        ids: nuevas.iter().map(|t| t.id.as_str().to_owned()).collect(),
    };
    let bytes = serde_json::to_vec(&tanda).unwrap_or_default();
    deps.cache
        .put(CACHE_NS, CACHE_CLAVE, &bytes, CONSERVACION)
        .await?;

    Ok(nuevas.len())
}

/// Lo mejor de un artista, según el catálogo. Un fallo suyo no tumba la tanda.
async fn del_artista(deps: &Arc<Dependencias>, id: &ArtistId) -> Vec<Track> {
    match deps.provider.artist_top_tracks(id).await {
        Ok(v) => v,
        Err(e) => {
            debug!(artista = %id, error = %e, "el catálogo no dio nada de este artista");
            Vec::new()
        }
    }
}

/// Los géneros de los artistas semilla, sin repetir.
///
/// Salen de la ficha guardada del artista, que es donde el catálogo los dejó al
/// buscarlo. No se piden otra vez.
async fn generos_de(
    deps: &Arc<Dependencias>,
    semilla: &[localify_core::domain::artist::ArtistRow],
) -> Vec<String> {
    let mut vistos = std::collections::HashSet::new();
    let mut generos = Vec::new();
    for fila in semilla {
        let Ok(Some(artista)) = deps.artistas.get(&fila.id).await else {
            continue;
        };
        for g in artista.genres {
            // Dos géneros bastan. Cada uno es una búsqueda más, y a partir del
            // tercero describen al artista, no al gusto.
            if generos.len() >= 2 {
                return generos;
            }
            if vistos.insert(g.clone()) {
                generos.push(g);
            }
        }
    }
    generos
}

/// Canciones de un género, preguntando al catálogo como lo haría el buscador.
async fn del_genero(deps: &Arc<Dependencias>, genero: &str) -> Vec<Track> {
    match deps.provider.search_tracks(genero, POR_SECCION, 0).await {
        Ok(p) => p.items,
        Err(e) => {
            debug!(genero, error = %e, "el catálogo no dio nada de este género");
            Vec::new()
        }
    }
}

/// Toma una de cada montón por vuelta, hasta agotarlos o llegar a `tope`.
///
/// Es lo que impide que el artista con más canciones se coma la fila entera:
/// intercalando, los cinco aparecen aunque uno traiga veinte y otro traiga dos.
/// Los repetidos se descartan, que los hay —un artista sale en su propio género—.
fn intercalar(mut montones: Vec<Vec<Track>>, tope: usize) -> Vec<Track> {
    let mut salida = Vec::new();
    let mut vistos = std::collections::HashSet::new();
    let mut vuelta = 0;

    while salida.len() < tope {
        let mut quedaba = false;
        for monton in &mut montones {
            let Some(pista) = monton.get(vuelta) else {
                continue;
            };
            quedaba = true;
            if vistos.insert(pista.id.clone()) {
                salida.push(pista.clone());
                if salida.len() >= tope {
                    return salida;
                }
            }
        }
        if !quedaba {
            break;
        }
        vuelta += 1;
    }
    salida
}

/// Añade la sección si tiene material suficiente.
fn empujar(secciones: &mut Vec<HomeSection>, key: &str, items: HomeItems) {
    let cuantos = match &items {
        HomeItems::Tracks(v) => v.len(),
        HomeItems::Albums(v) => v.len(),
        HomeItems::Artists(v) => v.len(),
        HomeItems::Playlists(v) => v.len(),
    };
    if cuantos < MINIMO_SECCION {
        debug!(seccion = key, cuantos, "seccion omitida por falta de datos");
        return;
    }
    secciones.push(HomeSection {
        key: key.to_owned(),
        params: Vec::new(),
        items,
    });
}

#[async_trait]
impl RecommendationService for RecommendationServiceImpl {
    async fn home(&self) -> CoreResult<Vec<HomeSection>> {
        // El orden de Inicio es una postura, no un detalle. Con las secciones de
        // historial arriba, la pantalla entera era un resumen de lo que ya se
        // había puesto: útil el primer día, insufrible el vigésimo. Lo que
        // justifica que exista una pantalla llamada Inicio es que enseñe algo
        // que no habrías buscado tú, y solo después te devuelva lo tuyo.
        let mut secciones = Vec::new();
        self.descubrimiento(&mut secciones).await?;
        self.lo_tuyo(&mut secciones).await?;

        // Con la biblioteca recién estrenada no hay historial que mirar, así
        // que se ofrece lo que sí hay: los favoritos.
        if secciones.is_empty() {
            let me_gusta = self
                .deps
                .favoritos
                .list(&PageRequest::new(0, u32::from(POR_SECCION)))
                .await?;
            empujar(
                &mut secciones,
                "home.favorites",
                HomeItems::Tracks(me_gusta.items),
            );
        }

        Ok(secciones)
    }

    async fn similar_to_track(&self, id: &TrackId, limit: u8) -> CoreResult<Vec<TrackRow>> {
        let afines = self.deps.similitud.similar_to_track(id, limit).await?;
        let ids: Vec<TrackId> = afines.into_iter().map(|(t, _)| t).collect();
        self.en_orden(&ids).await
    }

    async fn for_playlist(&self, id: &PlaylistId, limit: u8) -> CoreResult<Vec<TrackRow>> {
        self.afines_a_playlist(id, limit).await
    }
}

impl RecommendationServiceImpl {
    /// Lo que no conoces: las únicas secciones que pueden enseñar algo nuevo.
    async fn descubrimiento(&self, secciones: &mut Vec<HomeSection>) -> CoreResult<()> {
        self.del_catalogo(secciones).await;

        // "Descubre": de lo que ya está en la máquina, lo que encaja con lo que
        // escuchas y **no has puesto nunca**.
        let descubrir = self
            .deps
            .similitud
            .discover(VENTANA_DIAS, POR_SECCION)
            .await?;
        let ids: Vec<TrackId> = descubrir.into_iter().map(|(t, _)| t).collect();
        empujar(
            secciones,
            "home.discover",
            HomeItems::Tracks(self.en_orden(&ids).await?),
        );

        // "Porque escuchaste X": del mismo artista o álbum que lo reciente.
        let porque = self
            .deps
            .similitud
            .because_you_listened(POR_SECCION)
            .await?;
        let ids: Vec<TrackId> = porque.into_iter().map(|(t, _, _)| t).collect();
        empujar(
            secciones,
            "home.because_you_listened",
            HomeItems::Tracks(self.en_orden(&ids).await?),
        );

        // "Redescubre": favoritos que llevan meses sin sonar. Va aquí por el
        // mismo motivo: recuperar algo olvidado se parece más a descubrir que a
        // repetir.
        let olvidados = self
            .deps
            .historial
            .rediscover(DIAS_OLVIDO, POR_SECCION)
            .await?;
        empujar(secciones, "home.rediscover", HomeItems::Tracks(olvidados));
        Ok(())
    }

    /// "Puede que te guste": candidatos traídos del catálogo.
    ///
    /// **Nunca espera a la red.** Inicio se pinta al abrir la aplicación y con
    /// cada vuelta a la pestaña; hacer cinco peticiones ahí dejaría la pantalla
    /// en blanco unos segundos cada vez. Se enseña lo que haya guardado y, si
    /// está viejo o no hay nada, se pide en segundo plano para la próxima.
    ///
    /// La consecuencia es que la primera vez la fila no sale. Es preferible a la
    /// alternativa —una pantalla que tarda—, y a partir de ahí siempre hay algo.
    ///
    /// No devuelve error: quedarse sin esta fila no puede impedir que Inicio se
    /// pinte.
    async fn del_catalogo(&self, secciones: &mut Vec<HomeSection>) {
        let guardada = self
            .deps
            .cache
            .get(CACHE_NS, CACHE_CLAVE)
            .await
            .ok()
            .flatten()
            .and_then(|b| serde_json::from_slice::<Tanda>(&b).ok());

        // Una edad negativa es una tanda "del futuro": pasa si el reloj se
        // atrasa. Se trata como caducada, que es lo seguro —se vuelve a pedir—
        // en vez de darla por fresca durante las horas que el reloj vaya mal.
        let caduca = guardada.as_ref().is_none_or(|t| {
            let edad = chrono::Utc::now().timestamp().saturating_sub(t.generada);
            u64::try_from(edad).is_ok_and(|e| e >= FRESCURA.as_secs()) || edad < 0
        });
        if caduca {
            self.pedir_al_catalogo();
        }

        if let Some(tanda) = guardada {
            let ids: Vec<TrackId> = tanda.ids.into_iter().map(TrackId::from_trusted).collect();
            if let Ok(filas) = self.en_orden(&ids).await {
                empujar(secciones, "home.from_catalog", HomeItems::Tracks(filas));
            }
        }
    }

    /// Lanza la búsqueda de candidatos en segundo plano.
    fn pedir_al_catalogo(&self) {
        let deps = Arc::clone(&self.deps);
        tokio::spawn(async move {
            match candidatos(&deps).await {
                Ok(0) => debug!("el catálogo no dio candidatos nuevos"),
                Ok(n) => debug!(candidatos = n, "tanda del catálogo lista"),
                Err(e) => warn!(error = %e, "no se pudo pedir candidatos al catálogo"),
            }
        });
    }

    /// Y después, volver a lo tuyo.
    async fn lo_tuyo(&self, secciones: &mut Vec<HomeSection>) -> CoreResult<()> {
        // "Tus playlists favoritas": las que más se ponen. Una playlist es una
        // decisión que el usuario ya tomó —estas canciones, en este orden—, y
        // volver a ponerla es lo que quiere a menudo.
        //
        // Se mide por el contexto del historial, no por si contienen canciones
        // oídas: ver `most_played`.
        let favoritas = self
            .deps
            .playlists
            .most_played(VENTANA_DIAS, POR_SECCION)
            .await?;
        empujar(
            secciones,
            "home.top_playlists",
            HomeItems::Playlists(favoritas.clone()),
        );

        // "Sigue escuchando": lo más reciente.
        let recientes = self
            .deps
            .historial
            .recent_tracks(u16::from(POR_SECCION))
            .await?;
        empujar(
            secciones,
            "home.recent",
            HomeItems::Tracks(recientes.clone()),
        );

        // "Lo que más escuchas": el top del último mes.
        let mas_oidas = self
            .deps
            .historial
            .top_tracks(VENTANA_DIAS, POR_SECCION)
            .await?;
        empujar(secciones, "home.top_tracks", HomeItems::Tracks(mas_oidas));

        // "Álbumes que escuchas": ordenados por cuántas canciones suyas suenan,
        // no por escuchas totales. Un disco del que solo se repite el single no
        // es un disco que escuches.
        let albumes = self
            .deps
            .historial
            .top_albums(VENTANA_DIAS, POR_SECCION)
            .await?;
        empujar(secciones, "home.top_albums", HomeItems::Albums(albumes));

        // "Tus artistas": los más escuchados del último mes.
        let artistas = self
            .deps
            .historial
            .top_artists(VENTANA_DIAS, POR_SECCION)
            .await?;
        empujar(secciones, "home.top_artists", HomeItems::Artists(artistas));

        // "Tus playlists": el resto, por lo último que se tocó.
        //
        // Se descartan las que ya salen arriba: la misma playlist dos veces en
        // la misma pantalla hace que Inicio parezca corto de material, y ocupa
        // el sitio de una que el usuario no ha visto.
        let arriba: std::collections::HashSet<_> = favoritas.iter().map(|p| p.id).collect();
        let mut listas: Vec<_> = self
            .deps
            .playlists
            .list_summaries()
            .await?
            .into_iter()
            .filter(|p| !arriba.contains(&p.id))
            .collect();
        listas.truncate(POR_SECCION as usize);
        empujar(secciones, "home.playlists", HomeItems::Playlists(listas));

        Ok(())
    }

    /// Sugerencias para una playlist concreta.
    ///
    /// Vive aquí y no en el trait porque el trait ya la expone; esta es su
    /// implementación.
    async fn afines_a_playlist(&self, id: &PlaylistId, limit: u8) -> CoreResult<Vec<TrackRow>> {
        // Se recorren las entradas para tener la semilla completa: recomendar
        // a partir de las primeras cincuenta de una playlist de quinientas
        // daría sugerencias sesgadas hacia lo que se añadió primero.
        let mut dentro = Vec::new();
        let mut offset = 0_u32;
        loop {
            let pagina = self
                .deps
                .playlists
                .entries(id, &PageRequest::new(offset, 200))
                .await?;
            if pagina.items.is_empty() {
                break;
            }
            let leidas = u32::try_from(pagina.items.len()).unwrap_or(200);
            dentro.extend(pagina.items.into_iter().map(|e| e.track.id));
            offset += leidas;
        }

        if dentro.is_empty() {
            return Ok(Vec::new());
        }

        let afines = self.deps.similitud.similar_to_set(&dentro, limit).await?;
        let ids: Vec<TrackId> = afines.into_iter().map(|(t, _)| t).collect();
        self.en_orden(&ids).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fila(n: u8) -> TrackRow {
        TrackRow {
            id: TrackId::nuevo_local(),
            title: format!("P{n}"),
            artist_display: "A".into(),
            album_id: None,
            album_title: None,
            duration: localify_core::domain::audio::DurationMs::from_secs(180),
            availability: localify_core::domain::availability::Availability::Absent,
            is_favorite: false,
            explicit: false,
            popularity: None,
            added_at: None,
        }
    }

    #[test]
    fn una_seccion_con_pocos_elementos_no_se_muestra() {
        // Rellenar Inicio con lo que sea para que no se vea vacio da una
        // pantalla que finge conocerte y acierta menos que no decir nada.
        let mut s = Vec::new();
        empujar(
            &mut s,
            "home.recent",
            HomeItems::Tracks((0..3).map(fila).collect()),
        );
        assert!(s.is_empty(), "tres elementos dejan la fila coja");
    }

    #[test]
    fn una_seccion_con_material_suficiente_si_se_muestra() {
        let mut s = Vec::new();
        empujar(
            &mut s,
            "home.recent",
            HomeItems::Tracks(
                (0..u8::try_from(MINIMO_SECCION).unwrap_or(4))
                    .map(fila)
                    .collect(),
            ),
        );
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].key, "home.recent");
    }

    #[test]
    fn una_seccion_vacia_nunca_aparece() {
        let mut s = Vec::new();
        empujar(&mut s, "home.recent", HomeItems::Tracks(Vec::new()));
        empujar(&mut s, "home.playlists", HomeItems::Playlists(Vec::new()));
        assert!(s.is_empty());
    }
}

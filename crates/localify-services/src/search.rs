//! Servicio de búsqueda.
//!
//! Implementa el flujo del proyecto: **local primero, proveedor después,
//! YouTube jamás**.
//!
//! ## Las dos mitades
//!
//! Una búsqueda no es una respuesta: es un resultado local inmediato más un
//! refuerzo remoto opcional. Esperar al proveedor antes de pintar nada
//! convertiría cada pulsación en medio segundo de pantalla vacía, y la mayoría
//! de las veces lo que se busca ya está en la biblioteca.
//!
//! Por eso la consulta remota va en una tarea aparte y avisa por evento cuando
//! termina, con el mismo `query_id` que el cliente recibió: así puede descartar
//! respuestas de pulsaciones ya superadas.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use localify_core::domain::album::AlbumRow;
use localify_core::domain::artist::ArtistRow;
use localify_core::domain::ids::{ArtistId, TrackId};
use localify_core::domain::track::TrackRow;
use localify_core::domain::versiones;
use localify_core::error::CoreResult;
use localify_core::events::{DomainEvent, EventPublisher};
use localify_core::page::PageRequest;
use localify_core::ports::database::{SearchRepository, TrackRepository};
use localify_core::ports::metadata_provider::MetadataProvider;
use localify_core::ports::services::{
    GrupoDeVersiones, PrimeraCoincidencia, RemoteResults, SearchResults, SearchScope, SearchService,
};
use localify_core::text;
use tracing::debug;

use crate::metadata::MetadataServiceImpl;

/// Resultados que se piden al proveedor.
const LIMITE_REMOTO: u8 = 20;

/// Espera antes de salir a la red, para que no haya una petición por tecla.
///
/// Escribir "rick astley never gonna" son diez pulsaciones y, sin este freno,
/// diez consultas al proveedor de las que nueve se descartan. Además de
/// derrochar, es una forma eficaz de que YouTube empiece a responder con
/// captchas.
///
/// El freno vive aquí y no en la interfaz porque la parte cara es la red, y
/// frenarlo allí obligaría a frenar también la búsqueda local, que tarda
/// milisegundos y no lo necesita.
const REBOTE_REMOTO: std::time::Duration = std::time::Duration::from_millis(350);

// Spotify rechaza límites mayores de 50 en `/search`.
const _: () = assert!(LIMITE_REMOTO <= 50);

pub struct SearchServiceImpl {
    search: Arc<dyn SearchRepository>,
    tracks: Arc<dyn TrackRepository>,
    provider: Arc<dyn MetadataProvider>,
    metadata: Arc<MetadataServiceImpl>,
    bus: Arc<dyn EventPublisher>,
    contador: AtomicU64,
    /// Identificador de la última búsqueda pedida.
    ///
    /// Lo lee la tarea remota tras el rebote para saber si su consulta sigue
    /// siendo la vigente. Va aparte de `contador` porque este solo sabe
    /// repartir números, no cuál fue el último que se dio.
    ultimo_id: Arc<AtomicU64>,
    /// Última respuesta remota, en **el orden que la dio el proveedor**.
    ///
    /// Sin esto, los resultados remotos se persisten y se vuelven a leer por el
    /// índice local, que los reordena por relevancia de texto. Buscando
    /// "despacito" eso es fatal: veinte versiones con el mismo título empatan en
    /// bm25 y la buena queda donde caiga, mientras que el proveedor **sí** sabe
    /// cuál es la famosa y la pone primera.
    ///
    /// Solo se guarda la última consulta: es la que el cliente va a repetir al
    /// recibir `SearchRemoteReady`, y guardar más sería una caché que nadie
    /// pidió ni invalida.
    ultima_remota: Arc<std::sync::Mutex<Option<(String, Vec<TrackId>)>>>,
}

impl std::fmt::Debug for SearchServiceImpl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SearchServiceImpl").finish_non_exhaustive()
    }
}

impl SearchServiceImpl {
    #[must_use]
    pub fn nuevo(
        search: Arc<dyn SearchRepository>,
        tracks: Arc<dyn TrackRepository>,
        provider: Arc<dyn MetadataProvider>,
        metadata: Arc<MetadataServiceImpl>,
        bus: Arc<dyn EventPublisher>,
    ) -> Self {
        Self {
            search,
            tracks,
            provider,
            metadata,
            bus,
            contador: AtomicU64::new(0),
            ultimo_id: Arc::new(AtomicU64::new(0)),
            ultima_remota: Arc::new(std::sync::Mutex::new(None)),
        }
    }

    /// Decide si vale la pena consultar al proveedor.
    ///
    /// Antes se rendía cuando ya había ocho coincidencias locales, con el
    /// razonamiento de que quien busca algo que ya tiene está buscando lo suyo.
    /// Dejó de ser cierto al persistir cada búsqueda: esas ocho coincidencias
    /// son lo que devolvió el proveedor la última vez, así que el atajo hacía
    /// que buscar dos veces lo mismo enseñara para siempre la respuesta vieja,
    /// sin volver a preguntar nunca.
    async fn conviene_preguntar(&self) -> bool {
        self.provider.status().await.esta_operativo()
    }

    /// Resultados remotos ya recibidos para esta consulta, si los hay.
    ///
    /// Se guardan los identificadores y no las filas: una fila lleva
    /// disponibilidad y favorito, que son estado **local**. Construirlas desde
    /// lo que devolvió el proveedor obligaría a inventar esos campos, y una
    /// canción ya descargada aparecería como ausente. Se rehidratan del
    /// repositorio, que sí lo sabe, y se recolocan en el orden guardado.
    async fn remota_guardada(&self, consulta: &str) -> Option<Vec<TrackRow>> {
        let clave = text::normalize(consulta);
        let ids: Vec<TrackId> = {
            let hueco = self.ultima_remota.lock().ok()?;
            let (q, ids) = hueco.as_ref()?;
            if *q != clave {
                return None;
            }
            ids.clone()
        };

        let filas = self.tracks.rows_by_ids(&ids).await.ok()?;
        // `rows_by_ids` no garantiza orden; recolocarlas es justamente el
        // motivo de todo esto.
        Some(
            ids.iter()
                .filter_map(|id| filas.iter().find(|f| &f.id == id).cloned())
                .collect(),
        )
    }

    /// Lanza la consulta remota en segundo plano y avisa al terminar.
    fn lanzar_remota(&self, consulta: String, query_id: u64) {
        let provider = Arc::clone(&self.provider);
        let metadata = Arc::clone(&self.metadata);
        let bus = Arc::clone(&self.bus);
        let ultima = Arc::clone(&self.ultima_remota);

        let contador = Arc::clone(&self.ultimo_id);

        tokio::spawn(async move {
            // Se espera antes de salir a la red y se comprueba si esta consulta
            // sigue siendo la última. Si el usuario ha seguido escribiendo, la
            // petición no llega a hacerse: sus resultados no los querría nadie.
            tokio::time::sleep(REBOTE_REMOTO).await;
            if contador.load(Ordering::Relaxed) != query_id {
                debug!(query_id, "consulta superada antes de salir a la red");
                return;
            }

            let pistas = match provider.search_tracks(&consulta, LIMITE_REMOTO, 0).await {
                Ok(p) => p.items,
                Err(e) => {
                    debug!(error = %e, "la búsqueda remota falló");
                    // No se emite nada: el cliente ya recibió `Loading` y, si no
                    // llega el aviso, se queda con lo local. Emitir un error por
                    // cada pulsación fallida sería ruido.
                    return;
                }
            };

            if pistas.is_empty() {
                bus.publish(DomainEvent::SearchRemoteReady { query_id });
                return;
            }

            // Persistir antes de avisar es lo que hace que el cliente solo tenga
            // que repetir la consulta local: los resultados remotos ya están en
            // la base de datos cuando recibe el evento.
            let artistas: Vec<ArtistId> = pistas
                .iter()
                .flat_map(|p| p.artists.iter().map(|a| a.id.clone()))
                .collect();

            if let Err(e) = metadata.persistir(&pistas).await {
                debug!(error = %e, "no se pudieron persistir los resultados remotos");
                return;
            }

            // Se guarda **el orden del proveedor**, que es lo único que sabe
            // cuál de veinte "Despacito" es la de Luis Fonsi. Persistirlas y
            // releerlas del índice local las reordena por relevancia de texto,
            // donde todas empatan.
            if let Ok(mut hueco) = ultima.lock() {
                *hueco = Some((
                    text::normalize(&consulta),
                    pistas.iter().map(|p| p.id.clone()).collect(),
                ));
            }

            // Los géneros llegan aparte y alimentan las recomendaciones. Se
            // completan sin bloquear el aviso.
            let metadata_bg = Arc::clone(&metadata);
            tokio::spawn(async move {
                let _ = metadata_bg.completar_artistas(&artistas).await;
            });

            bus.publish(DomainEvent::SearchRemoteReady { query_id });
        });
    }
}

/// Une la respuesta del proveedor con lo que ya había en el catálogo.
///
/// Manda el orden del proveedor: es el único que sabe cuál de veinte
/// "Despacito" es la que la gente busca. Lo local que no venga en su respuesta
/// se añade detrás, porque haberlo encontrado antes no lo hace irrelevante.
///
/// ## Se descartan dos clases de repetido
///
/// El **mismo identificador** en las dos listas es el caso habitual: buscar
/// persiste sus resultados, así que la búsqueda local encuentra justo lo que el
/// proveedor devolvió la vez anterior. Sin esto, cada canción salía dos veces.
///
/// El **mismo título, artista y duración** con identificadores distintos es el
/// otro: en YouTube una canción está subida varias veces, y el proveedor
/// devuelve varias de esas subidas. Son filas idénticas en pantalla, y elegir
/// entre ellas es una decisión que el usuario no puede tomar porque no tiene
/// con qué. Se exigen las tres coincidencias —no basta el título— para no
/// esconder una versión en directo o un remix, que sí son otra canción.
fn fundir(remotas: Vec<TrackRow>, locales: Vec<TrackRow>) -> Vec<TrackRow> {
    let mut ids: std::collections::HashSet<TrackId> = std::collections::HashSet::new();
    let mut huellas: std::collections::HashSet<(String, String, u32)> =
        std::collections::HashSet::new();

    let mut salida = Vec::with_capacity(remotas.len() + locales.len());
    for fila in remotas.into_iter().chain(locales) {
        if !ids.insert(fila.id.clone()) {
            continue;
        }
        if !huellas.insert(huella(&fila)) {
            continue;
        }
        salida.push(fila);
    }
    salida
}

/// Lo que hace que dos filas sean la misma canción a ojos del usuario.
fn huella(fila: &TrackRow) -> (String, String, u32) {
    (
        text::normalize(&fila.title),
        text::normalize(&fila.artist_display),
        fila.duration.as_ms(),
    )
}

/// Agrupa versiones de una misma canción en una sola fila.
///
/// ## Se agrupa por título **y artista**
///
/// Solo por título, "Hurt" de Nine Inch Nails y "Hurt" de Johnny Cash caerían
/// en la misma fila, y una de las dos desaparecería de la vista. Son canciones
/// distintas por mucho que compartan nombre.
///
/// El precio es que un cover ajeno se queda como fila propia en vez de colgar
/// de la original. Es el lado correcto en el que equivocarse: enseñar de más se
/// arregla mirando, esconder de menos no se arregla.
///
/// ## La consulta manda
///
/// Quien busca "faint live" quiere el directo. Agruparlo bajo la versión de
/// estudio sería esconder justo lo que pidió, así que cuando la consulta ya
/// nombra una variante no se agrupa nada. Es la misma regla que el emparejador
/// aplica al elegir candidato.
///
/// ## Dentro del grupo manda la original
///
/// Y a igualdad, la que el proveedor puso primero: viene ordenado por
/// relevancia y ese orden ya costó una petición.
fn agrupar(consulta: &str, filas: Vec<TrackRow>) -> Vec<GrupoDeVersiones> {
    if versiones::clase(consulta).es_variante() {
        return filas
            .into_iter()
            .map(|f| GrupoDeVersiones {
                principal: f,
                versiones: Vec::new(),
            })
            .collect();
    }

    let mut orden: Vec<(String, String)> = Vec::new();
    let mut grupos: std::collections::HashMap<(String, String), Vec<TrackRow>> =
        std::collections::HashMap::new();

    for fila in filas {
        let clave = (
            versiones::titulo_canonico(&fila.title),
            text::normalize(&fila.artist_display),
        );
        if !grupos.contains_key(&clave) {
            orden.push(clave.clone());
        }
        grupos.entry(clave).or_default().push(fila);
    }

    orden
        .into_iter()
        .filter_map(|clave| {
            let mut miembros = grupos.remove(&clave)?;

            // La original primero. `sort_by_key` es estable, así que dentro de
            // cada clase se conserva el orden del proveedor.
            miembros.sort_by_key(|f| u8::from(versiones::clase(&f.title).es_variante()));

            let mut it = miembros.into_iter();
            let principal = it.next()?;
            Some(GrupoDeVersiones {
                principal,
                versiones: it.collect(),
            })
        })
        .collect()
}

/// Elige qué destacar como primera coincidencia.
///
/// ## El criterio es "coincide del todo", no "coincide un poco"
///
/// Se busca un **nombre exacto** —ya normalizado: sin tildes, sin mayúsculas,
/// sin puntuación— y solo entonces se destaca. Escribir "amor" no debe sacar un
/// artista enorme llamado "Amor Eterno" solo porque empieza igual; a esa altura
/// el usuario todavía está escribiendo y lo que necesita es la lista.
///
/// ## El desempate va artista, álbum y canción
///
/// Quien escribe el nombre de un grupo quiere el grupo, no una canción que se
/// llame como él. Y quien escribe el nombre de un disco quiere el disco. La
/// canción va la última no por ser menos importante, sino porque ya tiene la
/// lista entera justo debajo: es lo único que no se pierde si no se destaca.
///
/// ## Salvo que el nombre del grupo sea, sobre todo, el de una canción
///
/// Esa preferencia se rompía en un caso concreto y bastante común: existe una
/// banda que se llama **exactamente** igual que una canción famosa. Buscando
/// "bury the light" se destacaba un grupo oscuro con ese nombre mientras la
/// canción —que era lo que se buscaba, y de la que había ocho versiones justo
/// debajo— quedaba sin destacar.
///
/// El desempate es contar, con el listón alto a propósito: el artista solo
/// pierde si hay **varias** canciones tituladas como la consulta y son más que
/// las suyas. Que media docena de gente distinta haya grabado algo con ese
/// nombre es una señal fuerte de que el nombre es de la canción; que una sola
/// lo haya hecho, no —y ahí sigue mandando la regla de arriba—.
///
/// Al revés funciona igual de bien: buscando "queen" hay una o dos canciones
/// tituladas "Queen" y veinte del grupo, así que gana el grupo.
///
/// Si nada coincide del todo, se cae a la primera canción, que es la que el
/// proveedor puso primera. Eso sí es una respuesta: significa "lo más probable
/// es esto".
fn primera_coincidencia(
    consulta: &str,
    tracks: &[TrackRow],
    albums: &[AlbumRow],
    artists: &[ArtistRow],
) -> Option<PrimeraCoincidencia> {
    let q = text::normalize(consulta);

    // Entre las que se llaman exactamente como la consulta, manda la más
    // escuchada.
    //
    // Es lo único que distingue "la que busca la gente" de "la que llegó
    // primero" cuando hay seis grabaciones con el mismo título. Buscar "judas"
    // devuelve una de Lady Gaga con mil cien millones de reproducciones y otra
    // de un grupo con doscientas mil, y sin este dato la elección era el orden
    // del proveedor.
    //
    // `max_by_key` se queda con **la última** en caso de empate; se recorre al
    // revés para que el empate lo gane la primera, que es la que el proveedor
    // consideró más relevante. Las de popularidad desconocida —MusicBrainz no la
    // mide— valen cero aquí, así que solo ganan si no hay ninguna con dato.
    let homonima = tracks
        .iter()
        .filter(|t| text::normalize(&t.title) == q)
        .rev()
        .max_by_key(|t| t.popularity.unwrap_or(0));

    if let Some(a) = artists.iter().find(|a| text::normalize(&a.name) == q) {
        // Cuántas canciones se llaman como la consulta, y cuántas son suyas.
        // `artist_display` es la cadena denormalizada, así que se busca el
        // nombre dentro en lugar de traer las relaciones. Buscar por subcadena
        // puede contar de más —un artista llamado "Muse" casaría con "Museum
        // Of…"—, y se acepta porque el error empuja hacia el lado conservador:
        // hace que gane el artista, que es la regla por defecto.
        let nombre = text::normalize(&a.name);
        let tituladas = tracks
            .iter()
            .filter(|t| text::normalize(&t.title) == q)
            .count();
        let suyas = tracks
            .iter()
            .filter(|t| text::normalize(&t.artist_display).contains(&nombre))
            .count();

        // Dos es el listón: una sola canción homónima es una coincidencia, y
        // varias son un indicio de que el nombre es de la canción.
        if tituladas < 2 || suyas >= tituladas {
            return Some(PrimeraCoincidencia::Artist(a.clone()));
        }
    }

    // El álbum se mide con la misma vara, y no por simetría: al quitarle el
    // atajo al artista, el destacado cayó **en el álbum** y siguió enseñando
    // algo oscuro. Un disco que se llama como su única canción no puede ganarle
    // a media docena de artistas que grabaron una canción con ese nombre.
    if let Some(a) = albums.iter().find(|a| text::normalize(&a.title) == q) {
        let tituladas = tracks
            .iter()
            .filter(|t| text::normalize(&t.title) == q)
            .count();
        let suyas = tracks
            .iter()
            .filter(|t| t.album_id.as_ref() == Some(&a.id))
            .count();

        if tituladas < 2 || suyas >= tituladas {
            return Some(PrimeraCoincidencia::Album(a.clone()));
        }
    }

    if let Some(t) = homonima {
        return Some(PrimeraCoincidencia::Track(t.clone()));
    }

    // Nadie coincide del todo, pero la consulta puede nombrar **dos cosas**: un
    // artista y una canción. "casey edwards bury the light" no es el título de
    // nada, así que hasta aquí se caía a la primera fila del proveedor —una
    // remezcla— teniendo la respuesta en la lista.
    //
    // Se exige que aparezcan los dos. Solo con el título, cualquier versión
    // valdría; solo con el artista, cualquier canción suya.
    if let Some(t) = tracks.iter().find(|t| encaja_con_la_consulta(&q, t)) {
        return Some(PrimeraCoincidencia::Track(t.clone()));
    }

    tracks.first().cloned().map(PrimeraCoincidencia::Track)
}

/// `true` si la consulta nombra a la vez al artista y a la canción de esta fila.
///
/// Los artistas se separan porque `artist_display` viene unido —"Casey Edwards,
/// Victor Borba"— y quien busca casi nunca escribe la lista entera: nombra a uno.
fn encaja_con_la_consulta(consulta_norm: &str, fila: &TrackRow) -> bool {
    let titulo = text::normalize(&fila.title);
    if titulo.is_empty() || !consulta_norm.contains(&titulo) {
        return false;
    }
    fila.artist_display.split(',').any(|nombre| {
        let n = text::normalize(nombre);
        !n.is_empty() && consulta_norm.contains(&n)
    })
}

#[async_trait]
impl SearchService for SearchServiceImpl {
    async fn search(
        &self,
        query: &str,
        scope: SearchScope,
        page: &PageRequest,
    ) -> CoreResult<SearchResults> {
        let query_id = self.contador.fetch_add(1, Ordering::Relaxed) + 1;
        self.ultimo_id.store(query_id, Ordering::Relaxed);

        if text::normalize(query).is_empty() {
            return Ok(SearchResults {
                query_id,
                top: None,
                tracks: Vec::new(),
                albums: Vec::new(),
                artists: Vec::new(),
                playlists: Vec::new(),
                remote: RemoteResults::NotAttempted,
            });
        }

        // ── Primera mitad: local, siempre, y sin excepción ──────────────────
        let locales = self.search.search_tracks(query, page).await?.items;

        let (albums, artists, playlists) = match scope {
            SearchScope::Tracks => (Vec::new(), Vec::new(), Vec::new()),
            _ => (
                self.search.search_albums(query, 8).await?,
                self.search.search_artists(query, 8).await?,
                self.search.search_playlists(query, 5).await?,
            ),
        };

        // ── Segunda mitad: la respuesta del proveedor ───────────────────────
        //
        // Si ya llegó para esta misma consulta, manda su orden. Es la
        // repetición que hace el cliente al recibir `SearchRemoteReady`, y
        // respetarla aquí es lo que conserva el criterio del proveedor: lo
        // local viene reordenado por relevancia de texto, donde veinte
        // versiones del mismo título empatan.
        let (tracks, remote) = if let Some(remotas) = self.remota_guardada(query).await {
            (fundir(remotas, locales), RemoteResults::Ready)
        } else if self.conviene_preguntar().await {
            self.lanzar_remota(query.to_owned(), query_id);
            (locales, RemoteResults::Loading)
        } else {
            // Sin proveedor operativo. No es un error: lo que ya está en el
            // catálogo se busca igual y el cliente puede decirlo con calma.
            let estado = match self.provider.status().await {
                localify_core::events::ProviderStatus::NotConfigured => {
                    RemoteResults::Unavailable {
                        reason_key: "provider.not_configured".to_owned(),
                    }
                }
                localify_core::events::ProviderStatus::Unavailable { reason_key } => {
                    RemoteResults::Unavailable { reason_key }
                }
                localify_core::events::ProviderStatus::Ready => RemoteResults::NotAttempted,
            };
            (locales, estado)
        };

        // La primera coincidencia se decide sobre las canciones sueltas, antes
        // de agrupar: lo que se destaca es una grabación concreta, no un grupo.
        let top = primera_coincidencia(query, &tracks, &albums, &artists);
        let tracks = agrupar(query, tracks);

        Ok(SearchResults {
            query_id,
            top,
            tracks,
            albums,
            artists,
            playlists,
            remote,
        })
    }

    async fn suggest(&self, prefix: &str, limit: u8) -> CoreResult<Vec<String>> {
        if text::normalize(prefix).is_empty() {
            return Ok(Vec::new());
        }

        // Las sugerencias son siempre locales: salen mientras se teclea y una
        // petición de red por pulsación sería insostenible.
        let filas = self
            .search
            .search_tracks(prefix, &PageRequest::new(0, u32::from(limit)))
            .await?;

        let mut vistos = std::collections::HashSet::new();
        Ok(filas
            .items
            .into_iter()
            .map(|f| f.title)
            .filter(|t| vistos.insert(text::normalize(t)))
            .collect())
    }
}

impl SearchServiceImpl {
    /// Identificador de la última consulta emitida.
    #[must_use]
    pub fn ultimo_query_id(&self) -> u64 {
        self.contador.load(Ordering::Relaxed)
    }

    /// Repositorio de pistas, para composiciones futuras.
    #[must_use]
    pub fn repositorio_pistas(&self) -> &Arc<dyn TrackRepository> {
        &self.tracks
    }
}

// El comportamiento completo (local primero, aviso remoto, persistencia) se
// cubre en `tests/busqueda.rs`, con repositorios reales sobre una base de datos
// temporal y un proveedor de respuestas preparadas. Aquí solo la fusión, que es
// pura y merece sus casos de borde a mano.
#[cfg(test)]
mod tests {
    use localify_core::domain::audio::DurationMs;
    use localify_core::domain::availability::Availability;

    use super::*;

    fn fila(id: &str, titulo: &str, artista: &str, segundos: u32) -> TrackRow {
        TrackRow {
            id: TrackId::from_trusted(id.to_owned()),
            title: titulo.to_owned(),
            artist_display: artista.to_owned(),
            artist_id: None,
            album_id: None,
            album_title: None,
            duration: DurationMs::from_secs(segundos),
            availability: Availability::Absent,
            is_favorite: false,
            explicit: false,
            popularity: None,
            added_at: None,
        }
    }

    /// Igual, pero con popularidad conocida. La mayoría de los tests no la
    /// necesitan y arrastrarla en todas las llamadas solo añadiría ruido.
    fn fila_pop(
        id: &str,
        titulo: &str,
        artista: &str,
        segundos: u32,
        popularidad: Option<u8>,
    ) -> TrackRow {
        TrackRow {
            popularity: popularidad,
            ..fila(id, titulo, artista, segundos)
        }
    }

    fn titulos(filas: &[TrackRow]) -> Vec<&str> {
        filas.iter().map(|f| f.title.as_str()).collect()
    }

    #[test]
    fn manda_el_orden_del_proveedor() {
        // Lo local viene ordenado por relevancia de texto, donde veinte
        // versiones del mismo titulo empatan. El proveedor si sabe cual es la
        // que la gente busca, asi que va primero.
        let remotas = vec![fila("r1", "Despacito", "Luis Fonsi", 229)];
        let locales = vec![fila("l1", "Despacito", "Karaoke Band", 231)];

        let unidas = fundir(remotas, locales);
        assert_eq!(titulos(&unidas), ["Despacito", "Despacito"]);
        assert_eq!(unidas[0].artist_display, "Luis Fonsi");
    }

    #[test]
    fn lo_local_que_no_trae_el_proveedor_se_conserva() {
        let remotas = vec![fila("r1", "Una", "A", 100)];
        let locales = vec![fila("l1", "Otra", "B", 200)];

        assert_eq!(titulos(&fundir(remotas, locales)), ["Una", "Otra"]);
    }

    #[test]
    fn la_misma_pista_en_las_dos_listas_sale_una_vez() {
        // Es el caso habitual: buscar persiste sus resultados, asi que la
        // consulta local encuentra justo lo que el proveedor dio la vez
        // anterior. Sin esto cada cancion salia dos veces.
        let remotas = vec![fila("mismo", "Cancion", "Artista", 180)];
        let locales = vec![fila("mismo", "Cancion", "Artista", 180)];

        assert_eq!(fundir(remotas, locales).len(), 1);
    }

    #[test]
    fn dos_subidas_identicas_de_la_misma_cancion_se_colapsan() {
        // En YouTube la misma cancion esta subida varias veces y el proveedor
        // devuelve varias. Son filas identicas en pantalla y elegir entre ellas
        // no es una decision que el usuario pueda tomar.
        let remotas = vec![
            fila("v1", "Bloody Power Fame", "Tre Watson", 240),
            fila("v2", "Bloody Power Fame", "Tre Watson", 240),
        ];

        assert_eq!(fundir(remotas, Vec::new()).len(), 1);
    }

    #[test]
    fn una_version_distinta_no_se_confunde_con_un_duplicado() {
        // Un directo dura otra cosa, y un cover es de otro. Exigir las tres
        // coincidencias es lo que impide esconder musica de verdad.
        let remotas = vec![
            fila("v1", "Bloody Power Fame", "coldrain", 239),
            fila("v2", "Bloody Power Fame", "coldrain", 305),
            fila("v3", "Bloody Power Fame", "Senna Cover", 239),
        ];

        assert_eq!(fundir(remotas, Vec::new()).len(), 3);
    }

    fn album(titulo: &str) -> AlbumRow {
        AlbumRow {
            id: localify_core::domain::ids::AlbumId::from_trusted("MPREalgo".to_owned()),
            title: titulo.to_owned(),
            artist_display: "Artista".to_owned(),
            year: None,
            cover: None,
            track_count: 12,
            local_count: 0,
        }
    }

    fn artista(nombre: &str) -> ArtistRow {
        ArtistRow {
            id: localify_core::domain::ids::ArtistId::from_trusted("UCalgo".to_owned()),
            name: nombre.to_owned(),
            image_url: None,
            track_count: 20,
            local_track_count: 0,
        }
    }

    #[test]
    fn quien_escribe_el_nombre_de_un_grupo_quiere_el_grupo() {
        // Y no una cancion que se llame igual: el artista es lo que engloba a
        // todo lo demas, y la lista de canciones ya esta justo debajo.
        let top = primera_coincidencia(
            "radiohead",
            &[fila("t", "Radiohead", "Otro", 200)],
            &[album("Radiohead")],
            &[artista("Radiohead")],
        );
        assert!(matches!(top, Some(PrimeraCoincidencia::Artist(_))));
    }

    #[test]
    fn un_grupo_homonimo_de_una_cancion_famosa_no_roba_el_destacado() {
        // El caso real: existe una banda llamada exactamente "Bury the Light", y
        // se destacaba a ella mientras la canción que el usuario buscaba —con
        // ocho versiones justo debajo— se quedaba sin destacar.
        let top = primera_coincidencia(
            "bury the light",
            &[
                fila("a", "Bury the Light", "Casey Edwards", 582),
                fila("b", "Bury the Light", "Krater", 226),
                fila("c", "Bury the Light", "FamilyJules", 583),
                fila("d", "Solitude", "Bury the Light", 200),
            ],
            &[],
            &[artista("Bury the Light")],
        );
        assert!(
            matches!(top, Some(PrimeraCoincidencia::Track(_))),
            "tres artistas distintos con ese título pesan más que el grupo: {top:?}"
        );
    }

    #[test]
    fn un_disco_homonimo_de_una_cancion_famosa_tampoco_roba_el_destacado() {
        // Al arreglar lo del grupo, el destacado cayó en un álbum llamado igual
        // —el de uno de los que hizo una versión— y siguió sin enseñar la
        // canción. La regla tiene que valer para las dos puertas, no para una.
        let disco = album("Bury the Light");
        let top = primera_coincidencia(
            "bury the light",
            &[
                fila("a", "Bury the Light", "Casey Edwards", 582),
                fila("b", "Bury the Light", "Krater", 226),
                fila("c", "Bury the Light", "FamilyJules", 583),
            ],
            &[disco],
            &[],
        );
        assert!(
            matches!(top, Some(PrimeraCoincidencia::Track(_))),
            "{top:?}"
        );
    }

    #[test]
    fn nombrar_al_artista_y_la_cancion_destaca_esa_grabacion() {
        // "casey edwards bury the light" no es el título de nada, así que se
        // caía a la primera fila del proveedor —una remezcla— con la respuesta
        // dos filas más abajo.
        let top = primera_coincidencia(
            "casey edwards bury the light",
            &[
                fila(
                    "a",
                    "Bury The Light (Power Glove Remix)",
                    "Power Glove",
                    465,
                ),
                fila("b", "Bury the Light", "Krater", 226),
                fila("c", "Bury the Light", "Casey Edwards, Victor Borba", 582),
            ],
            &[],
            &[],
        );
        assert!(
            matches!(top, Some(PrimeraCoincidencia::Track(ref t)) if t.id.as_str() == "c"),
            "{top:?}"
        );
    }

    #[test]
    fn entre_homonimas_gana_la_mas_escuchada() {
        // El caso "judas": la de Lady Gaga tiene mil cien millones de
        // reproducciones y llega **detrás** de otras dos que se llaman igual.
        // Sin popularidad, se destacaba la primera que cayera.
        let top = primera_coincidencia(
            "judas",
            &[
                fila_pop("a", "Judas", "Los Suaves", 396, Some(52)),
                fila_pop("b", "Judas", "Zen P", 287, Some(53)),
                fila_pop("c", "Judas", "Lady Gaga", 250, Some(90)),
            ],
            &[],
            &[],
        );
        assert!(
            matches!(top, Some(PrimeraCoincidencia::Track(ref t)) if t.id.as_str() == "c"),
            "{top:?}"
        );
    }

    #[test]
    fn sin_popularidad_manda_el_orden_del_proveedor() {
        // MusicBrainz no mide popularidad. Su silencio no puede leerse como
        // "impopular" ni deshacer el orden que trae el catálogo.
        let top = primera_coincidencia(
            "bury the light",
            &[
                fila_pop("a", "Bury the Light", "Stereotide", 207, None),
                fila_pop("b", "Bury the Light", "Krater", 226, None),
            ],
            &[],
            &[],
        );
        assert!(
            matches!(top, Some(PrimeraCoincidencia::Track(ref t)) if t.id.as_str() == "a"),
            "el empate lo gana la primera del proveedor: {top:?}"
        );
    }

    #[test]
    fn con_el_titulo_solo_no_basta_para_elegir_version() {
        // Sin el artista en la consulta, "bury the light" no puede decidir cuál
        // de seis grabaciones homónimas es la buena, y elegir una por su orden
        // de llegada sería fingir un criterio. Se queda la primera del
        // proveedor, que al menos es una respuesta explicable.
        let top = primera_coincidencia(
            "bury the light",
            &[
                fila(
                    "a",
                    "Bury The Light (Power Glove Remix)",
                    "Power Glove",
                    465,
                ),
                fila("b", "Bury the Light", "Casey Edwards, Victor Borba", 582),
            ],
            &[],
            &[],
        );
        assert!(
            matches!(top, Some(PrimeraCoincidencia::Track(ref t)) if t.id.as_str() == "b"),
            "la homónima exacta gana a la remezcla: {top:?}"
        );
    }

    #[test]
    fn un_grupo_con_mas_canciones_que_homonimas_sigue_ganando() {
        // El lado contrario, que es el que no se puede romper: "queen" tiene
        // alguna canción titulada así y muchísimas del grupo.
        let top = primera_coincidencia(
            "queen",
            &[
                fila("a", "Queen", "Otro", 200),
                fila("b", "Queen", "Otro más", 200),
                fila("c", "Bohemian Rhapsody", "Queen", 355),
                fila("d", "Under Pressure", "Queen, David Bowie", 248),
                fila("e", "Radio Ga Ga", "Queen", 348),
            ],
            &[],
            &[artista("Queen")],
        );
        assert!(
            matches!(top, Some(PrimeraCoincidencia::Artist(_))),
            "{top:?}"
        );
    }

    #[test]
    fn el_nombre_de_un_disco_saca_el_disco() {
        let top = primera_coincidencia(
            "ok computer",
            &[fila("t", "OK Computer", "Radiohead", 200)],
            &[album("OK Computer")],
            &[artista("Radiohead")],
        );
        assert!(matches!(top, Some(PrimeraCoincidencia::Album(_))));
    }

    #[test]
    fn una_coincidencia_parcial_no_destaca_un_artista() {
        // Escribiendo "amor" el usuario todavia esta a medias, y sacarle enorme
        // un grupo llamado "Amor Eterno" es adivinar. Se cae a la primera
        // cancion, que es la que el proveedor considero mas probable.
        let top = primera_coincidencia(
            "amor",
            &[fila("t", "Amor de mis amores", "Alguien", 200)],
            &[],
            &[artista("Amor Eterno")],
        );
        assert!(
            matches!(top, Some(PrimeraCoincidencia::Track(ref t)) if t.id.as_str() == "t"),
            "deberia caer a la primera cancion, no a {top:?}"
        );
    }

    #[test]
    fn la_comparacion_ignora_mayusculas_y_tildes() {
        let top = primera_coincidencia("bjork", &[], &[], &[artista("Björk")]);
        assert!(matches!(top, Some(PrimeraCoincidencia::Artist(_))));
    }

    #[test]
    fn sin_nada_que_destacar_no_se_inventa() {
        assert!(primera_coincidencia("lo que sea", &[], &[], &[]).is_none());
    }

    #[test]
    fn las_versiones_de_una_cancion_caben_en_una_fila() {
        // El caso real: buscar "faint" devolvia diez filas que dicen lo mismo.
        let filas = vec![
            fila("v1", "Faint", "Linkin Park", 162),
            fila("v2", "Faint (Live)", "Linkin Park", 200),
            fila("v3", "Faint (Instrumental)", "Linkin Park", 162),
            fila("v4", "Faint (Meteora|20 Demo)", "Linkin Park", 170),
            fila("v5", "Faint (Live in Hamburg, 2011)", "Linkin Park", 210),
        ];

        let grupos = agrupar("faint", filas);
        assert_eq!(grupos.len(), 1, "una cancion, una fila");
        assert_eq!(grupos[0].principal.title, "Faint");
        assert_eq!(grupos[0].versiones.len(), 4, "las demas siguen ahi");
    }

    #[test]
    fn dos_canciones_distintas_con_el_mismo_nombre_no_se_juntan() {
        // "Hurt" de Nine Inch Nails y la de Johnny Cash son canciones
        // distintas. Agrupar solo por titulo haria desaparecer una de las dos,
        // que es exactamente lo que agrupar venia a evitar.
        let filas = vec![
            fila("a", "Hurt", "Nine Inch Nails", 373),
            fila("b", "Hurt", "Johnny Cash", 218),
        ];

        assert_eq!(agrupar("hurt", filas).len(), 2);
    }

    #[test]
    fn quien_pide_un_directo_no_lo_encuentra_escondido() {
        // Si la consulta ya nombra la variante, agruparla bajo la de estudio
        // seria esconder justo lo que se pidio.
        let filas = vec![
            fila("v1", "Faint", "Linkin Park", 162),
            fila("v2", "Faint (Live)", "Linkin Park", 200),
        ];

        let grupos = agrupar("faint live", filas);
        assert_eq!(grupos.len(), 2, "el directo no puede quedar escondido");
    }

    #[test]
    fn la_original_encabeza_el_grupo_aunque_llegue_despues() {
        // El proveedor no siempre pone la de estudio primera.
        let filas = vec![
            fila("v1", "Numb (Live)", "Linkin Park", 200),
            fila("v2", "Numb", "Linkin Park", 185),
        ];

        let grupos = agrupar("numb", filas);
        assert_eq!(grupos[0].principal.title, "Numb");
    }

    #[test]
    fn el_titulo_se_compara_normalizado() {
        // "DESPACITO " y "despacito" son la misma cancion escrita de dos
        // maneras: el proveedor no garantiza mayusculas ni espacios.
        let remotas = vec![fila("v1", "DESPACITO ", "Luis Fonsi", 229)];
        let locales = vec![fila("v2", "despacito", "luis fonsi", 229)];

        assert_eq!(fundir(remotas, locales).len(), 1);
    }
}

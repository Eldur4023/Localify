//! La cola de reproducción.
//!
//! ## El modelo de dos colas
//!
//! Es lo que hace que la reproducción se sienta como Spotify y no como una
//! lista de ficheros:
//!
//! - La **cola de usuario** (`add_next` / `add_last`) tiene prioridad absoluta,
//!   se consume al reproducirse y **sobrevive a un cambio de contexto**. Poner
//!   tres canciones a continuación y luego abrir otro álbum no las borra.
//! - La **cola de contexto** se deriva del álbum, playlist o búsqueda que
//!   originó la reproducción, y sí se regenera al cambiar de contexto.
//!
//! ## El aleatorio no es `rand()` en cada avance
//!
//! Se baraja **una vez**, con una semilla que se persiste, y se recorre la
//! permutación. Las tres consecuencias son las que el usuario espera:
//! "anterior" funciona, desactivar el aleatorio recupera el orden original
//! manteniendo la canción actual, y la permutación sobrevive a un reinicio.
//!
//! Sortear en cada avance daría un "anterior" imposible de implementar y
//! repetiría canciones antes de haber sonado todas.
//!
//! ## Por qué un `Mutex` y no un hilo de actor
//!
//! ADR-008 pide actores para el estado mutable con invariantes temporales. La
//! cola cumple el espíritu de esa decisión sin necesitar un hilo: todas sus
//! operaciones son inmediatas —mover elementos de un `VecDeque`— y **ninguna
//! llama a otro servicio mientras sostiene el estado**.
//!
//! Esa última condición es la que hace segura la simplificación, y no se
//! confía a la disciplina de quien edite el fichero: el estado va en un
//! [`std::sync::Mutex`], cuyo guardia no es `Send`. Sostenerlo a través de un
//! `await` hace que el `Future` deje de ser `Send` y **el crate no compila**.
//! La regla está en el sistema de tipos, no en un comentario.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;
use localify_core::domain::ids::{QueueEntryId, TrackId};
use localify_core::domain::queue::{
    AdvanceReason, PlaybackContext, QueueEntry, QueueSnapshot, RepeatMode, permutacion_estable,
};
use localify_core::error::CoreResult;
use localify_core::events::{DomainEvent, EventPublisher};
use localify_core::ports::database::TrackRepository;
use localify_core::ports::services::QueueService;

/// Cuántas pistas del contexto se envían en un `snapshot`.
///
/// Una biblioteca de 50 000 pistas no cabe en un evento IPC, y el panel de cola
/// no muestra más de un puñado. Se manda una ventana.
const VENTANA_CONTEXTO: usize = 50;

/// Entrada de la cola de usuario: la pista más su identificador efímero.
///
/// El identificador es propio de la entrada y no de la pista: la misma canción
/// puede estar dos veces en la cola, y quitar una no debe quitar la otra.
#[derive(Debug, Clone)]
struct Anotada {
    entry_id: QueueEntryId,
    track_id: TrackId,
}

/// La cola sin resolver contra la base de datos.
///
/// Se extrae con el estado bloqueado y se resuelve después, ya sin el lock: es
/// lo que permite consultar las filas sin frenar a nadie.
#[derive(Debug)]
struct Esqueleto {
    revision: u64,
    actual: Option<TrackId>,
    usuario: Vec<Anotada>,
    siguientes: Vec<TrackId>,
    contexto: Option<PlaybackContext>,
}

/// El estado que la cola posee.
#[derive(Debug)]
struct Estado {
    contexto: Option<PlaybackContext>,
    /// Pistas del contexto, en su orden natural.
    contexto_pistas: Vec<TrackId>,
    /// Permutación vigente cuando el aleatorio está activo.
    ///
    /// Guardar la permutación y no la lista ya barajada es lo que permite
    /// volver al orden original sin haber perdido nada.
    permutacion: Vec<usize>,
    semilla: u64,
    aleatorio: bool,
    repeticion: RepeatMode,
    /// Posición dentro del recorrido (permutado o no).
    indice: Option<usize>,
    cola_usuario: VecDeque<Anotada>,
    /// Pista sonando, venga de donde venga.
    actual: Option<TrackId>,
    /// Historial para "anterior" cuando la actual salió de la cola de usuario:
    /// esa entrada ya se consumió y no se puede volver a ella por el índice.
    historial: Vec<TrackId>,
    revision: u64,
}

impl Estado {
    const fn nuevo() -> Self {
        Self {
            contexto: None,
            contexto_pistas: Vec::new(),
            permutacion: Vec::new(),
            semilla: 0,
            aleatorio: false,
            repeticion: RepeatMode::Off,
            indice: None,
            cola_usuario: VecDeque::new(),
            actual: None,
            historial: Vec::new(),
            revision: 0,
        }
    }

    /// Pista en la posición `i` del recorrido.
    fn en(&self, i: usize) -> Option<TrackId> {
        let real = if self.aleatorio {
            self.permutacion.get(i).copied()?
        } else {
            i
        };
        self.contexto_pistas.get(real).cloned()
    }

    /// Posición del recorrido que ocupa `track`.
    fn posicion_de(&self, track: &TrackId) -> Option<usize> {
        let real = self.contexto_pistas.iter().position(|t| t == track)?;
        if self.aleatorio {
            self.permutacion.iter().position(|p| *p == real)
        } else {
            Some(real)
        }
    }

    /// Rebaraja conservando la pista actual en su sitio.
    ///
    /// Sin esto, activar el aleatorio saltaría de canción, que es justo lo que
    /// nadie espera al pulsar ese botón.
    ///
    /// ## Se rota, no se intercambia
    ///
    /// La actual va al principio **rotando** la permutación, no cambiándola de
    /// sitio con la que hubiera allí. Para el usuario es lo mismo —su canción
    /// sigue sonando y el recorrido cubre la lista entera antes de repetir
    /// ninguna—, pero para el estado es una diferencia grande.
    ///
    /// Con el intercambio, la permutación dejaba de poder reconstruirse: se
    /// persiste la semilla, y el intercambio dependía además de qué sonaba
    /// cuando se activó el aleatorio, que no se guarda en ninguna parte. Al
    /// reabrir salía una permutación distinta en dos posiciones, y "siguiente"
    /// llevaba a veces a otra canción. Es justo lo que persistir la semilla
    /// venía a evitar, y fallaba una de cada seis veces —solo si el recorrido
    /// pasaba por una de esas dos posiciones—, así que parecía ruido del test.
    ///
    /// Una rotación conserva el orden cíclico. Anclarla en la canción actual da
    /// **la misma sucesión** se haga desde donde se haga, así que al restaurar
    /// basta con volver a rotar sobre la canción que estuviera sonando: no hace
    /// falta guardar nada más.
    fn rebarajar(&mut self) {
        self.permutacion = permutacion_estable(self.contexto_pistas.len(), self.semilla);

        if let Some(actual) = self.actual.clone()
            && let Some(real) = self.contexto_pistas.iter().position(|t| *t == actual)
            && let Some(donde) = self.permutacion.iter().position(|p| *p == real)
        {
            self.permutacion.rotate_left(donde);
            self.indice = Some(0);
        }
    }

    fn tocar(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }

    fn esqueleto(&self) -> Esqueleto {
        let desde = self.indice.map_or(0, |i| i + 1);
        Esqueleto {
            revision: self.revision,
            actual: self.actual.clone(),
            usuario: self.cola_usuario.iter().cloned().collect(),
            siguientes: (desde..desde.saturating_add(VENTANA_CONTEXTO))
                .filter_map(|i| self.en(i))
                .collect(),
            contexto: self.contexto.clone(),
        }
    }
}

/// Dependencias del servicio.
pub struct Dependencias {
    pub tracks: Arc<dyn TrackRepository>,
    pub bus: Arc<dyn EventPublisher>,
}

impl std::fmt::Debug for Dependencias {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Dependencias").finish_non_exhaustive()
    }
}

/// La cola de reproducción. Barata de clonar.
#[derive(Clone)]
pub struct QueueActor {
    estado: Arc<Mutex<Estado>>,
    deps: Arc<Dependencias>,
}

impl std::fmt::Debug for QueueActor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QueueActor").finish_non_exhaustive()
    }
}

impl QueueActor {
    #[must_use]
    pub fn nuevo(deps: Dependencias) -> Self {
        Self {
            estado: Arc::new(Mutex::new(Estado::nuevo())),
            deps: Arc::new(deps),
        }
    }

    /// Acceso al estado.
    ///
    /// Un lock envenenado no debe callar la música: el estado es un puñado de
    /// vectores que no pueden quedar a medias, así que se recupera y se sigue.
    fn bloquear(&self) -> std::sync::MutexGuard<'_, Estado> {
        self.estado.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Avisa de que la cola cambió.
    ///
    /// Va solo la revisión, no la cola entera. Una cola con cien canciones
    /// añadidas a mano ocuparía decenas de kilobytes en cada evento; el
    /// frontend compara la revisión y pide el contenido cuando lo necesita.
    fn anunciar(&self, revision: u64) {
        self.deps
            .bus
            .publish(DomainEvent::QueueChanged { revision });
    }

    async fn resolver(&self, esq: Esqueleto) -> QueueSnapshot {
        let mut ids: Vec<TrackId> = Vec::new();
        if let Some(a) = &esq.actual {
            ids.push(a.clone());
        }
        ids.extend(esq.usuario.iter().map(|a| a.track_id.clone()));
        ids.extend(esq.siguientes.iter().cloned());

        let filas = self.deps.tracks.rows_by_ids(&ids).await.unwrap_or_default();
        let buscar = |id: &TrackId| filas.iter().find(|f| &f.id == id).cloned();

        QueueSnapshot {
            revision: esq.revision,
            current: esq
                .actual
                .as_ref()
                .and_then(buscar)
                .map(|track| QueueEntry {
                    entry_id: QueueEntryId::nuevo(),
                    track,
                }),
            user_queue: esq
                .usuario
                .iter()
                .filter_map(|a| {
                    buscar(&a.track_id).map(|track| QueueEntry {
                        entry_id: a.entry_id,
                        track,
                    })
                })
                .collect(),
            context_queue: esq
                .siguientes
                .iter()
                .filter_map(|id| {
                    buscar(id).map(|track| QueueEntry {
                        entry_id: QueueEntryId::nuevo(),
                        track,
                    })
                })
                .collect(),
            context: esq.contexto,
        }
    }

    // ── API que necesita `PlaybackService` ──────────────────────────────────

    /// Instala las pistas de un contexto que `PlaybackService` ya resolvió.
    ///
    /// Existe porque resolver un álbum o una playlist necesita repositorios que
    /// la cola no tiene ni debe tener.
    pub fn poner_pistas(&self, pistas: Vec<TrackId>, empezar_en: usize) {
        let revision = {
            let mut e = self.bloquear();
            e.contexto_pistas = pistas;
            if e.aleatorio {
                e.permutacion = permutacion_estable(e.contexto_pistas.len(), e.semilla);
            }
            e.indice = (empezar_en < e.contexto_pistas.len()).then_some(empezar_en);
            e.actual = e.indice.and_then(|i| e.en(i));
            e.historial.clear();
            e.tocar();
            e.revision
        };
        self.anunciar(revision);
    }

    /// Coloca la reproducción en una pista concreta del contexto.
    pub fn ir_a(&self, track: &TrackId) -> bool {
        let revision = {
            let mut e = self.bloquear();
            let Some(i) = e.posicion_de(track) else {
                return false;
            };
            // Ir a donde ya se está no es un movimiento. Apuntarlo en el
            // historial haría que "anterior" volviese a la misma canción, que
            // es exactamente lo que ese botón no debe hacer.
            if e.actual.as_ref() == Some(track) {
                return true;
            }
            if let Some(anterior) = e.actual.take() {
                e.historial.push(anterior);
            }
            e.indice = Some(i);
            e.actual = Some(track.clone());
            e.tocar();
            e.revision
        };
        self.anunciar(revision);
        true
    }

    /// Pista sonando ahora mismo.
    #[must_use]
    pub fn actual(&self) -> Option<TrackId> {
        self.bloquear().actual.clone()
    }

    /// Las `cuantas` siguientes, sin consumir nada.
    ///
    /// `peek_next` solo mira una. La precarga necesita ver más allá: la
    /// siguiente para el fundido y la de después para que un salto rápido
    /// tampoco tenga que esperar a una descarga.
    #[must_use]
    pub fn proximas(&self, cuantas: usize) -> Vec<TrackId> {
        let e = self.bloquear();

        // Con repetición de pista, la única "próxima" es ella misma, y
        // precargarla no aporta nada: ya está sonando.
        if e.repeticion == RepeatMode::Track {
            return Vec::new();
        }

        let mut salida: Vec<TrackId> = e
            .cola_usuario
            .iter()
            .take(cuantas)
            .map(|a| a.track_id.clone())
            .collect();

        let desde = e.indice.map_or(0, |i| i + 1);
        let mut i = desde;
        while salida.len() < cuantas {
            match e.en(i) {
                Some(t) => {
                    salida.push(t);
                    i += 1;
                }
                None if e.repeticion == RepeatMode::Queue && !e.contexto_pistas.is_empty() => {
                    // Al final de la cola con repetición, lo siguiente es el
                    // principio: precargarlo evita un hueco justo ahí.
                    if let Some(t) = e.en(0) {
                        salida.push(t);
                    }
                    break;
                }
                None => break,
            }
        }
        salida
    }

    /// Aleatorio y repetición, para componer el `PlayerState`.
    #[must_use]
    pub fn modos(&self) -> (bool, RepeatMode) {
        let e = self.bloquear();
        (e.aleatorio, e.repeticion)
    }

    /// Contexto vigente.
    #[must_use]
    pub fn contexto(&self) -> Option<PlaybackContext> {
        self.bloquear().contexto.clone()
    }

    /// Pistas del contexto y semilla del aleatorio, para persistir.
    #[must_use]
    pub fn para_persistir(&self) -> (Vec<TrackId>, u64) {
        let e = self.bloquear();
        (e.contexto_pistas.clone(), e.semilla)
    }

    /// Lo que queda por sonar de la cola de usuario.
    ///
    /// Se persiste aparte del contexto porque es lo que el usuario puso a mano:
    /// perderlo al cerrar sería, para él, perder trabajo.
    #[must_use]
    pub fn pendientes_de_usuario(&self) -> Vec<TrackId> {
        self.bloquear()
            .cola_usuario
            .iter()
            .map(|a| a.track_id.clone())
            .collect()
    }

    /// Restaura una sesión anterior tal cual estaba.
    ///
    /// Incluye la semilla: sin ella, reabrir con el aleatorio activo daría otro
    /// orden y "anterior" llevaría a una canción distinta de la que sonó.
    pub fn restaurar(
        &self,
        contexto: Option<PlaybackContext>,
        pistas: Vec<TrackId>,
        actual: Option<TrackId>,
        aleatorio: bool,
        semilla: u64,
        repeticion: RepeatMode,
    ) {
        let revision = {
            let mut e = self.bloquear();
            e.contexto = contexto;
            e.contexto_pistas = pistas;
            e.semilla = semilla;
            e.aleatorio = aleatorio;
            e.repeticion = repeticion;
            e.actual = actual;

            // Con el aleatorio puesto se rebaraja igual que en una sesión viva:
            // es lo que reconstruye la misma sucesión. Derivar la permutación a
            // secas de la semilla se saltaba la rotación y devolvía otro orden.
            if aleatorio {
                e.rebarajar();
            } else {
                e.permutacion = permutacion_estable(e.contexto_pistas.len(), semilla);
                e.indice = e.actual.clone().and_then(|t| e.posicion_de(&t));
            }
            e.tocar();
            e.revision
        };
        self.anunciar(revision);
    }
}

#[async_trait]
impl QueueService for QueueActor {
    async fn snapshot(&self) -> QueueSnapshot {
        let esq = self.bloquear().esqueleto();
        self.resolver(esq).await
    }

    async fn set_context(&self, ctx: PlaybackContext, start_index: usize) -> CoreResult<()> {
        let revision = {
            let mut e = self.bloquear();
            e.contexto_pistas = pistas_de(&ctx);
            e.contexto = Some(ctx);
            // La cola de usuario NO se toca: es lo que la distingue de la de
            // contexto, y vaciarla al abrir otro álbum sería, para el usuario,
            // perder algo que había puesto a mano.
            if e.aleatorio {
                e.semilla = nueva_semilla();
                e.permutacion = permutacion_estable(e.contexto_pistas.len(), e.semilla);
            }
            e.indice = (start_index < e.contexto_pistas.len()).then_some(start_index);
            e.actual = e.indice.and_then(|i| e.en(i));
            e.historial.clear();
            e.tocar();
            e.revision
        };
        self.anunciar(revision);
        Ok(())
    }

    async fn add_next(&self, tracks: &[TrackId]) -> CoreResult<()> {
        let revision = {
            let mut e = self.bloquear();
            // Al frente y en orden inverso: añadir [A, B] debe reproducir A y
            // luego B, no al revés.
            for t in tracks.iter().rev() {
                e.cola_usuario.push_front(Anotada {
                    entry_id: QueueEntryId::nuevo(),
                    track_id: t.clone(),
                });
            }
            e.tocar();
            e.revision
        };
        self.anunciar(revision);
        Ok(())
    }

    async fn add_last(&self, tracks: &[TrackId]) -> CoreResult<()> {
        let revision = {
            let mut e = self.bloquear();
            for t in tracks {
                e.cola_usuario.push_back(Anotada {
                    entry_id: QueueEntryId::nuevo(),
                    track_id: t.clone(),
                });
            }
            e.tocar();
            e.revision
        };
        self.anunciar(revision);
        Ok(())
    }

    async fn remove(&self, entry: QueueEntryId) -> CoreResult<()> {
        let revision = {
            let mut e = self.bloquear();
            e.cola_usuario.retain(|a| a.entry_id != entry);
            e.tocar();
            e.revision
        };
        self.anunciar(revision);
        Ok(())
    }

    async fn move_entry(&self, entry: QueueEntryId, to_index: usize) -> CoreResult<()> {
        let revision = {
            let mut e = self.bloquear();
            let Some(desde) = e.cola_usuario.iter().position(|a| a.entry_id == entry) else {
                return Ok(());
            };
            let Some(movida) = e.cola_usuario.remove(desde) else {
                return Ok(());
            };
            let hasta = to_index.min(e.cola_usuario.len());
            e.cola_usuario.insert(hasta, movida);
            e.tocar();
            e.revision
        };
        self.anunciar(revision);
        Ok(())
    }

    async fn clear_user_queue(&self) -> CoreResult<()> {
        let revision = {
            let mut e = self.bloquear();
            e.cola_usuario.clear();
            e.tocar();
            e.revision
        };
        self.anunciar(revision);
        Ok(())
    }

    async fn advance(&self, reason: AdvanceReason) -> CoreResult<Option<TrackId>> {
        let (revision, siguiente) = {
            let mut e = self.bloquear();

            // Repetir pista solo se aplica al final natural. Si el usuario
            // pulsa "siguiente", quiere la siguiente: es lo que hace Spotify.
            if e.repeticion == RepeatMode::Track && reason == AdvanceReason::NaturalEnd {
                return Ok(e.actual.clone());
            }

            if let Some(anterior) = e.actual.take() {
                e.historial.push(anterior);
            }

            // La cola de usuario manda sobre el contexto.
            if let Some(a) = e.cola_usuario.pop_front() {
                e.actual = Some(a.track_id.clone());
                e.tocar();
                (e.revision, Some(a.track_id))
            } else {
                let destino = match e.indice {
                    Some(i) if e.en(i + 1).is_some() => Some(i + 1),
                    // Fin del contexto: con repetición de cola, al principio.
                    Some(_)
                        if e.repeticion == RepeatMode::Queue && !e.contexto_pistas.is_empty() =>
                    {
                        Some(0)
                    }
                    Some(_) => None,
                    None => e.en(0).map(|_| 0),
                };
                e.indice = destino;
                e.actual = destino.and_then(|i| e.en(i));
                e.tocar();
                let actual = e.actual.clone();
                (e.revision, actual)
            }
        };
        self.anunciar(revision);
        Ok(siguiente)
    }

    async fn go_back(&self) -> CoreResult<Option<TrackId>> {
        let (revision, previa) = {
            let mut e = self.bloquear();

            // Si lo último vino de la cola de usuario, el índice del contexto
            // no se movió: hay que tirar del historial.
            if let Some(previa) = e.historial.pop() {
                if let Some(i) = e.posicion_de(&previa) {
                    e.indice = Some(i);
                }
                e.actual = Some(previa.clone());
                e.tocar();
                (e.revision, Some(previa))
            } else {
                // `None` significa "no se pudo retroceder", no "sigo aquí".
                // Devolver la actual haría indistinguibles los dos casos, y
                // quien llama necesita saberlo: sin pista anterior, "anterior"
                // reinicia la que suena en vez de recargarla entera.
                let Some(i) = e.indice.filter(|i| *i > 0) else {
                    return Ok(None);
                };
                e.indice = Some(i - 1);
                e.actual = e.en(i - 1);
                e.tocar();
                let actual = e.actual.clone();
                (e.revision, actual)
            }
        };
        self.anunciar(revision);
        Ok(previa)
    }

    async fn peek_next(&self) -> CoreResult<Option<TrackId>> {
        let e = self.bloquear();

        if e.repeticion == RepeatMode::Track {
            return Ok(e.actual.clone());
        }
        if let Some(a) = e.cola_usuario.front() {
            return Ok(Some(a.track_id.clone()));
        }
        Ok(match e.indice {
            Some(i) => e.en(i + 1).or_else(|| {
                (e.repeticion == RepeatMode::Queue)
                    .then(|| e.en(0))
                    .flatten()
            }),
            None => e.en(0),
        })
    }

    async fn set_shuffle(&self, enabled: bool) -> CoreResult<()> {
        let revision = {
            let mut e = self.bloquear();
            if e.aleatorio == enabled {
                return Ok(());
            }
            e.aleatorio = enabled;

            if enabled {
                e.semilla = nueva_semilla();
                e.rebarajar();
            } else {
                // Al desactivarlo se vuelve al orden natural, pero **sin
                // cambiar de canción**: el índice se recoloca donde esté.
                e.indice = e.actual.clone().and_then(|t| e.posicion_de(&t));
            }
            e.tocar();
            e.revision
        };
        self.anunciar(revision);
        Ok(())
    }

    async fn set_repeat(&self, mode: RepeatMode) -> CoreResult<()> {
        let revision = {
            let mut e = self.bloquear();
            e.repeticion = mode;
            e.tocar();
            e.revision
        };
        self.anunciar(revision);
        Ok(())
    }
}

/// Pistas que un contexto lleva consigo.
///
/// Solo la búsqueda y las recomendaciones traen su lista. Los demás se
/// resuelven desde la base de datos, y eso lo hace `PlaybackService`, que es
/// quien tiene los repositorios de álbumes y playlists.
fn pistas_de(ctx: &PlaybackContext) -> Vec<TrackId> {
    match ctx {
        PlaybackContext::Search { track_ids, .. }
        | PlaybackContext::Recommendation { track_ids, .. } => track_ids.clone(),
        _ => Vec::new(),
    }
}

/// Semilla nueva para el aleatorio.
///
/// El reloj basta: no hace falta calidad criptográfica, solo que dos sesiones
/// distintas no barajen igual.
fn nueva_semilla() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0x9E37_79B9_7F4A_7C15, |d| {
            u64::try_from(d.as_nanos() & u128::from(u64::MAX)).unwrap_or(1)
        })
}

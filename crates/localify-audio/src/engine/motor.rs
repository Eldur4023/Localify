//! El motor completo: implementa [`AudioEngine`].
//!
//! ## Lo que este componente NO decide
//!
//! No sabe qué es una cola, ni qué suena después, ni qué significa "aleatorio".
//! Recibe órdenes concretas —carga esto, funde a aquello— y avisa de lo que
//! pasa. Toda la política vive en `PlaybackService` (ADR-015).
//!
//! Es lo que permite probar la semántica de reproducción sin tarjeta de sonido,
//! y cambiar el motor sin tocar el negocio.
//!
//! ## Cómo llegan los eventos
//!
//! El hilo de tiempo real no manda eventos: publicar en un canal puede asignar.
//! En su lugar deja atómicos, y un hilo vigilante los mira cada 50 ms y traduce
//! los cambios a [`EngineEvent`]. Cincuenta milisegundos son inapreciables para
//! decidir un crossfade y suficientes para no molestar a nadie.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use localify_core::domain::audio::{AudioDevice, DurationMs, EqProfile, Volume};
use localify_core::ports::audio_engine::{
    AudioEngine, AudioError, AudioEventSource, AudioSource, EngineEvent, VoiceId,
};
use tracing::{debug, warn};

use crate::dsp::{EqCompartido, marcos_de};
use crate::engine::mezclador::{Mezclador, VolumenCompartido};
use crate::engine::salida::{self, Salida};
use crate::engine::voz::{self, ManejadorVoz, OrigenAudio};
use crate::source::EstadoDescarga;

/// Cada cuánto el vigilante traduce atómicos a eventos.
const PERIODO_VIGILANTE: Duration = Duration::from_millis(50);

/// Con cuánta antelación se avisa de que una pista se acaba.
///
/// Doce segundos es el crossfade más largo que admite el ajuste, así que este
/// margen garantiza que `PlaybackService` siempre tenga tiempo de precargar la
/// siguiente, sea cual sea la configuración.
const AVISO_FINAL: DurationMs = DurationMs::new(15_000);

/// Estado compartido entre el motor, el vigilante y el mezclador.
struct Interior {
    mezclador: Mutex<Arc<Mutex<Mezclador>>>,
    /// Voces vivas, por identificador. Solo lo toca el hilo de control.
    voces: Mutex<HashMap<VoiceId, ManejadorVoz>>,
    /// Voces ya decodificándose pero todavía no instaladas en el mezclador.
    ///
    /// Es lo que hace posible el crossfade: la siguiente canción se prepara
    /// entera mientras la actual suena, y entra sin un solo hueco.
    pendientes: Mutex<HashMap<VoiceId, crate::engine::Voz>>,
    /// Origen de cada voz, para poder reabrirlo en un salto de posición.
    origenes: Mutex<HashMap<VoiceId, OrigenAudio>>,
    volumen: Arc<VolumenCompartido>,
    pausado: Arc<AtomicBool>,
    eq: Arc<EqCompartido>,
    perfil: Mutex<EqProfile>,
    siguiente_id: AtomicU32,
    eventos: mpsc::Sender<EngineEvent>,
    cerrando: Arc<AtomicBool>,
}

/// El motor de audio.
pub struct MotorAudio {
    interior: Arc<Interior>,
    salida: Salida,
    vigilante: Option<std::thread::JoinHandle<()>>,
}

impl std::fmt::Debug for MotorAudio {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MotorAudio")
            .field("sample_rate", &self.salida.sample_rate())
            .finish_non_exhaustive()
    }
}

impl MotorAudio {
    /// Arranca el motor: abre el dispositivo y lanza el vigilante.
    ///
    /// # Errors
    /// Si no hay ningún dispositivo de salida utilizable.
    pub fn arrancar() -> Result<(Self, ReceptorEventos), AudioError> {
        let volumen = Arc::new(VolumenCompartido::nuevo(1.0));
        let pausado = Arc::new(AtomicBool::new(true));
        let eq = Arc::new(EqCompartido::nuevo());
        let (tx, rx) = mpsc::channel();

        let mezclador_compartido: Arc<Mutex<Option<Arc<Mutex<Mezclador>>>>> =
            Arc::new(Mutex::new(None));

        let salida = Salida::arrancar({
            let volumen = Arc::clone(&volumen);
            let pausado = Arc::clone(&pausado);
            let visible = Arc::clone(&mezclador_compartido);
            move |sample_rate, marcos| {
                let m = Arc::new(Mutex::new(Mezclador::nuevo(
                    sample_rate,
                    marcos,
                    volumen,
                    pausado,
                )));
                if let Ok(mut g) = visible.lock() {
                    *g = Some(Arc::clone(&m));
                }
                m
            }
        })?;

        let mezclador = mezclador_compartido
            .lock()
            .ok()
            .and_then(|g| g.clone())
            .ok_or(AudioError::NoDevice)?;

        // El ecualizador se calcula con la frecuencia real del dispositivo, que
        // solo se conoce ahora: hacerlo antes obligaría a recalcularlo entero.
        eq.publicar(&EqProfile::plano(), salida.sample_rate());

        let cerrando = Arc::new(AtomicBool::new(false));
        let interior = Arc::new(Interior {
            mezclador: Mutex::new(mezclador),
            voces: Mutex::new(HashMap::new()),
            pendientes: Mutex::new(HashMap::new()),
            origenes: Mutex::new(HashMap::new()),
            volumen,
            pausado,
            eq,
            perfil: Mutex::new(EqProfile::plano()),
            siguiente_id: AtomicU32::new(1),
            eventos: tx,
            cerrando: Arc::clone(&cerrando),
        });

        let vigilante = std::thread::Builder::new()
            .name("localify-audio-watch".to_owned())
            .spawn({
                let interior = Arc::clone(&interior);
                let sample_rate = salida.sample_rate();
                move || vigilar(&interior, sample_rate)
            })
            .map_err(|_| AudioError::NoDevice)?;

        Ok((
            Self {
                interior,
                salida,
                vigilante: Some(vigilante),
            },
            ReceptorEventos(rx),
        ))
    }

    /// Frecuencia de muestreo negociada con el dispositivo.
    #[must_use]
    pub fn sample_rate(&self) -> u32 {
        self.salida.sample_rate()
    }

    /// Carga una pista y la deja lista para sonar, sin instalarla todavía.
    ///
    /// Separar cargar de reproducir es lo que hace posible el crossfade: la
    /// voz siguiente se decodifica mientras la actual suena.
    fn cargar_interno(
        &self,
        origen: OrigenAudio,
        desde: DurationMs,
    ) -> Result<VoiceId, AudioError> {
        let id = VoiceId(self.interior.siguiente_id.fetch_add(1, Ordering::Relaxed));
        let (manejador, voz) = voz::arrancar(id, &origen, desde, self.salida.sample_rate())?;

        // La voz se guarda aparte del mezclador hasta que alguien pida tocarla.
        self.interior
            .voces
            .lock()
            .map_err(|_| AudioError::ShuttingDown)?
            .insert(id, manejador);
        self.interior
            .origenes
            .lock()
            .map_err(|_| AudioError::ShuttingDown)?
            .insert(id, origen);
        self.interior.pendientes().insert(id, voz);

        Ok(id)
    }
}

impl Interior {
    /// Acceso a las voces preparadas.
    ///
    /// Un lock envenenado no debe silenciar la música: el `HashMap` no puede
    /// quedar en un estado incoherente, así que se recupera y se sigue.
    fn pendientes(&self) -> std::sync::MutexGuard<'_, HashMap<VoiceId, crate::engine::Voz>> {
        self.pendientes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// El extremo de lectura de los eventos del motor.
pub struct ReceptorEventos(mpsc::Receiver<EngineEvent>);

impl std::fmt::Debug for ReceptorEventos {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ReceptorEventos")
    }
}

impl AudioEventSource for ReceptorEventos {
    fn recv(&mut self) -> Option<EngineEvent> {
        self.0.recv().ok()
    }

    fn try_recv(&mut self) -> Option<EngineEvent> {
        self.0.try_recv().ok()
    }
}

/// El hilo vigilante: traduce atómicos a eventos.
fn vigilar(interior: &Arc<Interior>, sample_rate: u32) {
    let mut avisadas: std::collections::HashSet<VoiceId> = std::collections::HashSet::new();
    let mut terminadas: std::collections::HashSet<VoiceId> = std::collections::HashSet::new();

    while !interior.cerrando.load(Ordering::Acquire) {
        std::thread::sleep(PERIODO_VIGILANTE);

        let Ok(voces) = interior.voces.lock() else {
            break;
        };

        for (id, v) in voces.iter() {
            if v.estado.underrun.swap(false, Ordering::Relaxed) {
                let _ = interior.eventos.send(EngineEvent::Underrun { voice: *id });
            }

            if v.estado.agotada.load(Ordering::Acquire) {
                if terminadas.insert(*id) {
                    let _ = interior.eventos.send(EngineEvent::Ended { voice: *id });
                }
                continue;
            }

            if let Some(fallo) = v.fallo() {
                if terminadas.insert(*id) {
                    let _ = interior.eventos.send(EngineEvent::Failed {
                        voice: *id,
                        reason_key: fallo,
                    });
                }
                continue;
            }

            // Aviso de final próximo, una sola vez por voz.
            if let Some(total) = v.duracion {
                let pos = v.posicion(sample_rate);
                let restante = total.as_ms().saturating_sub(pos.as_ms());
                if restante <= AVISO_FINAL.as_ms() && avisadas.insert(*id) {
                    let _ = interior.eventos.send(EngineEvent::ApproachingEnd {
                        voice: *id,
                        remaining: DurationMs::new(restante),
                    });
                }
            }
        }

        drop(voces);
    }
    debug!("vigilante terminado");
}

impl AudioEngine for MotorAudio {
    fn load(&self, source: AudioSource, start_at: DurationMs) -> Result<VoiceId, AudioError> {
        let origen = match source {
            AudioSource::File(ruta) => OrigenAudio::completo(ruta)?,
            AudioSource::Growing {
                path,
                expected_bytes,
            } => {
                // Sin estado de descarga compartido, lo que se sabe del fichero
                // es su tamaño actual. Quien tenga el estado real debe usar
                // `cargar_en_descarga`.
                let estado = EstadoDescarga::nuevo();
                if let Some(n) = expected_bytes {
                    estado.avanzar(n);
                }
                OrigenAudio::en_descarga(path, estado)
            }
        };
        self.cargar_interno(origen, start_at)
    }

    fn play(&self, voice: VoiceId) {
        let voz = self.interior.pendientes().remove(&voice);
        if let Some(v) = voz
            && let Ok(g) = self.interior.mezclador.lock()
            && let Ok(mut m) = g.lock()
        {
            m.poner_actual(Some(v));
        }
        self.interior.pausado.store(false, Ordering::Release);
        let _ = self.interior.eventos.send(EngineEvent::Started { voice });
    }

    fn pause(&self) {
        self.interior.pausado.store(true, Ordering::Release);
    }

    fn stop(&self, voice: VoiceId) {
        self.interior.pendientes().remove(&voice);
        if let Ok(mut voces) = self.interior.voces.lock() {
            voces.remove(&voice);
        }
        if let Ok(mut origenes) = self.interior.origenes.lock() {
            origenes.remove(&voice);
        }
        if let Ok(g) = self.interior.mezclador.lock()
            && let Ok(mut m) = g.lock()
            && m.id_actual() == Some(voice)
        {
            m.poner_actual(None);
        }
    }

    fn seek(&self, voice: VoiceId, position: DurationMs) {
        // Saltar reconstruye la voz desde la posición pedida: es lo que permite
        // descartar los tres segundos ya decodificados sin inventar un
        // mecanismo para vaciar un anillo que otro hilo está leyendo.
        let origen = self
            .interior
            .origenes
            .lock()
            .ok()
            .and_then(|g| g.get(&voice).cloned());
        let Some(origen) = origen else {
            warn!(?voice, "salto sobre una voz desconocida");
            return;
        };

        let sonaba = self
            .interior
            .mezclador
            .lock()
            .ok()
            .and_then(|g| g.lock().ok().and_then(|m| m.id_actual()))
            == Some(voice);

        match voz::arrancar(voice, &origen, position, self.salida.sample_rate()) {
            Ok((manejador, nueva)) => {
                // El manejador viejo se suelta aquí: su `Drop` para el hilo de
                // decodificación anterior y cierra su fichero.
                if let Ok(mut voces) = self.interior.voces.lock() {
                    voces.insert(voice, manejador);
                }
                if sonaba {
                    if let Ok(g) = self.interior.mezclador.lock()
                        && let Ok(mut m) = g.lock()
                    {
                        m.poner_actual(Some(nueva));
                    }
                } else {
                    self.interior.pendientes().insert(voice, nueva);
                }
            }
            Err(e) => {
                warn!(?voice, error = %e, "no se pudo saltar");
                let _ = self.interior.eventos.send(EngineEvent::Failed {
                    voice,
                    reason_key: e.to_string(),
                });
            }
        }
    }

    fn crossfade_to(&self, next: VoiceId, duration: DurationMs) {
        let Some(voz) = self.interior.pendientes().remove(&next) else {
            warn!(?next, "fundido hacia una voz que no estaba cargada");
            return;
        };
        let marcos = marcos_de(duration.as_ms(), self.salida.sample_rate());

        if let Ok(g) = self.interior.mezclador.lock()
            && let Ok(mut m) = g.lock()
        {
            m.fundir_a(voz, marcos);
        }
        self.interior.pausado.store(false, Ordering::Release);
        let _ = self
            .interior
            .eventos
            .send(EngineEvent::Started { voice: next });
    }

    fn set_volume(&self, volume: Volume) {
        // La curva perceptual se aplica aquí, no en el hilo de audio: es una
        // multiplicación que no hace falta repetir 48 000 veces por segundo.
        self.interior.volumen.poner(volume.gain());
    }

    fn set_equalizer(&self, profile: &EqProfile) {
        self.interior
            .eq
            .publicar(profile, self.salida.sample_rate());
        if let Ok(mut g) = self.interior.perfil.lock() {
            *g = profile.clone();
        }
        // El mezclador recoge los coeficientes en su siguiente bloque.
        if let Ok(g) = self.interior.mezclador.lock()
            && let Ok(mut m) = g.lock()
        {
            m.refrescar_eq(&self.interior.eq);
        }
    }

    fn position(&self) -> DurationMs {
        let actual = self
            .interior
            .mezclador
            .lock()
            .ok()
            .and_then(|g| g.lock().ok().and_then(|m| m.id_actual()));
        let Some(id) = actual else {
            return DurationMs::ZERO;
        };
        self.interior
            .voces
            .lock()
            .ok()
            .and_then(|g| g.get(&id).map(|v| v.posicion(self.salida.sample_rate())))
            .unwrap_or(DurationMs::ZERO)
    }

    fn buffered(&self) -> DurationMs {
        let actual = self
            .interior
            .mezclador
            .lock()
            .ok()
            .and_then(|g| g.lock().ok().and_then(|m| m.id_actual()));
        let Some(id) = actual else {
            return DurationMs::ZERO;
        };
        self.interior
            .voces
            .lock()
            .ok()
            .and_then(|g| {
                g.get(&id)
                    .map(|v| v.decodificado(self.salida.sample_rate()))
            })
            .unwrap_or(DurationMs::ZERO)
    }

    fn devices(&self) -> Vec<AudioDevice> {
        salida::dispositivos().unwrap_or_default()
    }

    fn set_device(&self, device_id: Option<&str>) -> Result<(), AudioError> {
        self.salida.cambiar_dispositivo(device_id)?;
        if let Some(d) = self.salida.dispositivo_actual() {
            let _ = self
                .interior
                .eventos
                .send(EngineEvent::DeviceChanged { device: d });
        }
        Ok(())
    }
}

impl Drop for MotorAudio {
    fn drop(&mut self) {
        self.interior.cerrando.store(true, Ordering::Release);
        if let Some(h) = self.vigilante.take() {
            let _ = h.join();
        }
    }
}

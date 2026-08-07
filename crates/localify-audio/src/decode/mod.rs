//! Demultiplexado y decodificación.
//!
//! Convierte un fichero de audio —o un `.part` a medio descargar— en PCM
//! estéreo intercalado a la frecuencia del dispositivo, que es lo único que el
//! mezclador entiende.
//!
//! ```text
//! MediaSource ─▶ demuxer ─▶ decodificador ─▶ a estéreo ─▶ remuestreo ─▶ PCM
//! ```
//!
//! ## El decodificador que symphonia no trae
//!
//! symphonia cubre FLAC, MP3, AAC, ALAC, Vorbis, WAV y AIFF, pero **no Opus**,
//! que es justamente el códec que YouTube sirve para música. Se registra
//! `libopus` —la implementación de referencia— en el registro de códecs de
//! symphonia, y el demuxer sigue siendo el suyo. Es la pieza mínima que falta,
//! no una cadena paralela.
//!
//! Se descartó un decodificador Opus en Rust puro: los que hay son muy jóvenes,
//! y un artefacto sutil de decodificación es de las cosas más difíciles de
//! atribuir cuando alguien reporta "esta canción suena rara".
//!
//! ## Dónde corre esto
//!
//! En el hilo de decodificación, **nunca** en el callback de audio. Aquí se
//! puede asignar memoria, bloquear en I/O y esperar a que la descarga avance.

pub mod canales;
pub mod resample;

use std::sync::OnceLock;

use localify_core::domain::audio::DurationMs;
use localify_core::ports::audio_engine::AudioError;
use symphonia::core::audio::GenericAudioBufferRef;
use symphonia::core::codecs::CodecParameters;
use symphonia::core::codecs::audio::{AudioDecoder, AudioDecoderOptions};
use symphonia::core::codecs::registry::CodecRegistry;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo, TrackType};
use symphonia::core::io::{MediaSource, MediaSourceStream, MediaSourceStreamOptions};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::units::{Duration, Time, TimeBase, Timestamp};

pub use canales::{Mezcla, Posicion};
pub use resample::Remuestreador;

/// Registro de códecs: los de symphonia más libopus.
///
/// Se construye una sola vez. Registrarlo por fichero costaría un puñado de
/// asignaciones en cada `play`, justo en el camino que debe ser rápido.
fn registro() -> &'static CodecRegistry {
    static REGISTRO: OnceLock<CodecRegistry> = OnceLock::new();
    REGISTRO.get_or_init(|| {
        let mut r = CodecRegistry::new();
        symphonia::default::register_enabled_codecs(&mut r);
        r.register_audio_decoder::<symphonia_adapter_libopus::OpusDecoder>();
        r
    })
}

/// Resultado de pedir más audio.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Avance {
    /// Se produjeron muestras.
    Muestras,
    /// El fichero se acabó. No quedan más.
    Fin,
}

/// Decodificador de una pista.
#[allow(
    clippy::struct_field_names,
    reason = "`decodificador` es el nombre exacto de lo que guarda"
)]
pub struct Decodificador {
    formato: Box<dyn FormatReader + 'static>,
    decodificador: Box<dyn AudioDecoder>,
    pista_id: u32,
    time_base: Option<TimeBase>,
    mezcla: Mezcla,
    remuestreador: Option<Remuestreador>,
    /// PCM estéreo intercalado a la frecuencia de origen, reutilizado.
    intermedio: Vec<f32>,
    /// Buffer que symphonia rellena, reutilizado.
    crudo: Vec<f32>,
    sr_origen: u32,
    sr_destino: u32,
    duracion: Option<DurationMs>,
    /// Marcos ya entregados, a `sr_destino`. Es la base de la posición.
    marcos_emitidos: u64,
}

impl std::fmt::Debug for Decodificador {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Decodificador")
            .field("sr_origen", &self.sr_origen)
            .field("sr_destino", &self.sr_destino)
            .field("mezcla", &self.mezcla)
            .field("duracion", &self.duracion)
            .finish_non_exhaustive()
    }
}

impl Decodificador {
    /// Abre un origen y prepara la decodificación hacia `sr_destino`.
    ///
    /// `extension` ayuda al sondeo a acertar a la primera; si no acierta,
    /// symphonia identifica el contenedor por sus marcadores igualmente.
    ///
    /// # Errors
    /// Si el contenedor no se reconoce, no tiene pista de audio, o su códec no
    /// está soportado.
    pub fn abrir(
        origen: Box<dyn MediaSource + 'static>,
        extension: Option<&str>,
        sr_destino: u32,
    ) -> Result<Self, AudioError> {
        let mss = MediaSourceStream::new(origen, MediaSourceStreamOptions::default());

        let mut hint = Hint::new();
        if let Some(ext) = extension {
            hint.with_extension(ext);
        }

        let formato = symphonia::default::get_probe()
            .probe(
                &hint,
                mss,
                FormatOptions::default(),
                MetadataOptions::default(),
            )
            .map_err(|e| AudioError::UnsupportedFormat(e.to_string()))?;

        Self::desde_formato(formato, sr_destino)
    }

    fn desde_formato(
        formato: Box<dyn FormatReader + 'static>,
        sr_destino: u32,
    ) -> Result<Self, AudioError> {
        let pista = formato
            .default_track(TrackType::Audio)
            .or_else(|| formato.tracks().iter().find(|t| t.codec_params.is_some()))
            .ok_or_else(|| AudioError::UnsupportedFormat("sin pista de audio".to_owned()))?;

        let pista_id = pista.id;
        let time_base = pista.time_base;
        let num_frames = pista.num_frames;

        let Some(CodecParameters::Audio(params)) = pista.codec_params.clone() else {
            return Err(AudioError::UnsupportedFormat(
                "la pista no declara parametros de audio".to_owned(),
            ));
        };

        let sr_origen = params.sample_rate.unwrap_or(sr_destino);
        let canales = params
            .channels
            .as_ref()
            .map_or(2, symphonia::core::audio::Channels::count);

        let decodificador = registro()
            .make_audio_decoder(&params, &AudioDecoderOptions::default())
            .map_err(|e| AudioError::UnsupportedFormat(e.to_string()))?;

        let duracion = duracion_de(num_frames, sr_origen, time_base, pista.duration);

        Ok(Self {
            formato,
            decodificador,
            pista_id,
            time_base,
            mezcla: Mezcla::decidir(canales, None),
            remuestreador: Remuestreador::nuevo(sr_origen, sr_destino)?,
            intermedio: Vec::with_capacity(8192),
            crudo: Vec::with_capacity(8192),
            sr_origen,
            sr_destino,
            duracion,
            marcos_emitidos: 0,
        })
    }

    /// Duración total, si el contenedor la declara.
    ///
    /// Un `.part` a medio descargar no la trae: la duración real la conoce
    /// Spotify, no el fichero.
    #[must_use]
    pub const fn duracion(&self) -> Option<DurationMs> {
        self.duracion
    }

    /// Posición del audio ya entregado.
    #[must_use]
    pub fn posicion(&self) -> DurationMs {
        DurationMs::new(marcos_a_ms(self.marcos_emitidos, self.sr_destino))
    }

    #[must_use]
    pub const fn sample_rate_origen(&self) -> u32 {
        self.sr_origen
    }

    /// Decodifica el siguiente paquete y **añade** PCM estéreo intercalado a
    /// `destino`, ya a la frecuencia del dispositivo.
    ///
    /// Puede devolver [`Avance::Muestras`] sin haber añadido nada: un paquete
    /// puede quedarse entero dentro del remuestreador a la espera de completar
    /// un bloque. Quien llame debe volver a pedir, no interpretarlo como fin.
    ///
    /// # Errors
    /// Si el flujo está corrupto o el origen falla.
    pub fn siguiente(&mut self, destino: &mut Vec<f32>) -> Result<Avance, AudioError> {
        loop {
            let paquete = match self.formato.next_packet() {
                Ok(Some(p)) => p,
                Ok(None) => return self.terminar(destino),
                Err(e) if es_fin(&e) => return self.terminar(destino),
                Err(e) => return Err(AudioError::Decode(e.to_string())),
            };

            if paquete.track_id != self.pista_id {
                continue;
            }

            // Se separan los campos porque el buffer que devuelve el
            // decodificador lo sigue prestando a él: sin esto, `volcar` no
            // podría ser un método.
            let Self {
                decodificador,
                mezcla,
                remuestreador,
                crudo,
                intermedio,
                marcos_emitidos,
                ..
            } = self;

            match decodificador.decode(&paquete) {
                Ok(buffer) => {
                    let antes = destino.len();
                    volcar(
                        &buffer,
                        *mezcla,
                        remuestreador.as_mut(),
                        crudo,
                        intermedio,
                        destino,
                    )?;
                    *marcos_emitidos += ((destino.len() - antes) / 2) as u64;
                    return Ok(Avance::Muestras);
                }
                // Un paquete roto no invalida la canción: symphonia se
                // resincroniza sola en el siguiente. Cortar aquí convertiría un
                // parpadeo en un fallo de reproducción.
                Err(e) if es_recuperable(&e) => {
                    tracing::debug!(error = %e, "paquete descartado");
                }
                Err(e) => return Err(AudioError::Decode(e.to_string())),
            }
        }
    }

    /// Salta a una posición.
    ///
    /// # Errors
    /// Si el origen no admite búsqueda o la posición no existe.
    pub fn buscar(&mut self, posicion: DurationMs) -> Result<DurationMs, AudioError> {
        let segundos = f64::from(posicion.as_ms()) / 1000.0;
        let time = Time::try_from_secs_f64(segundos)
            .ok_or_else(|| AudioError::Decode(format!("posicion invalida: {segundos} s")))?;

        let destino = self
            .formato
            .seek(
                SeekMode::Accurate,
                SeekTo::Time {
                    time,
                    track_id: Some(self.pista_id),
                },
            )
            .map_err(|e| AudioError::Decode(e.to_string()))?;

        // El estado que arrastran decodificador y remuestreador es audio de
        // antes del salto: sonaría como un eco al reanudar.
        self.decodificador.reset();
        if let Some(r) = self.remuestreador.as_mut() {
            r.reiniciar();
        }
        self.intermedio.clear();

        let real = self
            .time_base
            .and_then(|tb| tb.calc_time(destino.actual_ts))
            .map_or(posicion, a_duracion);
        self.marcos_emitidos = u64::from(real.as_ms()) * u64::from(self.sr_destino) / 1000;
        Ok(real)
    }

    /// Vacía lo que quede en el remuestreador y da la pista por terminada.
    fn terminar(&mut self, destino: &mut Vec<f32>) -> Result<Avance, AudioError> {
        if let Some(r) = self.remuestreador.as_mut() {
            let antes = destino.len();
            r.vaciar(destino)?;
            self.marcos_emitidos += ((destino.len() - antes) / 2) as u64;
        }
        Ok(Avance::Fin)
    }
}

/// Pasa el buffer de symphonia a estéreo intercalado y lo remuestrea.
///
/// Va suelta y no como método porque el buffer sigue prestado del
/// decodificador mientras se lee.
fn volcar(
    buffer: &GenericAudioBufferRef<'_>,
    mezcla: Mezcla,
    remuestreador: Option<&mut Remuestreador>,
    crudo: &mut Vec<f32>,
    intermedio: &mut Vec<f32>,
    destino: &mut Vec<f32>,
) -> Result<(), AudioError> {
    if buffer.frames() == 0 {
        return Ok(());
    }
    buffer.copy_to_vec_interleaved(crudo);

    match remuestreador {
        // Sin conversión de frecuencia, el estéreo va directo al destino: un
        // buffer intermedio de más por bloque no aporta nada.
        None => mezcla.aplicar(crudo, destino),
        Some(r) => {
            intermedio.clear();
            mezcla.aplicar(crudo, intermedio);
            r.procesar(intermedio, destino)?;
        }
    }
    Ok(())
}

/// `true` si el error solo significa "no hay más datos".
fn es_fin(e: &symphonia::core::errors::Error) -> bool {
    match e {
        symphonia::core::errors::Error::IoError(io) => {
            io.kind() == std::io::ErrorKind::UnexpectedEof
        }
        symphonia::core::errors::Error::ResetRequired => false,
        _ => false,
    }
}

/// `true` si conviene descartar el paquete y seguir con el siguiente.
fn es_recuperable(e: &symphonia::core::errors::Error) -> bool {
    matches!(
        e,
        symphonia::core::errors::Error::DecodeError(_)
            | symphonia::core::errors::Error::ResetRequired
    )
}

fn marcos_a_ms(marcos: u64, sample_rate: u32) -> u32 {
    if sample_rate == 0 {
        return 0;
    }
    u32::try_from(marcos * 1000 / u64::from(sample_rate)).unwrap_or(u32::MAX)
}

/// Duración de la pista, con el dato que resulte más fiable.
///
/// Se prefiere el número de marcos: excluye el relleno del codificador, así que
/// da la duración **audible**. La duración en unidades de la base de tiempo lo
/// incluye, y por eso un MP3 aparenta durar unas décimas más de lo que suena.
fn duracion_de(
    num_frames: Option<u64>,
    sample_rate: u32,
    time_base: Option<TimeBase>,
    duracion: Option<Duration>,
) -> Option<DurationMs> {
    if let Some(n) = num_frames.filter(|n| *n > 0) {
        return Some(DurationMs::new(marcos_a_ms(n, sample_rate)));
    }
    let (tb, d) = (time_base?, duracion?);
    // Una duración es un desplazamiento desde cero, así que se convierte
    // tratándola como marca de tiempo con la misma base.
    let ts = Timestamp::new(i64::try_from(d.get()).ok()?);
    tb.calc_time(ts).map(a_duracion)
}

/// Pasa un instante de symphonia a milisegundos.
fn a_duracion(t: Time) -> DurationMs {
    let segundos = t.as_secs_f64();
    if segundos <= 0.0 {
        return DurationMs::ZERO;
    }
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "acotado justo debajo, y una cancion no dura 49 dias"
    )]
    DurationMs::new((segundos * 1000.0).min(f64::from(u32::MAX)) as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn el_registro_incluye_opus() {
        // Es la razon de existir del registro propio: sin esto, la mitad de la
        // biblioteca (todo lo que viene de YouTube) seria indecodificable.
        use symphonia::core::codecs::audio::well_known::CODEC_ID_OPUS;
        assert!(
            registro().get_audio_decoder(CODEC_ID_OPUS).is_some(),
            "libopus no quedo registrado"
        );
    }

    #[test]
    fn el_registro_incluye_los_formatos_de_una_biblioteca_local() {
        use symphonia::core::codecs::audio::well_known::{
            CODEC_ID_AAC, CODEC_ID_FLAC, CODEC_ID_MP3, CODEC_ID_VORBIS,
        };
        for (id, nombre) in [
            (CODEC_ID_FLAC, "flac"),
            (CODEC_ID_MP3, "mp3"),
            (CODEC_ID_AAC, "aac"),
            (CODEC_ID_VORBIS, "vorbis"),
        ] {
            assert!(
                registro().get_audio_decoder(id).is_some(),
                "falta el decodificador de {nombre}"
            );
        }
    }

    #[test]
    fn el_registro_se_construye_una_sola_vez() {
        assert!(std::ptr::eq(registro(), registro()));
    }

    #[test]
    fn la_duracion_prefiere_los_marcos_audibles() {
        // El relleno del codificador esta en la duracion del contenedor pero no
        // se oye: un MP3 aparentaria durar unas decimas de mas.
        let d = duracion_de(Some(441_000), 44_100, None, None);
        assert_eq!(d, Some(DurationMs::new(10_000)));
    }

    #[test]
    fn sin_marcos_se_usa_la_base_de_tiempo() {
        let tb = TimeBase::try_new(1, 1000).expect("base valida");
        let d = duracion_de(None, 44_100, Some(tb), Some(Duration::new(5_500)));
        assert_eq!(d, Some(DurationMs::new(5_500)));
    }

    #[test]
    fn sin_ningun_dato_no_se_inventa_una_duracion() {
        // Un `.part` a medio descargar no la trae. Devolver cero haria que la
        // barra de progreso mintiera; `None` deja que la sepa Spotify.
        assert_eq!(duracion_de(None, 44_100, None, None), None);
        assert_eq!(duracion_de(Some(0), 44_100, None, None), None);
    }

    #[test]
    fn la_conversion_de_marcos_a_milisegundos_no_divide_por_cero() {
        assert_eq!(marcos_a_ms(1000, 0), 0);
        assert_eq!(marcos_a_ms(48_000, 48_000), 1000);
        assert_eq!(marcos_a_ms(24_000, 48_000), 500);
    }
}

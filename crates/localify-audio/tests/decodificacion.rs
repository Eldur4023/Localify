//! Decodificación de verdad, sobre ficheros reales.
//!
//! Los tests unitarios comprueban las piezas por separado. Este comprueba lo
//! único que al usuario le importa: que un fichero de audio entra y sale sonido
//! correcto. Es el criterio de la Fase 7 —"reproducción correcta de opus,
//! m4a/AAC, mp3, flac, ogg/vorbis, wav"— convertido en algo que la CI puede
//! verificar sola.
//!
//! ## Qué se mide
//!
//! Los ficheros son un tono de 440 Hz de un segundo. No basta con que la
//! decodificación no dé error: se comprueba que **la frecuencia que sale es la
//! que entró**, contando cruces por cero. Un decodificador mal configurado, un
//! remuestreo con la relación invertida o unos canales intercambiados producen
//! audio perfectamente válido y con la frecuencia equivocada, y solo esta
//! medida lo detecta.
//!
//! ## Por qué no dependen de FFmpeg
//!
//! Los ficheros se generan una vez con `scripts/gen-fixtures-audio.ps1` y se
//! versionan. La suite no llama a ningún binario externo, igual que la de
//! yt-dlp no lo necesita para probar el emparejamiento.

#![allow(clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};

use localify_audio::decode::{Avance, Decodificador};
use localify_audio::source::GrowingFileSource;

const SR: u32 = 48_000;
const TONO_HZ: f32 = 440.0;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// Decodifica un fichero entero a PCM estéreo intercalado.
fn decodificar(nombre: &str) -> Vec<f32> {
    let ruta = fixtures().join(nombre);
    assert!(
        ruta.exists(),
        "falta el fichero de prueba '{}'. Ejecuta scripts/gen-fixtures-audio.ps1",
        ruta.display()
    );

    let origen = GrowingFileSource::abrir_completo(&ruta).expect("abre el fichero");
    let extension = ruta.extension().and_then(|e| e.to_str());
    let mut d =
        Decodificador::abrir(Box::new(origen), extension, SR).expect("abre el decodificador");

    let mut pcm = Vec::new();
    while d.siguiente(&mut pcm).expect("decodifica") == Avance::Muestras {
        assert!(
            pcm.len() < SR as usize * 2 * 10,
            "el decodificador no termina: mas de 10 s para un tono de 1 s"
        );
    }
    pcm
}

/// Frecuencia dominante, contando cruces por cero del canal izquierdo.
///
/// Es una medida burda pero suficiente y sin dependencias: un tono puro tiene
/// exactamente dos cruces por ciclo.
fn frecuencia(pcm: &[f32]) -> f32 {
    // Se descartan los extremos: el arranque del codificador y su cola.
    let izquierdo: Vec<f32> = pcm.iter().step_by(2).copied().collect();
    let desde = izquierdo.len() / 4;
    let hasta = izquierdo.len() * 3 / 4;
    let tramo = &izquierdo[desde..hasta];

    let cruces = tramo
        .windows(2)
        .filter(|v| (v[0] < 0.0) != (v[1] < 0.0))
        .count();

    #[allow(clippy::cast_precision_loss, reason = "48000 muestras caben en f32")]
    let marcos = tramo.len() as f32;
    #[allow(clippy::cast_precision_loss, reason = "unos cientos de cruces")]
    let n = cruces as f32;
    #[allow(clippy::cast_precision_loss, reason = "48000 cabe exacto en f32")]
    let sr = SR as f32;
    (n / 2.0) * (sr / marcos)
}

fn rms(pcm: &[f32]) -> f32 {
    if pcm.is_empty() {
        return 0.0;
    }
    #[allow(clippy::cast_precision_loss, reason = "acotado")]
    let n = pcm.len() as f32;
    (pcm.iter().map(|v| v * v).sum::<f32>() / n).sqrt()
}

/// Comprueba un formato de principio a fin.
fn comprobar(nombre: &str) {
    let pcm = decodificar(nombre);

    let marcos = pcm.len() / 2;
    assert!(
        (SR as usize * 9 / 10..=SR as usize * 12 / 10).contains(&marcos),
        "{nombre}: un segundo deberia dar ~48000 marcos, dio {marcos}"
    );

    let hz = frecuencia(&pcm);
    assert!(
        (hz - TONO_HZ).abs() < 5.0,
        "{nombre}: se esperaba un tono de {TONO_HZ} Hz y salio de {hz:.1} Hz"
    );

    let nivel = rms(&pcm);
    assert!(
        nivel > 0.3,
        "{nombre}: el audio salio casi mudo (rms {nivel:.4})"
    );
}

#[test]
fn opus_es_el_formato_que_llega_de_youtube() {
    // symphonia no trae decodificador Opus: si este test falla, es que libopus
    // no quedo registrado y toda la biblioteca descargada seria inservible.
    comprobar("tono.opus");
}

#[test]
fn m4a_aac() {
    comprobar("tono.m4a");
}

#[test]
fn mp3() {
    comprobar("tono.mp3");
}

#[test]
fn flac_a_cuarenta_y_cuatro_uno_se_remuestrea_bien() {
    // Este fichero esta a 44.1 kHz: recorre el camino del remuestreador. Si la
    // relacion estuviera invertida, el tono saldria a 400 o a 480 Hz.
    comprobar("tono.flac");
}

#[test]
fn ogg_vorbis() {
    comprobar("tono.ogg");
}

#[test]
fn wav_pcm() {
    comprobar("tono.wav");
}

#[test]
fn los_dos_canales_salen_con_el_mismo_contenido() {
    // Una fuente mono duplicada o un intercambio de canales pasarian
    // desapercibidos midiendo solo el izquierdo.
    let pcm = decodificar("tono.wav");
    let izq: Vec<f32> = pcm.iter().step_by(2).copied().collect();
    let der: Vec<f32> = pcm.iter().skip(1).step_by(2).copied().collect();
    assert!(
        (rms(&izq) - rms(&der)).abs() < 0.01,
        "los canales no coinciden: {} vs {}",
        rms(&izq),
        rms(&der)
    );
}

#[test]
fn el_canal_central_de_una_mezcla_multicanal_llega_a_los_dos_altavoces() {
    // El tono esta SOLO en el canal central. Quedandose con los dos primeros
    // canales, este fichero sonaria a silencio: es el fallo que la mezcla
    // ITU-R BS.775 evita, y en una cancion real seria la voz principal.
    let pcm = decodificar("tono-5.1.flac");
    let nivel = rms(&pcm);
    assert!(
        nivel > 0.1,
        "la voz del canal central desaparecio (rms {nivel:.4})"
    );

    let hz = frecuencia(&pcm);
    assert!(
        (hz - TONO_HZ).abs() < 5.0,
        "el tono del canal central salio a {hz:.1} Hz"
    );
}

#[test]
fn la_duracion_declarada_coincide_con_el_audio_producido() {
    let ruta = fixtures().join("tono.flac");
    assert!(ruta.exists(), "falta {}", ruta.display());

    let origen = GrowingFileSource::abrir_completo(&ruta).expect("abre");
    let mut d = Decodificador::abrir(Box::new(origen), Some("flac"), SR).expect("decodificador");

    let declarada = d.duracion().expect("el FLAC declara su duracion");
    assert!(
        (900..=1100).contains(&declarada.as_ms()),
        "duracion declarada: {} ms",
        declarada.as_ms()
    );

    let mut pcm = Vec::new();
    while d.siguiente(&mut pcm).expect("decodifica") == Avance::Muestras {}

    let real_ms = (pcm.len() / 2) * 1000 / SR as usize;
    assert!(
        real_ms.abs_diff(declarada.as_ms() as usize) < 100,
        "declarados {} ms, producidos {real_ms} ms",
        declarada.as_ms()
    );
}

#[test]
fn buscar_deja_la_posicion_donde_se_pidio() {
    let ruta = fixtures().join("tono.wav");
    assert!(ruta.exists(), "falta {}", ruta.display());

    let origen = GrowingFileSource::abrir_completo(&ruta).expect("abre");
    let mut d = Decodificador::abrir(Box::new(origen), Some("wav"), SR).expect("decodificador");

    let pedida = localify_core::domain::audio::DurationMs::new(500);
    let real = d.buscar(pedida).expect("busca");
    assert!(
        real.as_ms().abs_diff(pedida.as_ms()) < 50,
        "se pidio {} ms y quedo en {} ms",
        pedida.as_ms(),
        real.as_ms()
    );

    // Y desde ahi solo queda medio segundo.
    let mut pcm = Vec::new();
    while d.siguiente(&mut pcm).expect("decodifica") == Avance::Muestras {}
    let restante_ms = (pcm.len() / 2) * 1000 / SR as usize;
    assert!(
        (400..=600).contains(&restante_ms),
        "tras saltar al medio deberia quedar ~500 ms, quedaron {restante_ms}"
    );
}

#[test]
fn un_fichero_que_no_es_audio_falla_sin_entrar_en_panico() {
    let basura = std::env::temp_dir().join("localify-no-es-audio.opus");
    std::fs::write(&basura, b"esto no es un contenedor de audio").expect("escribe");

    let origen = GrowingFileSource::abrir_completo(&basura).expect("abre");
    let resultado = Decodificador::abrir(Box::new(origen), Some("opus"), SR);
    assert!(resultado.is_err(), "deberia rechazarlo, no aceptarlo");

    let _ = std::fs::remove_file(&basura);
}

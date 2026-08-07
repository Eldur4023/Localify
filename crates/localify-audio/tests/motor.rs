//! El motor completo, contra el dispositivo de audio de verdad.
//!
//! Es el único sitio del proyecto que necesita una tarjeta de sonido. Si no la
//! hay —una CI sin audio—, los tests se saltan en vez de fallar: un entorno sin
//! altavoces no es un defecto del código.
//!
//! Lo que se comprueba aquí no se puede comprobar en ningún otro sitio: que la
//! frecuencia del dispositivo se negocia bien, que el hilo de decodificación
//! alimenta al de audio a tiempo, que la posición avanza al ritmo del reloj y
//! que soltar el motor no deja hilos ni ficheros abiertos.

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::print_stderr,
    reason = "un test que se salta debe decir por que, y aqui no hay tracing"
)]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use localify_audio::engine::MotorAudio;
use localify_core::domain::audio::{DurationMs, EqProfile, Volume};
use localify_core::ports::audio_engine::{AudioEngine, AudioEventSource, AudioSource, EngineEvent};

fn fixture(nombre: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(nombre)
}

/// Arranca el motor, o devuelve `None` si esta máquina no tiene salida de audio.
fn motor() -> Option<(MotorAudio, localify_audio::engine::ReceptorEventos)> {
    match MotorAudio::arrancar() {
        Ok(v) => Some(v),
        Err(e) => {
            eprintln!("sin dispositivo de audio, se salta el test: {e}");
            None
        }
    }
}

/// Espera a que llegue un evento que cumpla `condicion`.
fn esperar_evento(
    rx: &mut localify_audio::engine::ReceptorEventos,
    limite: Duration,
    condicion: impl Fn(&EngineEvent) -> bool,
) -> Option<EngineEvent> {
    let hasta = Instant::now() + limite;
    while Instant::now() < hasta {
        if let Some(e) = rx.try_recv() {
            if condicion(&e) {
                return Some(e);
            }
            continue;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    None
}

#[test]
fn el_motor_negocia_una_frecuencia_utilizable() {
    let Some((m, _rx)) = motor() else { return };
    let sr = m.sample_rate();
    assert!(
        (8_000..=384_000).contains(&sr),
        "frecuencia absurda: {sr} Hz"
    );
}

#[test]
fn cargar_y_reproducir_hace_avanzar_la_posicion() {
    // Es la prueba de que las tres capas —decodificacion, anillo y salida—
    // estan conectadas: si alguna fallara, la posicion se quedaria en cero.
    let Some((m, _rx)) = motor() else { return };

    let voz = m
        .load(AudioSource::File(fixture("tono.wav")), DurationMs::ZERO)
        .expect("carga");
    m.play(voz);

    std::thread::sleep(Duration::from_millis(400));
    let pos = m.position();
    m.pause();

    assert!(
        pos.as_ms() > 100,
        "la posicion no avanzo: {} ms tras 400 ms de reproduccion",
        pos.as_ms()
    );
    assert!(
        pos.as_ms() < 900,
        "la posicion avanzo demasiado deprisa: {} ms",
        pos.as_ms()
    );
}

#[test]
fn lo_decodificado_va_por_delante_de_lo_que_suena() {
    // Es lo que absorbe los retrasos del sistema: si el anillo no fuera por
    // delante, cualquier pausa del planificador seria un corte audible.
    let Some((m, _rx)) = motor() else { return };

    let voz = m
        .load(AudioSource::File(fixture("tono.wav")), DurationMs::ZERO)
        .expect("carga");
    m.play(voz);
    std::thread::sleep(Duration::from_millis(300));

    let sonando = m.position();
    let listo = m.buffered();
    m.pause();

    assert!(
        listo.as_ms() >= sonando.as_ms(),
        "lo decodificado ({} ms) va por detras de lo que suena ({} ms)",
        listo.as_ms(),
        sonando.as_ms()
    );
}

#[test]
fn una_pista_llega_a_su_fin_y_lo_anuncia() {
    let Some((m, mut rx)) = motor() else { return };

    let voz = m
        .load(AudioSource::File(fixture("tono.opus")), DurationMs::ZERO)
        .expect("carga");
    m.play(voz);

    let evento = esperar_evento(&mut rx, Duration::from_secs(5), |e| {
        matches!(e, EngineEvent::Ended { .. })
    });
    assert!(
        evento.is_some(),
        "un tono de un segundo deberia acabar y avisar"
    );
}

#[test]
fn saltar_deja_la_posicion_donde_se_pidio() {
    let Some((m, _rx)) = motor() else { return };

    let voz = m
        .load(AudioSource::File(fixture("tono.wav")), DurationMs::ZERO)
        .expect("carga");
    m.play(voz);
    std::thread::sleep(Duration::from_millis(100));

    m.seek(voz, DurationMs::new(600));
    std::thread::sleep(Duration::from_millis(100));
    let pos = m.position();
    m.pause();

    assert!(
        (550..=900).contains(&pos.as_ms()),
        "tras saltar a 600 ms, la posicion es {} ms",
        pos.as_ms()
    );
}

#[test]
fn detener_una_voz_la_saca_de_la_reproduccion() {
    let Some((m, _rx)) = motor() else { return };

    let voz = m
        .load(AudioSource::File(fixture("tono.wav")), DurationMs::ZERO)
        .expect("carga");
    m.play(voz);
    std::thread::sleep(Duration::from_millis(100));

    m.stop(voz);
    assert_eq!(
        m.position(),
        DurationMs::ZERO,
        "sin voz activa, la posicion debe ser cero"
    );
}

#[test]
fn el_volumen_y_el_ecualizador_se_aceptan_en_caliente() {
    // Cambiarlos mientras suena no debe fallar ni cortar el audio.
    let Some((m, _rx)) = motor() else { return };

    let voz = m
        .load(AudioSource::File(fixture("tono.wav")), DurationMs::ZERO)
        .expect("carga");
    m.play(voz);

    m.set_volume(Volume::new(0.5));
    for perfil in EqProfile::predefinidos() {
        m.set_equalizer(&perfil);
    }
    std::thread::sleep(Duration::from_millis(150));

    assert!(m.position().as_ms() > 0, "el audio se corto al ecualizar");
    m.pause();
}

#[test]
fn se_enumeran_los_dispositivos_de_salida() {
    let Some((m, _rx)) = motor() else { return };
    let lista = m.devices();
    assert!(
        !lista.is_empty(),
        "si el motor arranco, deberia haber al menos un dispositivo"
    );
    assert!(lista.iter().all(|d| !d.id.is_empty()));
}

#[test]
fn dos_pistas_seguidas_se_funden_sin_hueco() {
    // El crossfade es la razon de que existan dos voces. Se comprueba que la
    // segunda empieza a sonar sin que la primera haya tenido que terminar.
    let Some((m, mut rx)) = motor() else { return };

    let primera = m
        .load(AudioSource::File(fixture("tono.wav")), DurationMs::ZERO)
        .expect("carga la primera");
    m.play(primera);
    std::thread::sleep(Duration::from_millis(150));

    let segunda = m
        .load(AudioSource::File(fixture("tono.ogg")), DurationMs::ZERO)
        .expect("carga la segunda");
    m.crossfade_to(segunda, DurationMs::new(200));

    let arranco = esperar_evento(
        &mut rx,
        Duration::from_secs(2),
        |e| matches!(e, EngineEvent::Started { voice } if *voice == segunda),
    );
    assert!(arranco.is_some(), "la segunda voz nunca arranco");

    std::thread::sleep(Duration::from_millis(300));
    assert!(m.position().as_ms() > 0, "no suena nada tras el fundido");
    m.pause();
}

#[test]
fn soltar_el_motor_no_deja_el_fichero_abierto() {
    // Si el hilo de decodificacion sobreviviera al motor, Windows no dejaria
    // borrar ni renombrar el fichero: justo lo que hace cada descarga al
    // terminar.
    let copia = std::env::temp_dir().join("localify-motor-cierre.wav");
    std::fs::copy(fixture("tono.wav"), &copia).expect("copia");

    {
        let Some((m, _rx)) = motor() else {
            let _ = std::fs::remove_file(&copia);
            return;
        };
        let voz = m
            .load(AudioSource::File(copia.clone()), DurationMs::ZERO)
            .expect("carga");
        m.play(voz);
        std::thread::sleep(Duration::from_millis(150));
    }

    assert!(
        std::fs::remove_file(&copia).is_ok(),
        "el fichero seguia abierto tras soltar el motor"
    );
}

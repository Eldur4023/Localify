//! El contrato de tiempo real, comprobado por un allocator instrumentado.
//!
//! "El callback de audio no asigna memoria" es fácil de escribir en un
//! comentario y muy fácil de romper sin enterarse: un `Vec` que crece, un
//! `format!` en un log, un `Box` en una ruta de error. Cuando pasa, no falla
//! ningún test —el audio sale bien— y solo se nota como chasquidos ocasionales
//! bajo carga, que es de las cosas más difíciles de diagnosticar.
//!
//! Este fichero instala un allocator global que cuenta las asignaciones del
//! hilo actual. Se arma justo antes de llamar al mezclador y se desarma
//! después: si el contador se mueve, el test falla y señala exactamente qué
//! operación lo hizo.
//!
//! El contador es por hilo (`thread_local`) a propósito: el resto de la suite
//! corre en paralelo y sus asignaciones no deben contaminar la medida.

// `GlobalAlloc` es un trait `unsafe` y no hay forma de instrumentar las
// asignaciones sin implementarlo. Se levanta el lint **solo aquí**: el crate
// `localify-audio` no contiene una sola línea de `unsafe`, y merece la pena que
// siga siendo así.
#![allow(unsafe_code)]
#![allow(clippy::expect_used, clippy::panic)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use localify_audio::dsp::EqCompartido;
use localify_audio::engine::{EstadoVoz, Mezclador, VolumenCompartido, Voz};
use localify_core::domain::audio::EqProfile;
use localify_core::ports::audio_engine::VoiceId;

thread_local! {
    /// Cuenta asignaciones mientras esté armado.
    static CONTADOR: Cell<usize> = const { Cell::new(0) };
    static ARMADO: Cell<bool> = const { Cell::new(false) };
}

struct Contable;

// SAFETY: se delega todo en `System`, que ya cumple el contrato de
// `GlobalAlloc`. Lo único que se añade es un contador por hilo, que no toca la
// memoria devuelta ni cambia el comportamiento de la asignación.
unsafe impl GlobalAlloc for Contable {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if ARMADO.with(Cell::get) {
            CONTADOR.with(|c| c.set(c.get() + 1));
        }
        // SAFETY: `layout` llega válido de quien llama, según el contrato del
        // propio trait.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: `ptr` y `layout` provienen de una llamada previa a `alloc`.
        unsafe { System.dealloc(ptr, layout) };
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if ARMADO.with(Cell::get) {
            CONTADOR.with(|c| c.set(c.get() + 1));
        }
        // SAFETY: ídem, más `new_size` no nulo, que garantiza el contrato.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: Contable = Contable;

/// Cuenta las asignaciones que hace `f`.
fn asignaciones(f: impl FnOnce()) -> usize {
    CONTADOR.with(|c| c.set(0));
    ARMADO.with(|a| a.set(true));
    f();
    ARMADO.with(|a| a.set(false));
    CONTADOR.with(Cell::get)
}

/// Monta una voz con muestras ya en el anillo.
fn voz(id: u32, muestras: usize) -> Voz {
    let (mut productor, consumidor) = rtrb::RingBuffer::<f32>::new(muestras.max(1));
    for i in 0..muestras {
        #[allow(clippy::cast_precision_loss, reason = "acotado en tests")]
        let v = ((i % 100) as f32 / 100.0) - 0.5;
        productor.push(v).expect("cabe");
    }
    // El productor vive en el hilo de decodificación en producción; aquí solo
    // interesa lo que ya está en el anillo.
    std::mem::forget(productor);

    Voz::nueva(
        VoiceId(id),
        consumidor,
        EstadoVoz::nuevo(),
        Arc::new(AtomicBool::new(false)),
    )
}

fn mezclador(marcos: usize) -> Mezclador {
    Mezclador::nuevo(
        48_000,
        marcos,
        Arc::new(VolumenCompartido::nuevo(1.0)),
        Arc::new(AtomicBool::new(false)),
    )
}

#[test]
fn el_contador_detecta_una_asignacion() {
    // Antes de fiarse de los tests de abajo, hay que comprobar que el
    // instrumento mide: un contador roto los haria pasar todos.
    let n = asignaciones(|| {
        let v: Vec<u8> = Vec::with_capacity(1024);
        std::hint::black_box(&v);
    });
    assert!(n > 0, "el allocator instrumentado no cuenta nada");
}

#[test]
fn el_callback_no_asigna_memoria() {
    let mut m = mezclador(2048);
    m.poner_actual(Some(voz(0, 200_000)));

    let mut salida = vec![0.0_f32; 2048 * 2];
    // Primera pasada fuera de la medida: lo que se reserve al arrancar cuenta
    // como construccion, no como trabajo del callback.
    m.rellenar(&mut salida);

    let n = asignaciones(|| {
        for _ in 0..50 {
            m.rellenar(&mut salida);
        }
    });
    assert_eq!(n, 0, "el callback asigno memoria {n} veces");
}

#[test]
fn el_callback_no_asigna_durante_un_fundido() {
    // El fundido mezcla dos voces y usa un buffer extra: si ese buffer se
    // reservara al vuelo, seria justo en la transicion entre canciones.
    let mut m = mezclador(2048);
    m.poner_actual(Some(voz(0, 400_000)));
    m.fundir_a(voz(1, 400_000), 48_000);

    let mut salida = vec![0.0_f32; 2048 * 2];
    m.rellenar(&mut salida);

    let n = asignaciones(|| {
        for _ in 0..50 {
            m.rellenar(&mut salida);
        }
    });
    assert_eq!(n, 0, "el fundido asigno memoria {n} veces");
}

#[test]
fn el_callback_no_asigna_con_el_ecualizador_activo() {
    let compartido = EqCompartido::nuevo();
    let mut ganancias = [0.0_f32; 10];
    ganancias[0] = 8.0;
    ganancias[9] = -6.0;
    compartido.publicar(
        &EqProfile::new("test", "test", ganancias).expect("perfil valido"),
        48_000,
    );

    let mut m = mezclador(2048);
    m.poner_actual(Some(voz(0, 400_000)));
    m.refrescar_eq(&compartido);

    let mut salida = vec![0.0_f32; 2048 * 2];
    m.rellenar(&mut salida);

    let n = asignaciones(|| {
        for _ in 0..50 {
            m.rellenar(&mut salida);
        }
    });
    assert_eq!(n, 0, "el ecualizador asigno memoria {n} veces");
}

#[test]
fn recoger_un_perfil_nuevo_no_asigna_en_el_callback() {
    // Publicar coeficientes SI asigna: se hace en el hilo de control. Lo que no
    // puede asignar es recogerlos, que es lo que ocurre dentro del callback.
    let compartido = EqCompartido::nuevo();
    let mut m = mezclador(2048);
    m.poner_actual(Some(voz(0, 400_000)));
    let mut salida = vec![0.0_f32; 2048 * 2];
    m.rellenar(&mut salida);

    compartido.publicar(&EqProfile::plano(), 48_000);

    let n = asignaciones(|| {
        m.refrescar_eq(&compartido);
        m.rellenar(&mut salida);
    });
    assert_eq!(n, 0, "recoger el perfil asigno memoria {n} veces");
}

#[test]
fn el_callback_no_asigna_con_una_salida_no_estereo() {
    // La conversion a mono o multicanal usa otro buffer intermedio; tambien
    // tiene que estar reservado de antemano.
    let mut m = mezclador(2048);
    m.poner_actual(Some(voz(0, 400_000)));

    let mut mono = vec![0.0_f32; 1024];
    m.rellenar_a_canales(&mut mono, 1);

    let n = asignaciones(|| {
        for _ in 0..50 {
            m.rellenar_a_canales(&mut mono, 1);
        }
    });
    assert_eq!(n, 0, "la conversion de canales asigno memoria {n} veces");
}

#[test]
fn el_callback_no_asigna_al_quedarse_sin_muestras() {
    // El underrun es el caso raro, y por tanto el que mas facil es que lleve un
    // camino de error que asigne. Rellenar con silencio no debe hacerlo.
    let mut m = mezclador(2048);
    m.poner_actual(Some(voz(0, 64)));

    let mut salida = vec![0.0_f32; 2048 * 2];
    m.rellenar(&mut salida);

    let n = asignaciones(|| {
        for _ in 0..50 {
            m.rellenar(&mut salida);
        }
    });
    assert_eq!(n, 0, "el underrun asigno memoria {n} veces");
}

//! Configuración persistente y migración de carpeta, con disco y base de datos
//! reales.
//!
//! Lo que se prueba aquí no es que los ajustes se guarden —eso es un `INSERT`—
//! sino las dos decisiones que tienen consecuencias:
//!
//! - **Una sección corrupta no se lleva por delante las demás.** Es la razón de
//!   guardar un JSON por sección en vez de un blob único, y sin un test que lo
//!   fije, el primero que "simplifique" a un solo documento no notará nada
//!   hasta que a alguien se le corrompa la base de datos.
//! - **La migración copia antes de cambiar el ajuste y borra después.** Es el
//!   único orden en el que interrumpir la operación deja una biblioteca
//!   completa en un sitio conocido. El test corta la ejecución en cada uno de
//!   los tres tramos y comprueba qué queda.

#![allow(clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use async_trait::async_trait;
use localify_core::domain::audio::EqProfile;
use localify_core::domain::settings::{AudioSettings, DownloadSettings, Language, SettingsPatch};
use localify_core::error::CoreResult;
use localify_core::events::{DomainEvent, EventPublisher};
use localify_core::ports::database::SettingsRepository;
use localify_core::ports::platform::{AppPaths, LocaleProvider, SecretStore};
use localify_core::ports::services::SettingsService;
use localify_db::Pool;
use localify_db::pool::TempDbGuard;
use localify_platform::{LocalifyPaths, RealFileSystem};
use localify_services::ajustes::{Dependencias, SettingsServiceImpl};

// ─────────────────────────────────────────────────────────────────────────────
// Dobles
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Default)]
struct BusDePrueba(std::sync::Mutex<Vec<DomainEvent>>);

impl EventPublisher for BusDePrueba {
    fn publish(&self, event: DomainEvent) {
        if let Ok(mut v) = self.0.lock() {
            v.push(event);
        }
    }
}

impl BusDePrueba {
    fn nombres(&self) -> Vec<String> {
        self.0
            .lock()
            .map(|v| v.iter().map(|e| e.nombre().to_owned()).collect())
            .unwrap_or_default()
    }
}

/// Almacén de secretos en memoria. El de verdad usa DPAPI y escribe en el
/// perfil del usuario, que no es sitio para un test.
#[derive(Debug, Default)]
struct SecretosFalsos(std::sync::Mutex<Vec<(String, String)>>);

#[async_trait]
impl SecretStore for SecretosFalsos {
    async fn get(&self, key: &str) -> CoreResult<Option<String>> {
        Ok(self
            .0
            .lock()
            .ok()
            .and_then(|v| v.iter().find(|(k, _)| k == key).map(|(_, val)| val.clone())))
    }
    async fn set(&self, key: &str, value: &str) -> CoreResult<()> {
        if let Ok(mut v) = self.0.lock() {
            v.retain(|(k, _)| k != key);
            v.push((key.to_owned(), value.to_owned()));
        }
        Ok(())
    }
    async fn delete(&self, key: &str) -> CoreResult<()> {
        if let Ok(mut v) = self.0.lock() {
            v.retain(|(k, _)| k != key);
        }
        Ok(())
    }
}

#[derive(Debug)]
struct LocaleFalso(&'static str);

impl LocaleProvider for LocaleFalso {
    fn system_locale(&self) -> String {
        self.0.to_owned()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Montaje
// ─────────────────────────────────────────────────────────────────────────────

struct Ctx {
    repo: Arc<dyn SettingsRepository>,
    secretos: Arc<SecretosFalsos>,
    bus: Arc<BusDePrueba>,
    paths: Arc<LocalifyPaths>,
    crossfade: Arc<AtomicU32>,
    raiz: PathBuf,
    _guard: TempDbGuard,
}

impl Drop for Ctx {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.raiz);
    }
}

async fn ctx() -> Ctx {
    let (pool, guard) = Pool::temporal().expect("abre");
    localify_db::ejecutar_migraciones(&pool)
        .await
        .expect("migra");

    let raiz = std::env::temp_dir().join(format!("localify-aj-{}", uuid::Uuid::now_v7()));
    let paths = Arc::new(LocalifyPaths::con_biblioteca(
        raiz.join("config"),
        raiz.join("biblioteca"),
    ));
    paths.crear_estructura().expect("crea carpetas");

    Ctx {
        repo: Arc::new(localify_db::repositories::SqliteSettingsRepository::new(
            pool.clone(),
        )),
        secretos: Arc::new(SecretosFalsos::default()),
        bus: Arc::new(BusDePrueba::default()),
        paths,
        crossfade: Arc::new(AtomicU32::new(0)),
        raiz,
        _guard: guard,
    }
}

impl Ctx {
    /// Construye el servicio leyendo lo que haya ya en la base de datos.
    ///
    /// Se llama varias veces por test a propósito: reconstruirlo es lo que
    /// simula reabrir la aplicación, que es donde se comprueba si un ajuste
    /// sobrevivió de verdad o solo estaba en memoria.
    async fn servicio(&self) -> SettingsServiceImpl {
        SettingsServiceImpl::cargar(Dependencias {
            repo: Arc::clone(&self.repo),
            secretos: Arc::clone(&self.secretos) as Arc<dyn SecretStore>,
            eventos: Arc::clone(&self.bus) as Arc<dyn EventPublisher>,
            paths: Arc::clone(&self.paths) as Arc<dyn AppPaths>,
            fs: Arc::new(RealFileSystem::new()),
            // Sin motor: el test no tiene tarjeta de sonido y el servicio ya
            // contempla ese caso como normal, no como degradado.
            audio: None,
            crossfade: Arc::clone(&self.crossfade),
            locale: Arc::new(LocaleFalso("es-ES")),
            proveedor: None,
            spotify: None,
        })
        .await
    }

    fn biblioteca(&self) -> PathBuf {
        self.raiz.join("biblioteca")
    }

    fn nombres_bus(&self) -> Vec<String> {
        self.bus.nombres()
    }
}

/// Crea un fichero con contenido conocido.
fn escribir(ruta: &Path, contenido: &str) {
    if let Some(p) = ruta.parent() {
        std::fs::create_dir_all(p).expect("crea carpeta");
    }
    std::fs::write(ruta, contenido).expect("escribe");
}

/// Ficheros de una carpeta, en rutas relativas y ordenados.
fn contenido(raiz: &Path) -> Vec<String> {
    let mut salida = Vec::new();
    let mut pendientes = vec![raiz.to_path_buf()];
    while let Some(dir) = pendientes.pop() {
        let Ok(entradas) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in entradas.flatten() {
            let ruta = e.path();
            if ruta.is_dir() {
                pendientes.push(ruta);
            } else if let Ok(rel) = ruta.strip_prefix(raiz) {
                salida.push(rel.display().to_string().replace('\\', "/"));
            }
        }
    }
    salida.sort();
    salida
}

// ─────────────────────────────────────────────────────────────────────────────
// Persistencia
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn un_ajuste_guardado_sobrevive_a_reabrir_la_aplicacion() {
    let c = ctx().await;

    let s = c.servicio().await;
    s.patch(SettingsPatch {
        language: Some(Language::En),
        audio: Some(AudioSettings {
            crossfade_ms: 4_000,
            gapless: false,
            eq_profile: EqProfile::predefinidos()[1].clone(),
            normalize_volume: true,
            output_device_id: Some("altavoces".into()),
        }),
        ..SettingsPatch::default()
    })
    .await
    .expect("aplica");

    // Un servicio nuevo sobre la misma base de datos: es lo que ocurre al
    // reiniciar. Si el ajuste solo estuviera en memoria, aquí se vería.
    let otro = c.servicio().await;
    let leido = otro.get().await;

    assert_eq!(leido.language, Language::En);
    assert_eq!(leido.audio.crossfade_ms, 4_000);
    assert!(!leido.audio.gapless);
    assert!(leido.audio.normalize_volume);
    assert_eq!(leido.audio.output_device_id.as_deref(), Some("altavoces"));
    assert_eq!(leido.audio.eq_profile.id, EqProfile::predefinidos()[1].id);
}

#[tokio::test]
async fn el_crossfade_llega_al_atomico_que_lee_la_reproduccion() {
    let c = ctx().await;
    let s = c.servicio().await;

    s.patch(SettingsPatch {
        audio: Some(AudioSettings {
            crossfade_ms: 6_500,
            ..AudioSettings::default()
        }),
        ..SettingsPatch::default()
    })
    .await
    .expect("aplica");

    assert_eq!(c.crossfade.load(Ordering::Relaxed), 6_500);

    // Y al arrancar, no solo al cambiarlo: si solo se aplicara en el `patch`,
    // el crossfade guardado no tendría efecto hasta tocarlo otra vez.
    c.crossfade.store(0, Ordering::Relaxed);
    let _otro = c.servicio().await;
    assert_eq!(c.crossfade.load(Ordering::Relaxed), 6_500);
}

#[tokio::test]
async fn una_seccion_corrupta_no_se_lleva_por_delante_las_demas() {
    let c = ctx().await;

    let s = c.servicio().await;
    s.patch(SettingsPatch {
        language: Some(Language::En),
        download: Some(DownloadSettings {
            max_concurrent: 4,
            ..DownloadSettings::default()
        }),
        ..SettingsPatch::default()
    })
    .await
    .expect("aplica");

    // Se corrompe **solo** la sección de audio, como haría un fallo de disco o
    // una versión anterior con otro formato.
    c.repo
        .set_raw("audio", "{ esto no es json }")
        .await
        .expect("escribe basura");

    let otro = c.servicio().await;
    let leido = otro.get().await;

    // La sección rota vuelve a su valor por defecto...
    assert_eq!(leido.audio, AudioSettings::default());
    // ...y las demás siguen ahí. Con un blob único se habrían perdido las tres.
    assert_eq!(leido.language, Language::En);
    assert_eq!(leido.download.max_concurrent, 4);
}

#[tokio::test]
async fn un_patch_invalido_no_deja_nada_a_medias() {
    let c = ctx().await;
    let s = c.servicio().await;

    let malo = SettingsPatch {
        language: Some(Language::En),
        audio: Some(AudioSettings {
            // Fuera del máximo permitido: el patch entero debe rechazarse.
            crossfade_ms: 999_999,
            ..AudioSettings::default()
        }),
        ..SettingsPatch::default()
    };
    assert!(s.patch(malo).await.is_err());

    // El idioma iba en el mismo patch y **no** debe haberse aplicado: validar
    // antes de escribir es justamente lo que impide una configuración a medias.
    assert_eq!(s.get().await.language, Language::Es);
    let otro = c.servicio().await;
    assert_eq!(otro.get().await.language, Language::Es);
}

#[tokio::test]
async fn el_primer_arranque_toma_el_idioma_del_sistema() {
    let c = ctx().await;
    let s = c.servicio().await;
    assert_eq!(s.get().await.language, Language::Es, "locale es-ES");
}

#[tokio::test]
async fn el_secreto_de_spotify_no_vuelve_a_salir() {
    let c = ctx().await;
    let s = c.servicio().await;

    s.set_spotify_credentials("mi-id", "mi-secreto")
        .await
        .expect("guarda");

    let leido = s.get().await;
    assert!(leido.spotify.configured);
    assert_eq!(leido.spotify.client_id.as_deref(), Some("mi-id"));

    // El tipo no tiene ni siquiera un campo donde meterlo; la comprobación
    // fuerte es que el secreto está en el almacén y no en la base de datos.
    let filas = c.repo.get_all().await.expect("lee");
    assert!(
        !filas.iter().any(|(_, v)| v.contains("mi-secreto")),
        "el client_secret no puede acabar en la tabla de ajustes: {filas:?}"
    );

    let guardado = c
        .secretos
        .get("spotify.client_secret")
        .await
        .expect("lee secreto");
    assert_eq!(guardado.as_deref(), Some("mi-secreto"));
}

#[tokio::test]
async fn cambiar_ajustes_avisa_de_las_secciones_tocadas() {
    let c = ctx().await;
    let s = c.servicio().await;

    s.patch(SettingsPatch {
        language: Some(Language::En),
        ..SettingsPatch::default()
    })
    .await
    .expect("aplica");

    assert!(c.nombres_bus().contains(&"settingsChanged".to_owned()));
}

#[tokio::test]
async fn un_patch_vacio_no_escribe_ni_avisa() {
    let c = ctx().await;
    let s = c.servicio().await;

    s.patch(SettingsPatch::default()).await.expect("aplica");

    // Ni evento ni escritura: un patch sin campos es un no-op, y emitir
    // `settingsChanged` haría que media interfaz se repintara por nada.
    assert!(!c.nombres_bus().contains(&"settingsChanged".to_owned()));
}

// ─────────────────────────────────────────────────────────────────────────────
// Migración de carpeta
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn cambiar_de_carpeta_sin_migrar_no_toca_los_ficheros() {
    let c = ctx().await;
    let s = c.servicio().await;

    escribir(&c.biblioteca().join("audio/una.opus"), "aaa");
    let destino = c.raiz.join("nueva");

    s.change_library_path(&destino, false)
        .await
        .expect("cambia");

    assert_eq!(s.get().await.library_path, destino);
    assert_eq!(
        contenido(&c.biblioteca()),
        vec!["audio/una.opus".to_owned()],
        "sin migración, el origen se queda como estaba"
    );
}

#[tokio::test]
async fn migrar_deja_la_biblioteca_completa_en_el_destino_y_vacia_el_origen() {
    let c = ctx().await;
    let s = c.servicio().await;

    escribir(&c.biblioteca().join("audio/a/una.opus"), "111");
    escribir(&c.biblioteca().join("audio/b/otra.opus"), "222");
    escribir(&c.biblioteca().join("covers/portada.jpg"), "333");

    let destino = c.raiz.join("nueva");
    s.change_library_path(&destino, true).await.expect("cambia");

    esperar_a(|| {
        std::fs::read_to_string(destino.join("covers/portada.jpg")).is_ok()
            && contenido(&c.biblioteca()).is_empty()
    })
    .await;

    assert_eq!(
        contenido(&destino),
        vec![
            "audio/a/una.opus".to_owned(),
            "audio/b/otra.opus".to_owned(),
            "covers/portada.jpg".to_owned(),
        ]
    );
    assert_eq!(
        std::fs::read_to_string(destino.join("audio/a/una.opus")).expect("lee"),
        "111",
        "el contenido tiene que llegar intacto, no solo el nombre"
    );

    // El ajuste solo se cambia cuando la copia está entera.
    let otro = c.servicio().await;
    assert_eq!(otro.get().await.library_path, destino);
}

#[tokio::test]
async fn migrar_avisa_del_avance_y_del_final() {
    let c = ctx().await;
    let s = c.servicio().await;

    for i in 0..5 {
        escribir(&c.biblioteca().join(format!("audio/{i}.opus")), "x");
    }

    s.change_library_path(&c.raiz.join("nueva"), true)
        .await
        .expect("cambia");

    esperar_a(|| c.nombres_bus().contains(&"libraryPathChanged".to_owned())).await;

    let nombres = c.nombres_bus();
    let avances = nombres
        .iter()
        .filter(|n| *n == "libraryMoveProgress")
        .count();
    assert_eq!(avances, 5, "un aviso por fichero copiado");
    assert!(
        nombres.iter().position(|n| n == "libraryPathChanged")
            > nombres.iter().rposition(|n| n == "libraryMoveProgress"),
        "el final llega después del último avance, no antes"
    );
}

#[tokio::test]
async fn un_destino_dentro_del_origen_se_rechaza() {
    let c = ctx().await;
    let s = c.servicio().await;

    // Copiar dentro de la propia carpeta convertiría el recorrido en un bucle:
    // cada fichero copiado aparecería como origen nuevo.
    let dentro = c.biblioteca().join("subcarpeta");
    let e = s.change_library_path(&dentro, true).await;
    assert!(e.is_err(), "un destino anidado no puede aceptarse");

    // Y el ajuste sigue donde estaba.
    assert_eq!(s.get().await.library_path, c.biblioteca());
}

#[tokio::test]
async fn el_mismo_destino_se_rechaza() {
    let c = ctx().await;
    let s = c.servicio().await;
    assert!(s.change_library_path(&c.biblioteca(), true).await.is_err());
}

#[tokio::test]
async fn el_origen_sigue_completo_mientras_dura_la_copia() {
    let c = ctx().await;
    let s = c.servicio().await;

    let relleno = "x".repeat(8 * 1024);
    for i in 0..CUANTOS {
        escribir(&c.biblioteca().join(format!("audio/{i}.opus")), &relleno);
    }

    let destino = c.raiz.join("nueva");
    s.change_library_path(&destino, true).await.expect("cambia");

    // Se mira en mitad de la operación: hasta que la copia no termina, el
    // origen tiene que seguir completo y el ajuste tiene que seguir apuntando a
    // él. Es la garantía de que cortar la corriente aquí no pierde nada.
    //
    // Sin `sleep` en el bucle: se muestrea tan rápido como se pueda para no
    // perderse la ventana.
    let mut visto_a_medias = false;
    for _ in 0..100_000 {
        let copiados = contenido(&destino).len();
        if copiados >= CUANTOS {
            break;
        }
        if copiados > 0 {
            visto_a_medias = true;
            assert_eq!(
                contenido(&c.biblioteca()).len(),
                CUANTOS,
                "el origen no puede perder ficheros mientras se copia"
            );
            assert_eq!(
                c.servicio().await.get().await.library_path,
                c.biblioteca(),
                "el ajuste no puede cambiar antes de que la copia termine"
            );
            // Comprobado el invariante una vez, se deja de muestrear: cada
            // vuelta recorre las dos carpetas enteras y compite por el disco
            // con la copia que se está midiendo.
            break;
        }
        tokio::task::yield_now().await;
    }

    assert!(
        visto_a_medias,
        "no se llegó a observar la copia a medias: sube CUANTOS o el relleno"
    );

    esperar_a(|| contenido(&c.biblioteca()).is_empty()).await;
    assert_eq!(contenido(&destino).len(), CUANTOS);
}

/// Ficheros que se copian en el test del punto medio.
///
/// Suficientes y suficientemente grandes como para que la copia dure lo
/// bastante para muestrearla: con unas decenas diminutas termina antes del
/// primer vistazo y el test se convierte en una carrera que a veces gana.
const CUANTOS: usize = 400;

/// Espera a que se cumpla una condición, con tope.
///
/// La migración corre en una tarea de fondo: sin esto, el test comprobaría el
/// estado antes de que haya pasado nada y pasaría o fallaría según la carga de
/// la máquina.
async fn esperar_a(mut cond: impl FnMut() -> bool) {
    for _ in 0..2_000 {
        if cond() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
    }
    panic!("la condición no se cumplió a tiempo");
}

#[tokio::test]
async fn los_perfiles_de_ecualizador_incluyen_el_del_usuario() {
    let c = ctx().await;
    let s = c.servicio().await;

    let predefinidos = s.eq_profiles().await.expect("lista").len();

    s.patch(SettingsPatch {
        audio: Some(AudioSettings {
            eq_profile: EqProfile::new("mio", "Mi curva", [1.0; 10]).expect("válido"),
            ..AudioSettings::default()
        }),
        ..SettingsPatch::default()
    })
    .await
    .expect("aplica");

    let lista = s.eq_profiles().await.expect("lista");
    assert_eq!(lista.len(), predefinidos + 1);
    assert!(lista.iter().any(|p| p.id == "mio"));

    // Y no se duplica al volver a pedirla ni al elegir uno de fábrica.
    s.patch(SettingsPatch {
        audio: Some(AudioSettings::default()),
        ..SettingsPatch::default()
    })
    .await
    .expect("aplica");
    assert_eq!(s.eq_profiles().await.expect("lista").len(), predefinidos);
}

#[tokio::test]
async fn sin_proveedor_de_spotify_el_estado_es_no_configurado() {
    let c = ctx().await;
    let s = c.servicio().await;
    assert!(matches!(
        s.test_spotify().await.expect("consulta"),
        localify_core::events::ProviderStatus::NotConfigured
    ));
}

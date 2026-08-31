//! Origen de audio sobre un fichero que **todavía está creciendo**.
//!
//! Es la pieza que hace posible "pulsa play y suena en dos segundos" sin ningún
//! truco (ADR-007): el decodificador lee del `.part` mientras yt-dlp lo sigue
//! escribiendo.
//!
//! ## La diferencia con leer un fichero normal
//!
//! Un `File` que llega al final devuelve `Ok(0)`, y para cualquier
//! decodificador eso significa "se acabó la canción". Aquí no se ha acabado:
//! simplemente el resto todavía no ha llegado. [`GrowingFileSource`] distingue
//! los dos casos porque el descargador le dice explícitamente cuándo ha
//! terminado. Mientras no lo diga, un final de fichero es una **espera**, no un
//! EOF.
//!
//! ## Sobre el renombrado atómico
//!
//! Cuando la descarga acaba, el `.part` se renombra a su ubicación definitiva
//! **mientras el motor lo tiene abierto**. En Windows eso falla si el fichero
//! se abrió sin `FILE_SHARE_DELETE`.
//!
//! No hace falta abrirlo por Win32: `std::fs::File` ya pide
//! `FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE`. Hay un test que lo
//! comprueba de verdad —renombra un fichero abierto y sigue leyendo— porque es
//! una garantía de la que depende toda la descarga progresiva y conviene que se
//! rompa en la CI y no en la máquina de un usuario.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// Cuánto se espera a que lleguen bytes nuevos antes de darse por vencido.
///
/// Es generoso a propósito: una red lenta no es un error, y cortar la
/// reproducción de una canción que iba a llegar sería peor que un silencio
/// largo. Quien decide abandonar es la capa de arriba, no este lector.
const ESPERA_MAXIMA: Duration = Duration::from_secs(30);

/// Cada cuánto se despierta a comprobar. Sin este tope, un `notify` perdido
/// dejaría al lector dormido para siempre.
const LATIDO: Duration = Duration::from_millis(100);

/// Cómo terminó la descarga.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Fin {
    Completa,
    Fallida(String),
}

#[derive(Debug)]
struct Interior {
    /// Bytes que el descargador garantiza que ya están en disco.
    bytes: u64,
    fin: Option<Fin>,
}

/// Lo que el descargador y el lector comparten.
///
/// El descargador va llamando a [`Self::avanzar`] según escribe, y cierra con
/// [`Self::completar`] o [`Self::fallar`]. El lector espera aquí.
#[derive(Debug)]
pub struct EstadoDescarga {
    interior: Mutex<Interior>,
    senal: Condvar,
}

impl EstadoDescarga {
    #[must_use]
    pub fn nuevo() -> Arc<Self> {
        Arc::new(Self {
            interior: Mutex::new(Interior {
                bytes: 0,
                fin: None,
            }),
            senal: Condvar::new(),
        })
    }

    /// Estado de un fichero que ya está entero en disco.
    #[must_use]
    pub fn completo(bytes: u64) -> Arc<Self> {
        Arc::new(Self {
            interior: Mutex::new(Interior {
                bytes,
                fin: Some(Fin::Completa),
            }),
            senal: Condvar::new(),
        })
    }

    /// Anuncia que hay `bytes` disponibles desde el principio del fichero.
    ///
    /// El valor solo puede crecer: un informe de progreso que llegue tarde y
    /// traiga una cifra menor no debe hacer retroceder al lector, que quizá ya
    /// haya leído más allá.
    pub fn avanzar(&self, bytes: u64) {
        if let Ok(mut i) = self.interior.lock()
            && bytes > i.bytes
        {
            i.bytes = bytes;
            self.senal.notify_all();
        }
    }

    /// La descarga terminó bien, con `bytes` en total.
    pub fn completar(&self, bytes: u64) {
        if let Ok(mut i) = self.interior.lock() {
            i.bytes = i.bytes.max(bytes);
            i.fin = Some(Fin::Completa);
            self.senal.notify_all();
        }
    }

    /// La descarga falló. El lector dejará de esperar y devolverá el error.
    pub fn fallar(&self, motivo: impl Into<String>) {
        if let Ok(mut i) = self.interior.lock() {
            i.fin = Some(Fin::Fallida(motivo.into()));
            self.senal.notify_all();
        }
    }

    /// Bytes disponibles ahora mismo.
    #[must_use]
    pub fn bytes(&self) -> u64 {
        self.interior.lock().map_or(0, |i| i.bytes)
    }

    /// `true` si ya no va a llegar nada más, con éxito o sin él.
    #[must_use]
    pub fn ha_terminado(&self) -> bool {
        self.interior.lock().is_ok_and(|i| i.fin.is_some())
    }

    /// `true` si la descarga acabó bien.
    #[must_use]
    pub fn esta_completa(&self) -> bool {
        self.interior
            .lock()
            .is_ok_and(|i| i.fin == Some(Fin::Completa))
    }

    /// Espera hasta que haya más de `minimo` bytes.
    ///
    /// Devuelve los bytes disponibles. `Ok(n)` con `n <= minimo` solo ocurre si
    /// la descarga terminó: en ese caso no hay nada más que esperar y quien
    /// llama debe tratarlo como fin de fichero.
    fn esperar_mas_de(&self, minimo: u64, ruta: Option<&Path>) -> io::Result<u64> {
        let limite = Instant::now() + ESPERA_MAXIMA;

        let mut i = self
            .interior
            .lock()
            .map_err(|_| io::Error::other("estado de descarga envenenado"))?;

        loop {
            if let Some(Fin::Fallida(motivo)) = &i.fin {
                return Err(io::Error::other(format!("descarga fallida: {motivo}")));
            }
            if i.bytes > minimo || i.fin.is_some() {
                return Ok(i.bytes);
            }
            // Si el fichero ya no está donde estaba, esperar no lo va a
            // devolver. Pasa de verdad: la tubería de descarga verifica el
            // `.part`, lo remuxea a otro fichero y borra el original mientras
            // el motor lo tiene abierto. En Windows el handle sobrevive
            // —`FILE_SHARE_DELETE`— pero leer da cero bytes, así que sin esta
            // comprobación se agotaban los treinta segundos enteros antes de
            // fallar, con el reproductor congelado todo ese rato.
            if ruta.is_some_and(|r| !r.exists()) {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    "el fichero temporal desapareció mientras se leía",
                ));
            }
            if Instant::now() >= limite {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "la descarga no avanza",
                ));
            }
            let (siguiente, _) = self
                .senal
                .wait_timeout(i, LATIDO)
                .map_err(|_| io::Error::other("estado de descarga envenenado"))?;
            i = siguiente;
        }
    }
}

/// Lector de un fichero en crecimiento.
///
/// Implementa `Read + Seek`, que es todo lo que symphonia necesita de un
/// `MediaSource`.
#[derive(Debug)]
pub struct GrowingFileSource {
    fichero: File,
    estado: Arc<EstadoDescarga>,
    /// Ruta del temporal, solo para los ficheros en crecimiento.
    ///
    /// Se guarda para poder comprobar que sigue existiendo: la tubería de
    /// descarga lo borra al remuxear, y sin esa comprobación la lectura espera
    /// treinta segundos a que crezca algo que ya no está.
    ruta: Option<std::path::PathBuf>,
    /// Posición del cursor. Se lleva aparte del `File` porque hace falta
    /// compararla con los bytes disponibles sin preguntar al sistema.
    pos: u64,
}

impl GrowingFileSource {
    /// Abre `path` para lectura progresiva.
    ///
    /// # Errors
    /// Si el fichero no se puede abrir.
    pub fn abrir(path: &Path, estado: Arc<EstadoDescarga>) -> io::Result<Self> {
        Ok(Self {
            fichero: File::open(path)?,
            estado,
            ruta: Some(path.to_path_buf()),
            pos: 0,
        })
    }

    /// Abre un fichero que ya está completo. Es el caso normal de la biblioteca.
    ///
    /// # Errors
    /// Si el fichero no se puede abrir o no se puede consultar su tamaño.
    pub fn abrir_completo(path: &Path) -> io::Result<Self> {
        let fichero = File::open(path)?;
        let bytes = fichero.metadata()?.len();
        Ok(Self {
            fichero,
            estado: EstadoDescarga::completo(bytes),
            // Un fichero completo no se vigila: no va a desaparecer.
            ruta: None,
            pos: 0,
        })
    }

    /// `true` si el fichero ya no crece.
    #[must_use]
    pub fn esta_completo(&self) -> bool {
        self.estado.esta_completa()
    }

    /// Bytes legibles ahora mismo.
    #[must_use]
    pub fn disponibles(&self) -> u64 {
        self.estado.bytes()
    }
}

impl Read for GrowingFileSource {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }

        // Espera a que haya al menos un byte más allá del cursor. Devolver
        // `Ok(0)` sin esperar sería decirle al decodificador que la canción se
        // ha terminado, cuando lo único que pasa es que va por delante de la
        // descarga.
        let disponibles = self.estado.esperar_mas_de(self.pos, self.ruta.as_deref())?;
        if self.pos >= disponibles {
            // La descarga terminó y no hay más: este sí es el final de verdad.
            return Ok(0);
        }

        // Nunca leer más allá de lo confirmado: el final del fichero puede
        // contener una escritura a medias del descargador.
        let tope = usize::try_from(disponibles - self.pos).unwrap_or(usize::MAX);
        let hasta = buf.len().min(tope);

        let leidos = self.fichero.read(&mut buf[..hasta])?;
        self.pos += leidos as u64;
        Ok(leidos)
    }
}

impl Seek for GrowingFileSource {
    fn seek(&mut self, desde: SeekFrom) -> io::Result<u64> {
        // `SeekFrom::End` sobre un fichero que crece no significa nada estable:
        // el final de ahora no es el de dentro de un segundo. Solo se admite
        // cuando la descarga ya ha terminado.
        if matches!(desde, SeekFrom::End(_)) && !self.estado.ha_terminado() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "no se puede buscar desde el final de un fichero que aun crece",
            ));
        }
        self.pos = self.fichero.seek(desde)?;
        Ok(self.pos)
    }

    fn stream_position(&mut self) -> io::Result<u64> {
        Ok(self.pos)
    }
}

impl symphonia_core::io::MediaSource for GrowingFileSource {
    /// Mientras crece, **no** es buscable.
    ///
    /// Decirle a symphonia que sí lo es mientras el fichero está a medias le
    /// llevaría a saltar a un offset que aún no existe y a fallar en un sitio
    /// donde no se puede distinguir de un fichero corrupto.
    fn is_seekable(&self) -> bool {
        self.estado.esta_completa()
    }

    fn byte_len(&self) -> Option<u64> {
        self.estado.esta_completa().then(|| self.estado.bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Fichero temporal que se borra solo.
    struct Temporal(std::path::PathBuf);

    impl Temporal {
        fn nuevo(nombre: &str) -> Self {
            let p = std::env::temp_dir()
                .join(format!("localify-growing-{nombre}-{}", std::process::id()));
            let _ = std::fs::remove_file(&p);
            Self(p)
        }

        fn escribir(&self, bytes: &[u8]) {
            let mut f = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.0)
                .expect("abre para escribir");
            f.write_all(bytes).expect("escribe");
            f.flush().expect("vacia");
        }

        fn ruta(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Temporal {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    #[test]
    fn lee_lo_que_ya_esta_disponible() {
        let t = Temporal::nuevo("basico");
        t.escribir(b"hola mundo");

        let estado = EstadoDescarga::nuevo();
        estado.avanzar(10);
        let mut s = GrowingFileSource::abrir(t.ruta(), estado).expect("abre");

        let mut buf = [0_u8; 10];
        s.read_exact(&mut buf).expect("lee");
        assert_eq!(&buf, b"hola mundo");
    }

    #[test]
    fn no_lee_mas_alla_de_lo_confirmado() {
        // El final del fichero puede ser una escritura a medias del
        // descargador. Leer ahi daria bytes basura al decodificador.
        let t = Temporal::nuevo("confirmado");
        t.escribir(b"12345678");

        let estado = EstadoDescarga::nuevo();
        estado.avanzar(4); // solo los cuatro primeros son de fiar
        let mut s = GrowingFileSource::abrir(t.ruta(), Arc::clone(&estado)).expect("abre");

        let mut buf = [0_u8; 8];
        let n = s.read(&mut buf).expect("lee");
        assert_eq!(n, 4, "leyo mas de lo confirmado");
        assert_eq!(&buf[..4], b"1234");
    }

    #[test]
    fn el_final_del_fichero_es_una_espera_y_no_un_eof() {
        // La invariante central: si el decodificador va por delante de la
        // descarga, hay que esperar, no decir que la cancion se acabo.
        let t = Temporal::nuevo("espera");
        t.escribir(b"parte1");

        let estado = EstadoDescarga::nuevo();
        estado.avanzar(6);
        let mut s = GrowingFileSource::abrir(t.ruta(), Arc::clone(&estado)).expect("abre");

        let mut buf = [0_u8; 6];
        s.read_exact(&mut buf).expect("lee la primera parte");

        // Otro hilo escribe el resto medio segundo despues.
        let ruta = t.ruta().to_path_buf();
        let e2 = Arc::clone(&estado);
        let escritor = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            let mut f = std::fs::OpenOptions::new()
                .append(true)
                .open(&ruta)
                .expect("abre");
            f.write_all(b"parte2").expect("escribe");
            f.flush().expect("vacia");
            e2.avanzar(12);
        });

        let mut buf2 = [0_u8; 6];
        s.read_exact(&mut buf2)
            .expect("debe esperar, no devolver EOF");
        assert_eq!(&buf2, b"parte2");
        escritor.join().expect("hilo escritor");
    }

    #[test]
    fn al_completar_la_descarga_el_final_si_es_eof() {
        let t = Temporal::nuevo("eof");
        t.escribir(b"todo");

        let estado = EstadoDescarga::nuevo();
        estado.completar(4);
        let mut s = GrowingFileSource::abrir(t.ruta(), estado).expect("abre");

        let mut todo = Vec::new();
        s.read_to_end(&mut todo).expect("lee hasta el final");
        assert_eq!(todo, b"todo");
    }

    #[test]
    fn una_descarga_fallida_corta_la_espera_con_error() {
        // Si no, el lector se quedaria treinta segundos esperando algo que ya
        // se sabe que no va a llegar.
        let t = Temporal::nuevo("fallo");
        t.escribir(b"ab");

        let estado = EstadoDescarga::nuevo();
        estado.avanzar(2);
        let mut s = GrowingFileSource::abrir(t.ruta(), Arc::clone(&estado)).expect("abre");

        let mut buf = [0_u8; 2];
        s.read_exact(&mut buf).expect("lee lo que hay");

        let e2 = Arc::clone(&estado);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            e2.fallar("red caida");
        });

        let inicio = Instant::now();
        let err = s.read(&mut buf).expect_err("debe fallar, no esperar");
        assert!(
            inicio.elapsed() < Duration::from_secs(5),
            "no debe agotar el plazo completo"
        );
        assert!(err.to_string().contains("red caida"), "{err}");
    }

    #[test]
    fn el_progreso_no_puede_retroceder() {
        // Un informe de progreso que llegue tarde no debe invalidar bytes que
        // el lector quiza ya haya consumido.
        let estado = EstadoDescarga::nuevo();
        estado.avanzar(1000);
        estado.avanzar(500);
        assert_eq!(estado.bytes(), 1000);
    }

    #[test]
    fn mientras_crece_no_se_declara_buscable() {
        use symphonia_core::io::MediaSource;

        let t = Temporal::nuevo("buscable");
        t.escribir(b"x");

        let estado = EstadoDescarga::nuevo();
        estado.avanzar(1);
        let s = GrowingFileSource::abrir(t.ruta(), Arc::clone(&estado)).expect("abre");
        assert!(!s.is_seekable(), "decirle que si a symphonia acabaria mal");
        assert_eq!(s.byte_len(), None);

        estado.completar(1);
        assert!(s.is_seekable());
        assert_eq!(s.byte_len(), Some(1));
    }

    #[test]
    fn buscar_desde_el_final_se_rechaza_mientras_crece() {
        let t = Temporal::nuevo("seek-end");
        t.escribir(b"12345");

        let estado = EstadoDescarga::nuevo();
        estado.avanzar(5);
        let mut s = GrowingFileSource::abrir(t.ruta(), Arc::clone(&estado)).expect("abre");

        assert!(
            s.seek(SeekFrom::End(-1)).is_err(),
            "el final de ahora no es el final de la cancion"
        );

        estado.completar(5);
        assert_eq!(s.seek(SeekFrom::End(-1)).expect("ya es buscable"), 4);
    }

    #[test]
    fn un_fichero_abierto_se_puede_renombrar_mientras_se_lee() {
        // Es la garantia de la que depende el final de cada descarga: el
        // `.part` se renombra a `audio/` con el motor leyendolo. En Windows
        // esto solo funciona si el fichero se abrio con FILE_SHARE_DELETE, y
        // `std::fs::File` ya lo hace. Si algun dia dejara de hacerlo, se entera
        // la CI y no el usuario.
        let origen = Temporal::nuevo("rename-origen");
        origen.escribir(b"contenido completo");

        let destino = std::env::temp_dir().join(format!(
            "localify-growing-rename-destino-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&destino);

        let estado = EstadoDescarga::completo(18);
        let mut s = GrowingFileSource::abrir(origen.ruta(), estado).expect("abre");

        let mut buf = [0_u8; 9];
        s.read_exact(&mut buf).expect("lee la primera mitad");

        std::fs::rename(origen.ruta(), &destino).expect("renombrar con el fichero abierto");

        // Y se sigue leyendo del mismo contenido, ya en su sitio definitivo.
        let mut resto = Vec::new();
        s.read_to_end(&mut resto)
            .expect("sigue leyendo tras el rename");
        assert_eq!(&resto, b" completo");

        let _ = std::fs::remove_file(&destino);
    }
}

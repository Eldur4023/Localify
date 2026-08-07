//! Invocación de los binarios externos.
//!
//! Va detrás de un trait por el mismo motivo que el transporte HTTP de Spotify:
//! **la suite no debe depender de tener yt-dlp instalado**. Con un ejecutor
//! falso alimentado por salidas grabadas se prueban el parseo, la clasificación
//! de errores y el seguimiento del progreso de forma determinista.

use std::path::Path;
use std::process::Stdio;

use async_trait::async_trait;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

use crate::error::{YtDlpError, YtDlpResult};

/// Resultado de ejecutar un proceso hasta el final.
#[derive(Debug, Clone)]
pub struct Salida {
    pub codigo: i32,
    pub stdout: String,
    pub stderr: String,
}

impl Salida {
    #[must_use]
    pub const fn es_ok(&self) -> bool {
        self.codigo == 0
    }
}

/// Recibe cada línea de la salida conforme aparece.
///
/// Es lo que permite emitir progreso durante la descarga en lugar de esperar a
/// que termine.
pub trait ObservadorLineas: Send + Sync {
    fn linea(&self, texto: &str);
}

#[async_trait]
pub trait Ejecutor: Send + Sync + 'static {
    /// Ejecuta y espera al final, capturando la salida completa.
    async fn ejecutar(&self, binario: &'static str, args: &[String]) -> YtDlpResult<Salida>;

    /// Ejecuta informando de cada línea de `stdout` según llega.
    async fn ejecutar_con_progreso(
        &self,
        binario: &'static str,
        args: &[String],
        observador: &dyn ObservadorLineas,
    ) -> YtDlpResult<Salida>;
}

/// Ejecutor real sobre procesos del sistema.
#[derive(Debug, Clone)]
pub struct EjecutorReal {
    /// Carpeta donde buscar los binarios antes que en el `PATH`.
    binarios: std::path::PathBuf,
}

impl EjecutorReal {
    #[must_use]
    pub fn nuevo(binarios: std::path::PathBuf) -> Self {
        Self { binarios }
    }

    /// Localiza el binario, o falla con un error accionable.
    fn ruta(&self, binario: &'static str) -> YtDlpResult<std::path::PathBuf> {
        let nombre = if cfg!(windows) {
            format!("{binario}.exe")
        } else {
            binario.to_owned()
        };

        // Lo propio antes que lo del sistema: una versión del sistema
        // desactualizada no debe tener prioridad sobre la que gestionamos.
        let propio = self.binarios.join(&nombre);
        if propio.is_file() {
            return Ok(propio);
        }

        std::env::var_os("PATH")
            .and_then(|path| {
                std::env::split_paths(&path)
                    .map(|d| d.join(&nombre))
                    .find(|p| p.is_file())
            })
            .ok_or(YtDlpError::SinBinario(binario))
    }

    fn comando(&self, binario: &'static str, args: &[String]) -> YtDlpResult<Command> {
        let mut cmd = Command::new(self.ruta(binario)?);
        cmd.args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // Si la aplicación muere, el proceso hijo no debe quedarse
            // descargando en segundo plano para siempre.
            .kill_on_drop(true);

        #[cfg(windows)]
        {
            // Sin esto, cada invocación abre una consola negra durante un
            // instante. `tokio::process::Command` expone `creation_flags`
            // directamente en Windows.
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }

        Ok(cmd)
    }
}

#[async_trait]
impl Ejecutor for EjecutorReal {
    async fn ejecutar(&self, binario: &'static str, args: &[String]) -> YtDlpResult<Salida> {
        let salida = self.comando(binario, args)?.output().await?;

        Ok(Salida {
            codigo: salida.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&salida.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&salida.stderr).into_owned(),
        })
    }

    async fn ejecutar_con_progreso(
        &self,
        binario: &'static str,
        args: &[String],
        observador: &dyn ObservadorLineas,
    ) -> YtDlpResult<Salida> {
        let mut hijo = self.comando(binario, args)?.spawn()?;

        let stdout = hijo.stdout.take().ok_or_else(|| YtDlpError::Salida {
            binario,
            detalle: "no se pudo capturar stdout".into(),
        })?;
        let stderr = hijo.stderr.take();

        let mut lineas = BufReader::new(stdout).lines();
        let mut acumulado = String::new();

        while let Some(linea) = lineas.next_line().await? {
            observador.linea(&linea);
            acumulado.push_str(&linea);
            acumulado.push('\n');
        }

        let estado = hijo.wait().await?;

        // El stderr se lee al final: yt-dlp escribe ahí poco y solo importa
        // para diagnosticar un fallo.
        let error = match stderr {
            Some(s) => {
                let mut texto = String::new();
                let mut lineas = BufReader::new(s).lines();
                while let Ok(Some(l)) = lineas.next_line().await {
                    texto.push_str(&l);
                    texto.push('\n');
                }
                texto
            }
            None => String::new(),
        };

        Ok(Salida {
            codigo: estado.code().unwrap_or(-1),
            stdout: acumulado,
            stderr: error,
        })
    }
}

/// Clasifica el fallo de un proceso.
///
/// yt-dlp usa el mismo código de salida para todo, así que hay que mirar el
/// mensaje. Distinguir "el vídeo no existe" de "yt-dlp ya no entiende YouTube"
/// importa: el primero exige buscar otro candidato y el segundo, actualizar el
/// binario.
#[must_use]
pub fn clasificar(binario: &'static str, salida: &Salida) -> YtDlpError {
    let texto = format!("{} {}", salida.stdout, salida.stderr).to_lowercase();

    if texto.contains("video unavailable")
        || texto.contains("private video")
        || texto.contains("has been removed")
        || texto.contains("is not available")
        || texto.contains("no video formats found")
    {
        return YtDlpError::VideoNoDisponible;
    }

    if texto.contains("unable to extract")
        || texto.contains("signature extraction failed")
        || texto.contains("please report this issue")
        || texto.contains("nsig extraction failed")
        || texto.contains("update to the latest version")
    {
        return YtDlpError::ExtractorObsoleto;
    }

    YtDlpError::Proceso {
        binario,
        codigo: salida.codigo,
        // El mensaje puede ser larguísimo: se recorta para el log.
        detalle: salida
            .stderr
            .lines()
            .last()
            .unwrap_or("")
            .chars()
            .take(200)
            .collect(),
    }
}

/// Comprueba la ruta de un fichero de forma asíncrona.
pub async fn existe(ruta: &Path) -> bool {
    tokio::fs::try_exists(ruta).await.unwrap_or(false)
}

/// Ejecutor programable para tests.
pub mod falso {
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    use super::{Ejecutor, ObservadorLineas, Salida, YtDlpResult, async_trait};

    #[derive(Debug, Clone, Default)]
    pub struct EjecutorFalso {
        salidas: Arc<Mutex<VecDeque<Salida>>>,
        pub invocaciones: Arc<Mutex<Vec<(String, Vec<String>)>>>,
    }

    impl EjecutorFalso {
        #[must_use]
        pub fn nuevo() -> Self {
            Self::default()
        }

        /// Encola una salida correcta.
        #[must_use]
        pub fn con_stdout(self, stdout: &str) -> Self {
            self.encolar(Salida {
                codigo: 0,
                stdout: stdout.to_owned(),
                stderr: String::new(),
            })
        }

        /// Encola un fallo.
        #[must_use]
        pub fn con_error(self, codigo: i32, stderr: &str) -> Self {
            self.encolar(Salida {
                codigo,
                stdout: String::new(),
                stderr: stderr.to_owned(),
            })
        }

        #[must_use]
        pub fn encolar(self, s: Salida) -> Self {
            if let Ok(mut cola) = self.salidas.lock() {
                cola.push_back(s);
            }
            self
        }

        #[must_use]
        pub fn cuantas(&self) -> usize {
            self.invocaciones.lock().map_or(0, |i| i.len())
        }

        #[must_use]
        pub fn args_de(&self, indice: usize) -> Vec<String> {
            self.invocaciones
                .lock()
                .ok()
                .and_then(|i| i.get(indice).map(|(_, a)| a.clone()))
                .unwrap_or_default()
        }

        fn siguiente(&self, binario: &str, args: &[String]) -> YtDlpResult<Salida> {
            if let Ok(mut reg) = self.invocaciones.lock() {
                reg.push((binario.to_owned(), args.to_vec()));
            }
            self.salidas
                .lock()
                .ok()
                .and_then(|mut c| c.pop_front())
                .ok_or_else(|| super::YtDlpError::Salida {
                    binario: "falso",
                    detalle: "el ejecutor falso se quedó sin salidas preparadas".into(),
                })
        }
    }

    #[async_trait]
    impl Ejecutor for EjecutorFalso {
        async fn ejecutar(&self, binario: &'static str, args: &[String]) -> YtDlpResult<Salida> {
            self.siguiente(binario, args)
        }

        async fn ejecutar_con_progreso(
            &self,
            binario: &'static str,
            args: &[String],
            observador: &dyn ObservadorLineas,
        ) -> YtDlpResult<Salida> {
            let salida = self.siguiente(binario, args)?;
            for linea in salida.stdout.lines() {
                observador.linea(linea);
            }
            Ok(salida)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn salida(stderr: &str) -> Salida {
        Salida {
            codigo: 1,
            stdout: String::new(),
            stderr: stderr.to_owned(),
        }
    }

    #[test]
    fn un_video_retirado_se_reconoce() {
        for mensaje in [
            "ERROR: Video unavailable",
            "ERROR: Private video. Sign in if you've been granted access",
            "ERROR: This video has been removed by the uploader",
            "ERROR: No video formats found!",
        ] {
            assert!(
                matches!(
                    clasificar("yt-dlp", &salida(mensaje)),
                    YtDlpError::VideoNoDisponible
                ),
                "'{mensaje}'"
            );
        }
    }

    #[test]
    fn un_extractor_roto_se_distingue_de_un_video_ausente() {
        // Es la diferencia entre buscar otro candidato y actualizar el binario.
        for mensaje in [
            "ERROR: unable to extract player response",
            "WARNING: nsig extraction failed: Some formats may be missing",
            "ERROR: Signature extraction failed",
            "Please report this issue on https://github.com/yt-dlp/yt-dlp/issues",
        ] {
            assert!(
                matches!(
                    clasificar("yt-dlp", &salida(mensaje)),
                    YtDlpError::ExtractorObsoleto
                ),
                "'{mensaje}'"
            );
        }
    }

    #[test]
    fn un_fallo_desconocido_conserva_el_codigo_y_recorta_el_detalle() {
        let largo = "x".repeat(1000);
        match clasificar("yt-dlp", &salida(&largo)) {
            YtDlpError::Proceso {
                codigo, detalle, ..
            } => {
                assert_eq!(codigo, 1);
                assert!(
                    detalle.len() <= 200,
                    "el detalle debe recortarse para el log"
                );
            }
            otro => panic!("se esperaba Proceso, llegó {otro:?}"),
        }
    }

    #[test]
    fn una_salida_correcta_se_reconoce_como_tal() {
        let s = Salida {
            codigo: 0,
            stdout: "todo bien".into(),
            stderr: String::new(),
        };
        assert!(s.es_ok());
        assert!(!salida("error").es_ok());
    }

    #[tokio::test]
    async fn el_ejecutor_falso_registra_lo_invocado() {
        use falso::EjecutorFalso;

        let e = EjecutorFalso::nuevo().con_stdout("{}");
        let args = vec!["--dump-json".to_owned(), "abc".to_owned()];
        e.ejecutar("yt-dlp", &args).await.expect("ejecuta");

        assert_eq!(e.cuantas(), 1);
        assert_eq!(e.args_de(0), args);
    }

    #[tokio::test]
    async fn el_observador_recibe_cada_linea() {
        use std::sync::Mutex;

        use falso::EjecutorFalso;

        struct Acumulador(Mutex<Vec<String>>);
        impl ObservadorLineas for Acumulador {
            fn linea(&self, texto: &str) {
                if let Ok(mut v) = self.0.lock() {
                    v.push(texto.to_owned());
                }
            }
        }

        let e = EjecutorFalso::nuevo().con_stdout("uno\ndos\ntres");
        let acc = Acumulador(Mutex::new(Vec::new()));
        e.ejecutar_con_progreso("yt-dlp", &[], &acc)
            .await
            .expect("ejecuta");

        assert_eq!(
            acc.0.lock().expect("lock").clone(),
            vec!["uno", "dos", "tres"]
        );
    }

    #[test]
    fn un_binario_inexistente_da_un_error_accionable() {
        let e = EjecutorReal::nuevo(std::path::PathBuf::from(r"C:\no\existe"));
        let error = e
            .ruta("binario-que-no-existe-jamas")
            .expect_err("debe fallar");
        assert!(matches!(error, YtDlpError::SinBinario(_)));
    }
}

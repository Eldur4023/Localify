//! Búsqueda y descarga a través de yt-dlp.
//!
//! ## El fichero temporal
//!
//! Todo se descarga a `.tmp/<id>.<ext>.part`. Es lo que hace posible reproducir
//! mientras baja, y también lo que garantiza que en `audio/` nunca aparezca un
//! fichero incompleto: la carpeta temporal se purga al arrancar, y un `.part`
//! huérfano no se confunde jamás con biblioteca.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use localify_core::domain::audio::DurationMs;
use localify_core::domain::download::BYTES_MINIMOS_REPRODUCIBLE;
use localify_core::domain::settings::{CookieSource, CookiesVigentes, FormatPreference};
use serde::Deserialize;
use tracing::{debug, warn};

use crate::error::YtDlpResult;
use crate::proceso::{Ejecutor, ObservadorLineas, clasificar};
use crate::search::{Consulta, RESULTADOS_POR_CONSULTA, RawCandidate};
use crate::{formats, rules_de_consulta};

const BINARIO: &str = "yt-dlp";

/// Plantilla de progreso.
///
/// Se pide en una sola línea con separadores propios en lugar del formato
/// legible: parsear la barra de progreso de yt-dlp sería frágil, y este formato
/// no cambia entre versiones.
const PLANTILLA_PROGRESO: &str = "LOCALIFY|%(progress.downloaded_bytes)s|%(progress.total_bytes)s|%(progress.total_bytes_estimate)s";

/// Entrada de la salida JSON de yt-dlp.
///
/// Solo se declaran los campos que se usan: yt-dlp emite decenas, y exigirlos
/// todos convertiría cualquier cambio suyo en un fallo nuestro.
#[derive(Debug, Deserialize)]
struct EntradaJson {
    id: Option<String>,
    title: Option<String>,
    /// Canal y subidor son **dos campos distintos**, no dos nombres del mismo.
    ///
    /// Estaban unidos con `#[serde(alias = "uploader", alias = "channel")]` en
    /// un solo campo, y eso convertía cada resultado de yt-dlp en un error de
    /// deserialización —`duplicate field 'channel'`— porque yt-dlp emite los
    /// dos. Con el error tragado por un `.ok()`, el síntoma era que **ninguna
    /// búsqueda encontraba nada** y la descarga fallaba con "sin coincidencia",
    /// que es exactamente lo que se ve cuando YouTube no tiene la canción.
    ///
    /// Se prefiere `channel`, que es el nombre del canal tal cual; `uploader`
    /// es su versión para mostrar y a veces viene vacío.
    channel: Option<String>,
    uploader: Option<String>,
    description: Option<String>,
    /// Segundos. yt-dlp lo omite en algunos resultados de búsqueda.
    duration: Option<f64>,
    view_count: Option<u64>,
}

impl EntradaJson {
    fn a_candidato(self, desde_music: bool) -> Option<RawCandidate> {
        let id = self.id?;
        // Sin duración no se puede validar la coincidencia, y la duración es la
        // señal más fiable que tenemos: mejor descartarlo que puntuarlo a ciegas.
        let segundos = self.duration?;
        if segundos <= 0.0 {
            return None;
        }

        Some(RawCandidate {
            video_id: id,
            title: self.title.unwrap_or_default(),
            provided_to_youtube: rules_de_consulta::detectar_provided(self.description.as_deref()),
            description: self.description,
            channel: self.channel.or(self.uploader),
            #[allow(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "una duración positiva en segundos cabe de sobra en u32 ms"
            )]
            duration: DurationMs::new((segundos * 1000.0) as u32),
            view_count: self.view_count,
            from_youtube_music: desde_music,
        })
    }
}

/// Cliente de yt-dlp.
pub struct ClienteYtDlp {
    ejecutor: Arc<dyn Ejecutor>,
    /// Origen de las cookies, compartido con Ajustes.
    ///
    /// Se lee en cada invocación en vez de guardarse al construir: cambiar el
    /// ajuste tiene que servir para la siguiente canción, no para la siguiente
    /// sesión. Es el mismo trato que recibe el crossfade.
    cookies: Arc<CookiesVigentes>,
}

impl std::fmt::Debug for ClienteYtDlp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClienteYtDlp").finish_non_exhaustive()
    }
}

/// Los argumentos con los que yt-dlp se autentica, según el origen configurado.
///
/// ## Van en todas las invocaciones, no solo en la descarga
///
/// El muro de «Sign in to confirm you're not a bot» aparece también al extraer
/// la ficha de un vídeo. Sin cookies ahí, el emparejamiento falla antes de que
/// haya nada que descargar, y el error que se ve no habla de descargas.
#[must_use]
pub fn args_de_cookies(origen: &CookieSource) -> Vec<String> {
    match origen {
        CookieSource::Ninguna => Vec::new(),
        CookieSource::Navegador(nombre) => {
            vec!["--cookies-from-browser".to_owned(), nombre.clone()]
        }
        CookieSource::Fichero(ruta) => {
            vec!["--cookies".to_owned(), ruta.to_string_lossy().into_owned()]
        }
    }
}

impl ClienteYtDlp {
    #[must_use]
    pub fn nuevo(ejecutor: Arc<dyn Ejecutor>, cookies: Arc<CookiesVigentes>) -> Self {
        Self { ejecutor, cookies }
    }

    /// Los argumentos de autenticación vigentes.
    fn cookies(&self) -> Vec<String> {
        args_de_cookies(&self.cookies.leer())
    }

    /// Ejecuta una consulta y devuelve los candidatos.
    ///
    /// # Errors
    /// Si el proceso falla de forma no recuperable.
    pub async fn buscar(&self, consulta: &Consulta) -> YtDlpResult<Vec<RawCandidate>> {
        // Un vídeo concreto no se busca: se pide su ficha. `ytsearch` sobre una
        // URL buscaría la URL como si fuera texto y no devolvería nada.
        let objetivo = if consulta.directa {
            consulta.texto.clone()
        } else if consulta.music {
            // YouTube Music no tiene prefijo `ytsearch`: se consulta por URL.
            format!(
                "https://music.youtube.com/search?q={}",
                url_escape(&consulta.texto)
            )
        } else {
            format!("ytsearch{RESULTADOS_POR_CONSULTA}:{}", consulta.texto)
        };

        let mut args = vec![
            objetivo,
            // Un objeto JSON por línea, sin descargar nada.
            "--dump-json".to_owned(),
            "--flat-playlist".to_owned(),
            "--no-warnings".to_owned(),
            "--ignore-errors".to_owned(),
            // Sin esto, una lista de reproducción entre los resultados
            // desencadenaría cientos de extracciones.
            "--playlist-end".to_owned(),
            RESULTADOS_POR_CONSULTA.to_string(),
        ];
        args.extend(self.cookies());

        let salida = self.ejecutor.ejecutar(BINARIO, &args).await?;

        // `--ignore-errors` hace que yt-dlp devuelva un código distinto de cero
        // aunque haya encontrado resultados válidos. Solo se considera fallo si
        // además no salió nada.
        if !salida.es_ok() && salida.stdout.trim().is_empty() {
            return Err(clasificar(BINARIO, &salida));
        }

        Ok(parsear_candidatos(&salida.stdout, consulta.music))
    }

    /// Descarga el audio de un vídeo al fichero indicado.
    ///
    /// **No hay cancelación ni pausa**: no existen en el diseño (ADR-016). Un
    /// trabajo solo termina completándose o fallando.
    ///
    /// # Errors
    /// Si el proceso falla o el fichero no aparece.
    pub async fn descargar(
        &self,
        video_id: &str,
        destino: &Path,
        preferencia: FormatPreference,
        observador: &dyn ObservadorDescarga,
    ) -> YtDlpResult<PathBuf> {
        let mut args = vec![
            format!("https://www.youtube.com/watch?v={video_id}"),
            "-f".to_owned(),
            formats::expresion(preferencia).to_owned(),
            "-o".to_owned(),
            destino.to_string_lossy().into_owned(),
            // Nunca se transcodifica: recodificar un códec con pérdida a otro
            // no recupera nada y solo degrada.
            "--no-post-overwrites".to_owned(),
            "--newline".to_owned(),
            "--progress".to_owned(),
            "--progress-template".to_owned(),
            PLANTILLA_PROGRESO.to_owned(),
            "--no-warnings".to_owned(),
            "--no-playlist".to_owned(),
        ];
        args.extend(self.cookies());

        let puente = PuenteProgreso {
            observador,
            destino,
        };
        let salida = self
            .ejecutor
            .ejecutar_con_progreso(BINARIO, &args, &puente)
            .await?;

        if !salida.es_ok() {
            return Err(clasificar(BINARIO, &salida));
        }

        Ok(destino.to_path_buf())
    }
}

/// Recibe el avance de una descarga.
pub trait ObservadorDescarga: Send + Sync {
    /// Bytes descargados y total, si se conoce.
    fn progreso(&self, hechos: u64, total: Option<u64>);
    /// Hay bytes suficientes para empezar a sonar.
    fn reproducible(&self, ruta: &Path);
}

/// Traduce las líneas de yt-dlp en llamadas al observador.
struct PuenteProgreso<'a> {
    observador: &'a dyn ObservadorDescarga,
    destino: &'a Path,
}

impl ObservadorLineas for PuenteProgreso<'_> {
    fn linea(&self, texto: &str) {
        let Some((hechos, total)) = parsear_progreso(texto) else {
            return;
        };
        self.observador.progreso(hechos, total);

        // El aviso de "ya se puede reproducir" se emite en cuanto hay buffer
        // suficiente. Es lo que dispara la reproducción progresiva y lo que
        // hace que pulsar play suene en un par de segundos.
        if hechos >= BYTES_MINIMOS_REPRODUCIBLE {
            self.observador.reproducible(self.destino);
        }
    }
}

/// Interpreta una línea de progreso.
///
/// Formato: `LOCALIFY|<hechos>|<total>|<estimado>`. Los campos desconocidos
/// llegan como `NA`.
#[must_use]
pub fn parsear_progreso(linea: &str) -> Option<(u64, Option<u64>)> {
    let resto = linea.trim().strip_prefix("LOCALIFY|")?;
    let mut campos = resto.split('|');

    let hechos: u64 = campos.next()?.trim().parse().ok()?;
    let total = campos.next().and_then(|c| c.trim().parse::<u64>().ok());
    // Si no hay total exacto, sirve la estimación: es lo que ocurre con los
    // flujos DASH, que son la mayoría del audio de YouTube.
    let estimado = campos.next().and_then(|c| c.trim().parse::<u64>().ok());

    Some((hechos, total.or(estimado)))
}

/// Convierte la salida JSON en candidatos.
///
/// Las líneas ilegibles se descartan: `--ignore-errors` mezcla objetos válidos
/// con mensajes de error, y una respuesta parcialmente rota sigue siendo útil.
#[must_use]
pub fn parsear_candidatos(salida: &str, desde_music: bool) -> Vec<RawCandidate> {
    let mut ilegibles = 0_usize;
    let mut sin_duracion = 0_usize;

    let candidatos: Vec<RawCandidate> = salida
        .lines()
        .filter(|l| l.trim_start().starts_with('{'))
        .filter_map(|l| match serde_json::from_str::<EntradaJson>(l) {
            Ok(e) => Some(e),
            Err(e) => {
                // Antes esto era un `.ok()` mudo. Cuando yt-dlp cambió la forma
                // de un campo, la consecuencia fue que **todas** las búsquedas
                // devolvían cero candidatos y el único síntoma era "sin
                // coincidencia": un fallo de parseo disfrazado de "YouTube no
                // tiene esta canción".
                debug!(error = %e, "entrada de yt-dlp ilegible");
                ilegibles += 1;
                None
            }
        })
        .filter_map(|e| {
            let c = e.a_candidato(desde_music);
            if c.is_none() {
                sin_duracion += 1;
            }
            c
        })
        .collect();

    if candidatos.is_empty() && (ilegibles > 0 || sin_duracion > 0) {
        warn!(
            ilegibles,
            sin_duracion, "yt-dlp respondió pero no quedó ningún candidato"
        );
    }
    candidatos
}

/// Escapa un texto para una URL de consulta.
fn url_escape(valor: &str) -> String {
    use std::fmt::Write as _;

    let mut salida = String::with_capacity(valor.len() * 3);
    for byte in valor.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                salida.push(*byte as char);
            }
            b' ' => salida.push('+'),
            otro => {
                let _ = write!(salida, "%{otro:02X}");
            }
        }
    }
    salida
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::error::YtDlpError;
    use crate::proceso::falso::EjecutorFalso;

    fn json_candidato(id: &str, titulo: &str, canal: &str, segundos: f64) -> String {
        format!(
            r#"{{"id":"{id}","title":"{titulo}","channel":"{canal}","duration":{segundos},"view_count":100000}}"#
        )
    }

    #[test]
    fn una_entrada_con_channel_y_uploader_se_lee() {
        // yt-dlp emite **los dos** campos. Estaban unidos en uno con dos alias,
        // y serde respondía `duplicate field 'channel'` para cada resultado.
        // Como el error se tragaba con `.ok()`, el síntoma era que ninguna
        // búsqueda encontraba nada: la descarga fallaba con "sin coincidencia",
        // igual que si YouTube no tuviera la canción.
        //
        // Línea real, recortada, de `ytsearch10:Casey Edwards - bury the light`.
        let salida = r#"{"_type": "url", "ie_key": "Youtube", "id": "pvy9km7g6fw", "title": "Bury the Light - Vergil's battle theme from Devil May Cry 5 [OFFICIAL AUDIO]", "duration": 581, "channel": "Casey Edwards", "uploader": "Casey Edwards", "view_count": 29500587}"#;

        let candidatos = parsear_candidatos(salida, false);
        assert_eq!(candidatos.len(), 1, "la entrada tiene que sobrevivir");
        assert_eq!(candidatos[0].video_id, "pvy9km7g6fw");
        assert_eq!(candidatos[0].channel.as_deref(), Some("Casey Edwards"));
        assert_eq!(candidatos[0].duration.as_ms(), 581_000);
    }

    #[test]
    fn sin_channel_vale_el_uploader() {
        // No siempre vienen los dos, y el canal pesa en la puntuación: perderlo
        // por no mirar el segundo campo empeoraría el emparejamiento.
        let salida =
            r#"{"id": "abc12345678", "title": "X", "duration": 200, "uploader": "Canal - Topic"}"#;
        let candidatos = parsear_candidatos(salida, false);
        assert_eq!(candidatos[0].channel.as_deref(), Some("Canal - Topic"));
    }

    #[test]
    fn el_progreso_se_interpreta_con_total_exacto() {
        assert_eq!(
            parsear_progreso("LOCALIFY|1024|4096|NA"),
            Some((1024, Some(4096)))
        );
    }

    #[test]
    fn sin_total_exacto_sirve_la_estimacion() {
        // Es lo que ocurre con los flujos DASH, que son la mayoría del audio.
        assert_eq!(
            parsear_progreso("LOCALIFY|1024|NA|5000"),
            Some((1024, Some(5000)))
        );
    }

    #[test]
    fn sin_ningun_total_se_informa_solo_de_lo_hecho() {
        assert_eq!(parsear_progreso("LOCALIFY|1024|NA|NA"), Some((1024, None)));
    }

    #[test]
    fn las_lineas_ajenas_al_progreso_se_ignoran() {
        for linea in [
            "[youtube] Extracting URL: ...",
            "[download] Destination: x.webm",
            "",
            "LOCALIFY|",
            "LOCALIFY|NA|NA|NA",
        ] {
            assert_eq!(parsear_progreso(linea), None, "'{linea}'");
        }
    }

    #[test]
    fn los_candidatos_se_extraen_de_la_salida_json() {
        let salida = format!(
            "{}\n{}\n",
            json_candidato("abc123", "Under Pressure", "Queen - Topic", 248.0),
            json_candidato("def456", "Under Pressure (Live)", "Un Fan", 260.0)
        );

        let candidatos = parsear_candidatos(&salida, false);
        assert_eq!(candidatos.len(), 2);
        assert_eq!(candidatos[0].video_id, "abc123");
        assert_eq!(candidatos[0].duration, DurationMs::new(248_000));
        assert_eq!(candidatos[0].channel.as_deref(), Some("Queen - Topic"));
        assert!(!candidatos[0].from_youtube_music);
    }

    #[test]
    fn una_salida_con_lineas_rotas_conserva_las_buenas() {
        // `--ignore-errors` mezcla objetos válidos con mensajes de error.
        let salida = format!(
            "ERROR: Video unavailable\n{}\n{{roto\n",
            json_candidato("abc123", "Buena", "Canal", 248.0)
        );

        let candidatos = parsear_candidatos(&salida, false);
        assert_eq!(candidatos.len(), 1);
        assert_eq!(candidatos[0].video_id, "abc123");
    }

    #[test]
    fn un_candidato_sin_duracion_se_descarta() {
        // Sin duración no se puede validar la coincidencia, y puntuarlo a
        // ciegas es peor que no tenerlo.
        let salida = r#"{"id":"abc","title":"X","channel":"C"}"#;
        assert!(parsear_candidatos(salida, false).is_empty());

        let cero = r#"{"id":"abc","title":"X","duration":0}"#;
        assert!(parsear_candidatos(cero, false).is_empty());
    }

    #[test]
    fn la_marca_de_subida_oficial_se_detecta_en_la_descripcion() {
        // Cada candidato viene en una sola línea: yt-dlp emite JSON por líneas.
        let salida = r#"{"id":"abc","title":"X","duration":248,"description":"Provided to YouTube by Universal Music Group"}"#;
        let candidatos = parsear_candidatos(salida, false);
        assert_eq!(candidatos.len(), 1);
        assert!(candidatos[0].provided_to_youtube);
    }

    #[test]
    fn el_canal_se_lee_venga_en_el_campo_que_venga() {
        // Este test se llamaba "cubre uploader y channel" y **nunca los puso
        // juntos**: probaba cada uno por separado, que es justo el caso que no
        // fallaba. El que fallaba —los dos en la misma entrada, que es lo que
        // yt-dlp emite siempre— está arriba, en
        // `una_entrada_con_channel_y_uploader_se_lee`.
        let con_uploader = r#"{"id":"a","title":"X","duration":10,"uploader":"Queen"}"#;
        assert_eq!(
            parsear_candidatos(con_uploader, false)[0]
                .channel
                .as_deref(),
            Some("Queen")
        );

        let con_channel = r#"{"id":"a","title":"X","duration":10,"channel":"Queen"}"#;
        assert_eq!(
            parsear_candidatos(con_channel, false)[0].channel.as_deref(),
            Some("Queen")
        );
    }

    #[tokio::test]
    async fn buscar_en_youtube_music_usa_una_url_y_no_ytsearch() {
        let e = Arc::new(EjecutorFalso::nuevo().con_stdout(&json_candidato("a", "X", "C", 200.0)));
        let cliente = ClienteYtDlp::nuevo(e.clone(), Arc::default());

        let candidatos = cliente
            .buscar(&Consulta {
                texto: "queen under pressure".into(),
                music: true,
                directa: false,
                origen: "music",
            })
            .await
            .expect("busca");

        assert!(candidatos[0].from_youtube_music);
        let args = e.args_de(0);
        assert!(
            args[0].starts_with("https://music.youtube.com/search"),
            "{args:?}"
        );
        assert!(args[0].contains("queen+under+pressure"), "{args:?}");
    }

    #[tokio::test]
    async fn buscar_en_youtube_usa_ytsearch_con_limite() {
        let e = Arc::new(EjecutorFalso::nuevo().con_stdout(""));
        let cliente = ClienteYtDlp::nuevo(e.clone(), Arc::default());

        cliente
            .buscar(&Consulta {
                texto: "queen".into(),
                music: false,
                directa: false,
                origen: "general",
            })
            .await
            .expect("busca");

        let args = e.args_de(0);
        assert_eq!(args[0], format!("ytsearch{RESULTADOS_POR_CONSULTA}:queen"));
        assert!(args.contains(&"--dump-json".to_owned()));
    }

    #[tokio::test]
    async fn un_codigo_distinto_de_cero_con_resultados_no_es_un_fallo() {
        // `--ignore-errors` devuelve código != 0 aunque haya encontrado cosas.
        let e = Arc::new(EjecutorFalso::nuevo().encolar(crate::proceso::Salida {
            codigo: 1,
            stdout: json_candidato("abc", "X", "C", 200.0),
            stderr: "ERROR: uno de los resultados falló".into(),
        }));
        let cliente = ClienteYtDlp::nuevo(e, Arc::default());

        let candidatos = cliente
            .buscar(&Consulta {
                texto: "x".into(),
                music: false,
                directa: false,
                origen: "general",
            })
            .await
            .expect("no debe fallar si hubo resultados");
        assert_eq!(candidatos.len(), 1);
    }

    #[tokio::test]
    async fn un_fallo_sin_resultados_si_es_un_error() {
        let e = Arc::new(EjecutorFalso::nuevo().con_error(1, "ERROR: Video unavailable"));
        let cliente = ClienteYtDlp::nuevo(e, Arc::default());

        let error = cliente
            .buscar(&Consulta {
                texto: "x".into(),
                music: false,
                directa: false,
                origen: "general",
            })
            .await
            .expect_err("debe fallar");
        assert!(matches!(error, YtDlpError::VideoNoDisponible));
    }

    #[derive(Default)]
    struct ObservadorDePrueba {
        progresos: Mutex<Vec<(u64, Option<u64>)>>,
        reproducible: Mutex<bool>,
    }

    impl ObservadorDescarga for ObservadorDePrueba {
        fn progreso(&self, hechos: u64, total: Option<u64>) {
            if let Ok(mut p) = self.progresos.lock() {
                p.push((hechos, total));
            }
        }
        fn reproducible(&self, _ruta: &Path) {
            if let Ok(mut r) = self.reproducible.lock() {
                *r = true;
            }
        }
    }

    #[tokio::test]
    async fn la_descarga_informa_del_progreso_y_del_momento_reproducible() {
        let lineas = format!(
            "[download] Destination: x.webm\nLOCALIFY|1024|4000000|NA\nLOCALIFY|{}|4000000|NA\nLOCALIFY|4000000|4000000|NA",
            BYTES_MINIMOS_REPRODUCIBLE + 1
        );
        let e = Arc::new(EjecutorFalso::nuevo().con_stdout(&lineas));
        let cliente = ClienteYtDlp::nuevo(e, Arc::default());

        let obs = ObservadorDePrueba::default();
        cliente
            .descargar(
                "abc",
                Path::new("x.webm.part"),
                FormatPreference::Opus,
                &obs,
            )
            .await
            .expect("descarga");

        let progresos = obs.progresos.lock().expect("lock").clone();
        assert_eq!(
            progresos.len(),
            3,
            "las líneas ajenas al progreso se ignoran"
        );
        assert_eq!(progresos[0], (1024, Some(4_000_000)));
        assert!(
            *obs.reproducible.lock().expect("lock"),
            "debe avisarse en cuanto hay buffer suficiente"
        );
    }

    #[tokio::test]
    async fn una_descarga_lenta_no_avisa_de_reproducible_antes_de_tiempo() {
        let e = Arc::new(EjecutorFalso::nuevo().con_stdout("LOCALIFY|1024|4000000|NA"));
        let cliente = ClienteYtDlp::nuevo(e, Arc::default());

        let obs = ObservadorDePrueba::default();
        cliente
            .descargar("abc", Path::new("x.part"), FormatPreference::Opus, &obs)
            .await
            .expect("descarga");

        assert!(
            !*obs.reproducible.lock().expect("lock"),
            "con un kilobyte no hay nada que reproducir"
        );
    }

    #[tokio::test]
    async fn la_descarga_pide_el_formato_preferido_y_el_destino() {
        let e = Arc::new(EjecutorFalso::nuevo().con_stdout(""));
        let cliente = ClienteYtDlp::nuevo(e.clone(), Arc::default());

        let obs = ObservadorDePrueba::default();
        cliente
            .descargar(
                "dQw4w9WgXcQ",
                Path::new(".tmp/abc.webm.part"),
                FormatPreference::Opus,
                &obs,
            )
            .await
            .expect("descarga");

        let args = e.args_de(0);
        assert!(args[0].contains("dQw4w9WgXcQ"));
        assert!(args.contains(&formats::expresion(FormatPreference::Opus).to_owned()));
        assert!(
            args.iter().any(|a| a.contains(".part")),
            "el destino debe ser el temporal: {args:?}"
        );
    }

    #[test]
    fn el_escapado_de_url_protege_la_consulta() {
        assert_eq!(url_escape("queen under pressure"), "queen+under+pressure");
        assert_eq!(url_escape("AC/DC"), "AC%2FDC");
        assert_eq!(url_escape("Björk"), "Bj%C3%B6rk");
    }
}

#[cfg(test)]
mod tests_cookies {
    use super::*;

    #[test]
    fn sin_origen_no_se_pasa_ningun_argumento() {
        // Es el caso por defecto y el que tiene que seguir funcionando igual que
        // antes de que existieran las cookies.
        assert!(args_de_cookies(&CookieSource::Ninguna).is_empty());
    }

    #[test]
    fn el_navegador_va_como_cookies_from_browser() {
        assert_eq!(
            args_de_cookies(&CookieSource::Navegador("firefox".into())),
            vec!["--cookies-from-browser".to_owned(), "firefox".to_owned()]
        );
    }

    #[test]
    fn el_fichero_va_como_cookies() {
        // Argumentos distintos: `--cookies` toma un fichero en formato Netscape
        // y `--cookies-from-browser` un nombre de navegador. Confundirlos hace
        // que yt-dlp busque un perfil llamado "C:\...".
        assert_eq!(
            args_de_cookies(&CookieSource::Fichero(PathBuf::from(r"C:\c.txt"))),
            vec!["--cookies".to_owned(), r"C:\c.txt".to_owned()]
        );
    }

    #[tokio::test]
    async fn la_busqueda_tambien_lleva_las_cookies() {
        // El muro de "confirma que no eres un bot" aparece al extraer la ficha
        // de un vídeo, no solo al descargarlo: sin cookies aquí, el
        // emparejamiento falla antes de que haya nada que bajar.
        let e = Arc::new(EjecutorEspia::default());
        let cookies = Arc::new(CookiesVigentes::default());
        cookies.poner(CookieSource::Navegador("firefox".into()));

        let cliente = ClienteYtDlp::nuevo(e.clone(), cookies);
        let _ = cliente
            .buscar(&Consulta {
                texto: "algo".into(),
                music: false,
                directa: false,
                origen: "test",
            })
            .await;

        let args = e.ultimos();
        assert!(args.contains(&"--cookies-from-browser".to_owned()));
    }

    #[tokio::test]
    async fn cambiar_el_ajuste_surte_efecto_sin_reconstruir_el_cliente() {
        // Las cookies se leen en cada invocación a propósito: el usuario las
        // configura con la aplicación abierta, y tienen que servir para la
        // siguiente canción y no para la siguiente sesión.
        let e = Arc::new(EjecutorEspia::default());
        let cookies = Arc::new(CookiesVigentes::default());
        let cliente = ClienteYtDlp::nuevo(e.clone(), Arc::clone(&cookies));

        let consulta = Consulta {
            texto: "algo".into(),
            music: false,
            directa: false,
            origen: "test",
        };
        let _ = cliente.buscar(&consulta).await;
        assert!(!e.ultimos().contains(&"--cookies-from-browser".to_owned()));

        cookies.poner(CookieSource::Navegador("edge".into()));
        let _ = cliente.buscar(&consulta).await;
        assert!(e.ultimos().contains(&"edge".to_owned()));
    }

    /// Ejecutor que solo anota con qué se le llamó.
    #[derive(Debug, Default)]
    struct EjecutorEspia(std::sync::Mutex<Vec<String>>);

    impl EjecutorEspia {
        fn ultimos(&self) -> Vec<String> {
            self.0.lock().expect("sin envenenar").clone()
        }
    }

    #[async_trait::async_trait]
    impl Ejecutor for EjecutorEspia {
        async fn ejecutar(
            &self,
            _b: &'static str,
            args: &[String],
        ) -> YtDlpResult<crate::proceso::Salida> {
            *self.0.lock().expect("sin envenenar") = args.to_vec();
            Ok(crate::proceso::Salida {
                codigo: 0,
                stdout: String::new(),
                stderr: String::new(),
            })
        }

        async fn ejecutar_con_progreso(
            &self,
            b: &'static str,
            args: &[String],
            _o: &dyn ObservadorLineas,
        ) -> YtDlpResult<crate::proceso::Salida> {
            self.ejecutar(b, args).await
        }
    }
}

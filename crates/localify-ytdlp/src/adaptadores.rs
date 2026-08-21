//! Implementación de los puertos de [`localify_core::ports::youtube`].
//!
//! Compone las piezas del crate —búsqueda, puntuación, descarga, verificación y
//! remux— en las dos operaciones que el resto del sistema necesita: **encontrar
//! el mejor candidato** y **traer su audio verificado**.

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use localify_core::domain::audio::DurationMs;
use localify_core::domain::download::MatchResult;
use localify_core::domain::settings::FormatPreference;
use localify_core::domain::track::Track;
use localify_core::error::CoreResult;
use localify_core::ports::youtube::{
    DownloadObserver, DownloadedFile, MediaInfo, YoutubeDownloader, YoutubeMatcher,
};
use tracing::{debug, info};

use crate::download::{ClienteYtDlp, ObservadorDescarga};
use crate::error::YtDlpError;
use crate::proceso::Ejecutor;
use crate::remux::Remuxeador;
use crate::search::{RawCandidate, basta_con, plan_de_consultas};
use crate::verify::Inspector;
use crate::{formats, scoring};

/// Emparejador basado en yt-dlp.
pub struct MatcherYtDlp {
    cliente: Arc<ClienteYtDlp>,
}

impl std::fmt::Debug for MatcherYtDlp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MatcherYtDlp").finish_non_exhaustive()
    }
}

impl MatcherYtDlp {
    #[must_use]
    pub fn nuevo(cliente: Arc<ClienteYtDlp>) -> Self {
        Self { cliente }
    }
}

#[async_trait]
impl YoutubeMatcher for MatcherYtDlp {
    async fn find_best(
        &self,
        track: &Track,
        exclude: &[String],
        conocido: Option<&str>,
    ) -> CoreResult<MatchResult> {
        let plan = plan_de_consultas(track, conocido);
        if plan.is_empty() {
            return Err(YtDlpError::SinCoincidencia.into());
        }

        let mut acumulados: Vec<RawCandidate> = Vec::new();
        let mut mejor: Option<MatchResult> = None;

        for consulta in &plan {
            let nuevos = match self.cliente.buscar(consulta).await {
                Ok(c) => c,
                // Una consulta que falla no invalida el plan: puede que la
                // siguiente encuentre lo mismo por otra vía.
                Err(e) => {
                    debug!(origen = consulta.origen, error = %e, "consulta fallida");
                    continue;
                }
            };

            // Se acumulan sin repetir: las consultas se solapan a propósito, y
            // un mismo vídeo puede salir en varias.
            for candidato in nuevos {
                if !acumulados.iter().any(|c| c.video_id == candidato.video_id) {
                    acumulados.push(candidato);
                }
            }

            let Some(resultado) = scoring::elegir_mejor(track, &acumulados, exclude) else {
                continue;
            };

            let puntuacion = resultado.best.score;
            mejor = Some(resultado);

            // Con una coincidencia segura, seguir consultando solo añadiría
            // segundos de espera a una descarga que ya puede empezar.
            if basta_con(puntuacion) {
                debug!(
                    origen = consulta.origen,
                    score = puntuacion,
                    "coincidencia segura; se deja de buscar"
                );
                break;
            }
        }

        let resultado = mejor.ok_or(YtDlpError::SinCoincidencia)?;

        info!(
            pista = %track.id,
            video = %resultado.best.video_id,
            score = resultado.best.score,
            confianza = ?resultado.confidence,
            considerados = resultado.candidates_considered,
            "emparejamiento resuelto"
        );

        Ok(resultado)
    }
}

/// Descargador basado en yt-dlp, con verificación y remux.
pub struct DescargadorYtDlp {
    cliente: Arc<ClienteYtDlp>,
    inspector: Inspector,
    remuxeador: Remuxeador,
}

impl std::fmt::Debug for DescargadorYtDlp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DescargadorYtDlp").finish_non_exhaustive()
    }
}

impl DescargadorYtDlp {
    #[must_use]
    pub fn nuevo(cliente: Arc<ClienteYtDlp>, ejecutor: Arc<dyn Ejecutor>) -> Self {
        Self {
            cliente,
            inspector: Inspector::nuevo(Arc::clone(&ejecutor)),
            remuxeador: Remuxeador::nuevo(ejecutor),
        }
    }

    /// Extensión real de un `.part`.
    fn extension_de(ruta: &Path) -> String {
        let sin_part = ruta
            .file_stem()
            .map_or_else(|| std::path::PathBuf::from(ruta), std::path::PathBuf::from);
        sin_part
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("bin")
            .to_ascii_lowercase()
    }
}

/// Puente entre el observador del puerto y el del cliente.
struct PuenteObservador<'a>(&'a dyn DownloadObserver);

impl ObservadorDescarga for PuenteObservador<'_> {
    fn progreso(&self, hechos: u64, total: Option<u64>) {
        self.0
            .on_progress(&localify_core::domain::download::DownloadProgress {
                bytes_done: hechos,
                bytes_total: total,
                playable: false,
                state: localify_core::domain::download::DownloadState::Downloading,
            });
    }

    fn reproducible(&self, ruta: &Path) {
        self.0.on_playable(ruta);
    }
}

#[async_trait]
impl YoutubeDownloader for DescargadorYtDlp {
    async fn download(
        &self,
        video_id: &str,
        preference: FormatPreference,
        dest: &Path,
        expected: DurationMs,
        observer: &dyn DownloadObserver,
    ) -> CoreResult<DownloadedFile> {
        let puente = PuenteObservador(observer);
        let descargado = self
            .cliente
            .descargar(video_id, dest, preference, &puente)
            .await?;

        // Verificar antes de nada: si el fichero está truncado, remuxearlo solo
        // propagaría el problema a un fichero nuevo.
        let info = self.inspector.verificar(&descargado, expected).await?;
        let extension = Self::extension_de(&descargado);

        // Un WebM con extensión `.opus` sería mentira. El remux cambia el
        // envoltorio sin tocar un bit del audio (ADR-020).
        let Some(destino_ext) = crate::remux::destino_para(&extension, &info.codec) else {
            return Ok(DownloadedFile {
                path: descargado,
                info,
                extension,
            });
        };

        let remuxeado = self.remuxeador.remuxear(&descargado, destino_ext).await?;

        // Se vuelve a verificar: un remux que salga mal debe detectarse aquí y
        // no al reproducir.
        let info = self.inspector.verificar(&remuxeado, expected).await?;

        // Ahora sí se puede borrar el original: hay un sustituto verificado.
        let _ = tokio::fs::remove_file(&descargado).await;

        debug!(
            de = %extension,
            a = destino_ext,
            "contenedor cambiado tras verificar"
        );

        Ok(DownloadedFile {
            path: remuxeado,
            info,
            extension: destino_ext.to_owned(),
        })
    }

    async fn probe(&self, path: &Path) -> CoreResult<MediaInfo> {
        Ok(self.inspector.inspeccionar(path).await?)
    }
}

/// Extensión con la que se guarda el temporal de una descarga.
///
/// yt-dlp elige el formato al descargar, así que aquí solo se acierta con la
/// preferencia; lo que se obtuvo de verdad se sabe midiendo el fichero.
#[must_use]
pub fn extension_temporal(preferencia: FormatPreference) -> &'static str {
    match preferencia {
        // El audio Opus de YouTube llega en WebM: nombrarlo así desde el
        // principio evita tener que renombrar el `.part` a mitad.
        FormatPreference::Opus => "webm",
        FormatPreference::M4a => "m4a",
        FormatPreference::Best => "bin",
    }
}

/// `true` si el temporal admite reproducción mientras crece.
#[must_use]
pub fn temporal_es_progresivo(preferencia: FormatPreference) -> bool {
    formats::admite_reproduccion_progresiva(extension_temporal(preferencia))
}

#[cfg(test)]
mod tests {
    use localify_core::domain::ids::{ArtistId, TrackId};
    use localify_core::domain::track::ArtistRef;

    use super::*;
    use crate::proceso::falso::EjecutorFalso;

    fn pista(titulo: &str, artista: &str, segundos: u32) -> Track {
        Track {
            id: TrackId::nuevo_local(),
            title: titulo.to_owned(),
            album: None,
            artists: vec![ArtistRef {
                id: ArtistId::nuevo_local(),
                name: artista.to_owned(),
            }],
            duration: DurationMs::from_secs(segundos),
            track_number: None,
            disc_number: None,
            explicit: false,
            isrc: None,
            release_date: None,
            popularity: None,
            added_at: chrono::Utc::now(),
        }
    }

    fn json_candidato(id: &str, titulo: &str, canal: &str, segundos: f64) -> String {
        format!(
            r#"{{"id":"{id}","title":"{titulo}","channel":"{canal}","duration":{segundos},"view_count":500000,"description":"Provided to YouTube by Universal"}}"#
        )
    }

    #[test]
    fn la_extension_del_temporal_refleja_lo_que_sirve_youtube() {
        // El Opus de YouTube viene en WebM: nombrarlo `.opus` desde el
        // principio obligaría a renombrar el `.part` a mitad de descarga.
        assert_eq!(extension_temporal(FormatPreference::Opus), "webm");
        assert_eq!(extension_temporal(FormatPreference::M4a), "m4a");
    }

    #[test]
    fn el_temporal_de_opus_admite_reproduccion_progresiva() {
        // Es lo que permite pulsar play y oír música en un par de segundos.
        assert!(temporal_es_progresivo(FormatPreference::Opus));
        assert!(
            !temporal_es_progresivo(FormatPreference::M4a),
            "m4a depende de donde este el atomo moov"
        );
    }

    #[test]
    fn la_extension_real_se_lee_a_traves_del_sufijo_part() {
        assert_eq!(
            DescargadorYtDlp::extension_de(Path::new(".tmp/abc.webm.part")),
            "webm"
        );
        assert_eq!(
            DescargadorYtDlp::extension_de(Path::new(".tmp/abc.m4a.part")),
            "m4a"
        );
    }

    #[tokio::test]
    async fn el_emparejador_para_en_cuanto_encuentra_algo_seguro() {
        // La primera consulta ya da un canal Topic con duracion exacta.
        let e = Arc::new(EjecutorFalso::nuevo().con_stdout(&json_candidato(
            "bueno",
            "Under Pressure",
            "Queen - Topic",
            248.0,
        )));
        let matcher = MatcherYtDlp::nuevo(Arc::new(ClienteYtDlp::nuevo(e.clone(), Arc::default())));

        let resultado = matcher
            .find_best(&pista("Under Pressure", "Queen", 248), &[], None)
            .await
            .expect("empareja");

        assert_eq!(resultado.best.video_id, "bueno");
        assert_eq!(
            e.cuantas(),
            1,
            "seguir buscando tras una coincidencia segura solo haria esperar"
        );
    }

    #[tokio::test]
    async fn el_emparejador_agota_el_plan_si_no_encuentra_nada_seguro() {
        // Ninguna consulta da un buen candidato: deben probarse todas.
        let p = pista("Under Pressure", "Queen", 248);
        let esperadas = plan_de_consultas(&p, None).len();
        assert!(esperadas > 1, "el plan debe tener varias consultas");

        let flojo = json_candidato("flojo", "Otra Cosa", "Canal Random", 400.0);
        let mut e = EjecutorFalso::nuevo();
        for _ in 0..esperadas {
            e = e.con_stdout(&flojo);
        }
        let e = Arc::new(e);
        let matcher = MatcherYtDlp::nuevo(Arc::new(ClienteYtDlp::nuevo(e.clone(), Arc::default())));

        let resultado = matcher
            .find_best(&p, &[], None)
            .await
            .expect("devuelve el mejor aunque sea malo");

        assert_eq!(
            resultado.confidence,
            localify_core::domain::download::Confidence::Low,
            "y con confianza baja, que impide descargarlo"
        );
        assert_eq!(e.cuantas(), esperadas, "se agotan las consultas del plan");
    }

    #[tokio::test]
    async fn una_consulta_fallida_no_aborta_el_plan() {
        let e = Arc::new(
            EjecutorFalso::nuevo()
                .con_error(1, "ERROR: temporal")
                .con_stdout(&json_candidato(
                    "bueno",
                    "Under Pressure",
                    "Queen - Topic",
                    248.0,
                )),
        );
        let matcher = MatcherYtDlp::nuevo(Arc::new(ClienteYtDlp::nuevo(e, Arc::default())));

        let resultado = matcher
            .find_best(&pista("Under Pressure", "Queen", 248), &[], None)
            .await
            .expect("la segunda consulta salva el emparejamiento");
        assert_eq!(resultado.best.video_id, "bueno");
    }

    #[tokio::test]
    async fn los_candidatos_repetidos_entre_consultas_no_se_duplican() {
        let mismo = json_candidato("repetido", "Otra Cosa", "Canal", 400.0);
        let e = Arc::new(
            EjecutorFalso::nuevo()
                .con_stdout(&mismo)
                .con_stdout(&mismo)
                .con_stdout(&mismo)
                .con_stdout(&mismo),
        );
        let matcher = MatcherYtDlp::nuevo(Arc::new(ClienteYtDlp::nuevo(e, Arc::default())));

        let resultado = matcher
            .find_best(&pista("Under Pressure", "Queen", 248), &[], None)
            .await
            .expect("empareja");

        assert_eq!(
            resultado.candidates_considered, 1,
            "las consultas se solapan a proposito; el mismo video no cuenta dos veces"
        );
    }

    #[tokio::test]
    async fn sin_datos_de_la_pista_no_se_sale_a_buscar() {
        let e = Arc::new(EjecutorFalso::nuevo());
        let matcher = MatcherYtDlp::nuevo(Arc::new(ClienteYtDlp::nuevo(e.clone(), Arc::default())));

        let error = matcher
            .find_best(&pista("", "", 200), &[], None)
            .await
            .expect_err("debe fallar");
        assert_eq!(error.code(), "NOT_FOUND");
        assert_eq!(e.cuantas(), 0);
    }

    #[tokio::test]
    async fn un_candidato_excluido_no_se_propone() {
        let e = Arc::new(
            EjecutorFalso::nuevo()
                .con_stdout(&format!(
                    "{}\n{}",
                    json_candidato("rechazado", "Under Pressure", "Queen - Topic", 248.0),
                    json_candidato("otro", "Queen - Under Pressure", "Queen Oficial", 249.0)
                ))
                .con_stdout("")
                .con_stdout("")
                .con_stdout(""),
        );
        let matcher = MatcherYtDlp::nuevo(Arc::new(ClienteYtDlp::nuevo(e, Arc::default())));

        let resultado = matcher
            .find_best(
                &pista("Under Pressure", "Queen", 248),
                &["rechazado".to_owned()],
                None,
            )
            .await
            .expect("empareja");

        assert_eq!(resultado.best.video_id, "otro");
    }
}

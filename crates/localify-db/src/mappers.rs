//! Conversión entre filas de SQLite y entidades del dominio.
//!
//! Todo el mapeo vive aquí y no repartido por los repositorios: leer una pista
//! desde tres consultas distintas debe producir exactamente la misma entidad, y
//! la única forma de garantizarlo es que haya una sola función que lo haga.

use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use localify_core::domain::album::AlbumType;
use localify_core::domain::audio::{AudioFormat, DurationMs};
use localify_core::domain::availability::Availability;
use localify_core::domain::download::{Confidence, DownloadState, Priority};
use localify_core::domain::ids::{AlbumId, TrackId};
use localify_core::domain::library::AudioSource;
use localify_core::domain::playlist::PlaylistSource;
use localify_core::domain::queue::RepeatMode;
use localify_core::domain::track::TrackRow;
use rusqlite::Row;

use crate::error::{DbError, DbResult};

/// Convierte un unix timestamp en segundos a `DateTime<Utc>`.
///
/// Un valor fuera de rango indica corrupción; se sustituye por la época en
/// lugar de propagar un error, porque una fecha rara en el historial no debe
/// impedir que la biblioteca se abra.
#[must_use]
pub fn a_fecha(segundos: i64) -> DateTime<Utc> {
    Utc.timestamp_opt(segundos, 0)
        .single()
        .unwrap_or_else(|| Utc.timestamp_nanos(0))
}

#[must_use]
pub fn de_fecha(fecha: DateTime<Utc>) -> i64 {
    fecha.timestamp()
}

/// Parsea una fecha de lanzamiento de Spotify, que puede venir como `2011`,
/// `2011-05` o `2011-05-17`.
#[must_use]
pub fn a_fecha_lanzamiento(texto: Option<String>) -> Option<NaiveDate> {
    let t = texto?;
    match t.len() {
        4 => NaiveDate::parse_from_str(&format!("{t}-01-01"), "%Y-%m-%d").ok(),
        7 => NaiveDate::parse_from_str(&format!("{t}-01"), "%Y-%m-%d").ok(),
        _ => NaiveDate::parse_from_str(&t, "%Y-%m-%d").ok(),
    }
}

/// Extrae el año de una fecha de lanzamiento parcial sin parsearla entera.
#[must_use]
pub fn anyo_de(texto: Option<&str>) -> Option<i32> {
    texto?.get(..4)?.parse().ok()
}

// ─── Enumeraciones ──────────────────────────────────────────────────────────
//
// Se guardan como texto y no como enteros: una base de datos legible a simple
// vista vale más que los pocos bytes ahorrados, y un `CHECK` sobre texto
// documenta los valores válidos en el propio esquema.

/// # Errors
/// Si el texto no corresponde a ningún formato conocido.
pub fn a_formato(texto: &str) -> DbResult<AudioFormat> {
    AudioFormat::from_extension(texto)
        .ok_or_else(|| DbError::error_de_mapeo("format", format!("formato desconocido: {texto}")))
}

#[must_use]
pub fn a_origen_audio(texto: &str) -> AudioSource {
    if texto == "imported" {
        AudioSource::Imported
    } else {
        AudioSource::Youtube
    }
}

#[must_use]
pub fn de_origen_audio(origen: AudioSource) -> &'static str {
    match origen {
        AudioSource::Youtube => "youtube",
        AudioSource::Imported => "imported",
    }
}

#[must_use]
pub fn a_confianza(texto: &str) -> Confidence {
    match texto {
        "high" => Confidence::High,
        "medium" => Confidence::Medium,
        _ => Confidence::Low,
    }
}

#[must_use]
pub fn de_confianza(c: Confidence) -> &'static str {
    match c {
        Confidence::High => "high",
        Confidence::Medium => "medium",
        Confidence::Low => "low",
    }
}

/// # Errors
/// Si el texto no corresponde a ningún estado conocido. A diferencia de la
/// confianza, aquí no se degrada a un valor por defecto: interpretar mal un
/// estado de descarga podría dar por buena una pista a medio bajar.
pub fn a_estado_descarga(texto: &str) -> DbResult<DownloadState> {
    Ok(match texto {
        "queued" => DownloadState::Queued,
        "matching" => DownloadState::Matching,
        "downloading" => DownloadState::Downloading,
        "finalizing" => DownloadState::Finalizing,
        "done" => DownloadState::Done,
        "failed" => DownloadState::Failed,
        otro => {
            return Err(DbError::error_de_mapeo(
                "state",
                format!("estado desconocido: {otro}"),
            ));
        }
    })
}

#[must_use]
pub fn de_estado_descarga(e: DownloadState) -> &'static str {
    match e {
        DownloadState::Queued => "queued",
        DownloadState::Matching => "matching",
        DownloadState::Downloading => "downloading",
        DownloadState::Finalizing => "finalizing",
        DownloadState::Done => "done",
        DownloadState::Failed => "failed",
    }
}

#[must_use]
pub fn a_prioridad(texto: &str) -> Priority {
    if texto == "immediate" {
        Priority::Immediate
    } else {
        Priority::Prefetch
    }
}

#[must_use]
pub fn de_prioridad(p: Priority) -> &'static str {
    match p {
        Priority::Immediate => "immediate",
        Priority::Prefetch => "prefetch",
    }
}

#[must_use]
pub fn a_modo_repeticion(texto: &str) -> RepeatMode {
    match texto {
        "queue" => RepeatMode::Queue,
        "track" => RepeatMode::Track,
        _ => RepeatMode::Off,
    }
}

#[must_use]
pub fn de_modo_repeticion(m: RepeatMode) -> &'static str {
    match m {
        RepeatMode::Off => "off",
        RepeatMode::Queue => "queue",
        RepeatMode::Track => "track",
    }
}

#[must_use]
pub fn a_origen_playlist(texto: &str) -> PlaylistSource {
    if texto == "spotify_import" {
        PlaylistSource::SpotifyImport
    } else {
        PlaylistSource::Local
    }
}

#[must_use]
pub fn de_origen_playlist(o: PlaylistSource) -> &'static str {
    match o {
        PlaylistSource::Local => "local",
        PlaylistSource::SpotifyImport => "spotify_import",
    }
}

#[must_use]
pub fn de_tipo_album(t: AlbumType) -> &'static str {
    t.as_str()
}

// ─── Filas ──────────────────────────────────────────────────────────────────

/// Columnas de una fila de lista, en el orden que espera [`a_track_row`].
///
/// Se define como constante para que todas las consultas que producen un
/// `TrackRow` seleccionen exactamente lo mismo. Si una consulta añadiera o
/// reordenara una columna por su cuenta, el mapeo leería el campo equivocado sin
/// que nada lo advirtiera: los tipos de SQLite no lo impedirían.
pub const COLUMNAS_TRACK_ROW: &str = "
    t.id,
    t.title,
    t.artist_display,
    t.album_id,
    a.title AS album_title,
    t.duration_ms,
    t.explicit,
    t.popularity,
    (f.track_id IS NOT NULL) AS is_favorite,
    af.rel_path,
    af.format,
    af.size_bytes,
    dj.state AS dl_state,
    dj.bytes_done,
    dj.bytes_total,
    dj.attempts,
    dj.last_error
";

/// `JOIN` que acompaña a [`COLUMNAS_TRACK_ROW`].
///
/// Van juntos a propósito: separarlos permitiría escribir una consulta con las
/// columnas pero sin los `JOIN` que las alimentan.
pub const JOINS_TRACK_ROW: &str = "
    LEFT JOIN albums        a  ON a.id       = t.album_id
    LEFT JOIN favorites     f  ON f.track_id = t.id
    LEFT JOIN audio_files   af ON af.track_id = t.id
    LEFT JOIN download_jobs dj ON dj.track_id = t.id
";

/// Construye una fila de lista.
///
/// # Errors
/// Si alguna columna tiene un valor que el dominio no admite.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub fn a_track_row(row: &Row<'_>) -> DbResult<TrackRow> {
    let availability = disponibilidad_de_fila(row)?;
    let album_id: Option<String> = row.get("album_id")?;
    let added_at = fecha_de_fila(row)?;

    Ok(TrackRow {
        id: TrackId::from_trusted(row.get::<_, String>("id")?),
        title: row.get("title")?,
        artist_display: row.get("artist_display")?,
        album_id: album_id.map(AlbumId::from_trusted),
        album_title: row.get("album_title")?,
        duration: DurationMs::new(row.get::<_, i64>("duration_ms")? as u32),
        availability,
        is_favorite: row.get::<_, i64>("is_favorite")? != 0,
        explicit: row.get::<_, i64>("explicit")? != 0,
        popularity: row
            .get::<_, Option<i64>>("popularity")?
            .and_then(|p| u8::try_from(p.clamp(0, 100)).ok()),
        added_at,
    })
}

/// Nombre con el que una consulta feche sus filas. Ver [`FECHA_TRACK_ROW`].
pub const ALIAS_FECHA: &str = "row_added_at";

/// Trozo de `SELECT` que añade la fecha de la fila.
///
/// No va dentro de [`COLUMNAS_TRACK_ROW`] porque **la columna de origen cambia
/// con la lista**: `pi.added_at` en una playlist, `f.added_at` en los favoritos,
/// `t.added_at` en la biblioteca. Cada consulta dice de dónde sale la suya; las
/// que no fechan nada —búsqueda, pistas de un álbum— no la piden y la fila llega
/// con `None`.
#[must_use]
pub fn fecha_track_row(columna: &str) -> String {
    format!(", {columna} AS {ALIAS_FECHA}")
}

/// Lee la fecha de la fila, si la consulta la trajo.
///
/// Que la columna no exista es un caso legítimo —esa lista no fecha sus filas—,
/// no un fallo. Se distingue **por el tipo de error** y no con un `.ok()`: un
/// `.ok()` se tragaría igual un valor corrupto o un tipo que no encaja, que sí
/// son fallos y deben salir por su camino.
fn fecha_de_fila(row: &Row<'_>) -> DbResult<Option<DateTime<Utc>>> {
    match row.get::<_, Option<i64>>(ALIAS_FECHA) {
        Ok(v) => Ok(v.map(a_fecha)),
        Err(rusqlite::Error::InvalidColumnName(_)) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Calcula la disponibilidad a partir de una fila que traiga las columnas de
/// `audio_files` y `download_jobs`.
///
/// Lo usan tanto [`a_track_row`] como las consultas que solo piden estados, para
/// que ambas apliquen exactamente las mismas reglas.
///
/// # Errors
/// Si el formato o el estado de descarga almacenados no son válidos.
pub fn disponibilidad_de_fila(row: &Row<'_>) -> DbResult<Availability> {
    calcular_disponibilidad(
        row.get("rel_path")?,
        row.get("format")?,
        row.get("size_bytes")?,
        row.get("dl_state")?,
        row.get("bytes_done")?,
        row.get("bytes_total")?,
        row.get("attempts")?,
        row.get("last_error")?,
    )
}

/// Decide la disponibilidad a partir del fichero y del trabajo de descarga.
///
/// El orden de las comprobaciones importa y codifica una regla del proyecto:
/// **si hay fichero, la pista es local**, pase lo que pase con el trabajo de
/// descarga. Un `download_jobs` que quedó en `downloading` tras un cierre
/// abrupto no debe hacer que una pista ya presente aparezca como incompleta.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::too_many_arguments,
    reason = "los valores vienen acotados por el esquema y la fracción solo alimenta una barra de progreso"
)]
fn calcular_disponibilidad(
    rel_path: Option<String>,
    formato: Option<String>,
    size_bytes: Option<i64>,
    dl_state: Option<String>,
    bytes_done: Option<i64>,
    bytes_total: Option<i64>,
    attempts: Option<i64>,
    last_error: Option<String>,
) -> DbResult<Availability> {
    if let (Some(path), Some(fmt)) = (rel_path, formato) {
        return Ok(Availability::Local {
            rel_path: std::path::PathBuf::from(path),
            format: a_formato(&fmt)?,
            bytes: size_bytes.unwrap_or(0).max(0) as u64,
        });
    }

    let Some(estado) = dl_state else {
        return Ok(Availability::Absent);
    };

    Ok(match a_estado_descarga(&estado)? {
        DownloadState::Failed => Availability::Failed {
            reason_key: last_error.unwrap_or_else(|| "download.unknown".into()),
            attempts: attempts.unwrap_or(0).clamp(0, 255) as u8,
        },
        DownloadState::Downloading | DownloadState::Finalizing => {
            let hechos = bytes_done.unwrap_or(0).max(0);
            let progress = match bytes_total {
                Some(total) if total > 0 => (hechos as f32 / total as f32).clamp(0.0, 1.0),
                _ => 0.0,
            };
            Availability::Downloading {
                progress,
                playable: hechos as u64
                    >= localify_core::domain::download::BYTES_MINIMOS_REPRODUCIBLE,
            }
        }
        // Encolada o emparejando: todavía no hay nada que reproducir, y desde
        // fuera es indistinguible de no tenerla.
        DownloadState::Queued | DownloadState::Matching => Availability::Absent,
        // `done` sin fichero significa que el registro de audio_files se borró
        // (por ejemplo, tras un rescan que no encontró el fichero).
        DownloadState::Done => Availability::Absent,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn las_fechas_de_lanzamiento_parciales_se_completan() {
        assert_eq!(
            a_fecha_lanzamiento(Some("2011".into())),
            NaiveDate::from_ymd_opt(2011, 1, 1)
        );
        assert_eq!(
            a_fecha_lanzamiento(Some("2011-05".into())),
            NaiveDate::from_ymd_opt(2011, 5, 1)
        );
        assert_eq!(
            a_fecha_lanzamiento(Some("2011-05-17".into())),
            NaiveDate::from_ymd_opt(2011, 5, 17)
        );
        assert_eq!(a_fecha_lanzamiento(None), None);
        assert_eq!(a_fecha_lanzamiento(Some("basura".into())), None);
    }

    #[test]
    fn el_anyo_se_extrae_sin_parsear_la_fecha_entera() {
        assert_eq!(anyo_de(Some("2011-05-17")), Some(2011));
        assert_eq!(anyo_de(Some("2011")), Some(2011));
        assert_eq!(anyo_de(Some("ab")), None);
        assert_eq!(anyo_de(None), None);
    }

    #[test]
    fn las_enumeraciones_hacen_ida_y_vuelta() {
        for e in [
            DownloadState::Queued,
            DownloadState::Matching,
            DownloadState::Downloading,
            DownloadState::Finalizing,
            DownloadState::Done,
            DownloadState::Failed,
        ] {
            assert_eq!(
                a_estado_descarga(de_estado_descarga(e)).expect("ida y vuelta"),
                e
            );
        }
        for c in [Confidence::Low, Confidence::Medium, Confidence::High] {
            assert_eq!(a_confianza(de_confianza(c)), c);
        }
        for m in [RepeatMode::Off, RepeatMode::Queue, RepeatMode::Track] {
            assert_eq!(a_modo_repeticion(de_modo_repeticion(m)), m);
        }
        for p in [Priority::Immediate, Priority::Prefetch] {
            assert_eq!(a_prioridad(de_prioridad(p)), p);
        }
    }

    #[test]
    fn un_estado_de_descarga_desconocido_falla_en_vez_de_degradarse() {
        assert!(
            a_estado_descarga("paused").is_err(),
            "'paused' no existe en el diseño"
        );
        assert!(a_estado_descarga("").is_err());
    }

    #[test]
    fn tener_fichero_gana_a_cualquier_trabajo_de_descarga_pendiente() {
        // Escenario real: cierre abrupto durante la finalización. El fichero ya
        // estaba en su sitio, pero el job quedó en 'downloading'.
        let a = calcular_disponibilidad(
            Some("audio/3z/x.opus".into()),
            Some("opus".into()),
            Some(4_000_000),
            Some("downloading".into()),
            Some(1000),
            Some(9_000_000),
            Some(0),
            None,
        )
        .expect("mapea");

        assert!(
            a.es_local(),
            "una pista con fichero no puede aparecer como incompleta"
        );
    }

    #[test]
    fn una_descarga_con_buffer_suficiente_es_reproducible() {
        let a = calcular_disponibilidad(
            None,
            None,
            None,
            Some("downloading".into()),
            Some(500 * 1024),
            Some(4_000_000),
            Some(0),
            None,
        )
        .expect("mapea");

        assert!(a.es_reproducible_ya());
        assert!((a.progreso() - 0.128).abs() < 0.01);
    }

    #[test]
    fn una_descarga_recien_empezada_aun_no_suena() {
        let a = calcular_disponibilidad(
            None,
            None,
            None,
            Some("downloading".into()),
            Some(1024),
            Some(4_000_000),
            Some(0),
            None,
        )
        .expect("mapea");

        assert!(!a.es_reproducible_ya());
    }

    #[test]
    fn un_trabajo_encolado_es_indistinguible_de_no_tener_la_pista() {
        for estado in ["queued", "matching"] {
            let a = calcular_disponibilidad(
                None,
                None,
                None,
                Some(estado.into()),
                Some(0),
                None,
                Some(0),
                None,
            )
            .expect("mapea");
            assert_eq!(a, Availability::Absent, "estado '{estado}'");
        }
    }

    #[test]
    fn un_fallo_conserva_su_clave_i18n_y_los_intentos() {
        let a = calcular_disponibilidad(
            None,
            None,
            None,
            Some("failed".into()),
            Some(0),
            None,
            Some(3),
            Some("download.no_match".into()),
        )
        .expect("mapea");

        assert_eq!(
            a,
            Availability::Failed {
                reason_key: "download.no_match".into(),
                attempts: 3
            }
        );
    }

    #[test]
    fn sin_fichero_ni_trabajo_la_pista_esta_ausente() {
        let a =
            calcular_disponibilidad(None, None, None, None, None, None, None, None).expect("mapea");
        assert_eq!(a, Availability::Absent);
    }
}

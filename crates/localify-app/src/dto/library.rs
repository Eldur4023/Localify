//! DTOs de biblioteca, playlists, búsqueda, letras e inicio.

use localify_core::domain::library::{ImportReport, LibraryStats};
use localify_core::domain::lyrics::Lyrics;
use localify_core::domain::playlist::{
    PlaylistDetail, PlaylistEntry, PlaylistSource, PlaylistSummary,
};
use localify_core::domain::track::{TrackFilter, TrackSort};
use localify_core::ports::services::{
    GrupoDeVersiones, HomeItems, HomeSection, PrimeraCoincidencia, RemoteResults, SearchResults,
    SearchScope,
};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::catalog::{AlbumRowDto, ArtistRowDto, TrackRowDto};

// ─────────────────────────────────────────────────────────────────────────────
// Biblioteca
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Deserialize, TS)]
#[ts(export, export_to = "types.gen.ts")]
#[serde(rename_all = "camelCase", default)]
pub struct TrackFilterDto {
    pub favorites_only: bool,
    /// La vista Biblioteca lo activa; los resultados de búsqueda no.
    pub local_only: bool,
    pub album_id: Option<String>,
    pub artist_id: Option<String>,
    pub genre_id: Option<i64>,
    pub text: Option<String>,
}

impl TryFrom<TrackFilterDto> for TrackFilter {
    type Error = localify_core::error::CoreError;

    fn try_from(d: TrackFilterDto) -> Result<Self, Self::Error> {
        use localify_core::domain::ids::{AlbumId, ArtistId};
        Ok(Self {
            favorites_only: d.favorites_only,
            local_only: d.local_only,
            album_id: d.album_id.map(AlbumId::parse).transpose()?,
            artist_id: d.artist_id.map(ArtistId::parse).transpose()?,
            genre_id: d.genre_id,
            text: d.text,
        })
    }
}

/// # Errors
/// Si el texto no corresponde a ningún criterio conocido.
pub fn orden_desde_str(s: &str) -> Result<TrackSort, localify_core::error::CoreError> {
    Ok(match s {
        "addedDesc" => TrackSort::AddedDesc,
        "titleAsc" => TrackSort::TitleAsc,
        "artistAsc" => TrackSort::ArtistAsc,
        "albumAsc" => TrackSort::AlbumAsc,
        "durationAsc" => TrackSort::DurationAsc,
        "playCountDesc" => TrackSort::PlayCountDesc,
        "lastPlayedDesc" => TrackSort::LastPlayedDesc,
        otro => {
            return Err(localify_core::error::CoreError::invalid(format!(
                "criterio de ordenación desconocido: '{otro}'"
            )));
        }
    })
}

#[derive(Debug, Clone, Copy, Serialize, TS)]
#[ts(export, export_to = "types.gen.ts")]
#[serde(rename_all = "camelCase")]
pub struct LibraryStatsDto {
    pub track_count: u64,
    /// Cuántas están realmente en disco. Distinguirlo importa: el catálogo
    /// incluye resultados de búsqueda que nunca se han descargado.
    pub local_count: u64,
    pub album_count: u64,
    pub artist_count: u64,
    pub total_duration_ms: u64,
    pub total_bytes: u64,
    /// Canciones cuya descarga falló. Es lo que hace visible un fallo que si no
    /// no se ve en ninguna pantalla.
    pub failed_count: u64,
}

impl From<LibraryStats> for LibraryStatsDto {
    fn from(s: LibraryStats) -> Self {
        Self {
            track_count: s.track_count,
            local_count: s.local_count,
            album_count: s.album_count,
            artist_count: s.artist_count,
            total_duration_ms: s.total_duration_ms,
            total_bytes: s.total_bytes,
            failed_count: s.failed_count,
        }
    }
}

/// Resultado de importar ficheros propios del usuario.
#[derive(Debug, Clone, Copy, Default, Serialize, TS)]
#[ts(export, export_to = "types.gen.ts")]
#[serde(rename_all = "camelCase")]
pub struct ImportReportDto {
    pub files_selected: u32,
    pub imported: u32,
    pub skipped_unreadable: u32,
}

impl From<ImportReport> for ImportReportDto {
    fn from(r: ImportReport) -> Self {
        Self {
            files_selected: r.files_selected,
            imported: r.imported,
            skipped_unreadable: r.skipped_unreadable,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Playlists
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "types.gen.ts")]
#[serde(rename_all = "camelCase")]
pub struct PlaylistSummaryDto {
    pub id: String,
    pub name: String,
    pub track_count: u32,
    /// Álbumes cuyas portadas componen el mosaico, hasta cuatro.
    ///
    /// Identificadores, no rutas: el frontend los pide por `cover://` igual que
    /// los de un álbum, y ninguna ruta de disco cruza el puente (ADR-018).
    pub cover_albums: Vec<String>,
    /// `true` si el usuario eligió una imagen propia. Se pide entonces a
    /// `cover://playlist/<id>` en lugar de componer el mosaico.
    pub has_own_cover: bool,
    pub updated_at: i64,
    pub source: String,
}

const fn origen_a_str(s: PlaylistSource) -> &'static str {
    match s {
        PlaylistSource::Local => "local",
        PlaylistSource::SpotifyImport => "spotifyImport",
    }
}

impl From<PlaylistSummary> for PlaylistSummaryDto {
    fn from(p: PlaylistSummary) -> Self {
        Self {
            id: p.id.to_string(),
            name: p.name,
            track_count: p.track_count,
            cover_albums: p
                .cover_albums
                .into_iter()
                .map(|a| a.as_str().to_owned())
                .collect(),
            has_own_cover: p.has_own_cover,
            updated_at: p.updated_at.timestamp(),
            source: origen_a_str(p.source).to_owned(),
        }
    }
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "types.gen.ts")]
#[serde(rename_all = "camelCase")]
pub struct PlaylistEntryDto {
    /// Identidad de la **entrada**, no de la pista: la misma canción puede
    /// aparecer varias veces y "elimina esta fila" debe ser inequívoco.
    pub entry_id: String,
    pub track: TrackRowDto,
    pub added_at: i64,
}

impl From<PlaylistEntry> for PlaylistEntryDto {
    fn from(e: PlaylistEntry) -> Self {
        Self {
            entry_id: e.entry_id.to_string(),
            track: e.track.into(),
            added_at: e.added_at.timestamp(),
        }
    }
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "types.gen.ts")]
#[serde(rename_all = "camelCase")]
pub struct PlaylistDetailDto {
    #[serde(flatten)]
    pub summary: PlaylistSummaryDto,
    pub description: Option<String>,
    pub total_duration_ms: u32,
    pub entries: Vec<PlaylistEntryDto>,
}

impl From<PlaylistDetail> for PlaylistDetailDto {
    fn from(d: PlaylistDetail) -> Self {
        Self {
            summary: d.summary.into(),
            description: d.description,
            total_duration_ms: d.total_duration.as_ms(),
            entries: d.entries.into_iter().map(Into::into).collect(),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Búsqueda
// ─────────────────────────────────────────────────────────────────────────────

/// # Errors
/// Si el texto no corresponde a ningún ámbito conocido.
pub fn ambito_desde_str(s: &str) -> Result<SearchScope, localify_core::error::CoreError> {
    Ok(match s {
        "all" => SearchScope::All,
        "tracks" => SearchScope::Tracks,
        "albums" => SearchScope::Albums,
        "artists" => SearchScope::Artists,
        "playlists" => SearchScope::Playlists,
        otro => {
            return Err(localify_core::error::CoreError::invalid(format!(
                "ámbito de búsqueda desconocido: '{otro}'"
            )));
        }
    })
}

/// Si el proveedor va a aportar algo más a la lista de canciones.
///
/// Es una unión y no un `Option` porque los cuatro estados significan cosas
/// distintas para la interfaz: "no se preguntó" no es lo mismo que "está en
/// camino" ni que "el proveedor no responde".
///
/// `Ready` no lleva canciones: ya están fundidas en `tracks`. Que vinieran del
/// proveedor o del catálogo no es asunto de quien las pinta.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "types.gen.ts")]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum RemoteResultsDto {
    /// No se consultó al proveedor.
    NotAttempted,
    /// En curso. Llegará `searchRemoteReady` con el mismo `queryId`.
    Loading,
    /// El proveedor ya contestó.
    Ready,
    #[serde(rename_all = "camelCase")]
    Unavailable { reason_key: String },
}

impl From<RemoteResults> for RemoteResultsDto {
    fn from(r: RemoteResults) -> Self {
        match r {
            RemoteResults::NotAttempted => Self::NotAttempted,
            RemoteResults::Loading => Self::Loading,
            RemoteResults::Ready => Self::Ready,
            RemoteResults::Unavailable { reason_key } => Self::Unavailable { reason_key },
        }
    }
}

/// Una canción y sus otras versiones.
///
/// El directo, la instrumental y la maqueta cuelgan de la grabación de estudio
/// en lugar de ocupar cada una su fila. La interfaz enseña `principal` y ofrece
/// desplegar `versiones`: no se esconde nada, se ordena.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "types.gen.ts")]
#[serde(rename_all = "camelCase")]
pub struct GrupoDeVersionesDto {
    pub principal: TrackRowDto,
    pub versiones: Vec<TrackRowDto>,
}

impl From<GrupoDeVersiones> for GrupoDeVersionesDto {
    fn from(g: GrupoDeVersiones) -> Self {
        Self {
            principal: g.principal.into(),
            versiones: g.versiones.into_iter().map(Into::into).collect(),
        }
    }
}

/// Lo que mejor responde a la consulta, sea del tipo que sea.
///
/// Va etiquetado por tipo y no como tres campos opcionales: destacar dos cosas
/// a la vez no significa nada, y con campos sueltos el cliente tendría que
/// decidir a cuál hace caso.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "types.gen.ts")]
#[serde(tag = "kind", content = "item", rename_all = "camelCase")]
pub enum PrimeraCoincidenciaDto {
    Track(TrackRowDto),
    Album(AlbumRowDto),
    Artist(ArtistRowDto),
}

impl From<PrimeraCoincidencia> for PrimeraCoincidenciaDto {
    fn from(p: PrimeraCoincidencia) -> Self {
        match p {
            PrimeraCoincidencia::Track(t) => Self::Track(t.into()),
            PrimeraCoincidencia::Album(a) => Self::Album(a.into()),
            PrimeraCoincidencia::Artist(a) => Self::Artist(a.into()),
        }
    }
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "types.gen.ts")]
#[serde(rename_all = "camelCase")]
pub struct SearchResultsDto {
    /// Monótono. El cliente descarta respuestas de pulsaciones ya superadas.
    pub query_id: u64,
    /// Lo que mejor responde, para destacarlo. `null` si no hay nada claro.
    pub top: Option<PrimeraCoincidenciaDto>,
    /// Las canciones encontradas, ya ordenadas y agrupadas por versiones.
    pub tracks: Vec<GrupoDeVersionesDto>,
    pub albums: Vec<AlbumRowDto>,
    pub artists: Vec<ArtistRowDto>,
    pub playlists: Vec<PlaylistSummaryDto>,
    pub remote: RemoteResultsDto,
}

impl From<SearchResults> for SearchResultsDto {
    fn from(r: SearchResults) -> Self {
        Self {
            query_id: r.query_id,
            top: r.top.map(Into::into),
            tracks: r.tracks.into_iter().map(Into::into).collect(),
            albums: r.albums.into_iter().map(Into::into).collect(),
            artists: r.artists.into_iter().map(Into::into).collect(),
            playlists: r.playlists.into_iter().map(Into::into).collect(),
            remote: r.remote.into(),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Inicio y letras
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "types.gen.ts")]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum HomeItemsDto {
    #[serde(rename_all = "camelCase")]
    Tracks { items: Vec<TrackRowDto> },
    #[serde(rename_all = "camelCase")]
    Albums { items: Vec<AlbumRowDto> },
    #[serde(rename_all = "camelCase")]
    Artists { items: Vec<ArtistRowDto> },
    #[serde(rename_all = "camelCase")]
    Playlists { items: Vec<PlaylistSummaryDto> },
}

impl From<HomeItems> for HomeItemsDto {
    fn from(i: HomeItems) -> Self {
        match i {
            HomeItems::Tracks(v) => Self::Tracks {
                items: v.into_iter().map(Into::into).collect(),
            },
            HomeItems::Albums(v) => Self::Albums {
                items: v.into_iter().map(Into::into).collect(),
            },
            HomeItems::Artists(v) => Self::Artists {
                items: v.into_iter().map(Into::into).collect(),
            },
            HomeItems::Playlists(v) => Self::Playlists {
                items: v.into_iter().map(Into::into).collect(),
            },
        }
    }
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "types.gen.ts")]
#[serde(rename_all = "camelCase")]
pub struct HomeSectionDto {
    /// Clave i18n del título, con sus parámetros: "Porque escuchaste {artist}".
    pub key: String,
    pub params: Vec<(String, String)>,
    pub items: HomeItemsDto,
}

impl From<HomeSection> for HomeSectionDto {
    fn from(s: HomeSection) -> Self {
        Self {
            key: s.key,
            params: s.params,
            items: s.items.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "types.gen.ts")]
#[serde(rename_all = "camelCase")]
pub struct LyricLineDto {
    pub at_ms: u32,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "types.gen.ts")]
#[serde(rename_all = "camelCase")]
pub struct LyricsDto {
    /// Si viene, la interfaz puede resaltar línea a línea.
    pub synced: Option<Vec<LyricLineDto>>,
    pub plain: Option<String>,
    pub source: String,
}

impl From<Lyrics> for LyricsDto {
    fn from(l: Lyrics) -> Self {
        Self {
            synced: l.synced.map(|lineas| {
                lineas
                    .into_iter()
                    .map(|x| LyricLineDto {
                        at_ms: x.at.as_ms(),
                        text: x.text,
                    })
                    .collect()
            }),
            plain: l.plain,
            source: l.source,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn los_criterios_de_orden_se_validan() {
        assert_eq!(
            orden_desde_str("addedDesc").expect("válido"),
            TrackSort::AddedDesc
        );
        assert_eq!(
            orden_desde_str("lastPlayedDesc").expect("válido"),
            TrackSort::LastPlayedDesc
        );
        assert!(orden_desde_str("porLoQueSea").is_err());
    }

    #[test]
    fn los_ambitos_de_busqueda_se_validan() {
        assert_eq!(ambito_desde_str("all").expect("válido"), SearchScope::All);
        assert!(
            ambito_desde_str("youtube").is_err(),
            "no existe búsqueda en YouTube"
        );
    }

    #[test]
    fn los_cuatro_estados_remotos_son_distinguibles() {
        let casos = [
            (RemoteResults::NotAttempted, "notAttempted"),
            (RemoteResults::Loading, "loading"),
            (RemoteResults::Ready, "ready"),
            (
                RemoteResults::Unavailable {
                    reason_key: "error.provider_unavailable".into(),
                },
                "unavailable",
            ),
        ];

        for (estado, esperado) in casos {
            let json = serde_json::to_value(RemoteResultsDto::from(estado)).expect("serializa");
            assert_eq!(json["state"], esperado);
        }
    }

    #[test]
    fn el_filtro_rechaza_identificadores_invalidos() {
        let dto = TrackFilterDto {
            album_id: Some("basura".into()),
            ..Default::default()
        };
        assert!(TrackFilter::try_from(dto).is_err());
    }

    #[test]
    fn el_filtro_vacio_es_valido() {
        let filtro: TrackFilter = TrackFilterDto::default().try_into().expect("válido");
        assert!(!filtro.local_only);
        assert!(filtro.album_id.is_none());
    }

    #[test]
    fn las_secciones_de_inicio_llevan_su_tipo_de_contenido() {
        let seccion = HomeSection {
            key: "home.because_you_listened".into(),
            params: vec![("artist".to_owned(), "Queen".to_owned())],
            items: HomeItems::Tracks(vec![]),
        };
        let json = serde_json::to_value(HomeSectionDto::from(seccion)).expect("serializa");
        assert_eq!(json["key"], "home.because_you_listened");
        assert_eq!(json["items"]["kind"], "tracks");
        assert_eq!(json["params"][0][1], "Queen");
    }
}

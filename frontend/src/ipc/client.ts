/**
 * Cliente IPC.
 *
 * **Único punto del frontend que llama a `invoke`.** Ningún componente ni vista
 * debe hacerlo por su cuenta: concentrarlo aquí es lo que permite añadir
 * reintentos, trazas o caché en un solo sitio, y lo que hace que cambiar el
 * transporte (por ejemplo, a un servidor HTTP) no toque el resto del código.
 *
 * Los tipos vienen de `types.gen.ts`, generado desde los DTOs de Rust. No se
 * escriben a mano (ADR-014).
 */

import type {
  AlbumDetailDto,
  AlbumRowDto,
  ApiError,
  ArtistDetailDto,
  ArtistRowDto,
  AvailabilityDto,
  AudioDeviceDto,
  EqProfileDto,
  HomeSectionDto,
  LastfmAuthDto,
  LibraryStatsDto,
  LyricsDto,
  PageDto,
  PageRequestDto,
  PlaybackContextDto,
  PlayerStateDto,
  PlaylistDetailDto,
  PlaylistSummaryDto,
  PositionDto,
  ProviderStatusDto,
  QueueSnapshotDto,
  SearchResultsDto,
  SettingsDto,
  SettingsPatchDto,
  TrackFilterDto,
  TrackRowDto,
} from "./types.gen.js";

/** Criterios de ordenación aceptados por `library_tracks`. */
export type TrackSort =
  | "addedDesc"
  | "titleAsc"
  | "artistAsc"
  | "albumAsc"
  | "durationAsc"
  | "playCountDesc"
  | "lastPlayedDesc";

export type SearchScope = "all" | "tracks" | "albums" | "artists" | "playlists";
export type RepeatMode = "off" | "queue" | "track";

/**
 * Error de la API con su forma tipada.
 *
 * Se distingue de un fallo del transporte: un `ApiError` es una respuesta
 * legítima del backend que la interfaz sabe traducir, mientras que un fallo de
 * transporte significa que algo está roto de verdad.
 */
export class LocalifyError extends Error {
  constructor(readonly api: ApiError) {
    super(`${api.code}: ${api.messageKey}`);
    this.name = "LocalifyError";
  }

  /** `true` si el usuario puede resolverlo desde Ajustes. */
  get actionable(): boolean {
    return this.api.actionable;
  }

  /** `true` si reintentar la misma operación puede funcionar. */
  get retryable(): boolean {
    return this.api.retryable;
  }
}

/** Forma mínima del puente que expone Tauri en `window`. */
interface TauriBridge {
  core: {
    invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T>;
  };
  event: {
    listen<T>(
      event: string,
      handler: (e: { payload: T }) => void,
    ): Promise<() => void>;
  };
}

declare global {
  interface Window {
    __TAURI__?: TauriBridge;
  }
}

function puente(): TauriBridge {
  const t = window.__TAURI__;
  if (!t) {
    // Ocurre si se abre el HTML fuera de la aplicación. Fallar en voz alta
    // ahorra media hora de depurar respuestas vacías.
    throw new Error(
      "el puente de Tauri no está disponible: ¿se está sirviendo el frontend fuera de la aplicación?",
    );
  }
  return t;
}

/** Invoca un comando y normaliza los errores. */
async function invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await puente().core.invoke<T>(cmd, args);
  } catch (e) {
    // El backend devuelve siempre un `ApiError`; cualquier otra cosa es un
    // fallo del transporte y se propaga tal cual.
    if (typeof e === "object" && e !== null && "code" in e && "messageKey" in e) {
      throw new LocalifyError(e as ApiError);
    }
    throw e;
  }
}

const pagina: PageRequestDto = { offset: 0, limit: null, cursor: null };

/** Construye una petición de paginación con los valores por defecto. */
export function page(patch: Partial<PageRequestDto> = {}): PageRequestDto {
  return { ...pagina, ...patch };
}

// ─────────────────────────────────────────────────────────────────────────────
// API
// ─────────────────────────────────────────────────────────────────────────────

export const player = {
  playTrack: (trackId: string, context: PlaybackContextDto) =>
    invoke<PlayerStateDto>("player_play_track", { trackId, context }),
  toggle: () => invoke<PlayerStateDto>("player_toggle"),
  pause: () => invoke<PlayerStateDto>("player_pause"),
  resume: () => invoke<PlayerStateDto>("player_resume"),
  next: () => invoke<PlayerStateDto>("player_next"),
  previous: () => invoke<PlayerStateDto>("player_previous"),
  seek: (positionMs: number) => invoke<PlayerStateDto>("player_seek", { positionMs }),
  setVolume: (volume: number) => invoke<PlayerStateDto>("player_set_volume", { volume }),
  setRepeat: (mode: RepeatMode) => invoke<PlayerStateDto>("player_set_repeat", { mode }),
  setShuffle: (enabled: boolean) =>
    invoke<PlayerStateDto>("player_set_shuffle", { enabled }),
  /** Estado completo. Es el comando de resincronización tras perder eventos. */
  getState: () => invoke<PlayerStateDto>("player_get_state"),
  /** Se sondea a 4 Hz; la posición no viaja como evento. */
  position: () => invoke<PositionDto>("player_position"),
};

export const queue = {
  get: () => invoke<QueueSnapshotDto>("queue_get"),
  addNext: (trackIds: string[]) =>
    invoke<QueueSnapshotDto>("queue_add_next", { trackIds }),
  addLast: (trackIds: string[]) =>
    invoke<QueueSnapshotDto>("queue_add_last", { trackIds }),
  remove: (entryId: string) => invoke<QueueSnapshotDto>("queue_remove", { entryId }),
  move: (entryId: string, toIndex: number) =>
    invoke<QueueSnapshotDto>("queue_move", { entryId, toIndex }),
  clearUser: () => invoke<QueueSnapshotDto>("queue_clear_user"),
  jumpTo: (entryId: string) => invoke<PlayerStateDto>("queue_jump_to", { entryId }),
};

export const library = {
  tracks: (filter: TrackFilterDto, sort: TrackSort, req: PageRequestDto = page()) =>
    invoke<PageDto<TrackRowDto>>("library_tracks", { filter, sort, page: req }),
  albums: (req: PageRequestDto = page()) =>
    invoke<PageDto<AlbumRowDto>>("library_albums", { page: req }),
  artists: (req: PageRequestDto = page()) =>
    invoke<PageDto<ArtistRowDto>>("library_artists", { page: req }),
  favorites: (req: PageRequestDto = page()) =>
    invoke<PageDto<TrackRowDto>>("library_favorites", { page: req }),
  setFavorite: (trackId: string, enabled: boolean) =>
    invoke<void>("library_set_favorite", { trackId, enabled }),
  recent: (limit: number) => invoke<TrackRowDto[]>("library_recent", { limit }),
  /** Estado de la ventana visible completa, en una sola llamada. */
  availability: (trackIds: string[]) =>
    invoke<[string, AvailabilityDto][]>("library_availability", { trackIds }),
  stats: () => invoke<LibraryStatsDto>("library_stats"),
  rescan: () => invoke<string>("library_rescan"),
  /**
   * Borra el audio descargado de una pista.
   *
   * La pista no desaparece: sigue en sus playlists y en favoritos, y se vuelve
   * a bajar al reproducirla. Es la marcha atrás de una descarga mala.
   */
  deleteDownload: (trackId: string) =>
    invoke<void>("library_delete_download", { trackId }),
  /** Borra todo el audio descargado. Devuelve cuántas pistas. */
  wipeDownloads: () => invoke<number>("library_wipe_downloads"),
  albumDetail: (albumId: string) => invoke<AlbumDetailDto>("album_detail", { albumId }),
  artistDetail: (artistId: string) =>
    invoke<ArtistDetailDto>("artist_detail", { artistId }),
};

export const search = {
  /**
   * Busca. Devuelve lo local de inmediato; lo remoto llega por el evento
   * `searchRemoteReady` con el mismo `queryId`.
   */
  query: (q: string, scope: SearchScope = "all", req: PageRequestDto = page()) =>
    invoke<SearchResultsDto>("search_query", { q, scope, page: req }),
  suggest: (prefix: string, limit: number) =>
    invoke<string[]>("search_suggest", { prefix, limit }),
};

export const playlists = {
  list: () => invoke<PlaylistSummaryDto[]>("playlist_list"),
  create: (name: string) => invoke<PlaylistSummaryDto>("playlist_create", { name }),
  rename: (playlistId: string, name: string) =>
    invoke<void>("playlist_rename", { playlistId, name }),
  remove: (playlistId: string) => invoke<void>("playlist_delete", { playlistId }),
  detail: (playlistId: string, req: PageRequestDto = page()) =>
    invoke<PlaylistDetailDto>("playlist_detail", { playlistId, page: req }),
  addTracks: (playlistId: string, trackIds: string[], atIndex: number | null = null) =>
    invoke<void>("playlist_add_tracks", { playlistId, trackIds, atIndex }),
  removeEntries: (playlistId: string, entryIds: string[]) =>
    invoke<void>("playlist_remove_entries", { playlistId, entryIds }),
  /** Un solo `UPDATE` en el backend: se puede aplicar de forma optimista. */
  reorder: (playlistId: string, entryId: string, toIndex: number) =>
    invoke<void>("playlist_reorder", { playlistId, entryId, toIndex }),
  setDescription: (playlistId: string, description: string | null) =>
    invoke<void>("playlist_set_description", { playlistId, description }),
  /**
   * Importa una lista pública. El destino lo decide la URL, no el catálogo
   * activo: sirve tanto para Spotify como para YouTube Music.
   */
  import: (urlOrId: string) => invoke<string>("playlist_import", { urlOrId }),
  /** Abre el selector del sistema. `null` si se cancela. */
  pickImage: () => invoke<string | null>("playlist_pick_image"),
  setCover: (playlistId: string, imagePath: string) =>
    invoke<void>("playlist_set_cover", { playlistId, imagePath }),
  clearCover: (playlistId: string) =>
    invoke<void>("playlist_clear_cover", { playlistId }),
  suggestions: (playlistId: string, limit: number) =>
    invoke<TrackRowDto[]>("playlist_suggestions", { playlistId, limit }),
};

export const home = {
  sections: () => invoke<HomeSectionDto[]>("home_sections"),
  similarToTrack: (trackId: string, limit: number) =>
    invoke<TrackRowDto[]>("reco_similar_to_track", { trackId, limit }),
};

export const lyrics = {
  /** `null` significa que no hay letra. No es un error. */
  get: (trackId: string) => invoke<LyricsDto | null>("lyrics_get", { trackId }),
};

export const settings = {
  get: () => invoke<SettingsDto>("settings_get"),
  patch: (patch: SettingsPatchDto) => invoke<SettingsDto>("settings_patch", { patch }),
  audioDevices: () => invoke<AudioDeviceDto[]>("settings_audio_devices"),
  eqProfiles: () => invoke<EqProfileDto[]>("settings_eq_profiles"),
  setSpotifyCredentials: (clientId: string, clientSecret: string) =>
    invoke<ProviderStatusDto>("settings_set_spotify_credentials", {
      clientId,
      clientSecret,
    }),
  testSpotify: () => invoke<ProviderStatusDto>("settings_test_spotify"),
  /**
   * Abre en el navegador una página de configuración conocida.
   *
   * El destino es un nombre de una lista cerrada, no una URL: la pone el
   * backend. Así el frontend no puede mandar cualquier cosa al manejador de
   * protocolos del sistema.
   */
  openExternal: (destino: "lastfm_api" | "discord_apps") =>
    invoke<void>("settings_open_external", { destino }),
  setLastfmCredentials: (apiKey: string, apiSecret: string) =>
    invoke<SettingsDto>("settings_set_lastfm_credentials", { apiKey, apiSecret }),
  /**
   * Primer paso de la autenticación: devuelve la URL que hay que abrir.
   *
   * El token vuelve al frontend porque el segundo paso lo necesita. No es un
   * secreto: caduca en una hora y solo vale para esta autorización.
   */
  lastfmBeginAuth: () => invoke<LastfmAuthDto>("settings_lastfm_begin_auth"),
  lastfmCompleteAuth: (token: string) =>
    invoke<SettingsDto>("settings_lastfm_complete_auth", { token }),
  lastfmDisconnect: () => invoke<SettingsDto>("settings_lastfm_disconnect"),
  /** Escuchas pendientes de enviar. Empuja la cola de paso. */
  lastfmPending: () => invoke<number>("settings_lastfm_pending"),
  /**
   * Aplica un ecualizador **sin guardarlo**.
   *
   * Es lo que se llama en cada movimiento de un deslizador: el motor cambia
   * coeficientes sin cortar el sonido, y guardar a ese ritmo serían decenas de
   * transacciones por segundo.
   */
  previewEq: (profile: EqProfileDto) =>
    invoke<void>("settings_preview_eq", { profile }),
  /** Abre el selector nativo. `null` si el usuario cancela. */
  pickFolder: () => invoke<string | null>("settings_pick_folder"),
  /**
   * Cambia la carpeta de la biblioteca.
   *
   * Devuelve al instante el identificador de la operación: con `moveExisting`,
   * copiar la biblioteca puede tardar minutos. El avance llega por
   * `libraryMoveProgress` y el final por `libraryPathChanged`.
   */
  changeLibraryPath: (path: string, moveExisting: boolean) =>
    invoke<string>("settings_change_library_path", { path, moveExisting }),
};

export const system = {
  apiVersion: () => invoke<string>("api_version"),
};

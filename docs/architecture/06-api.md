# 06 — API interna (comandos y eventos Tauri)

Contrato entre el backend y **cualquier** frontend. Está diseñado para que un
segundo cliente (CLI, servidor HTTP, app móvil) pueda consumirlo sin cambiar
nada del backend.

---

## 1. Convenciones

| Aspecto | Regla |
|---|---|
| Nombre | `snake_case`, prefijo por dominio: `player_*`, `library_*`, `playlist_*` |
| Firma | Siempre `async`. Siempre `Result<T, ApiError>` |
| Payload | Un único objeto de argumentos con nombre; nunca posicionales |
| Serialización | `#[serde(rename_all = "camelCase")]` en todos los DTOs |
| Nulos | `Option<T>` → `T \| null`. Nunca se omite un campo declarado |
| Tiempos | milisegundos (`u32`) para duraciones; unix segundos (`i64`) para instantes |
| IDs | siempre `string` |
| Paginación | `{ offset, limit }` o cursor keyset; `limit` máximo 200 |
| Idempotencia | Todos los comandos de mutación son idempotentes o detectan conflicto |
| Errores | Nunca se lanza texto suelto: siempre `ApiError` con `code` + `messageKey` |
| Tipos TS | **Generados** con `ts-rs`. Nunca escritos a mano |

### Tipos base

```ts
interface ApiError {
  code: "NOT_FOUND" | "INVALID" | "CONFLICT" | "PROVIDER_UNAVAILABLE"
      | "RATE_LIMITED" | "NOT_CONFIGURED" | "STORAGE" | "INTERNAL";
  messageKey: string;                       // clave i18n
  params?: Record<string, string>;
  detail?: string;                          // solo en builds de desarrollo
}

interface Page<T> { items: T[]; total: number | null; nextCursor: string | null; }
interface PageReq { offset?: number; limit?: number; cursor?: string | null; }

type Availability =
  | { kind: "absent" }
  | { kind: "downloading"; progress: number }     // 0..1
  | { kind: "local"; format: string; bytes: number }
  | { kind: "failed"; reasonKey: string; attempts: number };

/** Fila de lista: plana, estrecha, una sola consulta. */
interface TrackRow {
  id: string;
  title: string;
  artistDisplay: string;
  albumId: string | null;
  albumTitle: string | null;
  durationMs: number;
  availability: Availability;
  isFavorite: boolean;
  explicit: boolean;
}

/** Detalle: solo cuando se abre una vista concreta. */
interface TrackDetail extends TrackRow {
  artists: { id: string; name: string }[];
  trackNumber: number | null;
  discNumber: number | null;
  isrc: string | null;
  releaseDate: string | null;
  addedAt: number;
  playCount: number;
  lastPlayedAt: number | null;
}
```

---

## 2. Reproductor — `player_*`

| Comando | Argumentos | Devuelve | Notas |
|---|---|---|---|
| `player_play_track` | `{ trackId, context }` | `PlayerState` | Punto de entrada principal. Descarga si hace falta, de forma transparente |
| `player_toggle` | — | `PlayerState` | |
| `player_pause` | — | `PlayerState` | |
| `player_resume` | — | `PlayerState` | |
| `player_next` | — | `PlayerState` | |
| `player_previous` | — | `PlayerState` | < 3 s reproducidos → pista anterior; si no, reinicia la actual |
| `player_seek` | `{ positionMs }` | `PlayerState` | Si supera lo descargado → `buffering`, no error |
| `player_set_volume` | `{ volume }` | `PlayerState` | 0.0–1.0, curva perceptual aplicada en el motor |
| `player_set_repeat` | `{ mode }` | `PlayerState` | `off` \| `queue` \| `track` |
| `player_set_shuffle` | `{ enabled }` | `PlayerState` | Genera/descarta la permutación estable |
| `player_get_state` | — | `PlayerState` | **Comando de resincronización** |
| `player_position` | — | `{ positionMs, bufferedMs }` | Lee atómicos. Sondeado a 4 Hz. Coste ≈ 0 |

```ts
type PlaybackContext =
  | { kind: "album";      id: string }
  | { kind: "playlist";   id: string }
  | { kind: "artist";     id: string }
  | { kind: "liked" }
  | { kind: "library" }
  | { kind: "search";     query: string; trackIds: string[] }
  | { kind: "recommendation"; seedTrackId: string; trackIds: string[] }
  | { kind: "single" };

interface PlayerState {
  track: TrackRow | null;
  status: "playing" | "paused" | "buffering" | "stopped";
  positionMs: number;
  durationMs: number;
  bufferedMs: number;
  volume: number;
  repeat: "off" | "queue" | "track";
  shuffle: boolean;
  context: PlaybackContext | null;
}
```

---

## 3. Cola — `queue_*`

| Comando | Argumentos | Devuelve |
|---|---|---|
| `queue_get` | — | `QueueSnapshot` |
| `queue_add_next` | `{ trackIds }` | `QueueSnapshot` |
| `queue_add_last` | `{ trackIds }` | `QueueSnapshot` |
| `queue_remove` | `{ entryId }` | `QueueSnapshot` |
| `queue_move` | `{ entryId, toIndex }` | `QueueSnapshot` |
| `queue_clear_user` | — | `QueueSnapshot` |
| `queue_jump_to` | `{ entryId }` | `PlayerState` |

```ts
interface QueueSnapshot {
  revision: number;                 // monótono; descarta respuestas obsoletas
  current: QueueEntry | null;
  userQueue: QueueEntry[];          // "Siguiente en la cola" — prioridad absoluta
  contextQueue: QueueEntry[];       // "Siguiente desde: <contexto>" (ventana de 50)
  contextLabel: string | null;
}
interface QueueEntry { entryId: string; track: TrackRow; }
```

---

## 4. Biblioteca — `library_*`

| Comando | Argumentos | Devuelve |
|---|---|---|
| `library_tracks` | `{ filter, sort, page }` | `Page<TrackRow>` |
| `library_albums` | `{ filter, page }` | `Page<AlbumRow>` |
| `library_artists` | `{ page }` | `Page<ArtistRow>` |
| `library_track_detail` | `{ trackId }` | `TrackDetail` |
| `library_set_favorite` | `{ trackId, enabled }` | `void` |
| `library_favorites` | `{ page }` | `Page<TrackRow>` |
| `library_recent` | `{ limit }` | `TrackRow[]` |
| `library_availability` | `{ trackIds }` | `Record<string, Availability>` |
| `library_stats` | — | `LibraryStats` |
| `library_rescan` | — | `{ scanId }` |

```ts
interface TrackFilter {
  favoritesOnly?: boolean;
  localOnly?: boolean;              // por defecto true en la vista Biblioteca
  albumId?: string;
  artistId?: string;
  genreId?: number;
  text?: string;
}
type TrackSort = "addedDesc" | "titleAsc" | "artistAsc" | "albumAsc"
               | "durationAsc" | "playCountDesc" | "lastPlayedDesc";

interface LibraryStats {
  trackCount: number; localCount: number; albumCount: number;
  artistCount: number; totalDurationMs: number; totalBytes: number;
}
```

`library_availability` existe para que la lista virtualizada pida el estado de
las ~40 filas visibles de golpe, en una sola llamada. Sin él, habría una
llamada por fila al hacer scroll.

---

## 5. Búsqueda — `search_*`

| Comando | Argumentos | Devuelve |
|---|---|---|
| `search_query` | `{ q, scope, page }` | `SearchResults` |
| `search_suggest` | `{ prefix, limit }` | `Suggestion[]` |

```ts
interface SearchResults {
  queryId: number;                  // monótono: descarta respuestas de pulsaciones viejas
  local: {
    tracks: TrackRow[];
    albums: AlbumRow[];
    artists: ArtistRow[];
    playlists: PlaylistSummary[];
  };
  remote:
    | { state: "notAttempted" }     // había suficiente en local
    | { state: "loading" }          // llegará SearchRemoteReady
    | { state: "ready"; tracks: TrackRow[]; albums: AlbumRow[]; artists: ArtistRow[] }
    | { state: "unavailable"; reasonKey: string };
}
```

**Contrato de uso desde el frontend:**
1. En **cada** pulsación → `search_query` (local, < 30 ms).
2. Con debounce de 180 ms, si `remote.state === "loading"` → esperar el evento
   `searchRemoteReady` con el mismo `queryId` y volver a llamar.
3. Descartar toda respuesta cuyo `queryId` sea menor que el último visto.

Nunca se expone un comando para buscar en YouTube. No existe en la API. Es una
decisión arquitectónica, no una omisión: YouTube es un detalle interno de la
capa de descarga.

---

## 6. Álbumes y artistas

| Comando | Argumentos | Devuelve |
|---|---|---|
| `album_detail` | `{ albumId }` | `AlbumDetail` |
| `album_play` | `{ albumId, startIndex? }` | `PlayerState` |
| `artist_detail` | `{ artistId }` | `ArtistDetail` |
| `artist_play_top` | `{ artistId }` | `PlayerState` |

```ts
interface AlbumDetail {
  id: string; title: string;
  artists: { id: string; name: string }[];
  releaseDate: string | null; albumType: string;
  coverUrl: string | null;                 // asset:// local si está cacheada
  totalDurationMs: number;
  tracks: TrackRow[];                      // completo: un álbum nunca excede ~50 pistas
  localCount: number;
}

interface ArtistDetail {
  id: string; name: string; imageUrl: string | null;
  genres: string[];
  topTracks: TrackRow[];
  albums: AlbumRow[];
  localTrackCount: number;
}
```

---

## 7. Playlists — `playlist_*`

| Comando | Argumentos | Devuelve |
|---|---|---|
| `playlist_list` | — | `PlaylistSummary[]` |
| `playlist_create` | `{ name }` | `PlaylistSummary` |
| `playlist_rename` | `{ playlistId, name }` | `void` |
| `playlist_delete` | `{ playlistId }` | `void` |
| `playlist_detail` | `{ playlistId, page }` | `PlaylistDetail` |
| `playlist_add_tracks` | `{ playlistId, trackIds, atIndex? }` | `void` |
| `playlist_remove_entries` | `{ playlistId, entryIds }` | `void` |
| `playlist_reorder` | `{ playlistId, entryId, toIndex }` | `void` |
| `playlist_set_cover` | `{ playlistId, imagePath }` | `void` |
| `playlist_play` | `{ playlistId, startIndex? }` | `PlayerState` |
| `playlist_import_spotify` | `{ urlOrId }` | `{ importId }` |
| `playlist_suggestions` | `{ playlistId, limit }` | `TrackRow[]` |

```ts
interface PlaylistSummary {
  id: string; name: string; trackCount: number;
  coverPaths: string[];             // 1 propia, o hasta 4 para el mosaico
  updatedAt: number; source: "local" | "spotifyImport";
}
interface PlaylistDetail extends PlaylistSummary {
  description: string | null;
  totalDurationMs: number;
  entries: { entryId: string; track: TrackRow; addedAt: number }[];
}
```

`playlist_reorder` responde inmediatamente (una sola fila actualizada gracias a
las claves fraccionarias) y emite `playlistChanged`. El frontend aplica el
movimiento **de forma optimista** en el DOM y solo revierte si el comando falla.

`playlist_import_spotify` devuelve al instante un `importId`; el progreso llega
por `playlistImportProgress`.

---

## 8. Recomendaciones y letras

| Comando | Argumentos | Devuelve |
|---|---|---|
| `home_sections` | — | `HomeSection[]` |
| `reco_similar_to_track` | `{ trackId, limit }` | `TrackRow[]` |
| `lyrics_get` | `{ trackId }` | `Lyrics \| null` |

```ts
interface HomeSection {
  key: string;                      // clave i18n del título
  params?: Record<string, string>;  // p.ej. { artist: "Queen" }
  kind: "tracks" | "albums" | "artists" | "playlists";
  items: (TrackRow | AlbumRow | ArtistRow | PlaylistSummary)[];
}
interface Lyrics {
  synced: { atMs: number; text: string }[] | null;
  plain: string | null;
  source: string;
}
```

`lyrics_get` devuelve `null` cuando no hay letra. **No es un error.** La UI
oculta el panel sin decir nada, como pide el prompt.

---

## 9. Configuración — `settings_*`

| Comando | Argumentos | Devuelve |
|---|---|---|
| `settings_get` | — | `Settings` |
| `settings_patch` | `{ patch }` | `Settings` |
| `settings_pick_library_folder` | — | `{ path } \| null` |
| `settings_change_library_path` | `{ path, moveExisting }` | `{ migrationId }` |
| `settings_audio_devices` | — | `AudioDevice[]` |
| `settings_eq_profiles` | — | `EqProfile[]` |
| `settings_set_spotify_credentials` | `{ clientId, clientSecret }` | `ProviderStatus` |
| `settings_test_spotify` | — | `ProviderStatus` |

```ts
interface Settings {
  language: "es" | "en";
  libraryPath: string;
  audio: {
    crossfadeMs: number;            // 0–12000
    gapless: boolean;
    eqProfileId: string;
    eqBands: number[];              // 10 bandas, −12..+12 dB
    normalizeVolume: boolean;
    outputDeviceId: string | null;  // null = predeterminado del sistema
  };
  download: {
    preferredFormat: "opus" | "m4a" | "best";
    maxConcurrent: number;          // 1–4
    maxRetries: number;
  };
  spotify: { configured: boolean; clientId: string | null };  // el secret NUNCA sale
  integrations: {
    discordEnabled: boolean;
  };
  ui: { listDensity: "compact" | "comfortable"; startView: string };
}
```

El `clientSecret` **nunca** se devuelve al frontend. `settings_get` solo indica
si está configurado.

---

## 10. Integraciones y diagnóstico

| Comando | Argumentos | Devuelve |
|---|---|---|
| `integrations_discord_set` | `{ enabled }` | `void` |
| `diagnostics_report` | — | `DiagnosticsReport` |
| `diagnostics_open_logs` | — | `void` |
| `sidecars_status` | — | `{ ytDlp: SidecarStatus; ffmpeg: SidecarStatus }` |
| `sidecars_update` | — | `{ updated: string[] }` |
| `updates_open_release_page` | — | `void` |

`updates_open_release_page` no acepta ninguna URL desde el frontend: abre la
que Rust guardó al comprobar contra los releases de GitHub (evento
`updateAvailable`), nunca una que llegue como argumento. Mismo motivo que
`settings_open_external`.

---

## 11. Eventos

Canal único: `localify://event`, con payload discriminado por `type`.
Canal auxiliar: `localify://resync` (sin payload) cuando el bus pierde
mensajes; el frontend recarga su estado y sigue.

```ts
type LocalifyEvent =
  // Reproducción
  | { type: "trackChanged";        trackId: string; source: "user" | "queue" | "restore" }
  | { type: "playStatusChanged";   status: PlayerState["status"] }
  | { type: "volumeChanged";       volume: number }
  | { type: "repeatModeChanged";   mode: PlayerState["repeat"] }
  | { type: "shuffleChanged";      enabled: boolean }
  | { type: "trackFinished";       trackId: string; completed: boolean }
  // Cola
  | { type: "queueChanged";        revision: number }
  // Descargas (invisibles para el usuario; solo actualizan indicadores sutiles)
  | { type: "downloadStarted";     trackId: string }
  | { type: "downloadPlayable";    trackId: string }
  | { type: "downloadProgress";    trackId: string; percent: number }   // máx. 2 Hz
  | { type: "downloadCompleted";   trackId: string }
  | { type: "downloadFailed";      trackId: string; reasonKey: string }
  | { type: "availabilityChanged"; trackId: string; availability: Availability }
  // Biblioteca y playlists
  | { type: "libraryChanged";      scope: "tracks" | "albums" | "artists" | "favorites" }
  | { type: "playlistChanged";     playlistId: string; kind: "created" | "renamed" | "deleted" | "items" }
  | { type: "playlistImportProgress"; importId: string; done: number; total: number }
  | { type: "playlistImportFinished"; importId: string; playlistId: string }
  | { type: "scanProgress";        scanId: string; done: number; total: number }
  // Búsqueda
  | { type: "searchRemoteReady";   queryId: number }
  // Sistema
  | { type: "settingsChanged";     sections: string[] }
  | { type: "providerStatusChanged"; provider: "spotify" | "discord"; status: string }
  | { type: "updateAvailable";     version: string }
  | { type: "toast";               level: "info" | "warn" | "error"; messageKey: string;
                                   params?: Record<string, string> };
```

**Reglas de los eventos**

1. Llevan IDs y deltas, nunca objetos grandes. El consumidor consulta si
   necesita más.
2. `positionMs` **no** se emite: se sondea con `player_position`.
3. Todo evento tiene un comando equivalente para reconstruir el estado
   completo. Perder eventos degrada la reactividad, nunca la corrección.
4. Los eventos de progreso están throttleados en el **backend**, antes de
   publicarse. El frontend no debe tener que defenderse de una avalancha.

---

## 12. Seguridad y permisos (Tauri v2)

`capabilities/default.json` concede el mínimo estricto:

- `core:event:allow-listen`, `core:event:allow-emit-to`
- `dialog:allow-open` (solo para elegir la carpeta de biblioteca)
- `shell:allow-open` (solo para abrir la carpeta de logs)
- **Sin** `fs:*` general: el frontend no toca el sistema de ficheros. Las
  portadas se sirven con el protocolo `asset:` restringido a la carpeta
  `covers/`.
- **Sin** `shell:allow-execute`: los sidecars los invoca Rust, nunca el
  WebView.
- CSP estricta: `default-src 'self'; img-src 'self' asset: data:;
  style-src 'self' 'unsafe-inline'; script-src 'self'`.

El WebView no accede a red directamente. Todas las peticiones HTTP salen de
Rust, donde se controlan cabeceras, timeouts y rate limits.

---

## 13. Versionado

`api_version()` devuelve un `semver`. El frontend lo comprueba al arrancar y
avisa si es incompatible (escenario real cuando en el futuro haya frontends
externos). Reglas:

- **Patch**: cambios internos, sin efecto en el contrato.
- **Minor**: comandos, campos opcionales o variantes de evento nuevos. Los
  clientes existentes siguen funcionando.
- **Major**: se elimina o cambia el significado de algo existente. Requiere
  entrada en `CHANGELOG.md` y guía de migración.

Añadir un campo obligatorio a un DTO de respuesta es un cambio **minor**;
añadirlo a un DTO de petición es **major**.

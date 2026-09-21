/**
* Error de la API con su forma tipada.
*
* Se distingue de un fallo del transporte: un `ApiError` es una respuesta
* legítima del backend que la interfaz sabe traducir, mientras que un fallo de
* transporte significa que algo está roto de verdad.
*/
export class LocalifyError extends Error {
	api;
	constructor(api) {
		super(`${api.code}: ${api.messageKey}`);
		this.api = api;
		this.name = "LocalifyError";
	}
	/** `true` si el usuario puede resolverlo desde Ajustes. */
	get actionable() {
		return this.api.actionable;
	}
	/** `true` si reintentar la misma operación puede funcionar. */
	get retryable() {
		return this.api.retryable;
	}
}
/**
 * El transporte del puente es la ÚNICA diferencia con el frontend original:
 * en Localify era `window.__TAURI__.core.invoke`; aquí es HTTP loopback
 * (`/api/invoke`), que es el equivalente natural en Lux. La posición del
 * reproductor y los diálogos nativos viven en el transporte, porque en este
 * port el elemento <audio> (y la ventana GTK) están del lado del WebView.
 */
import { invocar } from "./transport.js";
async function invoke(cmd, args) {
	try {
		return await invocar(cmd, args ?? {});
	} catch (e) {
		// El backend devuelve siempre un `ApiError`; cualquier otra cosa es un
		// fallo del transporte y se propaga tal cual.
		if (typeof e === "object" && e !== null && "code" in e && "messageKey" in e) {
			throw new LocalifyError(e);
		}
		throw e;
	}
}
const pagina = {
	offset: 0,
	limit: null,
	cursor: null
};
/** Construye una petición de paginación con los valores por defecto. */
export function page(patch = {}) {
	return {
		...pagina,
		...patch
	};
}
// ─────────────────────────────────────────────────────────────────────────────
// API
// ─────────────────────────────────────────────────────────────────────────────
export const player = {
	playTrack: (trackId, context) => invoke("player_play_track", {
		trackId,
		context
	}),
	toggle: () => invoke("player_toggle"),
	pause: () => invoke("player_pause"),
	resume: () => invoke("player_resume"),
	next: () => invoke("player_next"),
	previous: () => invoke("player_previous"),
	seek: (positionMs) => invoke("player_seek", { positionMs }),
	setVolume: (volume) => invoke("player_set_volume", { volume }),
	setRepeat: (mode) => invoke("player_set_repeat", { mode }),
	setShuffle: (enabled) => invoke("player_set_shuffle", { enabled }),
	/** Estado completo. Es el comando de resincronización tras perder eventos. */
	getState: () => invoke("player_get_state"),
	/** Se sondea a 4 Hz; la posición no viaja como evento. */
	position: () => invoke("player_position")
};
export const queue = {
	get: () => invoke("queue_get"),
	addNext: (trackIds) => invoke("queue_add_next", { trackIds }),
	addLast: (trackIds) => invoke("queue_add_last", { trackIds }),
	remove: (entryId) => invoke("queue_remove", { entryId }),
	move: (entryId, toIndex) => invoke("queue_move", {
		entryId,
		toIndex
	}),
	clearUser: () => invoke("queue_clear_user"),
	jumpTo: (entryId) => invoke("queue_jump_to", { entryId })
};
export const library = {
	tracks: (filter, sort, req = page()) => invoke("library_tracks", {
		filter,
		sort,
		page: req
	}),
	albums: (req = page()) => invoke("library_albums", { page: req }),
	artists: (req = page()) => invoke("library_artists", { page: req }),
	favorites: (req = page()) => invoke("library_favorites", { page: req }),
	setFavorite: (trackId, enabled) => invoke("library_set_favorite", {
		trackId,
		enabled
	}),
	recent: (limit) => invoke("library_recent", { limit }),
	/** Estado de la ventana visible completa, en una sola llamada. */
	availability: (trackIds) => invoke("library_availability", { trackIds }),
	stats: () => invoke("library_stats"),
	rescan: () => invoke("library_rescan"),
	/**
	* Borra el audio descargado de una pista.
	*
	* La pista no desaparece: sigue en sus playlists y en favoritos, y se vuelve
	* a bajar al reproducirla. Es la marcha atrás de una descarga mala.
	*/
	deleteDownload: (trackId) => invoke("library_delete_download", { trackId }),
	/** Borra todo el audio descargado. Devuelve cuántas pistas. */
	wipeDownloads: () => invoke("library_wipe_downloads"),
	/**
	* Vuelve a encolar lo que falló al descargarse. Devuelve cuántas.
	*
	* Es la única salida de un fallo de emparejamiento: sin esto, una canción que
	* no se pudo bajar se quedaba así para siempre.
	*/
	retryFailed: () => invoke("library_retry_failed"),
	albumDetail: (albumId) => invoke("album_detail", { albumId }),
	artistDetail: (artistId) => invoke("artist_detail", { artistId }),
	/** Abre el selector nativo de audio. Vacío si el usuario cancela. */
	pickImportFiles: () => invoke("library_pick_import_files"),
	/** Importa los ficheros elegidos, para que convivan con lo descargado. */
	importFiles: (paths) => invoke("library_import_files", { paths }),
	/**
	* Borra la pista del catálogo entero: playlists, favoritos e historial se
	* van con ella. A diferencia de `deleteDownload`, no tiene marcha atrás
	* fácil — pide confirmación antes de llamar a esto.
	*/
	deleteTrack: (trackId) => invoke("library_delete_track", { trackId }),
	/** Vuelve una pista a "sin identificar". El audio no se toca. */
	resetMetadata: (trackId) => invoke("library_reset_metadata", { trackId }),
	/** Candidatos del proveedor activo para reasignar metadatos a mano. */
	searchCandidates: (query, limit) => invoke("library_search_candidates", {
		query,
		limit
	}),
	/** Reasigna los metadatos de una pista al candidato elegido. */
	assignMetadata: (trackId, candidate) => invoke("library_assign_metadata", {
		trackId,
		candidate
	})
};
export const search = {
	/**
	* Busca. Devuelve lo local de inmediato; lo remoto llega por el evento
	* `searchRemoteReady` con el mismo `queryId`.
	*/
	query: (q, scope = "all", req = page()) => invoke("search_query", {
		q,
		scope,
		page: req
	}),
	suggest: (prefix, limit) => invoke("search_suggest", {
		prefix,
		limit
	})
};
export const playlists = {
	list: () => invoke("playlist_list"),
	create: (name) => invoke("playlist_create", { name }),
	rename: (playlistId, name) => invoke("playlist_rename", {
		playlistId,
		name
	}),
	remove: (playlistId) => invoke("playlist_delete", { playlistId }),
	detail: (playlistId, req = page()) => invoke("playlist_detail", {
		playlistId,
		page: req
	}),
	addTracks: (playlistId, trackIds, atIndex = null) => invoke("playlist_add_tracks", {
		playlistId,
		trackIds,
		atIndex
	}),
	removeEntries: (playlistId, entryIds) => invoke("playlist_remove_entries", {
		playlistId,
		entryIds
	}),
	/** Un solo `UPDATE` en el backend: se puede aplicar de forma optimista. */
	reorder: (playlistId, entryId, toIndex) => invoke("playlist_reorder", {
		playlistId,
		entryId,
		toIndex
	}),
	/** Reordena la playlist entre sus hermanas, en la barra lateral. */
	reorderList: (playlistId, toIndex) => invoke("playlist_reorder_list", {
		playlistId,
		toIndex
	}),
	setDescription: (playlistId, description) => invoke("playlist_set_description", {
		playlistId,
		description
	}),
	/**
	* Importa una lista pública. El destino lo decide la URL, no el catálogo
	* activo: sirve tanto para Spotify como para YouTube Music.
	*/
	import: (urlOrId) => invoke("playlist_import", { urlOrId }),
	/** Abre el selector del sistema. `null` si se cancela. */
	pickImage: () => invoke("playlist_pick_image"),
	setCover: (playlistId, imagePath) => invoke("playlist_set_cover", {
		playlistId,
		imagePath
	}),
	clearCover: (playlistId) => invoke("playlist_clear_cover", { playlistId }),
	suggestions: (playlistId, limit) => invoke("playlist_suggestions", {
		playlistId,
		limit
	})
};
export const home = {
	sections: () => invoke("home_sections"),
	similarToTrack: (trackId, limit) => invoke("reco_similar_to_track", {
		trackId,
		limit
	})
};
export const lyrics = { 
/** `null` significa que no hay letra. No es un error. */
get: (trackId) => invoke("lyrics_get", { trackId }) };
export const settings = {
	get: () => invoke("settings_get"),
	patch: (patch) => invoke("settings_patch", { patch }),
	audioDevices: () => invoke("settings_audio_devices"),
	eqProfiles: () => invoke("settings_eq_profiles"),
	setSpotifyCredentials: (clientId, clientSecret) => invoke("settings_set_spotify_credentials", {
		clientId,
		clientSecret
	}),
	testSpotify: () => invoke("settings_test_spotify"),
	/**
	* Abre en el navegador una página de configuración conocida.
	*
	* El destino es un nombre de una lista cerrada, no una URL: la pone el
	* backend. Así el frontend no puede mandar cualquier cosa al manejador de
	* protocolos del sistema.
	*/
	openExternal: (destino) => invoke("settings_open_external", { destino }),
	/**
	* Aplica un ecualizador **sin guardarlo**.
	*
	* Es lo que se llama en cada movimiento de un deslizador: el motor cambia
	* coeficientes sin cortar el sonido, y guardar a ese ritmo serían decenas de
	* transacciones por segundo.
	*/
	previewEq: (profile) => invoke("settings_preview_eq", { profile }),
	/** Abre el selector nativo. `null` si el usuario cancela. */
	pickFolder: () => invoke("settings_pick_folder"),
	/** Selector nativo para el fichero de cookies en formato Netscape. */
	pickCookies: () => invoke("settings_pick_cookies"),
	/**
	* Comprueba que las cookies configuradas sirven de verdad.
	*
	* Elegir un navegador en un desplegable no garantiza nada: yt-dlp puede no
	* saber descifrar su almacén. Sin esta comprobación uno se entera tres
	* canciones después, cuando el fallo ya no se parece a lo que tocó.
	*/
	testCookies: () => invoke("settings_test_cookies"),
	/** Fuerza la comprobación de versión de yt-dlp, que ya se hace al arrancar. */
	updateYtdlp: () => invoke("settings_update_ytdlp"),
	/**
	* Cambia la carpeta de la biblioteca.
	*
	* Devuelve al instante el identificador de la operación: con `moveExisting`,
	* copiar la biblioteca puede tardar minutos. El avance llega por
	* `libraryMoveProgress` y el final por `libraryPathChanged`.
	*/
	changeLibraryPath: (path, moveExisting) => invoke("settings_change_library_path", {
		path,
		moveExisting
	})
};
export const stats = { 
/** Tiempo total escuchado y las canciones y artistas que más se llevan. */
get: () => invoke("stats_get") };
export const system = { apiVersion: () => invoke("api_version") };
export const updates = { 
/**
* Abre en el navegador la página del release que se detectó disponible.
*
* No se le pasa ninguna URL: la decide Rust con lo último que encontró la
* comprobación de fondo. Mismo motivo que `settings.openExternal`.
*/
openReleasePage: () => invoke("updates_open_release_page") };

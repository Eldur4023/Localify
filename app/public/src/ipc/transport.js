/**
 * transport.js — El puente de este port.
 *
 * Sustituye a `window.__TAURI__` con tres piezas:
 *
 * 1. `invocar(cmd, args)`: HTTP loopback contra `/api/invoke`, el despachador
 *    que habla el dialecto de comandos de Localify. Dos comandos NO viajan:
 *    `player_position` (la posición la dueña el <audio> activo de este
 *    documento) y los diálogos nativos (`window.open_file` de Lux), que son
 *    locales por definición.
 *
 * 2. El motor de audio: DOS elementos <audio> que se alternan (ver "Gapless
 *    y crossfade reales" más abajo), siguiendo el estado del backend. En
 *    Localify el sonido salía del motor Rust; aquí lo decodifica GStreamer
 *    dentro del WebView, y el backend sigue siendo la fuente de verdad de
 *    QUÉ suena — el <audio> activo decide únicamente CUÁNDO exactamente pasa
 *    de una pista a la siguiente. El ecualizador de 10 bandas se monta con
 *    Web Audio (biquads peaking en cascada, Q=√2, coeficientes como el DSP
 *    original), compartido por los dos elementos.
 *
 * 3. El bombeo de eventos: sondea `/api/events/poll` y sintetiza los eventos
 *    del bus de Localify con sus nombres exactos, comparando el snapshot
 *    anterior y el nuevo.
 *
 * ## Gapless y crossfade reales
 *
 * Un único <audio> no puede sonar sin hueco: cargar la siguiente pista
 * (`src` + `load()`) siempre tarda un poco, y antes de esto el cambio de
 * pista ni siquiera se sabía hasta que el poll (hasta 1 s) se enteraba de
 * que la pista había cambiado en el backend.
 *
 * La solución es la misma que usa cualquier reproductor con crossfade real:
 * dos elementos <audio> (`slots`), uno "activo" (el que se oye) y otro
 * "siguiente" (precargado por adelantado). `actualizarPrediccion()` le
 * pregunta al backend qué sonaría después (cola de usuario > cola de
 * contexto > repetición de pista) y lo precarga en el slot inactivo en
 * cuanto se sabe — no cuando la pista actual está a punto de acabar. Un
 * temporizador corto vigila cuánto le queda a la pista activa: si el
 * crossfade está a 0 ms (gapless), el cambio de slot ocurre en el mismo
 * evento `ended` del elemento activo (hueco solo del orden de un tick de
 * evento, no de una petición de red); si crossfadeMs > 0, empieza a subir
 * la ganancia del slot siguiente y bajar la del activo esa cantidad de
 * milisegundos antes del final, con los dos sonando a la vez de verdad
 * (mezclados por Web Audio, no un fundido de CSS ni nada simulado).
 *
 * Esto solo se activa para transiciones PREVISTAS (la pista que llega
 * coincide con la que ya se había precargado). Un salto manual — el usuario
 * elige otra canción de la biblioteca — no se pudo prededecir, así que cae
 * al camino de siempre: cargar y sonar en el acto en el slot activo, sin
 * fundido que hacer.
 */

const slots = [new Audio(), new Audio()];
slots.forEach((a, i) => { a.id = "audio" + i; document.body.append(a); });
let activo = 0; // índice en `slots` del elemento que se oye ahora mismo
const audioActivo = () => slots[activo];
const audioInactivo = () => slots[1 - activo];

// ── Ecualizador de 10 bandas (Web Audio) ────────────────────────────────────
// BANDAS_EQ_HZ del original: 31..16k, biquads peaking con Q=1.41. Los dos
// <audio> comparten la misma cadena de filtros; cada uno llega a ella por su
// propio GainNode, que es también el que usa el crossfade para mezclarlos.

const BANDAS = [31, 62, 125, 250, 500, 1000, 2000, 4000, 8000, 16000];
let cadenaEq = null; // { ctx, filtros, ganancias: [GainNode, GainNode] }
let gananciasEq = new Float32Array(10);

function montarEq() {
	try {
		const ctx = new AudioContext();
		let nodo = null;
		const filtros = BANDAS.map((hz) => {
			const f = ctx.createBiquadFilter();
			f.type = "peaking";
			f.frequency.value = hz;
			f.Q.value = 1.41;
			f.gain.value = 0;
			if (nodo) nodo.connect(f);
			nodo = f;
			return f;
		});
		filtros[filtros.length - 1].connect(ctx.destination);
		const ganancias = slots.map((a, i) => {
			const fuente = ctx.createMediaElementSource(a);
			const gain = ctx.createGain();
			gain.gain.value = i === activo ? 1 : 0;
			fuente.connect(gain);
			gain.connect(filtros[0]);
			return gain;
		});
		cadenaEq = { ctx, filtros, ganancias };
	} catch {
		// Sin Web Audio (o sin user gesture aún): suena sin EQ; se reintentará
		// en la primera reproducción.
	}
}

function aplicarGanancias() {
	if (!cadenaEq) montarEq();
	if (!cadenaEq) return;
	cadenaEq.filtros.forEach((f, i) => {
		f.gain.value = gananciasEq[i] ?? 0;
	});
}

function aplicarVolumenPerceptual(v) {
	// El oído responde de forma logarítmica: v³ es la curva del motor
	// original. Se aplica a los dos elementos: el volumen del usuario no
	// depende de cuál esté activo ahora mismo, y durante un crossfade los
	// dos están sonando de verdad.
	const vol = Math.min(1, Math.max(0, v)) ** 3;
	slots.forEach((a) => { a.volume = vol; });
}

// ── Crossfade / gapless ──────────────────────────────────────────────────────

let crossfadeMsCache = 0;
let siguientePrevisto = null; // { id, disponible } | null — lo que se precargó en el slot inactivo
let crossfadeEnMarcha = false;
let prediccionEnMarcha = false;
// Id que el cliente ya adoptó por su cuenta (crossfade/gapless que acaban de
// conmutar) pero que el backend puede tardar hasta 1 poll (~1 s) en
// confirmar: mientras tanto sigue reportando la pista vieja. Sin esta
// bandera, sincronizarPista() se creía ese informe atrasado y trataba de
// recargar la pista vieja encima de la que acaba de empezar a sonar.
let transicionClienteId = null;

/** Le pregunta al backend qué pista sonaría después y la precarga (sin sonar) en el slot inactivo. */
async function actualizarPrediccion(estado) {
	if (prediccionEnMarcha || !estado?.track) return;
	// El slot inactivo puede seguir sonando de verdad todavía: el elemento
	// que un crossfade acaba de dejar "inactivo" sigue audible, apagándose,
	// hasta su propio "ended" natural. Tocar su `src` ahora lo cortaría en
	// seco. Esperar a que esté en pausa de verdad es sencillo y correcto: la
	// próxima llamada (1 s después, como mucho) lo intenta de nuevo.
	if (!audioInactivo().paused) return;
	prediccionEnMarcha = true;
	try {
		let siguienteId = null;
		if (estado.repeat === "track") {
			siguienteId = estado.track.id;
		} else {
			const r = await fetch("/api/invoke", {
				method: "POST",
				headers: { "Content-Type": "application/json" },
				body: JSON.stringify({ cmd: "queue_get", args: "{}" }),
			});
			const q = await r.json().catch(() => null);
			const siguiente = q?.userQueue?.[0]?.track ?? q?.contextQueue?.[0]?.track ?? null;
			if (siguiente?.availability?.kind === "local") siguienteId = siguiente.id;
		}
		if (!siguienteId || siguienteId === audioActivo().dataset.trackId) {
			siguientePrevisto = null;
			return;
		}
		if (siguientePrevisto?.id === siguienteId) return; // ya precargado
		const inactivo = audioInactivo();
		inactivo.dataset.trackId = siguienteId;
		inactivo.src = `/audio/${siguienteId}`;
		inactivo.load();
		siguientePrevisto = { id: siguienteId };
	} catch {
		// sin red/servidor: la siguiente predicción lo reintenta
	} finally {
		prediccionEnMarcha = false;
	}
}

/**
 * Cambia al vuelo cuál de los dos <audio> es "el activo" — sin fundido: el
 * que entra ya está sonando a ganancia 1, el que sale se para y se limpia en
 * el acto. Es el camino gapless (crossfadeMs = 0): lo llama el propio
 * `ended` del elemento activo, así que el hueco es solo el de manejar ese
 * evento, no el de pedir y cargar una URL nueva.
 */
function conmutarGapless() {
	const previo = audioActivo();
	const entrante = audioInactivo();
	transicionClienteId = entrante.dataset.trackId ?? null;
	entrante.currentTime = 0;
	entrante.play().catch(() => {});
	activo = 1 - activo;
	previo.pause();
	previo.currentTime = 0;
	delete previo.dataset.trackId;
	siguientePrevisto = null;
	if (cadenaEq) {
		const t = cadenaEq.ctx.currentTime;
		cadenaEq.ganancias[activo].gain.cancelScheduledValues(t);
		cadenaEq.ganancias[activo].gain.setValueAtTime(1, t);
		cadenaEq.ganancias[1 - activo].gain.cancelScheduledValues(t);
		cadenaEq.ganancias[1 - activo].gain.setValueAtTime(0, t);
	}
}

/**
 * Arranca el fundido cruzado: el slot siguiente empieza a sonar YA, mezclado
 * de verdad con el activo (dos <audio> sonando a la vez, no un fundido
 * simulado). A diferencia del gapless, el elemento saliente NO se para aquí:
 * sigue sonando (bajando de ganancia) hasta que llega solo a su propio
 * `ended` natural, que es lo que de verdad avisa al backend de que la pista
 * cambió — cortarlo a mano aquí perdería ese aviso.
 */
function iniciarCrossfade() {
	if (crossfadeEnMarcha || !siguientePrevisto) return;
	const entrante = audioInactivo();
	if (entrante.readyState < 2) return; // sin datos suficientes todavía: probar en el próximo tick
	crossfadeEnMarcha = true;
	entrante.currentTime = 0;
	entrante.play().catch(() => {});
	if (cadenaEq) {
		const t = cadenaEq.ctx.currentTime;
		const dur = Math.max(0.05, crossfadeMsCache / 1000);
		cadenaEq.ganancias[activo].gain.cancelScheduledValues(t);
		cadenaEq.ganancias[activo].gain.setValueAtTime(cadenaEq.ganancias[activo].gain.value, t);
		cadenaEq.ganancias[activo].gain.linearRampToValueAtTime(0, t + dur);
		cadenaEq.ganancias[1 - activo].gain.cancelScheduledValues(t);
		cadenaEq.ganancias[1 - activo].gain.setValueAtTime(0, t);
		cadenaEq.ganancias[1 - activo].gain.linearRampToValueAtTime(1, t + dur);
	}
	// La pista "actual" pasa a ser la entrante desde ya (a efectos de
	// posición/UI): perceptualmente ya es la protagonista. El elemento
	// saliente sigue vivo por debajo, ver comentario de la función — pero
	// SIN su `dataset.trackId`: el backend tarda hasta 1 poll (≈1 s) en
	// enterarse de la transición, y hasta entonces sigue reportando la
	// pista vieja. Sin borrar esto aquí, sincronizarPista() confundía ese
	// informe con "la pista precargada en el slot inactivo" (que ahora es
	// justo este elemento) y deshacía el crossfade recién arrancado.
	const saliente = audioActivo();
	transicionClienteId = entrante.dataset.trackId ?? null;
	activo = 1 - activo;
	delete saliente.dataset.trackId;
	siguientePrevisto = null;
}

// Vigila cuánto le queda a la pista activa y decide cuándo el slot
// siguiente (si ya está precargado) tiene que empezar a sonar. 200 ms es
// sobrado de margen para un crossfade que como mínimo dura medio segundo
// (el ajuste va de 0 a 12 s en pasos de 500 ms).
setInterval(() => {
	const a = audioActivo();
	if (a.paused || !siguientePrevisto || crossfadeEnMarcha) return;
	if (!isFinite(a.duration) || a.duration <= 0) return;
	const restanteMs = (a.duration - a.currentTime) * 1000;
	if (crossfadeMsCache > 0 && restanteMs <= crossfadeMsCache) {
		iniciarCrossfade();
	}
}, 200);

// ── Estado local sincronizado ───────────────────────────────────────────────

let ultimoEstado = null;
let resumePendiente = null; // { id, ms } | null — posición a restaurar en cuanto haya metadata

/**
 * Fin natural de UNO de los dos elementos — el backend decide qué viene
 * después (repeat track/queue, historial…). Era el AdvanceReason::NaturalEnd
 * del PlaybackActor. Puede llegar del elemento activo (nadie hizo crossfade
 * todavía: si hay algo precargado, es el momento de conmutar en el acto,
 * modo gapless) o del que acaba de dejar de ser activo tras un crossfade que
 * ya conmutó antes (aquí solo queda avisar al backend).
 */
function alTerminarPista(e) {
	const el = e.target;
	if (el === audioActivo() && siguientePrevisto && audioInactivo().readyState >= 2) {
		conmutarGapless();
	} else if (el !== audioActivo()) {
		// La cola de un crossfade que ya conmutó antes, terminando de bajar
		// de ganancia hasta su propio final: limpiarla deja el slot listo
		// para la próxima predicción en vez de con el id de la pista vieja.
		delete el.dataset.trackId;
		// Aquí es donde el crossfade termina de verdad — sin esto,
		// `crossfadeEnMarcha` se quedaba en true para siempre y ningún
		// crossfade futuro volvía a arrancar en toda la sesión.
		crossfadeEnMarcha = false;
	}
	const ms = Math.round((el.duration || 0) * 1000);
	fetch("/api/player/ended", {
		method: "POST",
		headers: { "Content-Type": "application/json" },
		body: JSON.stringify({ msPlayed: ms }),
	}).catch(() => {});
}

// La posición guardada solo puede aplicarse una vez el <audio> conoce su
// duración (tras load()); esperar aquí en vez de comprobar isFinite() en el
// mismo tick del poll evita perder el resume cuando la metadata aún no llegó.
function alCargarMetadata(e) {
	const el = e.target;
	if (!resumePendiente || resumePendiente.id !== el.dataset.trackId) return;
	const ms = resumePendiente.ms;
	resumePendiente = null;
	if (isFinite(el.duration) && el.duration > 0) {
		el.currentTime = Math.min(ms / 1000, el.duration);
	}
}

slots.forEach((a) => {
	a.addEventListener("ended", alTerminarPista);
	// Arrancar el AudioContext con el primer gesto (política de autoplay).
	a.addEventListener("play", () => {
		if (cadenaEq === null) montarEq();
		if (cadenaEq && cadenaEq.ctx.state === "suspended") cadenaEq.ctx.resume();
	});
	a.addEventListener("loadedmetadata", alCargarMetadata);
});

/**
 * Carga en el elemento activo la pista que el backend dice sonar — salvo
 * que ya sea, de hecho, lo que estaba precargado en el slot inactivo (una
 * transición prevista, la cola avanzó tal cual se esperaba): en ese caso no
 * hay nada que cargar, gapless/crossfade ya lo dejaron sonando.
 */
function sincronizarPista(track, positionMs) {
	const id = track?.id ?? "";
	if (transicionClienteId) {
		if (id === transicionClienteId) {
			transicionClienteId = null; // el backend ya se enteró: seguir por el camino normal
		} else {
			return; // informe del backend todavía atrasado respecto al crossfade/gapless: ignorarlo
		}
	}
	if (id === audioActivo().dataset.trackId) return; // ya es lo que suena (crossfade/gapless ya conmutó)
	if (id && id === audioInactivo().dataset.trackId && audioInactivo().readyState >= 2) {
		// El backend confirma una transición que ya habíamos previsto y
		// precargado, pero que el temporizador de crossfade/gapless aún no
		// disparó (crossfade a 0 ms sin que "ended" haya llegado todavía, por
		// ejemplo: el poll se adelantó). Conmutar ahora es mejor que cargar
		// la misma pista dos veces.
		conmutarGapless();
		return;
	}
	resumePendiente = null;
	const el = audioActivo();
	if (!id) {
		el.removeAttribute("src");
		delete el.dataset.trackId;
		el.load();
		return;
	}
	const disp = track.availability;
	el.dataset.trackId = id;
	if (disp && disp.kind === "local") {
		if (positionMs > 3000) resumePendiente = { id, ms: positionMs };
		el.src = `/audio/${id}`;
		el.load();
	} else {
		// descarga en curso: sin fuente todavía; el evento de progreso
		// reintentará (la disponibilidad cambia a "local" al terminar)
		el.removeAttribute("src");
		el.load();
	}
}

// ── Bombeo de eventos ───────────────────────────────────────────────────────

let alEvento = null;
let alResync = null;
let bombear = false;

function emitir(type, extra = {}) {
	if (!alEvento) return;
	alEvento({ type, ...extra });
}

/** Diferencia dos snapshots y emite los eventos del bus original que correspondan. */
function diferir(antes, ahora) {
	if (!antes) return;
	if (ahora.revision !== antes.revision) emitir("queueChanged", { revision: ahora.revision });
	if (ahora.trackId !== antes.trackId) emitir("trackChanged", { trackId: ahora.trackId, source: ahora.source ?? "queue" });
	if (ahora.status !== antes.status) emitir("playStatusChanged", { status: ahora.status });
	if (ahora.volume !== antes.volume) emitir("volumeChanged", { volume: ahora.volume });
	if (ahora.repeat !== antes.repeat) emitir("repeatModeChanged", { mode: ahora.repeat });
	if (ahora.shuffle !== antes.shuffle) emitir("shuffleChanged", { enabled: ahora.shuffle });

	// descargas: progreso, completado (→ disponibilidad) y fallo
	const previas = new Map(antes.downloads.map((d) => [d.trackId, d]));
	const vistas = new Set();
	for (const d of ahora.downloads) {
		vistas.add(d.trackId);
		const p = previas.get(d.trackId);
		if (d.state === "downloading") {
			const total = d.bytesTotal > 0 ? d.bytesTotal : 0;
			// fracción 0..1 — igual que `availability.progress` en el DTO inicial;
			// pintarProgreso() ya multiplica por 100 al pintar
			const frac = total > 0 ? Math.min(1, d.bytesDone / total) : 0;
			// limitado a 2 Hz por descarga, como en el emisor original
			const ahora_ = Date.now();
			const clave = "throttle:" + d.trackId;
			const ultima = moduleState.get(clave) ?? 0;
			if (ahora_ - ultima >= 500) {
				moduleState.set(clave, ahora_);
				emitir("downloadProgress", { trackId: d.trackId, percent: frac });
			}
		}
		if (p && p.state !== "failed" && d.state === "failed") {
			emitir("downloadFailed", { trackId: d.trackId, reasonKey: d.lastError ?? "download.failed", attempts: d.attempts });
			emitir("availabilityChanged", {
				trackId: d.trackId,
				availability: { kind: "failed", reasonKey: d.lastError ?? "download.failed", attempts: d.attempts },
			});
		}
	}
	for (const [tid, p] of previas) {
		if (!vistas.has(tid) && p.state !== "failed") {
			// el trabajo desapareció: o terminó bien (se borra la fila al
			// finalizar) o lo borró un wipe
			emitir("downloadCompleted", { trackId: tid });
			emitir("availabilityChanged", {
				trackId: tid,
				availability: { kind: "local", format: "opus", bytes: 0 },
			});
		}
	}

	if (ahora.libraryStamp !== antes.libraryStamp) emitir("libraryChanged", {});
	if (ahora.playlistStamp !== antes.playlistStamp) emitir("playlistChanged", { kind: ahora.playlistChangeKind ?? "" });
	if (ahora.settingsStamp !== antes.settingsStamp) emitir("settingsChanged", {});
	if (ahora.libraryPath !== antes.libraryPath) emitir("libraryPathChanged", { path: ahora.libraryPath });
	for (const r of ahora.remoteReady ?? []) {
		emitir("searchRemoteReady", { queryId: r.queryId });
	}
}

const moduleState = new Map();
let pollEnMarcha = false;

async function tick() {
	if (pollEnMarcha) return;
	pollEnMarcha = true;
	try {
		const r = await fetch("/api/events/poll", { method: "POST" });
		const snap = await r.json();
		// el estado del reproductor llega gratis con el mismo poll
		if (snap.estado) ultimoEstado = snap.estado;
		sincronizarDesde(ultimoEstado);
		diferir(moduleState.get("prev") ?? null, snap);
		moduleState.set("prev", snap);
		if (snap.resync) {
			// pista distinta a la que el <audio> tenía cargada al arrancar
			if (alResync) alResync();
		}
	} catch {
		// sin servidor (reinicio de la app): el siguiente tick reintenta
	}
	pollEnMarcha = false;
}

export function arrancarEventos(alEventoCb, alResyncCb) {
	alEvento = alEventoCb;
	alResync = alResyncCb;
	if (bombear) return;
	bombear = true;
	setInterval(tick, 1000);
	tick();
	iniciarEqGuardado();
}

/** Aplica el estado del backend al elemento activo: qué suena, play/pause, volumen. */
function sincronizarDesde(estado) {
	if (!estado) return;
	sincronizarPista(estado.track, estado.positionMs ?? 0);
	const a = audioActivo();
	const quiere = estado.status === "playing";
	if (quiere && estado.track?.availability?.kind === "local") {
		if (a.paused) a.play().catch(() => {});
	} else if (!quiere) {
		a.pause();
		// Un crossfade puede seguir sonando por debajo (la cola del elemento
		// que se está apagando): pausar solo el activo dejaría ese resto
		// audible aunque la interfaz diga "en pausa".
		if (!audioInactivo().paused) audioInactivo().pause();
	}
	if (quiere) void actualizarPrediccion(estado);
}

// ── Invocación ──────────────────────────────────────────────────────────────

/** Diálogos nativos: los abre la ventana GTK (módulo window de Lux). */
async function dialogoAbrir() {
	const r = await fetch("/api/native/open-file", { method: "POST" });
	if (!r.ok) return "";
	const j = await r.json();
	return j.path ?? "";
}

/**
 * Invoca un comando. La posición de reproducción y los diálogos no viajan:
 * el primero vive en este documento, los segundos en la ventana nativa.
 */
export async function invocar(cmd, args) {
	// ── comandos que se responden en el cliente ──
	if (cmd === "player_position") {
		const a = audioActivo();
		const dur = isFinite(a.duration) ? a.duration * 1000 : 0;
		const pos = a.currentTime * 1000;
		return {
			positionMs: Math.round(pos),
			bufferedMs: Math.round(dur > 0 ? dur : pos),
		};
	}
	if (cmd === "library_pick_import_files" || cmd === "settings_pick_cookies") {
		const ruta = await dialogoAbrir();
		return ruta ? [ruta] : [];
	}
	if (cmd === "settings_pick_folder") {
		const ruta = await dialogoAbrir();
		return ruta || null;
	}
	if (cmd === "playlist_pick_image") {
		const ruta = await dialogoAbrir();
		return ruta || null;
	}
	if (cmd === "playlist_set_cover") {
		// el original copiaba la imagen elegida; aquí manda la ruta al backend.
		// `args` va como STRING JSON, no como objeto anidado: la clase que lo
		// recibe en el backend (PeticionInvoke) declara `args` como string —
		// un objeto anidado aquí no deserializaba y este comando nunca hacía
		// nada.
		return await fetch("/api/invoke", {
			method: "POST",
			headers: { "Content-Type": "application/json" },
			body: JSON.stringify({ cmd, args: JSON.stringify(args) }),
		}).then(async (r) => {
			const j = await r.json().catch(() => null);
			if (!r.ok) throw j ?? { code: "INTERNAL", messageKey: "error.internal", params: [], actionable: false, retryable: false };
			return j;
		});
	}

	// la posición local viaja como metadato para la regla de los 3 s y el
	// historial: el backend del original la conocía de primera mano
	const conPos = { ...args };
	if (["player_toggle", "player_pause", "player_next", "player_previous"].includes(cmd)) {
		conPos.__posMs = Math.round((audioActivo().currentTime || 0) * 1000);
	}

	const r = await fetch("/api/invoke", {
		method: "POST",
		headers: { "Content-Type": "application/json" },
		body: JSON.stringify({ cmd, args: JSON.stringify(conPos) }),
	});
	const j = await r.json().catch(() => null);
	if (!r.ok) {
		throw j ?? { code: "INTERNAL", messageKey: "error.internal", params: [], actionable: false, retryable: false };
	}
	// ajustes de audio que el elemento activo aplica en el acto
	if (cmd === "player_set_volume" && args && typeof args.volume === "number") {
		aplicarVolumenPerceptual(args.volume);
	}
	if (cmd === "player_seek" && args && typeof args.positionMs === "number") {
		const a = audioActivo();
		if (isFinite(a.duration)) a.currentTime = Math.min(args.positionMs / 1000, a.duration || args.positionMs / 1000);
	}
	// se decide con `j` (la respuesta fresca de ESTA llamada), no con el poll
	// cacheado: dos toggles dentro de la misma ventana de 1s leerían el mismo
	// estado obsoleto y podrían dejar el audio sonando cuando el backend ya
	// dice "en pausa".
	if (cmd === "player_toggle") {
		if (j?.status === "playing" && j?.track?.availability?.kind === "local") {
			audioActivo().play().catch(() => {});
		} else {
			audioActivo().pause();
			if (!audioInactivo().paused) audioInactivo().pause(); // ver comentario en sincronizarDesde
		}
	} else if (cmd === "player_pause") {
		audioActivo().pause();
		if (!audioInactivo().paused) audioInactivo().pause();
	} else if (cmd === "player_resume" || cmd === "player_play_track") {
		if (j?.track?.availability?.kind === "local") audioActivo().play().catch(() => {});
	}
	if (j && typeof j === "object" && "status" in j && "track" in j) {
		ultimoEstado = j;
	}
	if (cmd === "settings_patch" || cmd === "settings_preview_eq") {
		const perfil = args?.patch?.audio?.eqProfile ?? args?.profile;
		if (perfil && Array.isArray(perfil.gainsDb)) {
			gananciasEq = new Float32Array(perfil.gainsDb);
			aplicarGanancias();
		}
	}
	if (cmd === "settings_patch" && typeof args?.patch?.audio?.crossfadeMs === "number") {
		crossfadeMsCache = args.patch.audio.crossfadeMs;
	}
	return j;
}

/** Arranque: aplica el EQ y el crossfade guardados en cuanto haya respuesta de ajustes. */
export async function iniciarEqGuardado() {
	try {
		const r = await fetch("/api/invoke", {
			method: "POST",
			headers: { "Content-Type": "application/json" },
			body: JSON.stringify({ cmd: "settings_get", args: "{}" }),
		});
		const s = await r.json();
		const ganancias = s?.audio?.eqProfile?.gainsDb;
		if (Array.isArray(ganancias)) {
			gananciasEq = new Float32Array(ganancias);
			aplicarGanancias();
		}
		if (typeof s?.audio?.crossfadeMs === "number") crossfadeMsCache = s.audio.crossfadeMs;
		// el volumen guardado lo aplica el primer sondeo de estado; aquí solo
		// montamos el EQ persistido
	} catch {}
}

// ── Diagnóstico: los fallos de la UI llegan al log del backend ──────────────
// Sin devtools en el empaquetado, un error de consola era invisible. Cada
// error de script o promesa rechazada se reporta a /api/client-log y queda
// en el log del servidor con su stack.

function reportar(tipo, detalle) {
	try {
		fetch("/api/client-log", {
			method: "POST",
			headers: { "Content-Type": "application/json" },
			body: JSON.stringify({ tipo, detalle: String(detalle).slice(0, 2000) }),
		}).catch(() => {});
	} catch {}
}

window.addEventListener("error", (e) => {
	reportar("error", `${e.message} @ ${e.filename}:${e.lineno}:${e.colno}`);
});
window.addEventListener("unhandledrejection", (e) => {
	reportar("promesa", e.reason && e.reason.stack ? e.reason.stack : String(e.reason));
});

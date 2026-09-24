/**
 * transport.js — El puente de este port.
 *
 * Sustituye a `window.__TAURI__` con:
 *
 * 1. `invocar(cmd, args)`: HTTP loopback contra `/api/invoke`, el despachador
 *    que habla el dialecto de comandos de Localify. Las órdenes del
 *    reproductor van por el controlador (reproductor.js), que es quien sabe
 *    del motor de audio; los diálogos nativos, por la ventana GTK.
 *
 * 2. El bombeo de eventos: sondea `/api/events/poll` y sintetiza los eventos
 *    del bus de Localify con sus nombres exactos, comparando el snapshot
 *    anterior y el nuevo. El backend puede además pedir un sondeo inmediato
 *    (`window.__localifyDespertar`, vía window.eval_js) cuando una orden le
 *    llega de fuera de la ventana -- teclas multimedia, bandeja, la tarjeta
 *    del escritorio --: el sondeo normal va a 1 s, y WebKitGTK lo frena a
 *    varios segundos con la ventana minimizada.
 */
import * as reproductor from "./reproductor.js";

// ── Bombeo de eventos ───────────────────────────────────────────────────────

let alEvento = null;
let alResync = null;
let bombear = false;

function emitir(type, extra = {}) {
	if (!alEvento) return;
	alEvento({ type, ...extra });
}

reproductor.conectar((evento) => {
	if (alEvento) alEvento(evento);
});

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
	if (ahora.statsStamp !== antes.statsStamp) emitir("statsChanged", {});
	if (ahora.libraryPath !== antes.libraryPath) emitir("libraryPathChanged", { path: ahora.libraryPath });
	for (const r of ahora.remoteReady ?? []) {
		emitir("searchRemoteReady", { queryId: r.queryId });
	}
}

const moduleState = new Map();
let pollEnMarcha = false;
let pollOtraVez = false;

async function tick() {
	if (pollEnMarcha) {
		// Un despertar durante un sondeo en curso no se pierde: se repite al
		// acabar, porque la respuesta en curso puede ser anterior a la orden.
		pollOtraVez = true;
		return;
	}
	pollEnMarcha = true;
	try {
		const r = await fetch("/api/events/poll", { method: "POST" });
		const snap = await r.json();
		if (snap.estado) reproductor.aplicar(snap.estado, true);
		diferir(moduleState.get("prev") ?? null, snap);
		moduleState.set("prev", snap);
		if (snap.resync) {
			if (alResync) alResync();
		}
	} catch {
		// sin servidor (reinicio de la app): el siguiente tick reintenta
	}
	pollEnMarcha = false;
	if (pollOtraVez) {
		pollOtraVez = false;
		void tick();
	}
}

export function arrancarEventos(alEventoCb, alResyncCb) {
	alEvento = alEventoCb;
	alResync = alResyncCb;
	if (bombear) return;
	bombear = true;
	window.__localifyDespertar = () => void tick();
	setInterval(tick, 1000);
	tick();
	void cargarAjustesAudio();
}

/** Arranque: EQ y crossfade guardados. El volumen llega con el estado. */
async function cargarAjustesAudio() {
	try {
		const r = await fetch("/api/invoke", {
			method: "POST",
			headers: { "Content-Type": "application/json" },
			body: JSON.stringify({ cmd: "settings_get", args: "{}" }),
		});
		reproductor.aplicarAjustesAudio((await r.json())?.audio);
	} catch {}
}

// ── Invocación ──────────────────────────────────────────────────────────────

/** Diálogos nativos: los abre la ventana GTK (módulo window de Lux). */
async function dialogoAbrir() {
	const r = await fetch("/api/native/open-file", { method: "POST" });
	if (!r.ok) return "";
	const j = await r.json();
	return j.path ?? "";
}

/** Invoca un comando. */
export async function invocar(cmd, args) {
	if (cmd.startsWith("player_") || cmd === "queue_jump_to") {
		return await reproductor.orden(cmd, args ?? {});
	}
	if (cmd === "library_pick_import_files" || cmd === "settings_pick_cookies") {
		const ruta = await dialogoAbrir();
		return ruta ? [ruta] : [];
	}
	if (cmd === "settings_pick_folder" || cmd === "playlist_pick_image") {
		const ruta = await dialogoAbrir();
		return ruta || null;
	}

	// `args` va como STRING JSON, no como objeto anidado: la clase que lo
	// recibe en el backend (PeticionInvoke) declara `args` como string.
	const r = await fetch("/api/invoke", {
		method: "POST",
		headers: { "Content-Type": "application/json" },
		body: JSON.stringify({ cmd, args: JSON.stringify(args ?? {}) }),
	});
	const j = await r.json().catch(() => null);
	if (!r.ok) {
		throw j ?? { code: "INTERNAL", messageKey: "error.internal", params: [], actionable: false, retryable: false };
	}
	// ajustes de audio que el motor aplica en el acto
	if (cmd === "settings_patch" && args?.patch?.audio) {
		reproductor.aplicarAjustesAudio(args.patch.audio);
	}
	if (cmd === "settings_preview_eq") {
		reproductor.previsualizarEq(args?.profile?.gainsDb);
	}
	return j;
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

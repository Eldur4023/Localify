/**
 * transport.js — El puente de este port.
 *
 * Sustituye a `window.__TAURI__` con:
 *
 * 1. `invocar(cmd, args)`: HTTP loopback contra `/api/invoke`, el despachador
 *    que habla el dialecto de comandos de Localify. Todo, también el
 *    reproductor: el backend es su único dueño y mueve el motor de audio
 *    nativo; esto solo manda órdenes. Los diálogos nativos, por la ventana GTK.
 *
 * 2. Los eventos: el backend EMPUJA una foto de su estado por SSE
 *    (`/api/events/stream`) cada vez que cambia, y aquí se sintetizan, por
 *    diferencias entre fotos, los eventos del bus de Localify con sus nombres
 *    exactos. No hay sondeo: un temporizador de página lo frena WebKitGTK a
 *    varios segundos con la ventana minimizada, y el servidor no depende de él.
 */

// ── Bombeo de eventos ───────────────────────────────────────────────────────

let alEvento = null;
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
	if (ahora.statsStamp !== antes.statsStamp) emitir("statsChanged", {});
	// salto pedido por otro equipo (reproducción sincronizada): el `<audio>` del móvil no tiene motor
	if (ahora.syncSeek && ahora.syncSeek !== antes.syncSeek) {
		emitir("remoteSeek", { positionMs: Number(ahora.syncSeek.split(":")[1]) || 0 });
	}
	if (ahora.libraryPath !== antes.libraryPath) emitir("libraryPathChanged", { path: ahora.libraryPath });
	for (const r of ahora.remoteReady ?? []) {
		emitir("searchRemoteReady", { queryId: r.queryId });
	}
}

const moduleState = new Map();

function aplicarFoto(snap) {
	// La posición es la del motor nativo, que viaja en cada foto mientras suena.
	if (snap.estado?.track && snap.estado.status === "playing") {
		emitir("positionTick", { positionMs: snap.estado.positionMs, durationMs: snap.estado.durationMs });
	}
	diferir(moduleState.get("prev") ?? null, snap);
	moduleState.set("prev", snap);
}

export function arrancarEventos(alEventoCb) {
	alEvento = alEventoCb;
	if (bombear) return;
	bombear = true;
	const fuente = new EventSource("/api/events/stream");
	fuente.addEventListener("snapshot", (m) => {
		try {
			aplicarFoto(JSON.parse(m.data));
		} catch (e) {
			reportar("eventos", e && e.stack ? e.stack : String(e));
		}
	});
	// EventSource reconecta solo; al volver, el servidor manda la foto actual.
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

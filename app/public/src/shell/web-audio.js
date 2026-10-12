/**
 * Reproducción por `<audio>` del WebView — solo en el móvil.
 *
 * En escritorio el motor nativo (audio_engine.cpp) es quien suena y quien
 * manda la posición; en el shell Android esos hooks no existen y quien suena
 * tiene que ser el propio WebView. Este módulo se activa solo si
 * `/api/capabilities` dice `mobile` y traduce en los dos sentidos:
 *
 *   backend → <audio>   trackChanged (src), playStatusChanged (play/pause),
 *                       volumeChanged (volumen); seek fino vía el gancho
 *                       `window.__localifyWebAudio.seek()` que llama
 *                       client.js antes de mandar el comando al backend
 *   <audio> → backend   "ended" (para que la cola avance: sin motor el
 *                       backend jamás se entera de que acabó la pista) y la
 *                       posición cada pocos segundos (ídem: audio.status()
 *                       no sabe nada allí)
 *
 * Además: MediaSession (título/artista en la pantalla de bloqueo) y el
 * servicio en primer plano vía `AndroidBridge` (que el proceso no muera —y
 * con él la música— al pasar a segundo plano).
 */

import { alRecibir } from "../ipc/events.js";
import { player } from "../ipc/client.js";

/** Arranca el shim si hace falta. Lo llama main.js tras montar la UI. */
export async function arrancarAudioWeb() {
	let caps;
	try {
		caps = await fetch("/api/capabilities").then((r) => r.json());
	} catch {
		return; // sin capabilities no hay shim: el motor nativo manda
	}
	if (!caps?.mobile) return;

	const audio = new Audio();
	audio.preload = "auto";
	let quererSonar = false;
	let pistaActual = "";

	const informar = (cuerpo) => {
		void fetch("/api/player/media-event", {
			method: "POST",
			headers: { "Content-Type": "application/json" },
			body: JSON.stringify(cuerpo)
		}).catch(() => {});
	};

	// ── MediaSession nativa (AndroidBridge.setMedia): el WebView no publica la
	// suya al sistema, así que pantalla de bloqueo, notificación, auriculares y
	// coche se alimentan desde aquí ──
	let meta = { title: "", artist: "", durationMs: 0 };
	const puente = window.AndroidBridge;
	const publicar = () => {
		try {
			puente?.setMedia?.(
				meta.title, meta.artist, meta.durationMs, !audio.paused,
				Math.round((audio.currentTime || 0) * 1000),
				pistaActual ? `${location.origin}/api/cover/track/${encodeURIComponent(pistaActual)}` : ""
			);
		} catch {
			// sin puente: solo la MediaSession web
		}
	};
	audio.addEventListener("play", publicar);
	audio.addEventListener("pause", publicar);
	audio.addEventListener("seeked", publicar);
	// Los botones del sistema (MediaSession nativa) entran por aquí.
	window.__localifyNative = {
		cmd(nombre, arg) {
			switch (nombre) {
				case "play": void player.resume(); break;
				case "pause": void player.pause(); break;
				case "next": void player.next(); break;
				case "previous": void player.previous(); break;
				case "seek": void player.seek(arg); break;
				default: break;
			}
		}
	};

	// ── MediaSession web ──
	const ponerMetadata = async (trackId) => {
		try {
			const info = await fetch(`/api/track-meta/${encodeURIComponent(trackId)}`)
				.then((r) => (r.ok ? r.json() : null));
			if ("mediaSession" in navigator) navigator.mediaSession.metadata = new MediaMetadata({
				title: info?.title || "Localify",
				artist: info?.artist || "",
				album: info?.album || ""
			});
			meta = { title: info?.title || "", artist: info?.artist || "", durationMs: info?.durationMs || 0 };
			publicar();
		} catch {
			// sin metadatos la sesión sigue funcionando
		}
	};

	if ("mediaSession" in navigator) {
		navigator.mediaSession.setActionHandler("play", () => void audio.play().catch(() => {}));
		navigator.mediaSession.setActionHandler("pause", () => audio.pause());
		navigator.mediaSession.setActionHandler("nexttrack", () => void player.next());
		navigator.mediaSession.setActionHandler("previoustrack", () => void player.previous());
		navigator.mediaSession.setActionHandler("seekto", (d) => {
			if (d.seekTime != null && audio.duration > 0) {
				audio.currentTime = Math.min(d.seekTime, audio.duration - 1);
			}
		});
		navigator.mediaSession.setActionHandler("seekbackward", () => {
			audio.currentTime = Math.max(0, audio.currentTime - 10);
		});
		navigator.mediaSession.setActionHandler("seekforward", () => {
			if (audio.duration > 0) audio.currentTime = Math.min(audio.currentTime + 10, audio.duration - 1);
		});
	}

	// ── Servicio en primer plano: proceso vivo mientras suena ──
	const sostenerProceso = (sonando) => {
		try {
			if (sonando) puente?.startPlayback?.();
			else puente?.stopPlayback?.();
		} catch {
			// sin puente (emulador raro, WebView viejo): la limitación de
			// segundo plano de siempre
		}
	};

	alRecibir((ev) => {
		switch (ev.type) {
			case "trackChanged": {
				if (!ev.trackId) break;
				pistaActual = ev.trackId;
				audio.src = `/audio/${encodeURIComponent(ev.trackId)}`;
				void ponerMetadata(ev.trackId);
				if (quererSonar) void audio.play().catch(() => {});
				break;
			}
			case "playStatusChanged": {
				quererSonar = ev.status === "playing";
				if (quererSonar) {
					void audio.play().catch(() => {});
					sostenerProceso(true);
					if ("mediaSession" in navigator) navigator.mediaSession.playbackState = "playing";
				} else {
					audio.pause();
					sostenerProceso(false);
					if ("mediaSession" in navigator) navigator.mediaSession.playbackState = "paused";
				}
				break;
			}
			case "volumeChanged": {
				audio.volume = Math.min(1, Math.max(0, ev.volume ?? 1));
				break;
			}
			default:
				break;
		}
	});

	// Al abrir la app con una canción ya cargada no llega ningún `trackChanged`:
	// sin esto el <audio> no tendría `src` (el primer play sería mudo) ni
	// habría metadatos para el sistema.
	void player.getState().then((estado) => {
		const id = estado?.track?.id;
		if (!id || pistaActual) return;
		pistaActual = id;
		audio.src = `/audio/${encodeURIComponent(id)}`;
		const ms = estado.positionMs ?? 0;
		if (ms > 0) audio.addEventListener("loadedmetadata", () => { audio.currentTime = ms / 1000; }, { once: true });
		void ponerMetadata(id);
		// Una app recién abierta no empieza a sonar sola: lo que quedó "sonando"
		// al cerrarla se retoma en pausa, en su posición.
		if (estado.status === "playing") void player.pause();
	}).catch(() => {});

	// ── fin de pista y posición → backend ──
	audio.addEventListener("ended", () => informar({ type: "ended" }));

	window.setInterval(() => {
		if (!audio.paused && !audio.ended && audio.duration > 0) {
			informar({ type: "position", ms: Math.round(audio.currentTime * 1000) });
			publicar();
		}
	}, 3000);

	// Seek fino: client.js avisa aquí antes de mandar el comando al backend,
	// para que el `<audio>` (la única realidad sonora en el móvil) salte a la
	// misma posición.
	window.__localifyWebAudio = {
		seek(ms) {
			if (audio.duration > 0) {
				audio.currentTime = Math.min(Math.max(ms / 1000, 0), audio.duration - 0.5);
			}
		},
		pista: () => pistaActual
	};
}

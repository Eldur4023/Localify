/**
 * reproductor.js — El controlador: lo único que une backend y motor de audio.
 *
 * ## Por qué existe (lo que fallaba antes)
 *
 * El reproductor anterior tenía dos dueños peleándose por lo mismo: un
 * sondeo cada segundo EMPUJABA el estado del backend dentro del <audio>
 * (cargar, parar, colocar la posición) y el cliente EMPUJABA su posición al
 * backend cada tres segundos, sin ningún orden entre mensajes. Un informe
 * atrasado de la canción anterior acababa aplicado a la nueva (empezaba en
 * el minuto X), y una respuesta de sondeo vieja deshacía una orden recién
 * dada.
 *
 * ## Las tres reglas
 *
 * 1. `revision`: el backend la sube con cada cambio. Un estado con una
 *    revision más vieja que la última aplicada se descarta.
 * 2. `cue`: el backend lo estrena cada vez que quiere mover el motor (otra
 *    pista, o saltar de posición). Cada cue se aplica UNA vez; entre cues, el
 *    motor manda sobre el tiempo y nadie lo recoloca. Todo lo que el cliente
 *    informa lleva el cue en el que estaba, y el backend descarta lo de cues
 *    viejos.
 * 3. La posición que ve la interfaz sale del motor, siempre. La del backend
 *    solo sirve para cargar (el punto de partida de un cue).
 *
 * Las órdenes de la interfaz tienen además efecto inmediato donde no hace
 * falta esperar al backend (pausa, volumen, salto): la respuesta llega luego
 * y, como dice lo mismo, no mueve nada.
 */
import * as motor from "./motor-audio.js";

let revisionAplicada = -1;
let cueAplicado = null;
let estadoActual = null;
/**
 * Pista a la que el motor ya pasó por su cuenta (gapless/crossfade), a la
 * espera de que el backend confirme el avance con su cue. Mientras tanto no
 * se informa de la posición: el cue vigente en el backend todavía es el de
 * la pista anterior.
 */
let adopcion = null;
/** El bus de eventos de la interfaz (transport.js lo conecta). */
let emitirEvento = () => {};

let ultimaPrevision = { revision: -1, cuando: 0, pendiente: false };
/**
 * Órdenes de la interfaz en vuelo, y la revision que había al enviarlas.
 * Mientras haya alguna, un estado del SONDEO que no sea más nuevo que eso se
 * descarta: pudo salir antes de la orden (pulsas pausa, llega un sondeo
 * atrasado que aún dice "sonando" y la música volvía un instante).
 */
let enVuelo = 0;
let revisionAlEnviar = -1;

const disponible = (pista) => pista?.availability?.kind === "local";

export function conectar(emitir) {
	emitirEvento = emitir;
}

// ── Del backend al motor ────────────────────────────────────────────────────

/**
 * Aplica un estado del backend (respuesta de una orden o del sondeo).
 * Devuelve false si era más viejo que lo ya aplicado.
 */
export function aplicar(estado, desdeSondeo = false) {
	if (!estado || typeof estado.revision !== "number") return false;
	if (estado.revision < revisionAplicada) return false;
	if (desdeSondeo && enVuelo > 0 && estado.revision <= revisionAlEnviar) return false;
	revisionAplicada = estado.revision;
	estadoActual = estado;
	const pista = estado.track;

	// 1) Qué suena y dónde: solo al estrenarse un cue.
	if (!pista) {
		if (estado.cue !== cueAplicado) {
			cueAplicado = estado.cue;
			adopcion = null;
			motor.descargar();
		}
	} else if (estado.cue !== cueAplicado) {
		cueAplicado = estado.cue;
		if (adopcion !== null && adopcion === pista.id) {
			// El motor ya está ahí desde el relevo: se confirma sin tocar nada
			// y se informa de dónde va de verdad (el cue nuevo nació en 0).
			adopcion = null;
			informar(motor.posicionMs());
		} else {
			adopcion = null;
			if (!disponible(pista)) motor.descargar();
			else if (motor.pista() === pista.id) motor.buscar(estado.positionMs);
			else motor.cargar(pista.id, estado.positionMs);
		}
	} else if (disponible(pista) && motor.pista() !== pista.id && adopcion === null) {
		// Mismo cue, pero la pista no estaba descargada al estrenarlo y ahora
		// sí: se carga donde diga el backend.
		motor.cargar(pista.id, estado.positionMs);
	}

	// 2) Sonar o no.
	if (estado.status === "playing" && pista && disponible(pista)) motor.reproducir();
	else motor.pausar();

	// 3) Volumen (idempotente; también llega desde MPRIS o la tarjeta).
	motor.fijarVolumen(estado.volume);

	// 4) Lo siguiente, precargado para el relevo.
	void prever(estado);
	return true;
}

/** Pregunta qué sonará después y lo precarga. Solo cuando cambia algo. */
async function prever(estado) {
	if (!estado.track || estado.status !== "playing") return;
	const ahora = Date.now();
	// Tras un cambio de estado, o reintento cada 5 s si lo siguiente aún se
	// estaba descargando (la descarga terminada no sube la revision).
	const cambio = estado.revision !== ultimaPrevision.revision;
	const reintento = ultimaPrevision.pendiente && ahora - ultimaPrevision.cuando > 5000;
	if (!cambio && !reintento) return;
	ultimaPrevision = { revision: estado.revision, cuando: ahora, pendiente: false };
	try {
		const r = await llamar("player_peek_next", {});
		const id = r?.trackId ?? null;
		// Repetir pista: lo siguiente es ella misma; no se funde consigo misma.
		if (!id || id === estado.track.id) {
			motor.precargar(null);
			return;
		}
		if (!r.available) {
			ultimaPrevision.pendiente = true;
			return;
		}
		motor.precargar(id);
	} catch {
		ultimaPrevision.pendiente = true;
	}
}

// ── Del motor al backend ────────────────────────────────────────────────────

function informar(posicionMs) {
	if (adopcion !== null || cueAplicado === null) return;
	if (!estadoActual?.track || motor.pista() !== estadoActual.track.id) return;
	void llamar("player_report", {
		cue: cueAplicado,
		positionMs: posicionMs,
		anchorMs: Date.now() - posicionMs,
		listenedMs: motor.escuchado(),
	}).catch(() => {});
}

motor.on("ancla", ({ posicionMs }) => informar(posicionMs));

motor.on("posicion", ({ posicionMs, duracionMs }) => {
	emitirEvento({ type: "positionTick", positionMs: posicionMs, durationMs: duracionMs });
});

// El motor pasó solo a la siguiente (gapless o crossfade): el backend avanza
// con el cue de la que acaba de terminar, y su respuesta se adopta.
motor.on("relevo", ({ trackId, escuchadoAnterior }) => {
	adopcion = trackId;
	void orden("player_ended", { cue: cueAplicado, __listenedMs: escuchadoAnterior });
});

// Fin natural sin nada precargado: el backend decide qué sigue.
motor.on("fin", ({ escuchado }) => {
	void orden("player_ended", { cue: cueAplicado, __listenedMs: escuchado });
});

/** Deja constancia en el log del backend (sin devtools, la consola no se ve). */
function avisarLog(detalle) {
	fetch("/api/client-log", {
		method: "POST",
		headers: { "Content-Type": "application/json" },
		body: JSON.stringify({ tipo: "reproductor", detalle }),
	}).catch(() => {});
}

motor.on("bloqueado", () => {
	avisarLog("WebKit bloqueó play() (política de autoplay): se pasa a pausa");
	// WebKit no dejó sonar: que la interfaz lo diga (botón de play) en vez
	// de fingir que suena.
	void orden("player_pause");
});

motor.on("error", ({ trackId, codigo }) => {
	avisarLog(`el motor de audio no pudo reproducir ${trackId}: ${codigo}`);
});

// ── Órdenes de la interfaz ──────────────────────────────────────────────────

async function llamar(cmd, args) {
	const r = await fetch("/api/invoke", {
		method: "POST",
		headers: { "Content-Type": "application/json" },
		body: JSON.stringify({ cmd, args: JSON.stringify(args ?? {}) }),
	});
	const j = await r.json().catch(() => null);
	if (!r.ok) throw j ?? { code: "INTERNAL", messageKey: "error.internal", params: [], actionable: false, retryable: false };
	return j;
}

/**
 * El estado que ve la interfaz: el del backend, con la posición y la
 * duración del motor cuando el motor tiene esa pista (es la fuente de
 * verdad del tiempo; la del backend puede ir un poco por detrás).
 */
export function conTiempoLocal(estado) {
	if (!estado || typeof estado !== "object" || !("track" in estado)) return estado;
	if (estado.track && motor.pista() === estado.track.id) {
		const dur = motor.duracionMs();
		return { ...estado, positionMs: motor.posicionMs(), durationMs: dur > 0 ? dur : estado.durationMs };
	}
	return estado;
}

const QUE_LLEVAN_POSICION = new Set(["player_toggle", "player_pause", "player_next", "player_previous"]);

/** Ejecuta una orden del reproductor (player_*, queue_jump_to). */
export async function orden(cmd, args = {}) {
	if (cmd === "player_position") {
		return { positionMs: motor.posicionMs(), bufferedMs: motor.duracionMs() || motor.posicionMs() };
	}
	const extra = { ...args };
	if (QUE_LLEVAN_POSICION.has(cmd)) extra.__posMs = motor.posicionMs();
	// Lo escuchado de la pista en curso, con cualquier orden: las
	// estadísticas se actualizan también al pausar, saltar, etc. El fin
	// natural y el relevo traen ya el suyo (el motor lo reinicia al acabar).
	if (!("__listenedMs" in extra) && estadoActual?.track && motor.pista() === estadoActual.track.id) {
		extra.__listenedMs = motor.escuchado();
	}

	// Efecto inmediato de lo que no necesita al backend para saber qué hacer.
	if (cmd === "player_set_volume") motor.fijarVolumen(args.volume);
	else if (cmd === "player_pause") motor.pausar();
	else if (cmd === "player_seek" && typeof args.positionMs === "number") motor.buscar(args.positionMs);
	else if (cmd === "player_toggle" && estadoActual?.track && motor.pista() === estadoActual.track.id) {
		// La intención, no el elemento: una segunda pulsación rápida llega
		// con el <audio> aún bajando el fundido de la primera.
		if (motor.quiereSonarAhora()) motor.pausar();
		else if (disponible(estadoActual.track)) motor.reproducir();
	}

	if (enVuelo === 0) revisionAlEnviar = revisionAplicada;
	enVuelo++;
	let j;
	try {
		j = await llamar(cmd, extra);
	} finally {
		enVuelo--;
	}
	if (j && typeof j === "object" && "revision" in j && "track" in j) aplicar(j);
	return conTiempoLocal(j);
}

/** Estado actual (último aplicado), con el tiempo del motor. */
export function estado() {
	return conTiempoLocal(estadoActual);
}

// ── Ajustes de audio ────────────────────────────────────────────────────────

export function aplicarAjustesAudio(audio) {
	if (Array.isArray(audio?.eqProfile?.gainsDb)) motor.fijarEq(audio.eqProfile.gainsDb);
	if (typeof audio?.crossfadeMs === "number") motor.fijarCrossfade(audio.crossfadeMs);
}

export function previsualizarEq(ganancias) {
	if (Array.isArray(ganancias)) motor.fijarEq(ganancias);
}

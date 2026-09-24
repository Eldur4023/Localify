/**
 * motor-audio.js — El motor de audio.
 *
 * Es el único dueño del SONIDO y del TIEMPO: la posición que ve la interfaz
 * sale de aquí y de ningún otro sitio. No sabe nada del backend: le dicen
 * "carga esto aquí", "suena", "para", "salta", y avisa de lo que pasa
 * (posición, fin, relevo a la siguiente). Quien decide es el controlador
 * (reproductor.js).
 *
 * ## Por qué no hay <audio>
 *
 * Con un <audio> enganchado a Web Audio, WebKitGTK decodifica por delante y
 * mete ~1,2 s de búfer entre el elemento y lo que suena: su currentTime iba
 * un segundo por delante de lo que se oía (barra, letras y Discord
 * adelantados), "ended" llegaba con un segundo de canción aún por sonar, y
 * cambiar la fuente de un elemento a medio decodificar dejaba pasar basura
 * (el chillido). Aquí el fichero se decodifica entero en memoria
 * (decodeAudioData) y suena con AudioBufferSourceNode: el reloj del
 * AudioContext es el de la salida, así que la posición es la que se oye, y
 * todo -- arrancar, parar, saltar, gapless, crossfade -- se PROGRAMA a la
 * muestra en ese reloj en vez de reaccionar a eventos que llegan tarde.
 *
 * Decodificar una canción entera tarda ~0,6 s (3:30); para no esperar eso,
 * primero se decodifican solo los primeros 256 KB (~15 s, ~40 ms) y se
 * empieza a sonar con eso. Cuando llega el resto, se cambia a la versión
 * completa en un instante programado: las muestras son idénticas hasta el
 * final del trozo (medido), así que el cambio no se oye.
 *
 * ## El grafo
 *
 *   voz (fuente → fundido) ┐
 *   voz (fuente → fundido) ┴→ xf A ┐
 *                             xf B ┴→ EQ (10 biquads) → maestro → salida
 *
 * - Una VOZ por cada vez que algo suena: su fuente y su propio fundido. Al
 *   saltar o reanudar se crea otra; la vieja se apaga sola. Dos voces nunca
 *   se pelean por el mismo nodo, y no hay clics: todo entra y sale fundido.
 * - `xf` (por pista): la mezcla del crossfade. Solo la toca el crossfade.
 * - `maestro`: el volumen del usuario. Solo lo toca fijarVolumen().
 *
 * ## Por qué no hay setTimeout en el camino del audio
 *
 * WebKitGTK frena los temporizadores de una ventana minimizada a varios
 * segundos. Lo que tiene que ocurrir en un instante del audio se programa en
 * el reloj de audio (programarEn: un ConstantSourceNode que termina).
 */

const BANDAS = [31, 62, 125, 250, 500, 1000, 2000, 4000, 8000, 16000];
/** Fundido de arranque/parada/salto: lo justo para que no haya clic. */
const FUNDIDO_S = 0.012;
/** Margen para programar algo "ya": el hilo de audio tiene que verlo antes. */
const ADELANTO_S = 0.03;
/** Lo que se decodifica primero para empezar a sonar sin esperar al resto. */
const TROZO_BYTES = 256 * 1024;
/** El final de un trozo decodificado no es fiable (medido: 31 muestras). */
const MARGEN_TROZO_S = 0.5;
/** Cada cuánto se avisa de la posición mientras suena. */
const TICK_S = 0.25;
/** Cada cuántos ticks se re-ancla la posición en el backend (~5 s). */
const TICKS_POR_ANCLA = 20;
/**
 * La siguiente se decodifica cuando a la activa le queda esto (más el
 * crossfade). Antes no: una canción decodificada ocupa ~10 MB por minuto, y
 * tener siempre dos en memoria llevaba el proceso de WebKit a medio GB. Su
 * principio está listo en ~40 ms, de sobra para el relevo.
 */
const PRECARGA_S = 30;

// ── Estado ──────────────────────────────────────────────────────────────────

function crearSlot() {
	return {
		trackId: null,
		/** Sube con cada cambio de pista: invalida decodificaciones viejas. */
		token: 0,
		/** El principio de la pista, mientras la completa se decodifica. */
		trozo: null,
		completo: null,
		error: null,
		/** La voz que suena (o está programada para sonar), o null. */
		voz: null,
		/** Instante del reloj de audio en que la posición 0 sonó (o sonaría). */
		inicioCtx: 0,
		/** Posición (s) cuando no hay voz: pausa, carga, antes de empezar. */
		posParada: 0,
		/** Terminó de sonar entera: nada la reinicia hasta que la coloquen. */
		finalizada: false,
		xf: null,
	};
}

const slots = [crearSlot(), crearSlot()];
let activo = 0;
/** Índice del slot que se está apagando tras un crossfade, o null. */
let cola = null;
/** Lo que sonará después de la activa (lo dice el controlador), o null. */
let siguiente = null;
/** Relevo a la siguiente ya programado en el reloj de audio, o null. */
let programado = null;

let ctx = null;
let maestro = null;
let sumidero = null;
let filtros = [];

let volumen = 1;
let gananciasEq = new Float32Array(10);
let crossfadeMs = 0;
/** Intención: ¿debería estar sonando la pista activa? */
let quiereSonar = false;

/** Tiempo de escucha de la pista activa (s), sin pausas: estadísticas. */
let escuchadoS = 0;
let tramoDesde = null;
let generacionTick = 0;

const oyentes = new Map();

const slotActivo = () => slots[activo];
const slotLibre = () => slots[1 - activo];
const curva = (v) => Math.min(1, Math.max(0, v)) ** 3;

function emitir(tipo, datos = {}) {
	for (const fn of oyentes.get(tipo) ?? []) {
		try {
			fn(datos);
		} catch (e) {
			console.error("oyente del motor falló", tipo, e);
		}
	}
}

/** Se suscribe a: posicion, ancla, fin, relevo, error, bloqueado. */
export function on(tipo, fn) {
	if (!oyentes.has(tipo)) oyentes.set(tipo, new Set());
	oyentes.get(tipo).add(fn);
	return () => oyentes.get(tipo).delete(fn);
}

// ── Grafo y reloj ───────────────────────────────────────────────────────────

function montarGrafo() {
	ctx = new AudioContext();
	maestro = ctx.createGain();
	maestro.gain.value = curva(volumen);
	let previo = null;
	filtros = BANDAS.map((hz, i) => {
		const f = ctx.createBiquadFilter();
		f.type = "peaking";
		f.frequency.value = hz;
		f.Q.value = 1.41;
		f.gain.value = gananciasEq[i] ?? 0;
		if (previo) previo.connect(f);
		previo = f;
		return f;
	});
	filtros[filtros.length - 1].connect(maestro);
	maestro.connect(ctx.destination);
	// Los temporizadores de programarEn() tienen que estar conectados para
	// avanzar, pero no deben oírse.
	sumidero = ctx.createGain();
	sumidero.gain.value = 0;
	sumidero.connect(ctx.destination);
	for (const s of slots) {
		s.xf = ctx.createGain();
		s.xf.connect(filtros[0]);
	}
	if (ctx.state !== "running") {
		ctx.resume().catch(() => {});
		// Por si WebKit sigue exigiendo un gesto (política de autoplay que no
		// se aplicó): el primero que llegue lo desbloquea.
		const alGesto = () => {
			ctx.resume().catch(() => {});
			document.removeEventListener("pointerdown", alGesto, true);
			document.removeEventListener("keydown", alGesto, true);
		};
		document.addEventListener("pointerdown", alGesto, true);
		document.addEventListener("keydown", alGesto, true);
	}
}

/**
 * Lo que tarda en OÍRSE lo que el hilo de audio ya entregó: el búfer del
 * sumidero GStreamer de WebKit más el de PipeWire. WebKitGTK dice 3 ms
 * (baseLatency) y 0 (outputLatency), y getOutputTimestamp trae un
 * performanceTime de otro origen; medido grabando la salida y correlándola
 * con el fichero: 80–95 ms.
 */
const LATENCIA_SALIDA_S = 0.085;

/** El instante del reloj de audio que está sonando AHORA. */
function relojOido() {
	return Math.max(0, ctx.currentTime - Math.max(ctx.outputLatency || 0, LATENCIA_SALIDA_S));
}

/** Ejecuta `cb` cuando el reloj de AUDIO llegue a `t`. */
function programarEn(t, cb) {
	const n = ctx.createConstantSource();
	n.connect(sumidero);
	n.onended = () => {
		n.disconnect();
		cb();
	};
	n.start();
	n.stop(Math.max(t, ctx.currentTime));
}

// ── Voces ───────────────────────────────────────────────────────────────────

/** El búfer con el que puede sonar `s` desde `pos` (s), o null si aún no hay. */
function bufferPara(s, pos) {
	if (s.completo) return { buf: s.completo, trozo: false };
	if (s.trozo && pos < s.trozo.duration - MARGEN_TROZO_S) return { buf: s.trozo, trozo: true };
	return null;
}

/**
 * Programa una voz de `s` que empieza a sonar en el instante `t` del reloj
 * de audio desde la posición `pos` (s). Con `suave`, entra fundida. Devuelve
 * la voz, o null si `s` no tiene aún con qué sonar ahí.
 */
function lanzar(s, pos, t, suave) {
	const b = bufferPara(s, pos);
	if (!b) return null;
	pos = Math.min(Math.max(0, pos), b.buf.duration);
	const fuente = ctx.createBufferSource();
	fuente.buffer = b.buf;
	const fundido = ctx.createGain();
	fuente.connect(fundido);
	fundido.connect(s.xf);
	if (suave) {
		fundido.gain.setValueAtTime(0, t);
		fundido.gain.linearRampToValueAtTime(1, t + FUNDIDO_S);
	}
	const voz = { fuente, fundido, t0: t, pos0: pos, suave, trozo: b.trozo, apagada: false };
	fuente.onended = () => alAcabarVoz(s, voz);
	fuente.start(t, pos);
	// Un trozo no se toca más allá de su parte fiable; si para entonces aún
	// no ha llegado la versión completa, la voz se acaba ahí y se espera.
	if (b.trozo) fuente.stop(t + (b.buf.duration - MARGEN_TROZO_S - pos));
	s.voz = voz;
	s.inicioCtx = t - pos;
	s.finalizada = false;
	return voz;
}

/** Apaga una voz con un fundido desde `t`. La que aún no empezó, no suena. */
function apagar(voz, t = ctx.currentTime) {
	if (!voz || voz.apagada) return;
	voz.apagada = true;
	if (t <= voz.t0) {
		try {
			voz.fuente.stop();
		} catch {}
		return;
	}
	const g = voz.fundido.gain;
	g.cancelAndHoldAtTime(t);
	g.linearRampToValueAtTime(0, t + FUNDIDO_S);
	try {
		voz.fuente.stop(t + FUNDIDO_S);
	} catch {}
}

/** Posición (s) de `s` en el instante que se oye ahora. */
function posicionDe(s) {
	const v = s.voz;
	if (!v || v.apagada) return s.posParada;
	const dur = s.completo?.duration ?? Infinity;
	// Hasta que la voz empieza de verdad, está donde va a empezar.
	return Math.min(dur, Math.max(v.pos0, relojOido() - s.inicioCtx));
}

function alAcabarVoz(s, voz) {
	voz.fuente.disconnect();
	voz.fundido.disconnect();
	if (voz.apagada || s.voz !== voz) return;
	s.voz = null;

	if (voz.trozo) {
		// Se acabó el principio decodificado y la versión completa aún no ha
		// llegado: se espera ahí (la posición no miente: no suena nada).
		s.posParada = voz.fuente.buffer.duration - MARGEN_TROZO_S;
		if (s === slotActivo()) {
			cerrarTramo();
			pararTicks();
		}
		return;
	}
	if (slots.indexOf(s) === cola) {
		terminarCola();
		return;
	}
	if (s !== slotActivo()) return;
	// La siguiente ya estaba programada justo aquí: es un relevo, no un fin
	// (el aviso del reloj puede llegar después que este).
	if (programado) {
		relevar();
		return;
	}
	s.posParada = s.completo ? s.completo.duration : s.posParada;
	s.finalizada = true;
	cerrarTramo();
	pararTicks();
	emitirPosicion();
	const oido = escuchado();
	reiniciarEscucha();
	emitir("fin", { escuchado: oido });
}

/** Pasa la voz de un trozo a la versión completa, sin que se note. */
function completarVoz(s) {
	const v = s.voz;
	if (!v || !v.trozo || v.apagada || !s.completo) return;
	const ahora = ctx.currentTime;
	if (v.t0 > ahora + ADELANTO_S) {
		// Aún no ha empezado: se sustituye tal cual.
		apagar(v);
		const nueva = lanzar(s, v.pos0, v.t0, v.suave);
		if (programado?.voz === v) programado.voz = nueva;
		return;
	}
	// Ya suena: la completa entra en el mismo punto y el trozo se corta a la
	// vez, a la muestra (las muestras son las mismas). Nunca a mitad del
	// fundido de entrada.
	const t = Math.max(ahora + ADELANTO_S, v.t0 + FUNDIDO_S * 2);
	const pos = t - s.inicioCtx;
	if (pos >= v.fuente.buffer.duration - MARGEN_TROZO_S) return; // alAcabarVoz la relanza
	v.apagada = true;
	v.fuente.stop(t);
	lanzar(s, pos, t, false);
}

// ── Slots ───────────────────────────────────────────────────────────────────

function fijarXf(s, valor) {
	s.xf.gain.cancelScheduledValues(0);
	s.xf.gain.setValueAtTime(valor, ctx.currentTime);
}

function vaciar(s) {
	s.token++;
	apagar(s.voz);
	s.voz = null;
	s.trackId = null;
	s.trozo = null;
	s.completo = null;
	s.error = null;
	s.posParada = 0;
	s.finalizada = false;
	fijarXf(s, 1);
}

/**
 * Descarga y decodifica `trackId` en `s`: primero el principio (para sonar
 * ya), luego entera. Cada parte que llega se ofrece a quien la espere.
 */
async function decodificar(s, trackId) {
	const token = s.token;
	try {
		const r = await fetch(`/audio/${encodeURIComponent(trackId)}`);
		if (!r.ok) throw new Error(`HTTP ${r.status}`);
		const datos = await r.arrayBuffer();
		if (s.token !== token) return;
		if (datos.byteLength > TROZO_BYTES * 2) {
			try {
				const trozo = await ctx.decodeAudioData(datos.slice(0, TROZO_BYTES));
				if (s.token !== token) return;
				s.trozo = trozo;
				alPrepararse(s);
			} catch {
				// Un formato que no se deja decodificar a trozos: se espera a la
				// completa.
			}
		}
		const completo = await ctx.decodeAudioData(datos);
		if (s.token !== token) return;
		s.completo = completo;
		s.trozo = null;
		alPrepararse(s);
	} catch (e) {
		if (s.token !== token) return;
		s.error = e;
		if (s === slotActivo()) emitir("error", { trackId, codigo: String(e?.message ?? e) });
	}
}

/** Llegó (más) audio decodificado a `s`. */
function alPrepararse(s) {
	if (s.voz?.trozo) completarVoz(s);
	if (s === slotActivo()) {
		if (!s.voz && quiereSonar && !s.finalizada) sonar(s, s.posParada);
		emitirPosicion();
	}
	programarSiguiente();
}

/** La pista activa suena desde `pos`: voz + escucha + avisos. */
function sonar(s, pos) {
	const t = ctx.currentTime + ADELANTO_S;
	if (!lanzar(s, pos, t, true)) {
		// Ese punto aún no está decodificado: sonará al llegar.
		s.posParada = pos;
		return false;
	}
	if (ctx.state !== "running") {
		ctx.resume().catch(() => emitir("bloqueado"));
	}
	abrirTramo(t);
	arrancarTicks();
	programarSiguiente();
	return true;
}

/** Calla la pista activa donde está. */
function callar(s) {
	if (!s.voz) return;
	s.posParada = posicionDe(s);
	apagar(s.voz);
	s.voz = null;
}

// ── Relevo a la siguiente (gapless / crossfade) ─────────────────────────────

function cancelarSiguiente() {
	if (!programado) return;
	const p = programado;
	programado = null;
	const s = slots[p.slot];
	apagar(p.voz);
	if (s.voz === p.voz) s.voz = null;
	fijarXf(slotActivo(), 1);
	fijarXf(s, 1);
}

/**
 * Programa en el reloj de audio el paso a lo precargado: gapless (empieza
 * justo donde acaba la actual) o crossfade (empieza `crossfadeMs` antes y se
 * mezclan). Se recalcula tras cualquier cambio; si nada cambió, no toca nada.
 */
function programarSiguiente() {
	const a = slotActivo();
	const l = slotLibre();
	const va = a.voz;
	const posible =
		quiereSonar && va && !va.apagada && !va.trozo && a.completo && cola === null &&
		l.trackId && !l.error && l.trackId !== a.trackId && bufferPara(l, 0);
	if (!posible) {
		cancelarSiguiente();
		return;
	}
	const dur = a.completo.duration;
	const tFin = a.inicioCtx + dur;
	const pronto = ctx.currentTime + ADELANTO_S * 2;
	let mezcla = Math.min(crossfadeMs / 1000, dur / 2, (l.completo?.duration ?? Infinity) / 2);
	if (tFin - mezcla < pronto) mezcla = Math.max(0, tFin - pronto);
	const tInicio = tFin - mezcla;
	if (programado && programado.slot === 1 - activo && programado.token === l.token &&
		Math.abs(programado.tInicio - tInicio) < 0.002 && Math.abs(programado.mezcla - mezcla) < 0.002) return;
	if (programado && programado.tInicio <= ctx.currentTime) return; // ya en marcha
	cancelarSiguiente();
	if (tFin < pronto) return; // demasiado tarde: al acabar, el backend decide
	const voz = lanzar(l, 0, tInicio, false);
	if (!voz) return;
	if (mezcla > 0) {
		const ax = a.xf.gain;
		const lx = l.xf.gain;
		ax.cancelScheduledValues(0);
		ax.setValueAtTime(1, ctx.currentTime);
		ax.setValueAtTime(1, tInicio);
		ax.linearRampToValueAtTime(0, tFin);
		lx.cancelScheduledValues(0);
		lx.setValueAtTime(0, ctx.currentTime);
		lx.setValueAtTime(0, tInicio);
		lx.linearRampToValueAtTime(1, tFin);
	} else {
		fijarXf(l, 1);
	}
	const p = (programado = { slot: 1 - activo, token: l.token, voz, tInicio, mezcla });
	programarEn(tInicio, () => {
		if (programado === p) relevar();
	});
}

/** La siguiente, que ya suena desde su instante programado, pasa a ser la activa. */
function relevar() {
	const p = programado;
	programado = null;
	const saliente = slotActivo();
	const entrante = slots[p.slot];
	cerrarTramo(p.tInicio);
	const escuchadoAnterior = escuchado();
	reiniciarEscucha();
	activo = p.slot;
	abrirTramo(p.tInicio);
	if (p.mezcla > 0) {
		// La saliente sigue sonando, apagándose, hasta su propio final.
		cola = 1 - activo;
	} else {
		vaciar(saliente);
	}
	arrancarTicks();
	emitirPosicion();
	emitir("relevo", { trackId: entrante.trackId, escuchadoAnterior });
}

function terminarCola() {
	if (cola === null) return;
	const s = slots[cola];
	cola = null;
	vaciar(s);
	revisarPrecarga();
}

/**
 * Decodifica en el slot libre la siguiente cuando toca (ver PRECARGA_S), y
 * lo vacía cuando lo que tiene ya no toca. Se revisa en cada tick.
 */
function revisarPrecarga() {
	if (cola !== null) return; // el slot libre aún es la cola del crossfade
	const a = slotActivo();
	const libre = slotLibre();
	const soltar = () => {
		if (!libre.trackId) return;
		cancelarSiguiente();
		vaciar(libre);
	};
	if (!siguiente || siguiente === a.trackId || !a.trackId) {
		soltar();
		return;
	}
	if (libre.trackId === siguiente) return;
	const restante = a.completo ? a.completo.duration - posicionDe(a) : Infinity;
	if (restante > PRECARGA_S + crossfadeMs / 1000) {
		soltar();
		return;
	}
	cancelarSiguiente();
	vaciar(libre);
	libre.trackId = siguiente;
	void decodificar(libre, siguiente);
}

// ── Escucha (tiempo real sonado, para las estadísticas) ─────────────────────

function abrirTramo(t = ctx.currentTime) {
	if (tramoDesde === null) tramoDesde = t;
}
function cerrarTramo(t = ctx.currentTime) {
	if (tramoDesde !== null) {
		escuchadoS += Math.max(0, t - tramoDesde);
		tramoDesde = null;
	}
}
function reiniciarEscucha() {
	escuchadoS = 0;
	tramoDesde = null;
}
/** Milisegundos que la pista activa ha sonado de verdad (sin pausas). */
export function escuchado() {
	const vivo = tramoDesde !== null ? Math.max(0, ctx.currentTime - tramoDesde) : 0;
	return Math.round((escuchadoS + vivo) * 1000);
}

// ── Avisos de posición ──────────────────────────────────────────────────────

/**
 * Mientras suena: posición cada TICK_S y ancla cada ~5 s, en el reloj de
 * audio (sigue a su ritmo con la ventana minimizada). El primer tick, nada
 * más empezar a sonar, ancla.
 */
function arrancarTicks() {
	const gen = ++generacionTick;
	let n = 0;
	const paso = () => {
		if (gen !== generacionTick) return;
		const s = slotActivo();
		if (!s.voz || s.voz.apagada) return;
		emitirPosicion();
		revisarPrecarga();
		if (n++ % TICKS_POR_ANCLA === 0) emitir("ancla", { posicionMs: posicionMs() });
		programarEn(ctx.currentTime + TICK_S, paso);
	};
	programarEn(ctx.currentTime + ADELANTO_S + 0.02, paso);
}

function pararTicks() {
	generacionTick++;
}

function emitirPosicion() {
	if (!slotActivo().trackId) return;
	emitir("posicion", { posicionMs: posicionMs(), duracionMs: duracionMs() });
}

// ── API ─────────────────────────────────────────────────────────────────────

/** Pista cargada en el slot activo (o null). */
export function pista() {
	return slotActivo().trackId;
}

/** Posición de la pista activa en lo que se OYE ahora. */
export function posicionMs() {
	const s = slotActivo();
	return s.trackId ? Math.round(posicionDe(s) * 1000) : 0;
}

/** 0 mientras no está decodificada entera: quien pregunte usa la del catálogo. */
export function duracionMs() {
	const s = slotActivo();
	return s.trackId && s.completo ? Math.round(s.completo.duration * 1000) : 0;
}

/** ¿Se ha pedido que suene? (la intención, no si ya/aún suena). */
export function quiereSonarAhora() {
	return quiereSonar;
}

/**
 * Carga `trackId` en `desdeMs`. Suena en cuanto hay audio si se ha pedido
 * reproducir() (antes o después): intención y carga son independientes.
 */
export function cargar(trackId, desdeMs = 0) {
	cancelarSiguiente();
	if (cola !== null) terminarCola();
	const desde = Math.max(0, (desdeMs || 0) / 1000);
	const saliente = slotActivo();
	cerrarTramo();
	reiniciarEscucha();
	pararTicks();
	callar(saliente);
	const libre = slotLibre();
	if (libre.trackId === trackId && !libre.error) {
		// Ya precargada (siguiente manual, o la cola avanzó como se preveía):
		// pasa a activa sin volver a decodificar nada.
		activo = 1 - activo;
		vaciar(saliente);
		fijarXf(libre, 1);
		libre.posParada = desde;
		libre.finalizada = false;
		if (quiereSonar) sonar(libre, desde);
		emitirPosicion();
		return;
	}
	vaciar(saliente);
	saliente.trackId = trackId;
	saliente.posParada = desde;
	void decodificar(saliente, trackId);
}

/** Deja el motor sin pista (fin de la cola, pista aún descargándose). */
export function descargar() {
	cancelarSiguiente();
	cerrarTramo();
	reiniciarEscucha();
	pararTicks();
	if (cola !== null) terminarCola();
	vaciar(slotActivo());
}

export function reproducir() {
	quiereSonar = true;
	const s = slotActivo();
	if (!s.trackId || s.voz || s.finalizada) return;
	sonar(s, s.posParada); // si aún no hay audio, sonará al llegar
}

export function pausar() {
	quiereSonar = false;
	cancelarSiguiente();
	if (cola !== null) terminarCola();
	const s = slotActivo();
	cerrarTramo();
	pararTicks();
	callar(s);
	emitirPosicion();
}

export function buscar(ms) {
	const s = slotActivo();
	if (!s.trackId) return;
	const dur = s.completo?.duration ?? Infinity;
	const destino = Math.min(Math.max(0, ms / 1000), Math.max(0, dur - 0.05));
	// Ya está ahí (la confirmación del backend de un salto que la interfaz ya
	// hizo por su cuenta).
	if (Math.abs(posicionDe(s) - destino) < 0.25 && !s.finalizada) return;
	s.finalizada = false;
	if (cola !== null) {
		terminarCola();
		fijarXf(s, 1);
	}
	if (s.voz) {
		// La vieja sale fundida mientras la nueva entra: sin clic ni hueco.
		apagar(s.voz);
		s.voz = null;
		if (!sonar(s, destino)) {
			cerrarTramo();
			pararTicks();
		}
	} else {
		// Parada: solo se coloca. Sonar lo decide reproducir().
		s.posParada = destino;
	}
	emitirPosicion();
}

/** Lo que vendrá después, para el relevo (se decodifica cuando toca). */
export function precargar(trackId) {
	siguiente = trackId || null;
	revisarPrecarga();
}

export function fijarVolumen(v) {
	if (typeof v !== "number" || !isFinite(v)) return;
	volumen = Math.min(1, Math.max(0, v));
	maestro.gain.setTargetAtTime(curva(volumen), ctx.currentTime, 0.01);
}

export function fijarEq(ganancias) {
	gananciasEq = new Float32Array(ganancias);
	filtros.forEach((f, i) => {
		f.gain.value = gananciasEq[i] ?? 0;
	});
}

export function fijarCrossfade(ms) {
	crossfadeMs = typeof ms === "number" && ms > 0 ? ms : 0;
	programarSiguiente();
}

montarGrafo();

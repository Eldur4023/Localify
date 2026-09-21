import { t } from "../i18n/index.js";
/** Frecuencias de las bandas. Deben coincidir con `BANDAS_EQ_HZ` en Rust. */
const BANDAS_HZ = [
	31,
	62,
	125,
	250,
	500,
	1e3,
	2e3,
	4e3,
	8e3,
	16e3
];
/** Ganancia máxima por banda, en dB. Espejo de `GANANCIA_MAX_DB`. */
const MAX_DB = 12;
/** Identificador del perfil que se crea al retocar uno de fábrica. */
const ID_PERSONALIZADO = "custom";
/** Cada cuánto, como mucho, se persiste mientras se arrastra. */
const PERIODO_GUARDADO_MS = 400;
/** Geometría del dibujo, en unidades del `viewBox`. */
const ANCHO = 300;
const ALTO = 100;
/** Etiqueta corta de una frecuencia: `16000` → `16k`. */
function etiqueta(hz) {
	return hz >= 1e3 ? `${hz / 1e3}k` : String(hz);
}
export function mountEqualizer(contenedor, opciones) {
	const el = document.createElement("div");
	el.className = "eq";
	// ── Curva ────────────────────────────────────────────────────────────────
	const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
	svg.setAttribute("class", "eq__curva");
	svg.setAttribute("viewBox", `0 0 ${ANCHO} ${ALTO}`);
	svg.setAttribute("preserveAspectRatio", "none");
	// Decorativa: lo que el ecualizador hace ya lo dicen los deslizadores, que
	// sí tienen nombre y valor. Anunciarla otra vez sería ruido.
	svg.setAttribute("aria-hidden", "true");
	const cero = document.createElementNS("http://www.w3.org/2000/svg", "line");
	cero.setAttribute("class", "eq__cero");
	cero.setAttribute("x1", "0");
	cero.setAttribute("x2", String(ANCHO));
	cero.setAttribute("y1", String(ALTO / 2));
	cero.setAttribute("y2", String(ALTO / 2));
	const curva = document.createElementNS("http://www.w3.org/2000/svg", "path");
	curva.setAttribute("class", "eq__linea");
	svg.append(cero, curva);
	// ── Deslizadores ─────────────────────────────────────────────────────────
	const bandas = document.createElement("div");
	bandas.className = "eq__bandas";
	let perfil = {
		...opciones.inicial,
		gainsDb: [...opciones.inicial.gainsDb]
	};
	let temporizador = null;
	const controles = [];
	const salidas = [];
	/** Punto de la curva para una banda. */
	function punto(i, db) {
		const x = i / (BANDAS_HZ.length - 1) * ANCHO;
		// El eje va al revés que en audio: +12 dB arriba es y=0.
		const y = ALTO / 2 - db / MAX_DB * (ALTO / 2);
		return [x, y];
	}
	/**
	* Traza la curva con una spline de Catmull-Rom convertida a Bézier.
	*
	* Unir los diez puntos con rectas dibujaría una línea quebrada que no se
	* parece a lo que hace un banco de filtros: la respuesta real entre dos
	* bandas es suave. Una curva recta mentiría sobre el efecto.
	*/
	function trazar() {
		const p = perfil.gainsDb.map((db, i) => punto(i, db));
		if (p.length === 0) return;
		const partes = [`M ${p[0]?.[0] ?? 0} ${p[0]?.[1] ?? 0}`];
		for (let i = 0; i < p.length - 1; i += 1) {
			const p0 = p[Math.max(0, i - 1)];
			const p1 = p[i];
			const p2 = p[i + 1];
			const p3 = p[Math.min(p.length - 1, i + 2)];
			if (!p0 || !p1 || !p2 || !p3) continue;
			const c1x = p1[0] + (p2[0] - p0[0]) / 6;
			const c1y = p1[1] + (p2[1] - p0[1]) / 6;
			const c2x = p2[0] - (p3[0] - p1[0]) / 6;
			const c2y = p2[1] - (p3[1] - p1[1]) / 6;
			partes.push(`C ${c1x} ${c1y}, ${c2x} ${c2y}, ${p2[0]} ${p2[1]}`);
		}
		curva.setAttribute("d", partes.join(" "));
	}
	/** Sincroniza deslizadores y lecturas con el perfil vigente. */
	function pintar() {
		perfil.gainsDb.forEach((db, i) => {
			const control = controles[i];
			const salida = salidas[i];
			if (control) control.value = String(db);
			if (salida) salida.textContent = `${db > 0 ? "+" : ""}${db.toFixed(0)}`;
		});
		trazar();
	}
	/** Programa la persistencia, reiniciando el reloj en cada movimiento. */
	function programarGuardado() {
		if (temporizador !== null) globalThis.clearTimeout(temporizador);
		temporizador = globalThis.setTimeout(() => {
			temporizador = null;
			opciones.alAsentarse(perfil);
		}, PERIODO_GUARDADO_MS);
	}
	BANDAS_HZ.forEach((hz, i) => {
		const banda = document.createElement("div");
		banda.className = "eq__banda";
		const salida = document.createElement("span");
		salida.className = "eq__db";
		const control = document.createElement("input");
		control.type = "range";
		control.className = "eq__slider";
		control.min = String(-MAX_DB);
		control.max = String(MAX_DB);
		control.step = "1";
		control.value = String(perfil.gainsDb[i] ?? 0);
		control.setAttribute("aria-label", t("eq.band", { hz: etiqueta(hz) }));
		// Vertical de verdad, no un `transform`: rotar un `range` con CSS deja el
		// ratón y el teclado invertidos respecto a lo que se ve.
		control.setAttribute("orient", "vertical");
		const nombre = document.createElement("span");
		nombre.className = "eq__hz";
		nombre.textContent = etiqueta(hz);
		control.addEventListener("input", () => {
			const gains = [...perfil.gainsDb];
			gains[i] = Number(control.value);
			// Retocar un perfil de fábrica lo convierte en el personalizado: seguir
			// llamándolo "Graves" haría creer que ese perfil suena así.
			perfil = {
				id: ID_PERSONALIZADO,
				nameKey: "eq.custom",
				gainsDb: gains
			};
			salida.textContent = `${gains[i] > 0 ? "+" : ""}${gains[i].toFixed(0)}`;
			trazar();
			opciones.alCambiar(perfil);
			programarGuardado();
		});
		banda.append(salida, control, nombre);
		bandas.append(banda);
		controles.push(control);
		salidas.push(salida);
	});
	el.append(svg, bandas);
	contenedor.append(el);
	pintar();
	return {
		el,
		mostrar(nuevo) {
			perfil = {
				...nuevo,
				gainsDb: [...nuevo.gainsDb]
			};
			pintar();
		},
		destroy() {
			if (temporizador !== null) {
				globalThis.clearTimeout(temporizador);
				// Lo que estuviera pendiente se guarda: desmontar la vista mientras el
				// temporizador corre no puede tirar el último ajuste del usuario.
				opciones.alAsentarse(perfil);
				temporizador = null;
			}
			el.remove();
		}
	};
}

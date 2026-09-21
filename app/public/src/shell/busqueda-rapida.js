import { player, search } from "../ipc/client.js";
import { alCambiarIdioma, t } from "../i18n/index.js";
import { comienzoDePista } from "../ui/cards.js";
import { abrirMenu } from "../ui/menu.js";
import { opcionesDePista } from "../ui/opciones-pista.js";
import { duracion } from "./player.js";
/** Resultados del desplegable. Ver la cabecera del módulo. */
const CUANTOS = 4;
/**
* Canciones que se le piden al servicio para quedarse con cuatro.
*
* La búsqueda pagina **antes** de agrupar versiones, así que pedir cuatro
* filas puede devolver un solo grupo: "faint" trae la de estudio, el directo,
* la instrumental y la maqueta, y las cuatro se pliegan en una. Con margen de
* sobra el desplegable enseña cuatro canciones distintas, que es lo que aquí
* sirve de algo.
*/
const PEDIDAS = 20;
/**
* Espera tras la última tecla antes de consultar.
*
* Aquí sí hay rebote, al contrario que en la vista Buscar: allí la consulta
* local es lo único que se pinta al instante y frenarla se notaría. Este
* desplegable aparece encima del contenido, y que parpadee con cada letra
* mientras se escribe es peor que aparecer un pelo más tarde.
*/
const REBOTE = 160;
export function mountBusquedaRapida(contenedor, router) {
	const caja = document.createElement("div");
	caja.className = "rapida";
	const entrada = document.createElement("input");
	entrada.type = "search";
	entrada.className = "rapida__input";
	entrada.autocomplete = "off";
	entrada.spellcheck = false;
	entrada.setAttribute("role", "combobox");
	entrada.setAttribute("aria-expanded", "false");
	entrada.setAttribute("aria-autocomplete", "list");
	const panel = document.createElement("div");
	panel.className = "rapida__panel";
	panel.setAttribute("role", "listbox");
	panel.hidden = true;
	caja.append(entrada, panel);
	contenedor.append(caja);
	let resultados = [];
	let activa = -1;
	let temporizador = null;
	/** El respiro entre `blur` y `cerrar()` (ver más abajo), para poder cancelarlo. */
	let cierreTemporizador = null;
	/**
	* Consulta cuya respuesta seguimos esperando.
	*
	* Escribir rápido lanza varias y no tienen por qué volver en orden. Sin este
	* número, la respuesta de «bo» puede pisar a la de «bohemian».
	*/
	let ultima = 0;
	function cerrar() {
		panel.hidden = true;
		panel.replaceChildren();
		entrada.setAttribute("aria-expanded", "false");
		entrada.removeAttribute("aria-activedescendant");
		resultados = [];
		activa = -1;
	}
	function marcar(indice) {
		activa = indice;
		for (const [i, fila] of [...panel.children].entries()) {
			fila.classList.toggle("is-active", i === indice);
		}
		if (indice >= 0) entrada.setAttribute("aria-activedescendant", `rapida-${indice}`);
		else entrada.removeAttribute("aria-activedescendant");
	}
	function reproducir(indice) {
		const pista = resultados[indice];
		if (!pista) return;
		void player.playTrack(pista.id, {
			kind: "search",
			query: entrada.value,
			trackIds: resultados.map((p) => p.id)
		});
		cerrar();
		entrada.blur();
	}
	function pintar() {
		panel.replaceChildren();
		if (resultados.length === 0) {
			cerrar();
			return;
		}
		for (const [i, pista] of resultados.entries()) {
			const fila = document.createElement("div");
			fila.className = "rapida__fila";
			fila.id = `rapida-${i}`;
			fila.setAttribute("role", "option");
			fila.setAttribute("aria-selected", "false");
			const textos = document.createElement("div");
			textos.className = "rapida__textos";
			const titulo = document.createElement("div");
			titulo.className = "rapida__titulo";
			titulo.textContent = pista.title;
			const artista = document.createElement("div");
			artista.className = "rapida__artista";
			artista.textContent = pista.artistDisplay;
			textos.append(titulo, artista);
			const tiempo = document.createElement("span");
			tiempo.className = "rapida__tiempo";
			tiempo.textContent = duracion(pista.durationMs);
			fila.append(comienzoDePista(pista.albumId, pista.id), textos, tiempo);
			// `mousedown` y no `click`: el `blur` del campo cierra el panel, y con
			// `click` la fila desaparecería antes de que el navegador lo entregara.
			//
			// `preventDefault` va para **cualquier** botón, porque es lo que impide
			// que el foco salga del campo y el panel se cierre solo. Pero reproducir
			// es cosa del botón izquierdo: sin esa comprobación, el clic derecho
			// ponía la canción a sonar y hacía desaparecer el panel antes de que el
			// menú contextual llegara a abrirse.
			fila.addEventListener("mousedown", (e) => {
				e.preventDefault();
				if (e.button === 0) reproducir(i);
			});
			fila.addEventListener("mouseenter", () => marcar(i));
			fila.addEventListener("contextmenu", (e) => {
				e.preventDefault();
				abrirMenu(e.clientX, e.clientY, opcionesDePista(pista, { contexto: () => ({
					kind: "search",
					query: entrada.value,
					trackIds: resultados.map((p) => p.id)
				}) }));
			});
			panel.append(fila);
		}
		panel.hidden = false;
		entrada.setAttribute("aria-expanded", "true");
		marcar(-1);
	}
	async function consultar() {
		const q = entrada.value.trim();
		if (q.length === 0) {
			cerrar();
			return;
		}
		ultima += 1;
		const mia = ultima;
		try {
			// Ámbito acotado a canciones: el desplegable no pinta álbumes ni
			// artistas, y pedirlos serían tres consultas más para tirarlas.
			const r = await search.query(q, "tracks", {
				offset: 0,
				limit: PEDIDAS,
				cursor: null
			});
			if (mia !== ultima) return;
			// `tracks` son **grupos de versiones**, no canciones sueltas: cada uno
			// lleva su `principal` y las variantes colgando. Tratarlos como pistas
			// dejaba el panel con cuatro filas sin título ni artista y "NaN:NaN" de
			// duración, porque los campos que se leían no existían en ese objeto.
			//
			// Para un atajo de cuatro huecos el principal es además lo que se quiere:
			// cuatro canciones distintas, no cuatro versiones de la misma.
			resultados = r.tracks.slice(0, CUANTOS).map((g) => g.principal);
			pintar();
		} catch {
			// Una búsqueda fallida no merece un aviso encima de la barra: se cierra
			// el panel y el usuario sigue escribiendo.
			cerrar();
		}
	}
	const alEscribir = () => {
		if (temporizador !== null) globalThis.clearTimeout(temporizador);
		temporizador = globalThis.setTimeout(() => void consultar(), REBOTE);
	};
	const alTeclear = (e) => {
		switch (e.key) {
			case "ArrowDown":
				e.preventDefault();
				if (resultados.length > 0) marcar(Math.min(resultados.length - 1, activa + 1));
				break;
			case "ArrowUp":
				e.preventDefault();
				if (resultados.length > 0) marcar(Math.max(-1, activa - 1));
				break;
			case "Enter": {
				e.preventDefault();
				// Con una fila elegida, suena. Sin ninguna, Enter lleva a la búsqueda
				// completa: es lo que hace todo el mundo cuando quiere ver más.
				if (activa >= 0) reproducir(activa);
				else irABuscar();
				break;
			}
			case "Escape":
				if (!panel.hidden) {
					e.stopPropagation();
					cerrar();
				} else {
					entrada.blur();
				}
				break;
			default: break;
		}
	};
	function irABuscar() {
		const q = entrada.value.trim();
		if (q.length === 0) return;
		cerrar();
		entrada.blur();
		// La consulta viaja en la ruta: así la vista Buscar arranca con ella
		// puesta y no hay que volver a escribirla.
		router.ir(`search/${encodeURIComponent(q)}`);
	}
	entrada.addEventListener("input", alEscribir);
	entrada.addEventListener("keydown", alTeclear);
	entrada.addEventListener("focus", () => {
		// Cancela el cierre pendiente de un `blur` anterior: sin esto, volver a
		// enfocar el campo justo tras un clic accidental fuera no evita que el
		// panel se cierre solo 120 ms después, ya con el campo otra vez enfocado.
		if (cierreTemporizador !== null) {
			globalThis.clearTimeout(cierreTemporizador);
			cierreTemporizador = null;
		}
		if (resultados.length > 0) panel.hidden = false;
	});
	entrada.addEventListener("blur", () => {
		// Un respiro antes de cerrar: sin él, soltar el ratón sobre una fila llega
		// después del cierre y el clic se pierde.
		cierreTemporizador = globalThis.setTimeout(() => {
			cierreTemporizador = null;
			cerrar();
		}, 120);
	});
	function etiquetas() {
		entrada.placeholder = t("search.quick");
		entrada.setAttribute("aria-label", t("search.quick"));
	}
	etiquetas();
	const dejarIdioma = alCambiarIdioma(etiquetas);
	return { destroy() {
		dejarIdioma();
		if (temporizador !== null) globalThis.clearTimeout(temporizador);
		if (cierreTemporizador !== null) globalThis.clearTimeout(cierreTemporizador);
		caja.remove();
	} };
}

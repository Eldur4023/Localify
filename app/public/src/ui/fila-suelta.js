import { player } from "../ipc/client.js";
import { duracion } from "../shell/player.js";
import { comienzoDePista } from "./cards.js";
import { arrastrable } from "./dnd.js";
import { abrirMenu } from "./menu.js";
import { opcionesDePista } from "./opciones-pista.js";
import { nodo } from "./dom.js";
export function filaSuelta(pista, opciones) {
	const el = nodo("div", "track track--suelta");
	if (opciones.indice !== undefined) {
		const num = nodo("span", "track__index", String(opciones.indice + 1));
		el.append(num);
	}
	el.append(comienzoDePista(pista.albumId, pista.id));
	const titulo = nodo("span", "track__title", pista.title);
	const secundario = nodo("span", "track__artist", opciones.secundario);
	const tiempo = nodo("span", "track__time", duracion(pista.durationMs));
	// Título y artista apilados, como en la lista de canciones de la biblioteca
	const principal = nodo("div", "track__main");
	principal.append(titulo, secundario);
	el.append(principal, tiempo);
	const reproducir = () => {
		void player.playTrack(pista.id, opciones.contexto());
	};
	el.addEventListener("click", reproducir);
	el.addEventListener("contextmenu", (e) => {
		e.preventDefault();
		abrirMenu(e.clientX, e.clientY, opcionesDePista(pista, { contexto: opciones.contexto }));
	});
	const soltarArrastre = arrastrable(el, () => [pista.id]);
	return {
		el,
		destroy() {
			soltarArrastre();
		}
	};
}

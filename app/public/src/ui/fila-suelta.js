import { player } from "../ipc/client.js";
import { duracion } from "../shell/player.js";
import { comienzoDePista } from "./cards.js";
import { arrastrable } from "./dnd.js";
import { abrirMenu } from "./menu.js";
import { opcionesDePista } from "./opciones-pista.js";
export function filaSuelta(pista, opciones) {
	const el = document.createElement("div");
	el.className = "track track--suelta";
	if (opciones.indice !== undefined) {
		const num = document.createElement("span");
		num.className = "track__index";
		num.textContent = String(opciones.indice + 1);
		el.append(num);
	}
	el.append(comienzoDePista(pista.albumId, pista.id));
	const titulo = document.createElement("span");
	titulo.className = "track__title";
	titulo.textContent = pista.title;
	const secundario = document.createElement("span");
	secundario.className = "track__artist";
	secundario.textContent = opciones.secundario;
	const tiempo = document.createElement("span");
	tiempo.className = "track__time";
	tiempo.textContent = duracion(pista.durationMs);
	el.append(titulo, secundario, tiempo);
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

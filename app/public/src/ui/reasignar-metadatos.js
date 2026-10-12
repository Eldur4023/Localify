import { library } from "../ipc/client.js";
import { t } from "../i18n/index.js";
import { duracion } from "../shell/player.js";
import { nodo } from "./dom.js";
const LIMITE = 15;
function artistas(c) {
	return c.artists.map((a) => a.name).join(", ");
}
/**
* Abre el diálogo con `consultaInicial` ya escrita. Resuelve al candidato
* elegido, o a `null` si se cancela sin elegir ninguno.
*/
export function elegirCandidato(consultaInicial) {
	return new Promise((resolver) => {
		const dlg = nodo("dialog", "dialogo dialogo--reasignar");
		const form = document.createElement("form");
		form.method = "dialog";
		const h = nodo("h2", "dialogo__titulo", t("reassign.title"));
		const fila = nodo("div", "reasignar__busqueda");
		const entrada = document.createElement("input");
		entrada.type = "text";
		entrada.className = "dialogo__input";
		entrada.value = consultaInicial;
		entrada.autocomplete = "off";
		entrada.spellcheck = false;
		const buscar = document.createElement("button");
		buscar.type = "button";
		buscar.className = "boton";
		buscar.textContent = t("reassign.search");
		fila.append(entrada, buscar);
		const resultados = nodo("ul", "reasignar__resultados");
		const estado = nodo("p", "dialogo__mensaje");
		estado.hidden = true;
		let elegido = null;
		function cerrar() {
			dlg.close();
		}
		function pintarResultados(candidatos) {
			resultados.replaceChildren();
			estado.hidden = candidatos.length > 0;
			if (candidatos.length === 0) {
				estado.textContent = t("reassign.empty");
				return;
			}
			for (const c of candidatos) {
				const li = nodo("li", "reasignar__opcion");
				li.tabIndex = 0;
				const titulo = nodo("span", "reasignar__opcion-titulo", c.title);
				const detalle = nodo("span", "reasignar__opcion-detalle");
				const partes = [
					artistas(c),
					c.album?.title,
					duracion(c.durationMs)
				].filter((p) => Boolean(p));
				detalle.textContent = partes.join(" · ");
				li.append(titulo, detalle);
				const elegir = () => {
					elegido = c;
					cerrar();
				};
				li.addEventListener("click", elegir);
				li.addEventListener("keydown", (e) => {
					if (e.key === "Enter") elegir();
				});
				resultados.append(li);
			}
		}
		async function ejecutarBusqueda() {
			const consulta = entrada.value.trim();
			if (!consulta) return;
			buscar.disabled = true;
			estado.hidden = false;
			estado.textContent = t("common.loading");
			try {
				const candidatos = await library.searchCandidates(consulta, LIMITE);
				pintarResultados(candidatos);
			} catch (e) {
				estado.hidden = false;
				estado.textContent = String(e);
			}
			buscar.disabled = false;
		}
		buscar.addEventListener("click", () => void ejecutarBusqueda());
		entrada.addEventListener("keydown", (e) => {
			if (e.key === "Enter") {
				e.preventDefault();
				void ejecutarBusqueda();
			}
		});
		const acciones = nodo("div", "dialogo__acciones");
		const cancelar = document.createElement("button");
		cancelar.type = "button";
		cancelar.className = "boton boton--sutil";
		cancelar.textContent = t("common.cancel");
		cancelar.addEventListener("click", cerrar);
		acciones.append(cancelar);
		form.append(h, fila, estado, resultados, acciones);
		dlg.append(form);
		document.body.append(dlg);
		dlg.addEventListener("click", (e) => {
			if (e.target === dlg) cerrar();
		});
		dlg.addEventListener("close", () => {
			dlg.remove();
			resolver(elegido);
		});
		dlg.showModal();
		entrada.select();
		// Búsqueda inicial con el título actual: quien abre esto casi siempre
		// quiere corregir lo que ya hay, no partir de cero.
		if (consultaInicial.trim()) void ejecutarBusqueda();
	});
}

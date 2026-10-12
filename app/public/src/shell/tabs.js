/**
* Pestañas inferiores (solo móvil, por CSS): los cuatro sitios principales a
* un toque, como en Spotify. El cajón queda para playlists y ajustes.
*/
import { alCambiarIdioma, t } from "../i18n/index.js";
import { icono } from "../ui/icons.js";
const PESTAÑAS = [
	["home", "nav.home", "home"],
	["search", "nav.search", "search"],
	["library", "nav.library", "library"],
	["social", "nav.social", "users"]
];
export function mountTabs(armazon, router) {
	const nav = document.createElement("nav");
	nav.className = "tabs";
	const enlaces = new Map();
	for (const [ruta, clave, ic] of PESTAÑAS) {
		const a = document.createElement("a");
		a.className = "tabs__item";
		a.href = `#/${ruta}`;
		const texto = document.createElement("span");
		a.append(icono(ic, 24), texto);
		nav.append(a);
		enlaces.set(ruta, [a, texto, clave]);
	}
	function etiquetas() {
		for (const [, texto, clave] of enlaces.values()) texto.textContent = t(clave);
	}
	etiquetas();
	alCambiarIdioma(etiquetas);
	armazon.append(nav);
	return {
		marcar(nombre) {
			for (const [ruta, [a]] of enlaces) a.classList.toggle("is-active", ruta === nombre);
		}
	};
}

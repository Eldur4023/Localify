import { library, page } from "../ipc/client.js";
import { alCambiarIdioma, t } from "../i18n/index.js";
import { mountTrackList } from "../ui/track-list.js";
const POR_PAGINA = 100;
export function mountLikedView(contenedor) {
	const el = document.createElement("section");
	el.className = "vista vista--lista";
	const cabecera = document.createElement("header");
	cabecera.className = "vista__header vista__header--destacada";
	const titulo = document.createElement("h2");
	const cuenta = document.createElement("span");
	cuenta.className = "vista__count";
	cabecera.append(titulo, cuenta);
	const cuerpo = document.createElement("div");
	cuerpo.className = "vista__body";
	const vacio = document.createElement("p");
	vacio.className = "vista__empty";
	vacio.hidden = true;
	el.append(cabecera, vacio, cuerpo);
	contenedor.replaceChildren(el);
	let offset = 0;
	let total = null;
	const trackList = mountTrackList(cuerpo, {
		contexto: () => ({ kind: "liked" }),
		numerar: true,
		conAlbum: true,
		conFecha: true,
		reiniciarOrigen() {
			offset = 0;
			total = null;
		},
		async cargar() {
			const req = page({
				offset,
				limit: POR_PAGINA
			});
			const respuesta = await library.favorites(req);
			offset += respuesta.items.length;
			total ??= respuesta.total;
			pintarCuenta();
			vacio.hidden = offset > 0;
			// Favoritos no expone cursor: una lista de me gusta rara vez pasa de unos
			// miles, y el desplazamiento por `offset` ahí no se nota.
			return {
				items: respuesta.items,
				hasMore: respuesta.items.length === POR_PAGINA
			};
		}
	});
	function pintarCuenta() {
		const cuantas = total === null ? trackList.lista.items.length : Number(total);
		cuenta.textContent = t("library.count", { count: cuantas });
	}
	function etiquetas() {
		titulo.textContent = t("liked.title");
		vacio.textContent = t("liked.empty");
		pintarCuenta();
		trackList.lista.refresh();
	}
	etiquetas();
	const dejarIdioma = alCambiarIdioma(etiquetas);
	return { destroy() {
		dejarIdioma();
		trackList.destroy();
		el.remove();
	} };
}

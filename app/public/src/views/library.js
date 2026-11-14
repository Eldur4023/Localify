import { library, page } from "../ipc/client.js";
import { alCambiarIdioma, t } from "../i18n/index.js";
import { alRecibirTipo } from "../ipc/events.js";
import { mountTrackList } from "../ui/track-list.js";
import { nodo } from "../ui/dom.js";
import { editarPista } from "../ui/editar-pista.js";
import { mostrarError } from "../ui/error-overlay.js";
const POR_PAGINA = 100;
/** Ordenaciones ofrecidas, con su clave de traducción. */
const ORDENES = [
	{
		valor: "titleAsc",
		clave: "library.sort.title"
	},
	{
		valor: "artistAsc",
		clave: "library.sort.artist"
	},
	{
		valor: "albumAsc",
		clave: "library.sort.album"
	},
	{
		valor: "addedDesc",
		clave: "library.sort.added"
	},
	{
		valor: "durationAsc",
		clave: "library.sort.duration"
	}
];
export function mountLibraryView(contenedor) {
	const el = nodo("section", "vista vista--lista");
	const cabecera = nodo("header", "vista__header");
	const titulo = document.createElement("h2");
	const cuenta = nodo("span", "vista__count");
	const controles = nodo("div", "vista__controls");
	const orden = nodo("select", "select");
	for (const o of ORDENES) {
		const opt = document.createElement("option");
		opt.value = o.valor;
		orden.append(opt);
	}
	// Añadir canciones propias, de una o de muchas: selector múltiple o arrastrar
	// ficheros a la vista. Se suben y se abre el editor una tras otra; lo vacío
	// de cada una hereda artista/álbum/fecha de la anterior (útil para un disco).
	const anadir = nodo("button", "boton");
	anadir.type = "button";
	const selector = document.createElement("input");
	selector.type = "file";
	selector.multiple = true;
	selector.accept = "audio/*,.mp3,.flac,.ogg,.opus,.m4a,.wav,.aac";
	selector.hidden = true;
	let ocupado = false;
	async function anadirArchivos(archivos) {
		const audios = [...archivos].filter((f) => f.type.startsWith("audio/") || /\.(mp3|flac|ogg|opus|m4a|wav|aac|wma)$/i.test(f.name));
		if (audios.length === 0 || ocupado) return;
		ocupado = true;
		anadir.disabled = true;
		const ids = [];
		try {
			for (const [i, f] of audios.entries()) {
				anadir.textContent = `${t("common.loading")} ${i + 1}/${audios.length}`;
				const informe = await library.uploadImport(f);
				ids.push(...informe.trackIds ?? []);
			}
			if (ids.length === 0) mostrarError(t("settings.import_failed"), "");
			let previo = null;
			for (const [i, id] of ids.entries()) {
				const r = await editarPista(id, {
					indice: i + 1,
					total: ids.length,
					previo
				});
				if (r) previo = r;
			}
		} catch (e) {
			mostrarError(t("settings.import_failed"), String(e));
		}
		ocupado = false;
		anadir.disabled = false;
		anadir.textContent = t("library.add");
	}
	anadir.addEventListener("click", () => selector.click());
	selector.addEventListener("change", () => {
		const elegidos = [...selector.files ?? []];
		selector.value = "";
		void anadirArchivos(elegidos);
	});
	el.addEventListener("dragover", (e) => e.preventDefault());
	el.addEventListener("drop", (e) => {
		e.preventDefault();
		void anadirArchivos(e.dataTransfer?.files ?? []);
	});
	controles.append(anadir, selector, orden);
	cabecera.append(titulo, cuenta, controles);
	const cuerpo = nodo("div", "vista__body");
	// Recién instalada, esta es la primera pantalla que se ve, y era una lista
	// vacía con un "0 canciones" encima: indistinguible de un fallo de carga. Que
	// el texto diga qué hacer —buscar algo— importa más aquí que en ningún otro
	// sitio, porque es donde alguien decide si la aplicación funciona.
	const vacio = nodo("p", "vista__empty");
	vacio.hidden = true;
	el.append(cabecera, vacio, cuerpo);
	contenedor.replaceChildren(el);
	// ── Estado de la consulta ───────────────────────────────────────────────
	let cursor = null;
	let total = null;
	/** Sin acotar: el catálogo entero, esté o no en disco. */
	const FILTRO = {
		favoritesOnly: false,
		localOnly: false,
		albumId: null,
		artistId: null,
		genreId: null,
		text: null
	};
	const trackList = mountTrackList(cuerpo, {
		contexto: () => ({ kind: "library" }),
		conAlbum: true,
		conFecha: true,
		reiniciarOrigen() {
			cursor = null;
			total = null;
		},
		async cargar() {
			const req = page({
				limit: POR_PAGINA,
				cursor
			});
			const respuesta = await library.tracks(FILTRO, orden.value, req);
			cursor = respuesta.nextCursor;
			total ??= respuesta.total;
			pintarCuenta();
			// Los elementos van siempre, incluida la última página: descartarlos
			// perdería las últimas cien canciones de la biblioteca.
			return {
				items: respuesta.items,
				hasMore: cursor !== null
			};
		}
	});
	function pintarCuenta() {
		const cuantas = total === null ? trackList.lista.items.length : Number(total);
		cuenta.textContent = t("library.count", { count: cuantas });
		// Solo con el total ya contado. Mientras es `null` no se sabe si está vacía
		// o si la primera página aún no ha llegado, y enseñar "no hay nada" durante
		// la carga es peor que no enseñar nada.
		vacio.hidden = total === null || cuantas > 0;
	}
	function etiquetas() {
		titulo.textContent = t("library.title");
		if (!ocupado) anadir.textContent = t("library.add");
		vacio.textContent = t("library.empty");
		for (const [i, o] of ORDENES.entries()) {
			const opt = orden.options.item(i);
			if (opt) opt.textContent = t(o.clave);
		}
		pintarCuenta();
		trackList.lista.refresh();
	}
	const recargar = () => trackList.refrescar();
	orden.addEventListener("change", recargar);
	// Un rescan y una importación de ficheros propios cambian qué pistas hay
	// sin que el usuario haga nada en esta vista: sin escuchar el evento, las
	// canciones nuevas solo aparecerían al navegar fuera y volver.
	const dejarLibraryChanged = alRecibirTipo("libraryChanged", (evento) => {
		if (evento.scope === "tracks") recargar();
	});
	etiquetas();
	const dejarIdioma = alCambiarIdioma(etiquetas);
	return { destroy() {
		dejarIdioma();
		dejarLibraryChanged();
		orden.removeEventListener("change", recargar);
		trackList.destroy();
		el.remove();
	} };
}

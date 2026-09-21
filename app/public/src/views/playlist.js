import { page, playlists as api } from "../ipc/client.js";
import { alCambiarIdioma, t } from "../i18n/index.js";
import { mountTrackList } from "../ui/track-list.js";
import { abrirMenu } from "../ui/menu.js";
import { cambiarFoto, opcionesDePlaylist } from "../ui/opciones-playlist.js";
import { zonaDeReordenacion, zonaDeSoltado, TIPO_PISTAS } from "../ui/dnd.js";
import { botonIcono, icono } from "../ui/icons.js";
import { ponerImagenDePlaylist } from "../ui/cards.js";
/** Tope de entradas que se traen de una vez. */
const TOPE = 5e3;
export function mountPlaylistView(contenedor, ruta) {
	const playlistId = ruta.params[0] ?? "";
	const el = document.createElement("section");
	el.className = "vista vista--lista";
	const cabecera = document.createElement("header");
	cabecera.className = "vista__header vista__header--ficha";
	const arte = document.createElement("div");
	arte.className = "ficha__arte";
	arte.append(icono("music", 48));
	const texto = document.createElement("div");
	texto.className = "ficha__texto";
	const titulo = document.createElement("h2");
	titulo.className = "vista__title";
	const cuenta = document.createElement("span");
	cuenta.className = "vista__count";
	// La imagen se cambia pulsándola. Es donde la gente lo intenta antes de
	// buscar un botón, y así el gesto está donde está la cosa que modifica.
	arte.classList.add("ficha__arte--editable");
	arte.tabIndex = 0;
	arte.setAttribute("role", "button");
	arte.addEventListener("click", () => void cambiarFoto(accionable(), reacciones));
	arte.addEventListener("keydown", (e) => {
		if (e.key === "Enter" || e.key === " ") {
			e.preventDefault();
			void cambiarFoto(accionable(), reacciones);
		}
	});
	const acciones = document.createElement("div");
	acciones.className = "ficha__acciones";
	// Un solo botón que abre el mismo menú que el clic derecho. Antes había tres,
	// dos de ellos con el icono de cerrar: uno quitaba la foto y el otro borraba
	// la playlist entera, y distinguirlos era cuestión de acertar.
	const masOpciones = botonIcono("more", "", () => {
		const caja = masOpciones.getBoundingClientRect();
		abrirOpciones(caja.left, caja.bottom);
	}, { tamano: 20 });
	acciones.append(masOpciones);
	// La descripción va entre el título y el recuento. Se oculta cuando no hay
	// ninguna —que es el caso de casi todas— en vez de dejar un hueco vacío que
	// se lee como un fallo de pintado.
	const descripcion = document.createElement("p");
	descripcion.className = "ficha__descripcion";
	descripcion.hidden = true;
	texto.append(titulo, descripcion, cuenta, acciones);
	cabecera.append(arte, texto);
	const vacio = document.createElement("p");
	vacio.className = "vista__empty";
	vacio.hidden = true;
	const cuerpo = document.createElement("div");
	cuerpo.className = "vista__body";
	el.append(cabecera, vacio, cuerpo);
	contenedor.replaceChildren(el);
	let detalle = null;
	let entregado = false;
	/** Imagen ya pintada, para no rehacerla en cada repintado. */
	let imagenPintada = "";
	const trackList = mountTrackList(cuerpo, {
		playlistId,
		conAlbum: true,
		conFecha: true,
		contexto: () => ({
			kind: "playlist",
			id: playlistId
		}),
		entryIdDe: (_pista, indice) => detalle?.entries[indice]?.entryId,
		alQuitar: () => recargar(),
		reiniciarOrigen() {
			entregado = false;
			detalle = null;
		},
		async cargar() {
			if (entregado) return {
				items: [],
				hasMore: false
			};
			detalle = await api.detail(playlistId, page({ limit: TOPE }));
			entregado = true;
			pintarCabecera();
			vacio.hidden = detalle.entries.length > 0;
			return {
				items: detalle.entries.map((e) => e.track),
				hasMore: false
			};
		}
	});
	function pintarCabecera() {
		if (!detalle) return;
		titulo.textContent = detalle.name;
		descripcion.textContent = detalle.description ?? "";
		descripcion.hidden = !detalle.description;
		cuenta.textContent = t("library.count", { count: detalle.trackCount });
		// La imagen se rehace solo si cambió: `pintarCabecera` corre también al
		// cambiar de idioma, y sin la guarda se apilarían imágenes iguales.
		// La marca de tiempo entra en la clave porque cambiar la foto la mueve.
		const clave = `${detalle.hasOwnCover}:${detalle.updatedAt}:${detalle.coverAlbums.join(",")}`;
		if (clave !== imagenPintada) {
			imagenPintada = clave;
			arte.querySelector(".mosaico")?.remove();
			arte.querySelector(".portada")?.remove();
			ponerImagenDePlaylist(arte, detalle);
		}
	}
	function recargar() {
		trackList.refrescar();
	}
	/** Lo que las acciones necesitan saber de esta playlist. */
	function accionable() {
		return {
			id: playlistId,
			name: detalle?.name ?? "",
			hasOwnCover: detalle?.hasOwnCover ?? false,
			description: detalle?.description ?? null
		};
	}
	const reacciones = {
		alCambiar: () => recargar(),
		alBorrar: () => {
			globalThis.location.hash = "#/library";
		}
	};
	function abrirOpciones(x, y) {
		if (!detalle) return;
		abrirMenu(x, y, opcionesDePlaylist(accionable(), reacciones));
	}
	// ── Arrastrar y soltar ──────────────────────────────────────────────────
	/** Soltar pistas desde fuera las añade al final. */
	const dejarSoltado = zonaDeSoltado(cuerpo, TIPO_PISTAS, async (ids) => {
		await api.addTracks(playlistId, ids, null);
		recargar();
	});
	/** Arrastrar dentro reordena. */
	const dejarReorden = zonaDeReordenacion(cuerpo, (destino) => destino instanceof Element ? destino.closest(".track") : null, (fila) => Number(fila.dataset["indice"] ?? "0"), async (entryId, indice) => {
		// Movimiento optimista: la fila salta ya y la recarga confirma. Se puede
		// porque reordenar es un solo `UPDATE` (ADR-009) y no falla salvo que la
		// playlist haya desaparecido.
		//
		// Hay que mover las dos copias: `entries` guarda los identificadores de
		// entrada —que la lista necesita para el menú— y la lista guarda las
		// pistas. Mover solo una las descuadraría.
		const desde = detalle?.entries.findIndex((e) => e.entryId === entryId) ?? -1;
		if (desde >= 0 && detalle) {
			const [movida] = detalle.entries.splice(desde, 1);
			if (movida) {
				detalle.entries.splice(indice > desde ? indice - 1 : indice, 0, movida);
			}
			trackList.lista.move(desde, indice);
		}
		// Sin recarga si sale bien: ya se pintó el movimiento, y volver a pedirlo
		// todo al backend es lo que deshacía el gesto suave (reinicia el scroll y
		// vuelve a poblar la lista virtualizada de cero). Solo hace falta corregir
		// la vista si el `UPDATE` falla de verdad.
		try {
			await api.reorder(playlistId, entryId, indice);
		} catch (error) {
			console.error("Fallo al reordenar la playlist, recargando", error);
			recargar();
		}
	});
	function etiquetas() {
		vacio.textContent = t("playlist.empty");
		masOpciones.setAttribute("aria-label", t("common.more"));
		masOpciones.title = t("common.more");
		arte.setAttribute("aria-label", t("playlist.cover"));
		arte.title = t("playlist.cover");
		pintarCabecera();
		trackList.lista.refresh();
	}
	etiquetas();
	const dejarIdioma = alCambiarIdioma(etiquetas);
	return { destroy() {
		dejarIdioma();
		dejarSoltado();
		dejarReorden();
		trackList.destroy();
		el.remove();
	} };
}

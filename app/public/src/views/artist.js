import { library, player } from "../ipc/client.js";
import { alRecibirTipo } from "../ipc/events.js";
import { alCambiarIdioma, t } from "../i18n/index.js";
import { carrusel, conEspera, ponerFotoDeArtista, tarjeta, vacio } from "../ui/cards.js";
import { filaSuelta } from "../ui/fila-suelta.js";
import { botonIcono, icono } from "../ui/icons.js";
import { nodo } from "../ui/dom.js";
export function mountArtistView(contenedor, ruta) {
	const artistId = ruta.params[0] ?? "";
	const el = nodo("section", "vista vista--scroll");
	const cabecera = nodo("header", "vista__header vista__header--ficha");
	const arte = nodo("div", "ficha__arte ficha__arte--redonda");
	arte.append(icono("music", 48));
	// La foto se pide antes de tener la ficha: el identificador ya está en la
	// ruta y el backend la resuelve solo. Esperar a `detalle` retrasaría la
	// imagen una petición entera sin ganar nada.
	ponerFotoDeArtista(arte, artistId);
	const texto = nodo("div", "ficha__texto");
	const titulo = nodo("h2", "vista__title");
	const meta = nodo("p", "ficha__meta");
	const acciones = nodo("div", "ficha__acciones");
	const reproducir = botonIcono("play", "", () => void reproducirTop(), {
		tamano: 22,
		clase: "bicono--primario"
	});
	// Oculto hasta saber si hay algo que reproducir: un botón que aparece durante
	// la carga y podría no tener nada detrás promete lo que no puede cumplir.
	reproducir.hidden = true;
	acciones.append(reproducir);
	texto.append(titulo, meta, acciones);
	cabecera.append(arte, texto);
	const secciones = nodo("div", "vista__body");
	el.append(cabecera, secciones);
	contenedor.replaceChildren(el);
	let detalle = null;
	const desmontadores = [];
	let dejarBiblioteca = null;
	let recargaTimer = 0;
	/**
	* En el móvil la ficha vuelve con lo que ya hay y la discografía se trae en
	* segundo plano. Cuando termina, `libraryChanged` pide repintar; si sigue
	* importando, se mantiene la suscripción para la siguiente tanda.
	*/
	function sincronizarSuscripcion() {
		const esperando = detalle?.importing === true;
		if (esperando && !dejarBiblioteca) {
			dejarBiblioteca = alRecibirTipo("libraryChanged", programarRecarga);
		} else if (!esperando && dejarBiblioteca) {
			dejarBiblioteca();
			dejarBiblioteca = null;
		}
	}
	/** Huella de lo que se pinta: si no cambia, no se reconstruye la página. */
	function huella(d) {
		const pistas = (l) => (l ?? []).map((p) => `${p.id}:${p.availability?.kind ?? ""}`).join(",");
		const discos = (l) => (l ?? []).map((a) => a.id).join(",");
		return [d.name, d.localTrackCount, d.importing, pistas(d.topTracks), discos(d.albums), discos(d.singles), pistas(d.allTracks)].join("|");
	}
	function programarRecarga() {
		// Varios cambios de biblioteca seguidos (una descarga, la importación)
		// no deben reconstruir cientos de filas una vez por evento.
		if (recargaTimer) return;
		recargaTimer = setTimeout(() => {
			recargaTimer = 0;
			void library.artistDetail(artistId).then((d) => {
				const igual = detalle !== null && huella(d) === huella(detalle);
				detalle = d;
				sincronizarSuscripcion();
				if (!igual) etiquetas();
			}).catch(() => {});
		}, 350);
	}
	function filaDePista(pista, indice) {
		// El álbum en la segunda columna: en la ficha de un artista, repetir su
		// nombre en cada fila no dice nada que la pantalla no diga ya.
		const fila = filaSuelta(pista, {
			indice,
			secundario: pista.albumTitle ?? "",
			contexto: () => ({
				kind: "artist",
				id: artistId
			})
		});
		// Una discografía entera puede ser de cientos de filas. `content-visibility`
		// deja que el navegador se salte las que están fuera de pantalla (y con
		// `loading="lazy"` tampoco se piden sus portadas).
		fila.el.classList.add("track--perezosa");
		desmontadores.push(() => fila.destroy());
		return fila.el;
	}
	function pintar() {
		for (const quitar of desmontadores.splice(0)) quitar();
		secciones.replaceChildren();
		if (!detalle) return;
		titulo.textContent = detalle.name;
		// `localTrackCount` cuenta lo descargado, no lo que hay en el catálogo. Con
		// la clave genérica de "canciones" se leería "0 canciones" encima de una
		// lista de seis, que es justo lo contrario de lo que pasa.
		const partes = [t("artist.local_count", { count: detalle.localTrackCount })];
		if (detalle.genres.length > 0) partes.push(detalle.genres.slice(0, 3).join(", "));
		meta.textContent = partes.join(" · ");
		reproducir.hidden = detalle.topTracks.length === 0;
		if (detalle.topTracks.length > 0) {
			const bloque = nodo("section", "resultados__bloque");
			const h = nodo("h3", "carrusel__titulo", t("artist.popular"));
			bloque.append(h);
			detalle.topTracks.forEach((p, i) => bloque.append(filaDePista(p, i)));
			secciones.append(bloque);
		}
		if (detalle.albums.length > 0) {
			const { el: bloque, cuerpo } = carrusel(t("artist.albums"));
			for (const album of detalle.albums) {
				cuerpo.append(tarjeta({
					titulo: album.title,
					subtitulo: album.year ? String(album.year) : t("library.albums"),
					destino: `#/album/${album.id}`,
					albumId: album.id
				}));
			}
			secciones.append(bloque);
		}
		if (detalle.singles?.length > 0) {
			const { el: bloque, cuerpo } = carrusel(t("artist.singles"));
			for (const album of detalle.singles) {
				cuerpo.append(tarjeta({
					titulo: album.title,
					subtitulo: album.year ? String(album.year) : t("library.albums"),
					destino: `#/album/${album.id}`,
					albumId: album.id
				}));
			}
			secciones.append(bloque);
		}
		if (detalle.allTracks?.length > detalle.topTracks.length) {
			const bloque = nodo("section", "resultados__bloque");
			const h = nodo("h3", "carrusel__titulo", `${t("artist.all_songs")} (${detalle.allTracks.length})`);
			bloque.append(h);
			detalle.allTracks.forEach((p, i) => bloque.append(filaDePista(p, i)));
			secciones.append(bloque);
		}
		if (detalle.topTracks.length === 0 && detalle.albums.length === 0) {
			secciones.append(vacio(t("artist.empty")));
		}
	}
	async function reproducirTop() {
		const primera = detalle?.topTracks[0];
		if (!primera) return;
		await player.playTrack(primera.id, {
			kind: "artist",
			id: artistId
		});
	}
	function etiquetas() {
		reproducir.setAttribute("aria-label", t("menu.play"));
		reproducir.title = t("menu.play");
		pintar();
	}
	void conEspera(secciones, library.artistDetail(artistId)).then((d) => {
		detalle = d;
		sincronizarSuscripcion();
		etiquetas();
	}).catch(() => {
		secciones.replaceChildren(vacio(t("artist.empty")));
	});
	const dejarIdioma = alCambiarIdioma(etiquetas);
	return { destroy() {
		dejarIdioma();
		dejarBiblioteca?.();
		clearTimeout(recargaTimer);
		for (const quitar of desmontadores.splice(0)) quitar();
		el.remove();
	} };
}

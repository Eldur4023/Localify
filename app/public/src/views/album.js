import { library, player, playlists } from "../ipc/client.js";
import { pedirTexto } from "../ui/dialogo.js";
import { alCambiarIdioma, t } from "../i18n/index.js";
import { ponerPortada } from "../ui/cards.js";
import { mountTrackList } from "../ui/track-list.js";
import { botonIcono, icono } from "../ui/icons.js";
import { duracion } from "../shell/player.js";
/**
* Tipos de álbum que sabemos traducir.
*
* El proveedor los manda en inglés y podría añadir alguno. Comprobarlos contra
* este conjunto es lo que permite mostrar el valor crudo en ese caso, en vez
* de un `[album.type.ep]` delante del usuario.
*/
const TIPOS = new Set([
	"album",
	"single",
	"compilation"
]);
export function mountAlbumView(contenedor, ruta) {
	const albumId = ruta.params[0] ?? "";
	const el = document.createElement("section");
	el.className = "vista vista--lista";
	const cabecera = document.createElement("header");
	cabecera.className = "vista__header vista__header--ficha";
	const arte = document.createElement("div");
	arte.className = "ficha__arte";
	arte.append(icono("music", 48));
	const texto = document.createElement("div");
	texto.className = "ficha__texto";
	const tipo = document.createElement("span");
	tipo.className = "ficha__tipo";
	const titulo = document.createElement("h2");
	titulo.className = "vista__title";
	const meta = document.createElement("p");
	meta.className = "ficha__meta";
	texto.append(tipo, titulo, meta);
	const acciones = document.createElement("div");
	acciones.className = "ficha__acciones";
	const reproducir = botonIcono("play", "", () => void reproducirTodo(), { tamano: 22 });
	reproducir.classList.add("bicono--primario");
	const guardar = botonIcono("plus", "", () => void guardarComoPlaylist(), { tamano: 20 });
	acciones.append(reproducir, guardar);
	texto.append(acciones);
	cabecera.append(arte, texto);
	const cuerpo = document.createElement("div");
	cuerpo.className = "vista__body";
	el.append(cabecera, cuerpo);
	contenedor.replaceChildren(el);
	let detalle = null;
	let entregado = false;
	const trackList = mountTrackList(cuerpo, {
		numerar: true,
		contexto: () => ({
			kind: "album",
			id: albumId
		}),
		reiniciarOrigen() {
			entregado = false;
			detalle = null;
		},
		async cargar() {
			if (entregado) return {
				items: [],
				hasMore: false
			};
			detalle = await library.albumDetail(albumId);
			entregado = true;
			pintarCabecera();
			return {
				items: detalle.tracks,
				hasMore: false
			};
		}
	});
	function pintarCabecera() {
		if (!detalle) return;
		titulo.textContent = detalle.title;
		// `pintarCabecera` corre también al cambiar de idioma: sin esta guarda se
		// apilarían imágenes iguales una encima de otra.
		if (!arte.querySelector(".portada")) ponerPortada(arte, albumId);
		tipo.textContent = TIPOS.has(detalle.albumType) ? t(`album.type.${detalle.albumType}`) : detalle.albumType;
		const artistas = detalle.artists.map((a) => a.name).join(", ");
		const anyo = detalle.releaseDate?.slice(0, 4) ?? "";
		const partes = [
			artistas,
			anyo,
			t("library.count", { count: detalle.tracks.length }),
			duracion(detalle.totalDurationMs)
		].filter((p) => p.length > 0);
		meta.textContent = partes.join(" · ");
	}
	async function reproducirTodo() {
		const primera = detalle?.tracks[0];
		if (!primera) return;
		await player.playTrack(primera.id, {
			kind: "album",
			id: albumId
		});
	}
	/**
	* Copia el álbum entero a una playlist nueva.
	*
	* Es una **copia, no un enlace**: a partir de aquí son cosas distintas, y
	* quitar una canción de la playlist no la quita del álbum. Un álbum es un
	* hecho —esas son sus canciones— y una playlist es una decisión.
	*
	* El nombre viene propuesto con el del álbum, ya seleccionado, porque casi
	* siempre vale y quien quiera otro solo tiene que escribir encima.
	*/
	async function guardarComoPlaylist() {
		const pistas = detalle?.tracks ?? [];
		if (pistas.length === 0) return;
		const nombre = await pedirTexto({
			titulo: t("album.save_as_playlist"),
			etiqueta: t("playlist.name"),
			valor: detalle?.title ?? "",
			aceptar: t("playlist.create"),
			maxLength: 100
		});
		if (!nombre) return;
		// Se añaden en una sola llamada para conservar el orden del disco: con una
		// por canción, dos respuestas que se cruzaran bastarían para descolocarlo.
		const creada = await playlists.create(nombre);
		await playlists.addTracks(creada.id, pistas.map((p) => p.id), null);
		globalThis.location.hash = `#/playlist/${creada.id}`;
	}
	function etiquetas() {
		reproducir.setAttribute("aria-label", t("menu.play"));
		reproducir.title = t("menu.play");
		guardar.setAttribute("aria-label", t("album.save_as_playlist"));
		guardar.title = t("album.save_as_playlist");
		pintarCabecera();
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

import { lyrics as api, player } from "../ipc/client.js";
import { alRecibir } from "../ipc/events.js";
import { alCambiarIdioma, t } from "../i18n/index.js";
import { botonIcono, icono } from "../ui/icons.js";
import { ponerPortadaDePista } from "../ui/cards.js";
/** Cada cuánto se comprueba qué línea toca. El mismo ritmo que la barra. */
const SONDEO_MS = 250;
export function mountNowPlaying(contenedor) {
	const el = document.createElement("section");
	el.className = "ampliada";
	el.hidden = true;
	const cerrarBoton = botonIcono("chevron-down", "", () => cerrar(), { tamano: 22 });
	cerrarBoton.classList.add("ampliada__cerrar");
	const arte = document.createElement("div");
	arte.className = "ampliada__arte";
	arte.append(icono("music", 96));
	// Canción cuya portada grande está puesta.
	//
	// Se guarda la canción y no su álbum. Con el álbum, todo lo que no tiene
	// disco —una búsqueda, una playlist importada— compartía la clave `null` y
	// esta vista no pintaba nada; y aunque la pintara, pediría una imagen
	// distinta de la que sale en la lista y en la barra del reproductor.
	let pistaPintada = null;
	const titulo = document.createElement("h1");
	titulo.className = "ampliada__titulo";
	const artista = document.createElement("p");
	artista.className = "ampliada__artista";
	const album = document.createElement("p");
	album.className = "ampliada__album";
	// El aviso de "sin letra" va con la canción, no donde iría la letra: ese
	// panel desaparece cuando no hay nada que poner en él, y un mensaje dentro de
	// algo que no se muestra no lo lee nadie.
	const sinLetra = document.createElement("p");
	sinLetra.className = "ampliada__sin-letra";
	sinLetra.hidden = true;
	const izquierda = document.createElement("div");
	izquierda.className = "ampliada__izquierda";
	izquierda.append(arte, titulo, artista, album, sinLetra);
	const letra = document.createElement("div");
	letra.className = "ampliada__letra";
	el.append(cerrarBoton, izquierda, letra);
	contenedor.append(el);
	let pistaActual = null;
	let actual = null;
	let lineas = [];
	let nodos = [];
	let activa = -1;
	let temporizador = null;
	function pintarCabecera(estado) {
		titulo.textContent = estado.track?.title ?? t("player.nothing");
		if (estado.track?.id !== pistaPintada) {
			pistaPintada = estado.track?.id ?? null;
			arte.querySelector(".portada")?.remove();
			if (pistaPintada) ponerPortadaDePista(arte, pistaPintada);
		}
		artista.textContent = estado.track?.artistDisplay ?? "";
		album.textContent = estado.track?.albumTitle ?? "";
	}
	function pintarLetra() {
		letra.replaceChildren();
		nodos = [];
		activa = -1;
		if (!actual) {
			sinLetra.textContent = t("lyrics.none");
			sinLetra.hidden = false;
			el.classList.add("ampliada--sin-letra");
			return;
		}
		sinLetra.hidden = true;
		el.classList.remove("ampliada--sin-letra");
		if (actual.synced && actual.synced.length > 0) {
			lineas = [...actual.synced];
			for (const linea of lineas) {
				const p = document.createElement("p");
				p.className = "ampliada__linea";
				// Una línea vacía en un LRC es un silencio instrumental. Se pinta
				// igual, con un espacio duro, para que el desplazamiento siga siendo
				// regular en vez de dar un salto.
				p.textContent = linea.text.length > 0 ? linea.text : "\xA0";
				letra.append(p);
				nodos.push(p);
			}
			return;
		}
		lineas = [];
		const p = document.createElement("p");
		p.className = "ampliada__plana";
		p.textContent = actual.plain ?? t("lyrics.none");
		letra.append(p);
	}
	/** Índice de la línea que suena, o -1 antes de la primera. */
	function lineaEn(posicionMs) {
		// Se parte de la anterior porque el tiempo casi siempre avanza; solo se
		// retrocede cuando el usuario ha saltado hacia atrás.
		let i = activa;
		if (i >= lineas.length) i = lineas.length - 1;
		while (i >= 0 && (lineas[i]?.atMs ?? 0) > posicionMs) i -= 1;
		while (i + 1 < lineas.length && (lineas[i + 1]?.atMs ?? 0) <= posicionMs) i += 1;
		return i;
	}
	function resaltar(indice) {
		if (indice === activa) return;
		nodos[activa]?.classList.remove("is-activa");
		activa = indice;
		const nodo = nodos[indice];
		if (!nodo) return;
		nodo.classList.add("is-activa");
		// Centrar en el contenedor de la letra, sin tocar ningún otro scroll.
		const destino = nodo.offsetTop - letra.clientHeight / 2 + nodo.offsetHeight / 2;
		letra.scrollTo({
			top: Math.max(0, destino),
			behavior: "smooth"
		});
	}
	async function cargarLetra(trackId) {
		pistaActual = trackId;
		if (!trackId) {
			actual = null;
			pintarLetra();
			return;
		}
		try {
			actual = await api.get(trackId);
		} catch {
			// Sin letra no es un error que merezca una alerta: se muestra el estado
			// vacío y se sigue.
			actual = null;
		}
		// La canción puede haber cambiado mientras se pedía.
		if (pistaActual !== trackId) return;
		pintarLetra();
	}
	async function sincronizar() {
		const estado = await player.getState();
		pintarCabecera(estado);
		if (estado.track?.id !== pistaActual) await cargarLetra(estado.track?.id ?? null);
	}
	function sondear() {
		if (lineas.length === 0) return;
		void player.position().then((p) => resaltar(lineaEn(p.positionMs))).catch(() => {
			// Un sondeo perdido no rompe nada: el siguiente lo corrige.
		});
	}
	const dejarEventos = alRecibir((evento) => {
		if (el.hidden) return;
		if (evento.type === "trackChanged" || evento.type === "playStatusChanged") {
			void sincronizar();
		}
	});
	function abrir() {
		el.hidden = false;
		void sincronizar();
		// El sondeo solo corre con la vista abierta: mantenerlo de fondo sería un
		// comando cuatro veces por segundo para mover algo que nadie ve.
		temporizador ??= globalThis.setInterval(sondear, SONDEO_MS);
	}
	function cerrar() {
		el.hidden = true;
		if (temporizador !== null) {
			globalThis.clearInterval(temporizador);
			temporizador = null;
		}
	}
	const alTeclado = (e) => {
		if (e.key === "Escape" && !el.hidden) cerrar();
	};
	globalThis.addEventListener("keydown", alTeclado);
	function etiquetas() {
		el.setAttribute("aria-label", t("player.expand"));
		cerrarBoton.setAttribute("aria-label", t("player.collapse"));
		cerrarBoton.title = t("player.collapse");
		if (!actual) pintarLetra();
	}
	etiquetas();
	const dejarIdioma = alCambiarIdioma(etiquetas);
	return {
		abrir,
		cerrar,
		alternar() {
			if (el.hidden) abrir();
			else cerrar();
		},
		abierta: () => !el.hidden,
		destroy() {
			cerrar();
			dejarIdioma();
			dejarEventos();
			globalThis.removeEventListener("keydown", alTeclado);
			el.remove();
		}
	};
}

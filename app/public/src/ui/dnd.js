/**
* Arrastrar y soltar.
*
* ## Requiere `dragDropEnabled: false` en `tauri.conf.json`
*
* Es lo contrario de lo que suena. Esa opción activa el manejo de arrastres
* **del sistema operativo** —soltar ficheros del explorador sobre la ventana—,
* y para hacerlo el host intercepta los eventos antes de que lleguen al
* WebView. Con ella puesta, arrastrar dentro de la página no dispara `drop`
* nunca: el HTML está bien, los manejadores están puestos, y no pasa nada.
*
* Localify no acepta ficheros sueltos, así que la opción va desactivada. Como
* `tauri.conf.json` no admite comentarios, la razón vive aquí.
*
* ## El arrastre no puede ser el único camino
*
* Aunque funcione, arrastrar exige mantener el botón pulsado y cruzar la
* ventana. Toda acción que se pueda hacer arrastrando tiene que poder hacerse
* también desde el menú contextual: ver `opciones-pista.js`.
*
* ## Por qué un tipo MIME propio
*
* `text/plain` funcionaría, pero entonces soltar texto arrastrado desde el
* navegador o desde un editor activaría las zonas de la aplicación. Con un tipo
* propio (`application/x-localify-tracks`), solo lo que sale de Localify puede
* soltarse en Localify.
*
* ## El truco de `dragover`
*
* Una zona solo acepta un soltado si su manejador de `dragover` llama a
* `preventDefault`. Es contraintuitivo —parece que se está cancelando algo— y
* es la causa número uno de que un `drop` no llegue nunca. Aquí está en un solo
* sitio para no tener que recordarlo en cada zona.
*
* ## Reordenar necesita saber el lado
*
* Al arrastrar dentro de una lista, importa si se suelta en la mitad de arriba
* o de abajo de una fila: determina si la pista va antes o después. Se calcula
* con el punto medio de la fila y se refleja con una línea, para que el usuario
* vea dónde va a caer antes de soltar.
*/
/** Tipo MIME de un arrastre de pistas. El contenido es JSON con sus IDs. */
export const TIPO_PISTAS = "application/x-localify-tracks";
/** Tipo MIME de un arrastre de entradas de playlist, para reordenar. */
const TIPO_ENTRADA = "application/x-localify-entry";
/** Tipo MIME de un arrastre de una playlist entera, para reordenarlas entre sí. */
export const TIPO_PLAYLIST = "application/x-localify-playlist";
/** Marca un elemento como origen de arrastre de pistas. */
export function arrastrable(el, ids) {
	el.draggable = true;
	const alEmpezar = (e) => {
		const lista = ids();
		if (lista.length === 0) return;
		e.dataTransfer?.setData(TIPO_PISTAS, JSON.stringify(lista));
		if (e.dataTransfer) e.dataTransfer.effectAllowed = "copyMove";
		el.classList.add("is-dragging");
	};
	const alTerminar = () => el.classList.remove("is-dragging");
	el.addEventListener("dragstart", alEmpezar);
	el.addEventListener("dragend", alTerminar);
	return () => {
		el.removeEventListener("dragstart", alEmpezar);
		el.removeEventListener("dragend", alTerminar);
		el.draggable = false;
	};
}
/**
* Marca un elemento como entrada reordenable de una playlist.
*
* `tipo` distingue qué se está arrastrando —una entrada dentro de una
* playlist, o una playlist entera en la barra lateral— para que una zona no
* reaccione a un arrastre que no es el suyo.
*/
export function reordenable(el, entryId, tipo = TIPO_ENTRADA) {
	el.draggable = true;
	const alEmpezar = (e) => {
		e.dataTransfer?.setData(tipo, entryId());
		if (e.dataTransfer) e.dataTransfer.effectAllowed = "move";
		el.classList.add("is-dragging");
	};
	const alTerminar = () => el.classList.remove("is-dragging");
	el.addEventListener("dragstart", alEmpezar);
	el.addEventListener("dragend", alTerminar);
	return () => {
		el.removeEventListener("dragstart", alEmpezar);
		el.removeEventListener("dragend", alTerminar);
		el.draggable = false;
	};
}
/**
* Convierte un elemento en zona de soltado para pistas.
*
* `manejar` recibe los identificadores. Si falla, la zona no se queda marcada:
* un error no debe dejar la interfaz con un resaltado permanente.
*/
export function zonaDeSoltado(el, tipo, manejar) {
	const alPasar = (e) => {
		if (!e.dataTransfer?.types.includes(tipo)) return;
		// Sin esto, `drop` no se dispara nunca.
		e.preventDefault();
		e.dataTransfer.dropEffect = "copy";
		el.classList.add("is-drop-target");
	};
	const alSalir = () => el.classList.remove("is-drop-target");
	const alSoltar = (e) => {
		const crudo = e.dataTransfer?.getData(tipo);
		el.classList.remove("is-drop-target");
		if (!crudo) return;
		e.preventDefault();
		let datos;
		try {
			const analizado = JSON.parse(crudo);
			datos = Array.isArray(analizado) ? analizado.map(String) : [crudo];
		} catch {
			// No era JSON: es un identificador suelto.
			datos = [crudo];
		}
		void manejar(datos);
	};
	el.addEventListener("dragover", alPasar);
	el.addEventListener("dragleave", alSalir);
	el.addEventListener("drop", alSoltar);
	return () => {
		el.removeEventListener("dragover", alPasar);
		el.removeEventListener("dragleave", alSalir);
		el.removeEventListener("drop", alSoltar);
		el.classList.remove("is-drop-target");
	};
}
/** Mitad de la fila sobre la que está el cursor. */
function ladoDeSoltado(fila, e) {
	const caja = fila.getBoundingClientRect();
	return e.clientY < caja.top + caja.height / 2 ? "antes" : "despues";
}
/** Primer ascendiente (o el propio nodo) que desplaza en vertical. */
function desplazableDe(nodo) {
	for (let n = nodo instanceof Element ? nodo : nodo?.parentElement; n; n = n.parentElement) {
		const oy = getComputedStyle(n).overflowY;
		if ((oy === "auto" || oy === "scroll" || oy === "overlay") && n.scrollHeight > n.clientHeight) return n;
	}
	return null;
}
/** Franja, en px, junto a cada borde de la lista donde el arrastre la desplaza. */
const BORDE_AUTOSCROLL = 64;
/** Velocidad máxima, en px por fotograma, cuando el cursor está justo en el borde (o fuera). */
const VELOCIDAD_AUTOSCROLL = 22;
/**
* Desplazamiento automático durante un arrastre: mientras el cursor esté cerca del borde de
* arriba o de abajo de la lista (o más allá), la lista se mueve hacia ese lado, más deprisa cuanto
* más cerca del borde, para poder llegar a cualquier posición sin soltar. `dragover` sigue
* disparándose con el ratón quieto, y cada aviso rearma un vigilante: si dejan de llegar, para.
*/
function autodesplazamiento() {
	let lista = null;
	let y = 0;
	let cuadro = 0;
	let ultimoAviso = 0;
	const paso = () => {
		cuadro = 0;
		if (!lista?.isConnected || performance.now() - ultimoAviso > 300) {
			lista = null;
			return;
		}
		const caja = lista.getBoundingClientRect();
		const dentroArriba = y - caja.top;
		const dentroAbajo = caja.bottom - y;
		let v = 0;
		if (dentroArriba < BORDE_AUTOSCROLL) v = -Math.min(1, (BORDE_AUTOSCROLL - dentroArriba) / BORDE_AUTOSCROLL);
		else if (dentroAbajo < BORDE_AUTOSCROLL) v = Math.min(1, (BORDE_AUTOSCROLL - dentroAbajo) / BORDE_AUTOSCROLL);
		if (v !== 0) lista.scrollTop += Math.round(v * VELOCIDAD_AUTOSCROLL) || Math.sign(v);
		cuadro = requestAnimationFrame(paso);
	};
	return {
		/** Llamar en cada `dragover`. */
		avisar(e) {
			const encontrada = desplazableDe(e.target);
			if (encontrada) lista = encontrada;
			y = e.clientY;
			ultimoAviso = performance.now();
			if (lista && !cuadro) cuadro = requestAnimationFrame(paso);
		},
		/** La lista que se está desplazando (para conservar su posición al soltar). */
		get lista() {
			return lista;
		},
		parar() {
			lista = null;
			if (cuadro) cancelAnimationFrame(cuadro);
			cuadro = 0;
		}
	};
}
/**
* Zona de reordenación sobre una lista.
*
* `filaDe` localiza la fila bajo el cursor; `indiceDe` dice qué posición ocupa.
* `manejar` recibe la entrada arrastrada y el índice destino ya resuelto.
*/
export function zonaDeReordenacion(contenedor, filaDe, indiceDe, manejar, tipo = TIPO_ENTRADA) {
	let marcada = null;
	const desplazamiento = autodesplazamiento();
	const limpiar = () => {
		marcada?.classList.remove("is-drop-before", "is-drop-after");
		marcada = null;
	};
	const alPasar = (e) => {
		if (!e.dataTransfer?.types.includes(tipo)) return;
		e.preventDefault();
		e.dataTransfer.dropEffect = "move";
		desplazamiento.avisar(e);
		const fila = filaDe(e.target);
		if (!fila) return;
		if (marcada !== fila) limpiar();
		marcada = fila;
		const lado = ladoDeSoltado(fila, e);
		fila.classList.toggle("is-drop-before", lado === "antes");
		fila.classList.toggle("is-drop-after", lado === "despues");
	};
	const alSoltar = (e) => {
		const entryId = e.dataTransfer?.getData(tipo);
		const fila = filaDe(e.target);
		const lado = fila ? ladoDeSoltado(fila, e) : "despues";
		// La lista que se estaba desplazando: se anota dónde estaba para dejarla ahí
		const lista = desplazamiento.lista ?? desplazableDe(e.target);
		const posicion = lista?.scrollTop ?? 0;
		desplazamiento.parar();
		limpiar();
		if (!entryId || !fila) return;
		e.preventDefault();
		const base = indiceDe(fila);
		const hecho = Promise.resolve(manejar(entryId, lado === "antes" ? base : base + 1));
		// Reordenar repinta (o recarga) la lista y eso la devolvía arriba del todo: se repone la
		// posición cuando el repintado ha acabado, y una vez más tras el siguiente fotograma
		const reponer = () => {
			if (lista?.isConnected && Math.abs(lista.scrollTop - posicion) > 1) lista.scrollTop = posicion;
		};
		requestAnimationFrame(reponer);
		void hecho.then(() => requestAnimationFrame(reponer), () => requestAnimationFrame(reponer));
	};
	const alTerminarArrastre = () => desplazamiento.parar();
	contenedor.addEventListener("dragover", alPasar);
	contenedor.addEventListener("dragleave", limpiar);
	contenedor.addEventListener("drop", alSoltar);
	document.addEventListener("dragend", alTerminarArrastre);
	return () => {
		contenedor.removeEventListener("dragover", alPasar);
		contenedor.removeEventListener("dragleave", limpiar);
		contenedor.removeEventListener("drop", alSoltar);
		document.removeEventListener("dragend", alTerminarArrastre);
		desplazamiento.parar();
		limpiar();
	};
}

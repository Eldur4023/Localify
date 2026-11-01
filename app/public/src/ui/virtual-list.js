import { nodo } from "./dom.js";
/** Margen por defecto, en filas. */
const OVERSCAN_POR_DEFECTO = 6;
/**
* A cuántas filas del final se pide la página siguiente.
*
* Con un margen holgado, la carga ocurre mientras todavía hay contenido que
* mirar y el usuario no llega a ver el final de la lista.
*/
const UMBRAL_CARGA = 20;
export function mountVirtualList(container, options) {
	const overscan = options.overscan ?? OVERSCAN_POR_DEFECTO;
	const el = nodo("div", "vlist");
	// El espaciador solo existe para dar al contenedor la altura total y que la
	// barra de scroll sea la de verdad. Está vacío.
	const spacer = nodo("div", "vlist__spacer");
	spacer.setAttribute("aria-hidden", "true");
	const viewport = nodo("div", "vlist__viewport");
	el.append(spacer, viewport);
	container.replaceChildren(el);
	const items = [];
	const pool = [];
	/** Limpiadores devueltos por `createRow`, uno por fila del grupo. */
	const limpiadores = [];
	/** Qué ítem/índice pintó cada hueco: si coincide, se salta `renderRow`. */
	const pintadoItem = [];
	const pintadoIndice = [];
	function invalidar() {
		pintadoItem.length = 0;
		pintadoIndice.length = 0;
	}
	let agotada = false;
	let cargando = false;
	let destruida = false;
	/**
	* Se incrementa en cada `reset()`, para poder descartar una carga que
	* arrancó antes del reset y responde después.
	*
	* Sin esto, `cargando` por sí sola no basta: si `reset()` llega mientras un
	* `cargar()` anterior sigue esperando su `loadMore()`, esa respuesta vieja
	* acaba llenando la lista que el reset acababa de vaciar, con datos de la
	* consulta anterior (p. ej. el orden de antes de cambiar el criterio).
	*/
	let generacion = 0;
	// Qué se avisó la última vez. Se guardan los dos extremos, no solo el
	// primero: al llegar la primera página el índice inicial sigue siendo cero,
	// y comparando solo ese valor el aviso nunca llegaría. Quien lo usa para
	// precargar la disponibilidad se quedaría sin la primera pantalla entera.
	let avisadoDesde = -1;
	let avisadoHasta = -1;
	/** Cuántos nodos hacen falta para cubrir la vista más el margen. */
	function tamanoGrupo() {
		const visibles = Math.ceil(el.clientHeight / options.rowHeight);
		return visibles + overscan * 2;
	}
	/** Ajusta el grupo de nodos al tamaño necesario. */
	function ajustarGrupo(necesarios) {
		if (pool.length !== necesarios) invalidar();
		while (pool.length < necesarios) {
			const { el: fila, limpiar } = options.createRow();
			limpiadores.push(limpiar);
			fila.classList.add("vlist__row");
			fila.style.height = `${options.rowHeight}px`;
			viewport.append(fila);
			pool.push(fila);
		}
		// Al encoger la ventana sobran nodos. Se quitan de verdad: dejarlos
		// ocultos gastaría memoria sin dar nada a cambio.
		while (pool.length > necesarios) {
			pool.pop()?.remove();
			limpiadores.pop()?.();
		}
	}
	/** Última fila pintada, para no repetir el trabajo si el scroll no cruzó
	 * ninguna fila (en el móvil los eventos llegan a decenas por segundo). */
	let ultimaPrimera = -1;
	let ultimoTamano = -1;
	/** Coloca y rellena los nodos según la posición del scroll. */
	function pintar(forzar = false) {
		if (destruida) return;
		const primera = Math.max(0, Math.floor(el.scrollTop / options.rowHeight) - overscan);
		const necesarios = tamanoGrupo();
		if (!forzar && primera === ultimaPrimera && necesarios === ultimoTamano) return;
		ultimaPrimera = primera;
		ultimoTamano = necesarios;
		ajustarGrupo(necesarios);
		for (let i = 0; i < pool.length; i += 1) {
			const indice = primera + i;
			// Anillo: un índice cae siempre en el mismo hueco, así cruzar una fila
			// solo repinta la que entra, no las ~30 del grupo.
			const hueco = indice % pool.length;
			const fila = pool[hueco];
			if (!fila) continue;
			const item = items[indice];
			if (item === undefined) {
				// Más allá de lo cargado: se esconde en vez de eliminarse, para que el
				// grupo siga teniendo el mismo tamaño.
				fila.hidden = true;
				pintadoItem[hueco] = undefined;
				continue;
			}
			if (pintadoItem[hueco] === item && pintadoIndice[hueco] === indice) continue;
			pintadoItem[hueco] = item;
			pintadoIndice[hueco] = indice;
			fila.hidden = false;
			fila.style.transform = `translateY(${indice * options.rowHeight}px)`;
			options.renderRow(fila, item, indice);
		}
		const hasta = Math.min(items.length, primera + pool.length);
		if (primera !== avisadoDesde || hasta !== avisadoHasta) {
			avisadoDesde = primera;
			avisadoHasta = hasta;
			options.onVisibleChange?.(items.slice(primera, hasta), primera, hasta);
		}
		// Se pide más cuando el final se acerca, no cuando se alcanza: así la
		// página siguiente llega antes de que haya nada que esperar.
		if (!agotada && primera + pool.length + UMBRAL_CARGA >= items.length) {
			void cargar();
		}
	}
	/**
	* Posición a recuperar tras un `reset(true)`: la lista se vacía, así que el navegador la
	* manda arriba; se vuelve a la posición en cuanto hay filas suficientes para alcanzarla.
	*/
	let posicionPendiente = null;
	function recuperarPosicion() {
		if (posicionPendiente === null) return;
		if (agotada || items.length * options.rowHeight >= posicionPendiente + el.clientHeight) {
			el.scrollTop = posicionPendiente;
			posicionPendiente = null;
			pintar(true);
		} else {
			void cargar();
		}
	}
	async function cargar() {
		if (cargando || agotada || destruida) return;
		cargando = true;
		const miGeneracion = generacion;
		try {
			const pagina = await options.loadMore();
			// Un reset() de por medio invalida esta respuesta: pertenece a la
			// consulta anterior, y la lista que llenaría ya no es la que hay.
			if (destruida || miGeneracion !== generacion) return;
			if (!pagina.hasMore) agotada = true;
			if (pagina.items.length > 0) {
				items.push(...pagina.items);
				spacer.style.height = `${items.length * options.rowHeight}px`;
				pintar(true);
			}
		} catch {
			posicionPendiente = null;
		} finally {
			// Si ya hubo un reset(), `cargando` pertenece a la carga *nueva* que ese
			// reset lanzó: tocarlo aquí la dejaría creyéndose libre a mitad de vuelo.
			if (miGeneracion === generacion) {
				cargando = false;
				if (!destruida) recuperarPosicion();
			}
		}
	}
	// `passive` porque nunca se llama a `preventDefault`: sin él, el navegador
	// tiene que esperar a que el manejador termine antes de desplazar. El
	// repintado se agrupa en un frame: en el móvil los eventos de scroll llegan
	// más rápido que los frames y se repetía el mismo trabajo varias veces.
	let cuadro = 0;
	const alDesplazar = () => {
		if (cuadro) return;
		cuadro = requestAnimationFrame(() => {
			cuadro = 0;
			pintar();
		});
	};
	el.addEventListener("scroll", alDesplazar, { passive: true });
	// Al cambiar de tamaño la ventana cambia cuántas filas caben.
	const observador = new ResizeObserver(() => pintar(true));
	observador.observe(el);
	void cargar();
	return {
		el,
		get items() {
			return items;
		},
		refresh: () => {
			invalidar();
			pintar(true);
		},
		move(desde, hasta) {
			if (desde < 0 || desde >= items.length) return;
			const [movido] = items.splice(desde, 1);
			if (movido === undefined) return;
			// Al sacar el elemento, todo lo que venía detrás baja una posición: un
			// destino posterior al origen hay que corregirlo o el elemento acaba una
			// fila más abajo de donde se soltó.
			const destino = hasta > desde ? hasta - 1 : hasta;
			items.splice(Math.max(0, Math.min(items.length, destino)), 0, movido);
			invalidar();
			pintar(true);
		},
		reset(conservarPosicion = false) {
			posicionPendiente = conservarPosicion && el.scrollTop > 0 ? el.scrollTop : null;
			generacion += 1;
			items.length = 0;
			agotada = false;
			avisadoDesde = -1;
			avisadoHasta = -1;
			ultimaPrimera = -1;
			ultimoTamano = -1;
			invalidar();
			spacer.style.height = "0px";
			el.scrollTop = 0;
			// Descarta cualquier carga en curso de la generación anterior: sin esto,
			// el guardián de más arriba vería `cargando` en `true` y esta llamada no
			// haría nada.
			cargando = false;
			for (const fila of pool) fila.hidden = true;
			void cargar();
		},
		nodeCount() {
			return pool.length;
		},
		destroy() {
			destruida = true;
			if (cuadro) cancelAnimationFrame(cuadro);
			observador.disconnect();
			el.removeEventListener("scroll", alDesplazar);
			el.remove();
			for (const limpiar of limpiadores.splice(0)) limpiar?.();
		}
	};
}

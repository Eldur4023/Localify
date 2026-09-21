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
	const el = document.createElement("div");
	el.className = "vlist";
	// El espaciador solo existe para dar al contenedor la altura total y que la
	// barra de scroll sea la de verdad. Está vacío.
	const spacer = document.createElement("div");
	spacer.className = "vlist__spacer";
	spacer.setAttribute("aria-hidden", "true");
	const viewport = document.createElement("div");
	viewport.className = "vlist__viewport";
	el.append(spacer, viewport);
	container.replaceChildren(el);
	const items = [];
	const pool = [];
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
	function ajustarGrupo() {
		const necesarios = tamanoGrupo();
		while (pool.length < necesarios) {
			const fila = options.createRow();
			fila.classList.add("vlist__row");
			fila.style.height = `${options.rowHeight}px`;
			viewport.append(fila);
			pool.push(fila);
		}
		// Al encoger la ventana sobran nodos. Se quitan de verdad: dejarlos
		// ocultos gastaría memoria sin dar nada a cambio.
		while (pool.length > necesarios) {
			pool.pop()?.remove();
		}
	}
	/** Coloca y rellena los nodos según la posición del scroll. */
	function pintar() {
		if (destruida) return;
		const primera = Math.max(0, Math.floor(el.scrollTop / options.rowHeight) - overscan);
		ajustarGrupo();
		for (let i = 0; i < pool.length; i += 1) {
			const indice = primera + i;
			const fila = pool[i];
			if (!fila) continue;
			const item = items[indice];
			if (item === undefined) {
				// Más allá de lo cargado: se esconde en vez de eliminarse, para que el
				// grupo siga teniendo el mismo tamaño.
				fila.hidden = true;
				continue;
			}
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
				pintar();
			}
		} catch {} finally {
			// Si ya hubo un reset(), `cargando` pertenece a la carga *nueva* que ese
			// reset lanzó: tocarlo aquí la dejaría creyéndose libre a mitad de vuelo.
			if (miGeneracion === generacion) cargando = false;
		}
	}
	// `passive` porque nunca se llama a `preventDefault`: sin él, el navegador
	// tiene que esperar a que el manejador termine antes de desplazar.
	const alDesplazar = () => pintar();
	el.addEventListener("scroll", alDesplazar, { passive: true });
	// Al cambiar de tamaño la ventana cambia cuántas filas caben.
	const observador = new ResizeObserver(() => pintar());
	observador.observe(el);
	void cargar();
	return {
		el,
		get items() {
			return items;
		},
		refresh: pintar,
		move(desde, hasta) {
			if (desde < 0 || desde >= items.length) return;
			const [movido] = items.splice(desde, 1);
			if (movido === undefined) return;
			// Al sacar el elemento, todo lo que venía detrás baja una posición: un
			// destino posterior al origen hay que corregirlo o el elemento acaba una
			// fila más abajo de donde se soltó.
			const destino = hasta > desde ? hasta - 1 : hasta;
			items.splice(Math.max(0, Math.min(items.length, destino)), 0, movido);
			pintar();
		},
		reset() {
			generacion += 1;
			items.length = 0;
			agotada = false;
			avisadoDesde = -1;
			avisadoHasta = -1;
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
			observador.disconnect();
			el.removeEventListener("scroll", alDesplazar);
			el.remove();
		}
	};
}

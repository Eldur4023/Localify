/** Analiza un fragmento como `#/album/abc?x=1`. */
export function analizar(fragmento) {
	const limpio = fragmento.replace(/^#\/?/, "");
	const [camino = "", consulta = ""] = limpio.split("?", 2);
	const partes = camino.split("/").filter((p) => p.length > 0).map(decodeURIComponent);
	return {
		nombre: partes[0] ?? "home",
		params: partes.slice(1),
		query: new URLSearchParams(consulta)
	};
}
export function crearRouter(contenedor, rutas, porDefecto = "home") {
	// Pila propia: `indice` apunta a la entrada visible. Navegar recorta lo que
	// hubiera delante, igual que en un navegador.
	const pila = [];
	let indice = -1;
	let montada = null;
	let navegandoNosotros = false;
	const oyentes = new Set();
	function rutaActual() {
		return analizar(globalThis.location.hash);
	}
	function montar() {
		const ruta = rutaActual();
		const montador = rutas[ruta.nombre] ?? rutas[porDefecto];
		if (!montador) return;
		// Desmontar antes de montar: si no, la vista saliente y la entrante
		// coexisten un instante y las dos escuchan los mismos eventos.
		montada?.destroy();
		montada = montador(contenedor, ruta);
		for (const oyente of oyentes) oyente(ruta);
	}
	function alCambiarFragmento() {
		if (!navegandoNosotros) {
			// El usuario editó la URL o usó los atajos del navegador: se trata como
			// una navegación nueva.
			pila.splice(indice + 1);
			pila.push(globalThis.location.hash);
			indice = pila.length - 1;
		}
		navegandoNosotros = false;
		montar();
	}
	globalThis.addEventListener("hashchange", alCambiarFragmento);
	function aplicar(destino) {
		const fragmento = destino.startsWith("#") ? destino : `#/${destino}`;
		if (globalThis.location.hash === fragmento) {
			// Mismo destino: no hay evento de cambio, así que se remonta a mano.
			// Pulsar "Inicio" estando en Inicio debe refrescar, no quedarse quieto.
			montar();
			return;
		}
		navegandoNosotros = true;
		globalThis.location.hash = fragmento;
	}
	const router = {
		ir(destino) {
			pila.splice(indice + 1);
			pila.push(destino.startsWith("#") ? destino : `#/${destino}`);
			indice = pila.length - 1;
			aplicar(destino);
		},
		reemplazar(destino) {
			const fragmento = destino.startsWith("#") ? destino : `#/${destino}`;
			if (indice >= 0) pila[indice] = fragmento;
			else {
				pila.push(fragmento);
				indice = 0;
			}
			aplicar(destino);
		},
		atras() {
			if (indice <= 0) return;
			indice -= 1;
			aplicar(pila[indice] ?? `#/${porDefecto}`);
		},
		adelante() {
			if (indice >= pila.length - 1) return;
			indice += 1;
			aplicar(pila[indice] ?? `#/${porDefecto}`);
		},
		puedeIrAtras: () => indice > 0,
		puedeIrAdelante: () => indice < pila.length - 1,
		actual: rutaActual,
		alNavegar(oyente) {
			oyentes.add(oyente);
			return () => oyentes.delete(oyente);
		},
		destroy() {
			globalThis.removeEventListener("hashchange", alCambiarFragmento);
			montada?.destroy();
			montada = null;
			oyentes.clear();
		}
	};
	// Arranque: si no hay fragmento, se pone el de por defecto sin dejar una
	// entrada vacía en el historial detrás.
	if (!globalThis.location.hash || globalThis.location.hash === "#") {
		router.reemplazar(porDefecto);
	} else {
		pila.push(globalThis.location.hash);
		indice = 0;
		montar();
	}
	return router;
}

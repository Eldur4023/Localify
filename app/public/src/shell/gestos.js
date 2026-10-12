/**
* Gestos del móvil (≤700 px). Solo cuenta la dirección, no dónde empieza:
*
* - Deslizar a la derecha: abre el cajón de navegación (o cierra la vista de la canción).
* - Deslizar a la izquierda: abre la vista de la canción (o cierra el cajón).
*
* Los sliders y las filas deslizables (carruseles) no disparan nada: ahí el deslizado es suyo.
*/
export function mountGestos(app, ampliada) {
	const movil = matchMedia("(max-width: 700px)");
	const cajon = () => app.classList.contains("is-drawer-abierto");
	const cerrarCajon = () => app.classList.remove("is-drawer-abierto");

	// tocar el fondo oscuro o elegir un destino cierra el cajón
	const alPulsar = (e) => {
		if (e.target === app || e.target.closest?.(".sidebar a")) cerrarCajon();
	};

	let toque = null;
	const alTocar = (e) => {
		const t = e.touches[0];
		toque = movil.matches && e.touches.length === 1 && !e.target.closest?.("input[type=range], .carrusel__cuerpo, .perfil__rejilla")
			? { x: t.clientX, y: t.clientY } : null;
	};
	const alMover = (e) => {
		if (!toque) return;
		const t = e.touches[0];
		const dx = t.clientX - toque.x;
		const dy = t.clientY - toque.y;
		if (Math.abs(dx) < 40 || Math.abs(dx) < 2 * Math.abs(dy)) return;
		// tras actuar, el mismo dedo solo vuelve a actuar al invertir el sentido
		if (Math.sign(dx) === toque.dir) { toque.x = t.clientX; toque.y = t.clientY; return; }
		if (dx > 0) {
			if (ampliada.abierta()) ampliada.cerrar();
			else if (!cajon()) app.classList.add("is-drawer-abierto");
		} else if (cajon()) {
			cerrarCajon();
		} else if (!ampliada.abierta()) {
			ampliada.abrir();
		}
		toque = { x: t.clientX, y: t.clientY, dir: Math.sign(dx) };
	};
	app.addEventListener("click", alPulsar);
	document.addEventListener("touchstart", alTocar, { passive: true });
	document.addEventListener("touchmove", alMover, { passive: true });
	return () => {
		app.removeEventListener("click", alPulsar);
		document.removeEventListener("touchstart", alTocar);
		document.removeEventListener("touchmove", alMover);
	};
}

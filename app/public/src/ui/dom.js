/** Crea un elemento con clase y texto opcionales. */
export function nodo(etiqueta, clase, texto) {
	const n = document.createElement(etiqueta);
	if (clase) n.className = clase;
	if (texto !== undefined) n.textContent = texto;
	return n;
}

/**
* Recorte de imagen con zoom, para la foto de perfil.
*
* Una ventana con un marco cuadrado (el círculo del avatar se ve como máscara): la imagen se
* arrastra con el ratón o el dedo, se acerca con la rueda, el deslizador o pellizcando, y no
* se puede sacar del marco —siempre lo cubre entero—. Al aceptar se dibuja la zona visible en
* un lienzo cuadrado y se devuelve como JPEG, que es lo que guarda el servidor.
*/
import { t } from "../i18n/index.js";
import { nodo } from "./dom.js";
/** Zoom máximo sobre el mínimo (el que cubre el marco). */
const ZOOM_MAXIMO = 5;
/**
* Abre el recortador con `archivo`. Resuelve al JPEG recortado (`Blob`), o `null` si se cancela
* o el fichero no es una imagen que el navegador pueda abrir.
*/
export function recortarImagen(archivo, salida = 512) {
	return new Promise((resolver) => {
		const url = URL.createObjectURL(archivo);
		const img = new Image();
		img.alt = "";
		img.draggable = false;
		img.addEventListener("error", () => {
			URL.revokeObjectURL(url);
			resolver(null);
		});
		img.addEventListener("load", () => abrir(img, url, salida, resolver));
		img.src = url;
	});
}
function abrir(img, url, salida, resolver) {
	const dlg = nodo("dialog", "dialogo recorte");
	const form = nodo("form");
	form.method = "dialog";
	const marco = nodo("div", "recorte__marco");
	img.classList.add("recorte__img");
	marco.append(img, nodo("div", "recorte__mascara"));
	const zoom = nodo("input", "recorte__zoom");
	zoom.type = "range";
	zoom.min = "1";
	zoom.max = String(ZOOM_MAXIMO);
	zoom.step = "0.01";
	zoom.value = "1";
	zoom.setAttribute("aria-label", t("social.crop_zoom"));
	const acciones = nodo("div", "dialogo__acciones");
	const cancelar = nodo("button", "boton boton--sutil", t("common.cancel"));
	cancelar.type = "button";
	const guardar = nodo("button", "boton", t("common.save"));
	guardar.type = "button";
	acciones.append(cancelar, guardar);
	form.append(nodo("h2", "dialogo__titulo", t("social.crop_title")), marco, nodo("label", "recorte__fila", t("social.crop_zoom")), acciones);
	form.querySelector(".recorte__fila").append(zoom);
	dlg.append(form);
	document.body.append(dlg);
	let resultado = null;
	// Estado del recorte: lado del marco (px de pantalla), escala (px de pantalla por px de la imagen)
	// y posición de la esquina superior izquierda de la imagen dentro del marco.
	let lado = 0;
	let minimo = 1;
	let escala = 1;
	let x = 0;
	let y = 0;
	const nw = img.naturalWidth;
	const nh = img.naturalHeight;
	function colocar() {
		// la imagen siempre cubre el marco
		x = Math.min(0, Math.max(lado - nw * escala, x));
		y = Math.min(0, Math.max(lado - nh * escala, y));
		img.style.width = `${nw * escala}px`;
		img.style.height = `${nh * escala}px`;
		img.style.transform = `translate(${x}px, ${y}px)`;
	}
	/** Cambia el zoom manteniendo fijo el punto (cx, cy) del marco. */
	function acercar(nuevaEscala, cx, cy) {
		const e = Math.min(minimo * ZOOM_MAXIMO, Math.max(minimo, nuevaEscala));
		const px = (cx - x) / escala;
		const py = (cy - y) / escala;
		escala = e;
		x = cx - px * escala;
		y = cy - py * escala;
		zoom.value = String(escala / minimo);
		colocar();
	}
	function iniciar() {
		lado = marco.clientWidth;
		minimo = Math.max(lado / nw, lado / nh);
		escala = minimo;
		x = (lado - nw * escala) / 2;
		y = (lado - nh * escala) / 2;
		colocar();
	}
	// ── arrastrar y pellizcar ──
	const punteros = new Map();
	let distancia = 0;
	marco.addEventListener("pointerdown", (e) => {
		marco.setPointerCapture(e.pointerId);
		punteros.set(e.pointerId, {
			x: e.clientX,
			y: e.clientY
		});
		distancia = 0;
	});
	marco.addEventListener("pointermove", (e) => {
		const p = punteros.get(e.pointerId);
		if (!p) return;
		const antes = {
			x: p.x,
			y: p.y
		};
		p.x = e.clientX;
		p.y = e.clientY;
		if (punteros.size === 1) {
			x += p.x - antes.x;
			y += p.y - antes.y;
			colocar();
		} else if (punteros.size === 2) {
			const [a, b] = [...punteros.values()];
			const d = Math.hypot(a.x - b.x, a.y - b.y);
			if (distancia > 0) {
				const caja = marco.getBoundingClientRect();
				acercar(escala * d / distancia, (a.x + b.x) / 2 - caja.left, (a.y + b.y) / 2 - caja.top);
			}
			distancia = d;
		}
	});
	const soltar = (e) => {
		punteros.delete(e.pointerId);
		distancia = 0;
	};
	marco.addEventListener("pointerup", soltar);
	marco.addEventListener("pointercancel", soltar);
	marco.addEventListener("wheel", (e) => {
		e.preventDefault();
		const caja = marco.getBoundingClientRect();
		acercar(escala * Math.exp(-e.deltaY * 0.0015), e.clientX - caja.left, e.clientY - caja.top);
	}, { passive: false });
	zoom.addEventListener("input", () => acercar(minimo * Number(zoom.value), lado / 2, lado / 2));
	// ── cerrar ──
	const cerrar = () => {
		if (dlg.open) dlg.close();
	};
	cancelar.addEventListener("click", cerrar);
	guardar.addEventListener("click", () => {
		const lienzo = document.createElement("canvas");
		lienzo.width = salida;
		lienzo.height = salida;
		const c = lienzo.getContext("2d");
		if (!c) return cerrar();
		c.fillStyle = "#222";
		c.fillRect(0, 0, salida, salida);
		c.imageSmoothingQuality = "high";
		c.drawImage(img, -x / escala, -y / escala, lado / escala, lado / escala, 0, 0, salida, salida);
		lienzo.toBlob((blob) => {
			resultado = blob;
			cerrar();
		}, "image/jpeg", 0.9);
	});
	dlg.addEventListener("close", () => {
		dlg.remove();
		URL.revokeObjectURL(url);
		resolver(resultado);
	});
	dlg.showModal();
	iniciar();
}

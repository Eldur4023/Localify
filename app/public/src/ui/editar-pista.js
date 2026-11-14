import { library } from "../ipc/client.js";
import { recortarImagen } from "./recortar-imagen.js";
import { t } from "../i18n/index.js";
import { nodo } from "./dom.js";
const CAMPOS = [
	["title", "edit.title", "text"],
	["artist", "edit.artist", "text"],
	["album", "edit.album", "text"],
	["trackNumber", "edit.track_number", "number"],
	["date", "edit.date", "text"]
];
/**
* Diálogo para editar y completar los metadatos de una pista (título, artista,
* álbum, nº, fecha y foto). Guarda por sí solo; resuelve a lo guardado (truthy) o `false`.
*/
export async function editarPista(trackId, { indice = 1, total = 1, previo = null } = {}) {
	const meta = await library.trackMeta(trackId);
	return new Promise((resolver) => {
		const dlg = nodo("dialog", "dialogo");
		const form = document.createElement("form");
		form.method = "dialog";
		form.append(nodo("h2", "dialogo__titulo", total > 1 ? `${t("edit.title_dialog")} (${indice}/${total})` : t("edit.title_dialog")));
		const entradas = {};
		for (const [clave, etiqueta, tipo] of CAMPOS) {
			const campo = nodo("label", "dialogo__campo");
			const rotulo = document.createElement("span");
			rotulo.textContent = t(etiqueta);
			const entrada = document.createElement("input");
			entrada.type = tipo;
			entrada.className = "dialogo__input";
			// En tandas, lo vacío hereda lo de la anterior (mismo artista/álbum/fecha/foto).
			const heredable = previo && ["artist", "album", "date"].includes(clave);
			entrada.value = meta[clave] ? String(meta[clave]) : (heredable ? previo[clave] : "");
			entrada.autocomplete = "off";
			campo.append(rotulo, entrada);
			form.append(campo);
			entradas[clave] = entrada;
		}
		// ── Imagen: vista previa, cambiar (con recorte cuadrado) o quitar ──
		let blobNuevo = null;
		let quitar = false;
		const fila = nodo("div", "dialogo__imagen");
		const vista = nodo("div", "editar__portada");
		const previa = document.createElement("img");
		previa.alt = "";
		previa.addEventListener("error", () => {
			previa.hidden = true;
		});
		previa.addEventListener("load", () => {
			previa.hidden = false;
		});
		previa.src = `/api/cover/track/${encodeURIComponent(trackId)}?v=${Date.now()}`;
		vista.append(previa);
		const selector = document.createElement("input");
		selector.type = "file";
		selector.accept = "image/*";
		selector.hidden = true;
		const boton = document.createElement("button");
		boton.type = "button";
		boton.className = "boton boton--sutil";
		boton.textContent = t("edit.cover_change");
		boton.addEventListener("click", () => selector.click());
		selector.addEventListener("change", () => {
			const archivo = selector.files?.[0];
			selector.value = "";
			if (!archivo) return;
			void recortarImagen(archivo, 600).then((blob) => {
				if (!blob) return;
				blobNuevo = blob;
				quitar = false;
				previa.src = URL.createObjectURL(blob);
			});
		});
		const sinFoto = document.createElement("button");
		sinFoto.type = "button";
		sinFoto.className = "boton boton--sutil";
		sinFoto.textContent = t("edit.cover_remove");
		sinFoto.addEventListener("click", () => {
			blobNuevo = null;
			quitar = true;
			previa.hidden = true;
		});
		const botones = nodo("div", "editar__botones");
		botones.append(boton, sinFoto, selector);
		fila.append(vista, botones);
		const acciones = nodo("div", "dialogo__acciones");
		const cancelar = document.createElement("button");
		cancelar.type = "button";
		cancelar.className = "boton boton--sutil";
		cancelar.textContent = total > 1 ? t("edit.skip") : t("common.cancel");
		const guardar = document.createElement("button");
		guardar.type = "submit";
		guardar.className = "boton";
		guardar.textContent = t("common.save");
		acciones.append(cancelar, guardar);
		form.append(fila, acciones);
		dlg.append(form);
		document.body.append(dlg);
		let guardado = false;
		cancelar.addEventListener("click", () => dlg.close());
		form.addEventListener("submit", () => {
			const titulo = entradas.title.value.trim();
			if (!titulo) return;
			guardado = true;
			void library.updateTrack(trackId, {
				title: titulo,
				artist: entradas.artist.value,
				album: entradas.album.value,
				trackNumber: Number(entradas.trackNumber.value) || 0,
				date: entradas.date.value,
				removeCover: quitar
			}).then(() => blobNuevo ? library.uploadTrackCover(trackId, blobNuevo) : null).then(() => resolver({
				artist: entradas.artist.value,
				album: entradas.album.value,
				date: entradas.date.value
			}), () => resolver(false));
		});
		dlg.addEventListener("close", () => {
			dlg.remove();
			if (!guardado) resolver(false);
		});
		dlg.showModal();
		entradas.title.select();
	});
}

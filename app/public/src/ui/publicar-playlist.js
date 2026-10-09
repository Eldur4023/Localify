/**
* Compartir una playlist en el servidor común (Serverify).
*
* El servidor solo guarda una REFERENCIA (título, artista, duración de cada
* pista); la música y la playlist de verdad siguen en este equipo.
*/
import { LocalifyError, social } from "../ipc/client.js";
import { t } from "../i18n/index.js";
import { mostrarError } from "./error-overlay.js";
import { confirmar } from "./dialogo.js";
/** Texto de un error del backend, traducido si es tipado. */
export function textoDeError(e) {
	return e instanceof LocalifyError ? t(e.api.messageKey) : String(e);
}
export async function publicarPlaylist(p) {
	let enlace;
	try {
		enlace = await social.playlistLink(p.id);
	} catch (e) {
		mostrarError(t("social.share"), textoDeError(e));
		return;
	}
	const dlg = document.createElement("dialog");
	dlg.className = "dialogo";
	const form = document.createElement("form");
	form.method = "dialog";
	const h = document.createElement("h2");
	h.className = "dialogo__titulo";
	h.textContent = t("social.share_title", { name: p.name });
	const vis = document.createElement("select");
	vis.className = "ajustes__select";
	for (const [valor, clave] of [["private", "social.private"], ["public", "social.public"]]) {
		const op = document.createElement("option");
		op.value = valor;
		op.textContent = t(clave);
		vis.append(op);
	}
	const colab = document.createElement("input");
	colab.type = "checkbox";
	colab.className = "ajustes__check";
	const colabEtiqueta = document.createElement("label");
	colabEtiqueta.className = "social__check";
	colabEtiqueta.append(colab, document.createTextNode(` ${t("social.collaborative")}`));
	const ayuda = document.createElement("p");
	ayuda.className = "dialogo__mensaje";
	ayuda.textContent = t("social.share_help");
	const error = document.createElement("p");
	error.className = "ajustes__ayuda ajustes__ayuda--aviso";
	const acciones = document.createElement("div");
	acciones.className = "dialogo__acciones";
	function boton(texto, clase, accion) {
		const b = document.createElement("button");
		b.type = "button";
		b.className = `boton ${clase}`;
		b.textContent = texto;
		b.addEventListener("click", () => void (async () => {
			b.disabled = true;
			try {
				await accion();
				dlg.close();
			} catch (e) {
				error.textContent = textoDeError(e);
				b.disabled = false;
			}
		})());
		return b;
	}
	const cancelar = boton(t("common.cancel"), "boton--sutil", async () => {});
	acciones.append(cancelar);
	if (enlace) {
		acciones.append(boton(t("social.stop_sharing"), "boton--sutil", async () => {
			if (await confirmar(t("social.stop_sharing_confirm"), t("social.stop_sharing"), undefined, false)) await social.unpublish(p.id);
		}), boton(t("social.sync"), "boton--sutil", () => social.sync(p.id)));
	}
	acciones.append(boton(enlace ? t("social.update") : t("social.publish"), "", () => social.publish(p.id, vis.value, colab.checked)));
	form.append(h, vis, colabEtiqueta, ayuda, error, acciones);
	dlg.append(form);
	document.body.append(dlg);
	dlg.addEventListener("close", () => dlg.remove());
	dlg.showModal();
}

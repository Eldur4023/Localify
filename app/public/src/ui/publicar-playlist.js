/**
* Compartir una playlist y hacerla conjunta (Serverify).
*
* Tu playlist es solo tuya: nadie puede tocarla salvo las personas que invites, y
* invitar a alguien es lo que la hace conjunta. La visibilidad (oculta/pública)
* solo decide quién puede VERLA; los demás nunca pueden editarla.
*
* El servidor guarda una REFERENCIA (título, artista, duración); la música y la
* playlist de verdad siguen en este equipo.
*/
import { LocalifyError, social } from "../ipc/client.js";
import { t } from "../i18n/index.js";
import { mostrarError } from "./error-overlay.js";
/** Texto de un error del backend, traducido si es tipado. */
export function textoDeError(e) {
	return e instanceof LocalifyError ? t(e.api.messageKey) : String(e);
}
function nodo(etiqueta, clase, texto) {
	const n = document.createElement(etiqueta);
	if (clase) n.className = clase;
	if (texto !== undefined) n.textContent = texto;
	return n;
}
export async function publicarPlaylist(p) {
	let ficha;
	try {
		ficha = await social.ensure(p.id);
	} catch (e) {
		mostrarError(t("social.share"), textoDeError(e));
		return;
	}
	const dlg = nodo("dialog", "dialogo");
	const form = nodo("form");
	form.method = "dialog";
	const error = nodo("p", "ajustes__ayuda ajustes__ayuda--aviso");
	const cuerpo = nodo("div");
	const vis = nodo("select", "ajustes__select");
	for (const [valor, clave] of [["private", "social.private"], ["public", "social.public"]]) {
		const op = nodo("option", "", t(clave));
		op.value = valor;
		vis.append(op);
	}
	vis.addEventListener("change", () => void (async () => {
		try {
			ficha = await social.publish(p.id, vis.value, false).then(() => social.ensure(p.id));
			error.textContent = "";
		} catch (e) {
			error.textContent = textoDeError(e);
		}
	})());
	/** Pinta la lista de personas con las que es conjunta. */
	function pintarMiembros() {
		cuerpo.replaceChildren();
		cuerpo.append(nodo("h3", "ajustes__titulo", t("social.joint_with")));
		if (ficha.members.length === 0) cuerpo.append(nodo("p", "vista__empty", t("social.only_you")));
		for (const m of ficha.members) {
			const fila = nodo("div", "social__fila social__fila--plana");
			const nombreM = `${m.displayName} (@${m.username})`;
			fila.append(nodo("span", "social__texto", m.status === "pending" ? `${nombreM} · ${t("social.pending")}` : nombreM));
			const quitar = nodo("button", "boton boton--sutil", t("social.remove"));
			quitar.type = "button";
			quitar.addEventListener("click", () => void cambiar(() => social.removeMember(ficha.id, m.username)));
			fila.append(quitar);
			cuerpo.append(fila);
		}
		const nuevo = nodo("input", "ajustes__input");
		nuevo.placeholder = t("social.invite_placeholder");
		const invitar = nodo("button", "boton", t("social.invite"));
		invitar.type = "button";
		invitar.addEventListener("click", () => void cambiar(() => social.addMember(ficha.id, nuevo.value.trim())));
		nuevo.addEventListener("keydown", (e) => {
			if (e.key === "Enter") {
				e.preventDefault();
				invitar.click();
			}
		});
		const filaNueva = nodo("div", "social__fila social__fila--plana");
		filaNueva.append(nuevo, invitar);
		cuerpo.append(filaNueva);
	}
	async function cambiar(accion) {
		try {
			await accion();
			ficha = await social.ensure(p.id);
			error.textContent = "";
			pintarMiembros();
		} catch (e) {
			error.textContent = textoDeError(e);
		}
	}
	vis.value = ficha.visibility;
	pintarMiembros();
	const cerrar = nodo("button", "boton", t("common.close"));
	cerrar.type = "button";
	cerrar.addEventListener("click", () => dlg.close());
	const acciones = nodo("div", "dialogo__acciones");
	acciones.append(cerrar);
	form.append(nodo("h2", "dialogo__titulo", t("social.share_title", { name: p.name })), nodo("p", "ajustes__etiqueta", t("social.who_sees")), vis, cuerpo, error, acciones);
	dlg.append(form);
	document.body.append(dlg);
	dlg.addEventListener("close", () => dlg.remove());
	dlg.showModal();
}

/**
* Ajustes › Cuenta: todo lo que depende del servidor (perfil, privacidad,
* sincronización, nube, invitaciones y sesión), agrupado por tema en vez de
* apilado en una columna.
*
* Con sesión iniciada pide su propio estado (`social.status`); sin ella solo
* ofrece ir a conectar, que se hace en la pantalla Social.
*/
import { social } from "../ipc/client.js";
import { t } from "../i18n/index.js";
import { textoDeError } from "../ui/publicar-playlist.js";
import { mostrarError } from "../ui/error-overlay.js";
import { botonIcono, icono } from "../ui/icons.js";
import { nodo } from "../ui/dom.js";
/** Sección con título y filas; mismas clases que el resto de Ajustes. */
function seccion(titulo, descripcion) {
	const s = nodo("section", "ajustes__seccion");
	const cab = nodo("div", "ajustes__cabecera");
	cab.append(nodo("h3", "ajustes__titulo", titulo));
	if (descripcion) cab.append(nodo("p", "ajustes__subtitulo", descripcion));
	const cuerpo = nodo("div", "ajustes__cuerpo");
	s.append(cab, cuerpo);
	return {
		s,
		cuerpo
	};
}
/** Fila: título y ayuda a la izquierda, control a la derecha. */
function fila(titulo, ayuda, control) {
	const f = nodo("div", "ajuste");
	const texto = nodo("div", "ajuste__texto");
	texto.append(nodo("strong", "", titulo));
	if (ayuda) texto.append(nodo("span", "", ayuda));
	f.append(texto);
	if (control) f.append(control);
	return f;
}
function interruptor(valor, alCambiar) {
	const i = nodo("input", "interruptor");
	i.type = "checkbox";
	i.checked = valor;
	i.addEventListener("change", () => void Promise.resolve(alCambiar(i.checked)).catch((e) => {
		i.checked = !i.checked;
		mostrarError(t("error.internal"), textoDeError(e));
	}));
	return i;
}
function filaInterruptor(titulo, ayuda, valor, alCambiar) {
	const f = fila(titulo, ayuda, interruptor(valor, alCambiar));
	f.classList.add("ajuste--pulsable");
	return f;
}
/** Botón con estado de carga; los errores salen en `aviso` si se da, o en el overlay. */
function boton(texto, clase, accion, aviso) {
	const b = nodo("button", `boton ${clase ?? ""}`, texto);
	b.type = "button";
	b.addEventListener("click", () => void (async () => {
		b.disabled = true;
		if (aviso) aviso.textContent = "";
		try {
			await accion();
		} catch (e) {
			if (aviso) aviso.textContent = textoDeError(e);
			else mostrarError(t("error.internal"), textoDeError(e));
		}
		b.disabled = false;
	})());
	return b;
}
function avatar(u) {
	const a = nodo("span", "perfil__avatar perfil__avatar--media", (u.displayName || u.username || "?").slice(0, 1).toUpperCase());
	if (u.hasAvatar) {
		const img = nodo("img");
		img.alt = "";
		img.src = `/api/social/img/avatar/${encodeURIComponent(u.username)}?v=${Date.now()}`;
		img.addEventListener("load", () => a.replaceChildren(img));
		img.addEventListener("error", () => img.remove());
	}
	return a;
}
/** "visto hace 5 min" */
function haceTiempo(s) {
	const n = s < 3600 ? Math.max(1, Math.round(s / 60)) : s < 86400 ? Math.round(s / 3600) : Math.round(s / 86400);
	return t("social.seen_ago", { time: `${n} ${s < 3600 ? "min" : s < 86400 ? "h" : "d"}` });
}
function enlace(destino, texto, clase) {
	const a = nodo("a", clase, texto);
	a.href = destino;
	return a;
}
/** Sin servidor o sin sesión: una sola tarjeta que lleva a conectar. */
function sinSesion(panel) {
	const { s, cuerpo } = seccion(t("account.title"));
	cuerpo.append(fila(t("account.disconnected"), t("account.disconnected_help"), enlace("#/social", t("account.connect"), "boton")));
	panel.append(s);
}
function seccionPerfil(u, estado) {
	const { s, cuerpo } = seccion(t("account.profile"));
	const cab = nodo("div", "cuenta__perfil");
	const texto = nodo("div", "cuenta__perfil-texto");
	texto.append(nodo("strong", "", u.displayName || u.username), nodo("span", "", `@${u.username} · ${estado.host}:${estado.port}`));
	const acciones = nodo("div", "cuenta__perfil-acciones");
	acciones.append(enlace("#/social/me", t("account.view_profile"), "boton"));
	cab.append(avatar(u), texto, acciones);
	cuerpo.append(cab);
	return s;
}
function seccionPrivacidad(u, estado) {
	const { s, cuerpo } = seccion(t("social.privacy"), t("account.privacy_help"));
	cuerpo.append(filaInterruptor(t("social.presence"), t("social.presence_help"), estado.presence !== false, async (v) => {
		await social.setPresence(v);
		estado.presence = v;
	}));
	// El servidor no calcula ni envía lo que se oculta
	const ocultas = new Set((u.hide ?? "").split(",").filter(Boolean));
	for (const [clave, texto] of [["top", "show_top"], ["time", "show_time"], ["library", "show_library"]]) {
		cuerpo.append(filaInterruptor(t(`social.${texto}`), t(`social.${texto}_help`), !ocultas.has(clave), async (v) => {
			if (v) ocultas.delete(clave); else ocultas.add(clave);
			await social.setPrivacy([...ocultas]);
			u.hide = [...ocultas].join(",");
		}));
	}
	return s;
}
async function seccionSincronizacion() {
	const { s, cuerpo } = seccion(t("social.sync_title"), t("social.sync_help"));
	let info;
	try {
		info = await social.syncStatus();
	} catch {
		info = {
			enabled: false,
			mode: "both",
			deviceId: "",
			devices: []
		};
	}
	const pintarCuerpo = () => {
		cuerpo.replaceChildren();
		cuerpo.append(filaInterruptor(t("social.sync_enable"), "", info.enabled, async (v) => {
			await social.setSync({ enabled: v });
			// el móvil necesita el servicio en primer plano para seguir sincronizado en segundo plano
			if (v) globalThis.AndroidBridge?.startPlayback?.();
			info = await social.syncStatus();
			pintarCuerpo();
		}));
		if (info.enabled) {
			const modos = nodo("div", "social__modos");
			for (const [clave, titulo, ayuda] of [
				["both", t("social.sync_both"), t("social.sync_both_help")],
				["here", t("social.sync_here"), t("social.sync_here_help")]
			]) {
				const b = nodo("button", `social__modo${info.mode === clave ? " is-activo" : ""}`);
				b.type = "button";
				b.append(nodo("strong", "", titulo), nodo("span", "social__sub", ayuda));
				b.addEventListener("click", () => void social.setSync({ mode: clave }).then(() => {
					info.mode = clave;
					pintarCuerpo();
				}).catch((e) => mostrarError(t("error.internal"), textoDeError(e))));
				modos.append(b);
			}
			cuerpo.append(modos);
		}
		cuerpo.append(nodo("h4", "social__subtitulo", t("social.devices")));
		if (info.devices.length === 0) cuerpo.append(nodo("p", "social__sub", t("social.no_devices")));
		for (const d of info.devices) {
			const este = d.deviceId === info.deviceId;
			const enLinea = este ? info.enabled : d.agoS < 150;
			const f = nodo("div", "social__equipo");
			const ico = nodo("span", `social__equipo-icono${enLinea ? " is-linea" : ""}`);
			ico.append(icono(d.platform === "android" ? "smartphone" : "monitor", 20));
			const texto = nodo("span", "social__texto");
			texto.append(nodo("strong", "", este ? `${d.name || t("social.this_device")} · ${t("social.this_device")}` : d.name || d.deviceId), nodo("span", "social__sub", enLinea ? t("social.online") : haceTiempo(d.agoS)));
			f.append(ico, texto);
			if (!este) {
				f.append(botonIcono("trash", t("social.remove_device"), () => void social.removeDevice(d.deviceId).then(() => {
					info.devices = info.devices.filter((x) => x.deviceId !== d.deviceId);
					pintarCuerpo();
				}).catch((e) => mostrarError(t("error.internal"), textoDeError(e))), { tamano: 18 }));
			}
			cuerpo.append(f);
		}
	};
	pintarCuerpo();
	return s;
}
function seccionNube(estado) {
	const { s, cuerpo } = seccion(t("social.cloud_title"), t("account.cloud_help"));
	cuerpo.append(filaInterruptor(t("social.upload"), t("social.upload_help"), estado.upload, async (v) => {
		await social.setUpload(v);
		estado.upload = v;
	}));
	cuerpo.append(filaInterruptor(t("social.stream"), t("social.stream_help"), estado.stream, async (v) => {
		await social.setStream(v);
		estado.stream = v;
	}));
	const aviso = nodo("p", "ajustes__ayuda");
	const acciones = nodo("div", "ajustes__acciones");
	acciones.append(boton(t("social.cloud_pull"), "boton--sutil", async () => {
		aviso.textContent = t("social.pulling");
		const r = await social.pull();
		aviso.textContent = t("social.cloud_result", {
			had: r.alreadyHad,
			queued: r.queued,
			playlists: r.playlists
		});
	}, aviso), boton(t("social.contribute_library_run"), "boton--sutil", async () => {
		let desde = 0;
		let subidas = 0;
		let existentes = 0;
		for (;;) {
			const r = await social.syncLibrary(desde);
			subidas += r.uploaded;
			existentes += r.alreadyThere;
			aviso.textContent = t("social.contribute_progress", {
				uploaded: subidas,
				existing: existentes
			});
			if (r.done) break;
			desde = r.next;
		}
	}, aviso));
	cuerpo.append(acciones, aviso);
	return s;
}
function seccionInvitaciones() {
	const { s, cuerpo } = seccion(t("social.invites"), t("social.invites_help"));
	const lista = nodo("div", "cuenta__codigos");
	const aviso = nodo("p", "ajustes__ayuda ajustes__ayuda--aviso");
	const pintarCodigos = async () => {
		lista.replaceChildren();
		for (const c of await social.invites()) lista.append(nodo("code", "", c.code));
	};
	void pintarCodigos().catch(() => {});
	cuerpo.append(lista, aviso, boton(t("social.invite_new"), "boton--sutil cuenta__solo", async () => {
		await social.createInvite();
		await pintarCodigos();
	}, aviso));
	return s;
}
function seccionSesion(estado, recargar) {
	const { s, cuerpo } = seccion(t("social.session"));
	cuerpo.append(fila(t("social.logout"), t("account.logout_help"), boton(t("social.logout"), "boton--sutil", async () => {
		await social.logout();
		recargar();
	})), fila(t("social.disconnect"), t("account.disconnect_help", { server: `${estado.host}:${estado.port}` }), boton(t("social.disconnect"), "boton--peligro", async () => {
		await social.disconnect();
		recargar();
	})));
	return s;
}
/** Pinta la pestaña en `panel`. `recargar` vuelve a pintarla (tras cerrar sesión). */
export async function pintarCuenta(panel, recargar) {
	let estado;
	try {
		estado = await social.status();
	} catch (e) {
		panel.append(nodo("p", "ajustes__ayuda ajustes__ayuda--aviso", textoDeError(e)));
		return;
	}
	if (!estado.host || !estado.reachable || !estado.loggedIn) {
		sinSesion(panel);
		return;
	}
	try {
		const u = await social.user(estado.username);
		panel.append(seccionPerfil(u, estado), seccionPrivacidad(u, estado), await seccionSincronizacion(), seccionNube(estado), seccionInvitaciones(), seccionSesion(estado, recargar));
	} catch (e) {
		panel.append(nodo("p", "ajustes__ayuda ajustes__ayuda--aviso", textoDeError(e)));
	}
}

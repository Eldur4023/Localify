/**
* Ajustes › Servidor: para cualquiera con sesión en un servidor común. Todos ven
* invitar (si el servidor se lo permite) y pueden borrar su cuenta. Los admins,
* además: quién puede invitar, los códigos sin usar, los usuarios (admins y bans)
* y el historial de lo que se hace en el servidor, filtrable por fechas, usuario y acción.
*
* Lo de admin va por `social.admin` → `/api/admin/*`: el servidor decide quién es
* admin (403 a los demás); aquí solo se pinta.
*/
import { social } from "../ipc/client.js";
import { idioma, t } from "../i18n/index.js";
import { textoDeError } from "../ui/publicar-playlist.js";
import { mostrarError } from "../ui/error-overlay.js";
import { confirmar } from "../ui/dialogo.js";
import { nodo } from "../ui/dom.js";
import { boton, fila, seccion } from "./cuenta.js";
/** Acciones del historial, en el orden del filtro. */
const ACCIONES = ["register", "login", "login_failed", "login_banned", "invite_create", "invite_revoke", "ban", "unban", "admin_grant", "admin_revoke", "settings", "playlist_create", "playlist_delete", "track_upload", "account_delete"];
const fecha = (s) => new Date(s * 1000).toLocaleString(idioma());
function selector(opciones, valor, alCambiar) {
	const s = nodo("select", "ajustes__select");
	for (const [v, texto] of opciones) {
		const o = nodo("option", "", texto);
		o.value = v;
		s.append(o);
	}
	s.value = valor;
	s.addEventListener("change", () => alCambiar(s.value));
	return s;
}
function fallo(e) {
	mostrarError(t("error.internal"), textoDeError(e));
}
/** Un código recién generado: el código y su enlace (la página del servidor que abre Localify), con copiar. */
function codigoNuevo(r) {
	const f = nodo("div", "servidor__codigo");
	const copiar = nodo("button", "boton boton--sutil", t("server.copy_link"));
	copiar.type = "button";
	copiar.addEventListener("click", () => void navigator.clipboard.writeText(r.link).then(() => {
		copiar.textContent = t("server.copied");
	}).catch(fallo));
	f.append(nodo("code", "", r.code), nodo("span", "social__sub servidor__enlace", r.link), copiar);
	return f;
}
function seccionInvitaciones(ajustes) {
	const { s, cuerpo } = seccion(t("social.invites"), t("server.invites_help"));
	const nuevos = nodo("div", "servidor__lista");
	const aviso = nodo("p", "ajustes__ayuda ajustes__ayuda--aviso");
	const pendientes = nodo("div", "servidor__lista");
	// Los códigos de otros solo se ven abreviados: el código entero sale una vez, al generarlo
	async function pintarPendientes() {
		if (!ajustes) return;
		const lista = await social.admin("GET", "/invites");
		pendientes.replaceChildren();
		if (lista.length === 0) pendientes.append(nodo("p", "social__sub", t("server.no_pending")));
		for (const c of lista) {
			pendientes.append(fila(`${c.code.slice(0, 4)}…`, `${c.createdBy ? `@${c.createdBy} · ` : ""}${fecha(c.createdAt)}`, boton(t("server.revoke"), "boton--sutil", async () => {
				await social.admin("DELETE", `/invites/${encodeURIComponent(c.code)}`);
				await pintarPendientes();
			})));
		}
	}
	if (ajustes) {
		const politica = selector([["everyone", t("server.everyone")], ["admins", t("server.admins")]], ajustes.invites, (v) => void social.admin("PUT", "/settings", { invites: v }).then(() => {
			ajustes.invites = v;
			void pintarPendientes();
		}).catch((e) => {
			politica.value = ajustes.invites;
			fallo(e);
		}));
		cuerpo.append(fila(t("server.invite_policy"), t("server.invite_policy_help"), politica));
	}
	cuerpo.append(nuevos, aviso, boton(t("social.invite_new"), "boton--sutil cuenta__solo", async () => {
		nuevos.append(codigoNuevo(await social.createInvite()));
		await pintarPendientes();
	}, aviso));
	if (ajustes) {
		cuerpo.append(nodo("h4", "social__subtitulo", t("server.pending")), pendientes);
		void pintarPendientes().catch(fallo);
	}
	return s;
}
function seccionCuenta(estado, recargar) {
	const { s, cuerpo } = seccion(t("server.my_account"));
	const clave = nodo("input", "ajustes__select");
	clave.type = "password";
	clave.placeholder = t("social.password");
	clave.autocomplete = "current-password";
	const aviso = nodo("p", "ajustes__ayuda ajustes__ayuda--aviso");
	const acciones = nodo("div", "ajustes__acciones");
	acciones.append(clave, boton(t("server.delete_account"), "boton--peligro", async () => {
		if (!clave.value) {
			clave.focus();
			return;
		}
		if (!await confirmar(t("server.delete_confirm", { server: `${estado.host}:${estado.port}` }), t("server.delete_account"), t("server.delete_confirm_help"))) return;
		await social.deleteAccount(clave.value);
		recargar();
	}, aviso));
	cuerpo.append(fila(t("server.delete_account"), t("server.delete_help"), acciones), aviso);
	return s;
}
function seccionUsuarios(usuarios, yo, recargar) {
	const { s, cuerpo } = seccion(t("server.users"));
	for (const u of usuarios) {
		const etiquetas = [`@${u.username}`, u.username === yo ? t("server.you") : "", u.isAdmin ? t("server.admin_badge") : "", u.bannedAt ? t("server.banned_badge") : ""].filter(Boolean);
		const acciones = nodo("div", "ajustes__acciones");
		if (!u.bannedAt) acciones.append(boton(t(u.isAdmin ? "server.remove_admin" : "server.make_admin"), "boton--sutil", async () => {
			await social.admin("PUT", `/users/${encodeURIComponent(u.username)}`, { admin: !u.isAdmin });
			recargar();
		}));
		if (u.username !== yo) acciones.append(boton(t(u.bannedAt ? "server.unban" : "server.ban"), u.bannedAt ? "boton--sutil" : "boton--peligro", async () => {
			if (!u.bannedAt && !await confirmar(t("server.ban_confirm", { name: u.displayName || u.username }), t("server.ban"), t("server.ban_confirm_help"))) return;
			await social.admin("PUT", `/users/${encodeURIComponent(u.username)}`, { banned: !u.bannedAt });
			recargar();
		}));
		cuerpo.append(fila(u.displayName || u.username, etiquetas.join(" · "), acciones));
	}
	return s;
}
function seccionHistorial(usuarios) {
	const { s, cuerpo } = seccion(t("server.log"), t("server.log_help"));
	const filtro = { from: "", to: "", user: "", action: "" };
	const dia = (clave) => {
		const i = nodo("input", "ajustes__select");
		i.type = "date";
		i.addEventListener("change", () => {
			filtro[clave] = i.value;
			void cargar(true).catch(fallo);
		});
		return i;
	};
	const quien = selector([["", t("server.any")], ...usuarios.map((u) => [u.username, `@${u.username}`])], "", (v) => {
		filtro.user = v;
		void cargar(true).catch(fallo);
	});
	const que = selector([["", t("server.any")], ...ACCIONES.map((a) => [a, t(`server.act_${a}`)])], "", (v) => {
		filtro.action = v;
		void cargar(true).catch(fallo);
	});
	const filtros = nodo("div", "servidor__filtros");
	for (const [etiqueta, control] of [[t("server.from"), dia("from")], [t("server.to"), dia("to")], [t("server.user"), quien], [t("server.action"), que]]) {
		const l = nodo("label", "servidor__filtro");
		l.append(nodo("span", "social__sub", etiqueta), control);
		filtros.append(l);
	}
	const lista = nodo("div", "servidor__log");
	const mas = boton(t("server.more"), "boton--sutil cuenta__solo", () => cargar(false));
	let ultimo = 0;
	// "Hasta" incluye ese día entero: el límite es el comienzo del siguiente, en hora local
	const segundos = (d, finDeDia) => d ? Math.floor(new Date(`${d}T00:00:00`).getTime() / 1000) + (finDeDia ? 86400 : 0) : 0;
	async function cargar(deNuevo) {
		const q = new URLSearchParams({ from: segundos(filtro.from, false), to: segundos(filtro.to, true), user: filtro.user, action: filtro.action, before: deNuevo ? 0 : ultimo, limit: 100 });
		const filas = await social.admin("GET", `/log?${q}`);
		if (deNuevo) lista.replaceChildren();
		for (const r of filas) {
			const f = nodo("div", "servidor__evento");
			f.append(nodo("span", "social__sub", fecha(r.at)), nodo("strong", "", r.actor ? `@${r.actor}` : "—"), nodo("span", "", t(`server.act_${r.action}`)), nodo("span", "social__sub", [r.target, r.detail].filter(Boolean).join(" · ")));
			lista.append(f);
		}
		if (deNuevo && filas.length === 0) lista.append(nodo("p", "social__sub", t("server.no_log")));
		if (filas.length) ultimo = filas[filas.length - 1].id;
		mas.hidden = filas.length < 100;
	}
	cuerpo.append(filtros, lista, mas);
	void cargar(true).catch(fallo);
	return s;
}
/** Pinta la pestaña en `panel`. `recargar` vuelve a preguntar mi papel y repinta (tras cambiar admins,
* bans, o borrar la cuenta). */
export async function pintarServidor(panel, esAdmin, recargar) {
	try {
		const estado = await social.status();
		if (!estado.loggedIn) {
			recargar();
			return;
		}
		const [u, ajustes, usuarios] = await Promise.all([social.user(estado.username), esAdmin ? social.admin("GET", "/settings") : null, esAdmin ? social.admin("GET", "/users") : []]);
		if (u.canInvite !== false) panel.append(seccionInvitaciones(ajustes));
		if (esAdmin) panel.append(seccionUsuarios(usuarios, estado.username, recargar), seccionHistorial(usuarios));
		panel.append(seccionCuenta(estado, recargar));
	} catch (e) {
		panel.append(nodo("p", "ajustes__ayuda ajustes__ayuda--aviso", textoDeError(e)));
	}
}

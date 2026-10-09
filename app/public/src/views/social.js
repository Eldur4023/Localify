/**
* Social: usuarios del servidor común y playlists compartidas.
*
* Todo lo que ve esta pestaña vive en el servidor (Serverify); la música y las
* playlists de verdad siguen en este equipo. Sin servidor o sin sesión la app
* funciona exactamente igual: aquí solo se pide conectar.
*/
import { social } from "../ipc/client.js";
import { alCambiarIdioma, t } from "../i18n/index.js";
import { textoDeError } from "../ui/publicar-playlist.js";
import { mostrarError } from "../ui/error-overlay.js";
function nodo(etiqueta, clase, texto) {
	const n = document.createElement(etiqueta);
	if (clase) n.className = clase;
	if (texto !== undefined) n.textContent = texto;
	return n;
}
function duracion(ms) {
	const s = Math.round((ms || 0) / 1e3);
	return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
}
export function mountSocialView(contenedor, ruta) {
	const el = nodo("section", "vista vista--scroll ajustes social");
	contenedor.replaceChildren(el);
	let estado = null;
	let pestaña = "users";
	let destruida = false;
	/** Resultado de la última descarga de la nube, para enseñarlo arriba. */
	let avisoNube = "";
	async function cargar() {
		try {
			estado = await social.status();
		} catch (e) {
			estado = {
				host: "",
				port: 8787,
				reachable: false,
				loggedIn: false,
				username: "",
				upload: true
			};
		}
		if (!destruida) void pintar();
	}
	function textoNube(r) {
		return t("social.cloud_result", {
			had: r.alreadyHad,
			queued: r.queued,
			playlists: r.playlists
		});
	}
	/** Descarga de la nube: nunca baja lo que ya hay en este equipo. */
	async function descargarNube() {
		try {
			avisoNube = textoNube(await social.pull());
		} catch (e) {
			avisoNube = textoDeError(e);
		}
		if (!destruida) void pintar();
	}
	// ── Piezas ──────────────────────────────────────────────────────────────
	function campoTexto(etiqueta, valor, tipo = "text") {
		const fila = nodo("div", "ajustes__campo");
		const l = nodo("label", "ajustes__etiqueta", etiqueta);
		const i = nodo("input", "ajustes__input");
		i.type = tipo;
		i.value = valor ?? "";
		i.id = `so-${Math.random().toString(36).slice(2, 9)}`;
		l.htmlFor = i.id;
		fila.append(l, i);
		return {
			fila,
			input: i
		};
	}
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
	function seccion(titulo) {
		const s = nodo("section", "ajustes__seccion");
		const cuerpo = nodo("div", "ajustes__cuerpo");
		s.append(nodo("h3", "ajustes__titulo", titulo), cuerpo);
		return {
			s,
			cuerpo
		};
	}
	function enlaceA(destino, texto, clase) {
		const a = nodo("a", clase, texto);
		a.href = `#/${destino}`;
		return a;
	}
	function fila(titulo, subtitulo, destino, etiquetas = []) {
		const a = nodo("a", "social__fila");
		a.href = `#/${destino}`;
		const texto = nodo("div", "social__texto");
		texto.append(nodo("strong", "", titulo), nodo("span", "social__sub", subtitulo));
		a.append(texto);
		for (const e of etiquetas) a.append(nodo("span", "social__etiqueta", e));
		return a;
	}
	// ── Conexión y sesión ───────────────────────────────────────────────────
	function panelConexion(panel) {
		const { s, cuerpo } = seccion(t("social.connect_title"));
		cuerpo.append(nodo("p", "ajustes__ayuda", t("social.connect_help")));
		const host = campoTexto(t("social.host"), estado.host);
		const puerto = campoTexto(t("social.port"), String(estado.port || 8787), "number");
		const aviso = nodo("p", "ajustes__ayuda ajustes__ayuda--aviso");
		if (estado.host && !estado.reachable) aviso.textContent = t("error.server_unreachable");
		cuerpo.append(host.fila, puerto.fila, aviso, boton(t("social.connect"), "", async () => {
			estado = await social.connect(host.input.value.trim(), Number(puerto.input.value));
			void pintar();
		}, aviso));
		panel.append(s);
	}
	function panelLogin(panel) {
		const { s, cuerpo } = seccion(t("social.login_title", { server: `${estado.host}:${estado.port}` }));
		const usuario = campoTexto(t("social.username"), "");
		const clave = campoTexto(t("social.password"), "", "password");
		const nombre = campoTexto(t("social.display_name"), "");
		const aviso = nodo("p", "ajustes__ayuda ajustes__ayuda--aviso");
		const acciones = nodo("div", "ajustes__acciones");
		const entrar = async () => {
			await social.login(usuario.input.value.trim(), clave.input.value);
			// Al entrar con mi usuario se trae lo que tengo guardado en el servidor
			avisoNube = t("social.pulling");
			await cargar();
			await descargarNube();
		};
		acciones.append(boton(t("social.login"), "", entrar, aviso), boton(t("social.register"), "boton--sutil", async () => {
			await social.register(usuario.input.value.trim(), clave.input.value, nombre.input.value.trim());
			await cargar();
		}, aviso), boton(t("social.other_server"), "boton--sutil", async () => {
			await social.disconnect();
			await cargar();
		}));
		clave.input.addEventListener("keydown", (e) => {
			if (e.key === "Enter") void entrar().catch((x) => {
				aviso.textContent = textoDeError(x);
			});
		});
		cuerpo.append(usuario.fila, clave.fila, nombre.fila, nodo("p", "ajustes__ayuda", t("social.register_help")), aviso, acciones);
		panel.append(s);
	}
	// ── Pestañas con sesión ─────────────────────────────────────────────────
	function pestañas(panel) {
		const nav = nodo("div", "ajustes__tabs");
		nav.setAttribute("role", "tablist");
		for (const [clave, texto] of [
			["users", t("social.users")],
			["playlists", t("social.playlists")],
			["me", t("social.me")]
		]) {
			const b = nodo("button", `ajustes__tab${clave === pestaña ? " is-activa" : ""}`, texto);
			b.type = "button";
			b.addEventListener("click", () => {
				pestaña = clave;
				void pintar();
			});
			nav.append(b);
		}
		panel.append(nav);
	}
	async function pintarUsuarios(panel) {
		const lista = await social.users();
		const { s, cuerpo } = seccion(t("social.users"));
		for (const u of lista) {
			cuerpo.append(fila(u.displayName, `@${u.username}${u.bio ? ` · ${u.bio}` : ""}`, `social/user/${encodeURIComponent(u.username)}`, [t("social.n_playlists", { count: u.nPlaylists }), t("social.n_tracks", { count: u.nTracks })]));
		}
		panel.append(s);
	}
	function etiquetasDe(p) {
		const e = [p.visibility === "public" ? t("social.public") : t("social.private")];
		if (p.collaborative) e.push(t("social.collaborative"));
		if (p.localId) e.push(t("social.in_library"));
		return e;
	}
	async function pintarPlaylists(panel) {
		const lista = await social.playlists();
		const { s, cuerpo } = seccion(t("social.playlists"));
		if (lista.length === 0) cuerpo.append(nodo("p", "vista__empty", t("social.no_playlists")));
		for (const p of lista) {
			cuerpo.append(fila(p.name, `${p.ownerName} · ${t("social.n_tracks", { count: p.nTracks })}`, `social/playlist/${p.id}`, etiquetasDe(p)));
		}
		panel.append(s);
	}
	async function pintarPerfil(panel) {
		const yo = await social.me();
		const { s, cuerpo } = seccion(t("social.me"));
		const nombre = campoTexto(t("social.display_name"), yo.displayName);
		const bio = campoTexto(t("social.bio"), yo.bio);
		const aviso = nodo("p", "ajustes__ayuda");
		cuerpo.append(nodo("p", "ajustes__ayuda", `@${yo.username} · ${estado.host}:${estado.port}`), nombre.fila, bio.fila, aviso, boton(t("common.save"), "", async () => {
			await social.updateMe(nombre.input.value.trim(), bio.input.value);
			aviso.textContent = t("social.saved");
		}, aviso));
		const filaNube = nodo("div", "ajustes__campo");
		const avisoPull = nodo("p", "ajustes__ayuda", avisoNube);
		filaNube.append(nodo("label", "ajustes__etiqueta", t("social.cloud")), boton(t("social.cloud_pull"), "boton--sutil", async () => {
			avisoPull.textContent = t("social.pulling");
			avisoPull.textContent = textoNube(await social.pull());
		}, avisoPull), avisoPull, nodo("p", "ajustes__ayuda", t("social.cloud_help")));
		cuerpo.append(filaNube);
		const subida = nodo("input", "ajustes__check");
		subida.type = "checkbox";
		subida.checked = estado.upload;
		subida.addEventListener("change", () => void social.setUpload(subida.checked));
		const filaSubida = nodo("div", "ajustes__campo");
		const etiqueta = nodo("label", "ajustes__etiqueta", t("social.upload"));
		subida.id = "so-upload";
		etiqueta.htmlFor = "so-upload";
		filaSubida.append(etiqueta, subida, nodo("p", "ajustes__ayuda", t("social.upload_help")));
		cuerpo.append(filaSubida);
		// Aportar lo que ya tengo descargado: lo que el servidor ya tenga (misma
		// canción por título + artista + duración) no se vuelve a subir.
		const progreso = nodo("p", "ajustes__ayuda");
		const filaBiblioteca = nodo("div", "ajustes__campo");
		filaBiblioteca.append(nodo("label", "ajustes__etiqueta", t("social.contribute_library")), boton(t("social.contribute_library_run"), "boton--sutil", async () => {
			let desde = 0;
			let subidas = 0;
			let existentes = 0;
			for (;;) {
				const r = await social.syncLibrary(desde);
				subidas += r.uploaded;
				existentes += r.alreadyThere;
				progreso.textContent = t("social.contribute_progress", {
					uploaded: subidas,
					existing: existentes
				});
				if (r.done) break;
				desde = r.next;
			}
		}, progreso), progreso, nodo("p", "ajustes__ayuda", t("social.contribute_library_help")));
		cuerpo.append(filaBiblioteca);
		const acciones = nodo("div", "ajustes__acciones");
		acciones.append(boton(t("social.logout"), "boton--sutil", async () => {
			await social.logout();
			await cargar();
		}), boton(t("social.disconnect"), "boton--peligro", async () => {
			await social.disconnect();
			await cargar();
		}));
		cuerpo.append(acciones);
		panel.append(s);
	}
	// ── Perfil de otro usuario ──────────────────────────────────────────────
	async function pintarUsuario(panel, nombre) {
		const u = await social.user(nombre);
		const { s, cuerpo } = seccion(u.displayName);
		cuerpo.append(nodo("p", "ajustes__ayuda", `@${u.username}`), nodo("p", "", u.bio || t("social.no_bio")));
		panel.append(enlaceA("social", t("social.back"), "social__volver"), s);
		const { s: sp, cuerpo: cp } = seccion(t("social.public_playlists"));
		if (u.playlists.length === 0) cp.append(nodo("p", "vista__empty", t("social.no_playlists")));
		for (const p of u.playlists) cp.append(fila(p.name, t("social.n_tracks", { count: p.nTracks }), `social/playlist/${p.id}`, etiquetasDe(p)));
		panel.append(sp);
	}
	// ── Detalle de una playlist del servidor ────────────────────────────────
	async function pintarPlaylist(panel, id) {
		const p = await social.playlist(id);
		const { s, cuerpo } = seccion(p.name);
		cuerpo.append(nodo("p", "ajustes__ayuda", `${t("social.by")} ${p.ownerName} (@${p.owner}) · ${etiquetasDe(p).join(" · ")}`));
		const aviso = nodo("p", "ajustes__ayuda ajustes__ayuda--aviso");
		const acciones = nodo("div", "ajustes__acciones");
		const abrir = (localId) => {
			globalThis.location.hash = `#/playlist/${localId}`;
		};
		if (p.localId) {
			acciones.append(boton(t("social.open"), "", async () => abrir(p.localId), aviso), boton(t("social.sync"), "boton--sutil", async () => {
				await social.importPlaylist(id);
				await pintar();
			}, aviso));
		} else {
			acciones.append(boton(t("social.add_to_library"), "", async () => {
				const r = await social.importPlaylist(id);
				abrir(r.localId);
			}, aviso));
		}
		cuerpo.append(acciones, aviso);
		// Miembros (colaboradores)
		const { s: sm, cuerpo: cm } = seccion(t("social.members"));
		for (const m of p.members) {
			const f = nodo("div", "social__fila social__fila--plana");
			f.append(nodo("span", "social__texto", `${m.displayName} (@${m.username})`));
			if (p.access === "owner" || m.username === estado.username) {
				f.append(boton(p.access === "owner" ? t("social.remove") : t("social.leave"), "boton--sutil", async () => {
					await social.removeMember(id, m.username);
					await pintar();
				}));
			}
			cm.append(f);
		}
		if (p.members.length === 0) cm.append(nodo("p", "vista__empty", t("social.no_members")));
		if (p.access === "owner") {
			const nuevo = campoTexto(t("social.add_member"), "");
			const avisoM = nodo("p", "ajustes__ayuda ajustes__ayuda--aviso");
			const accionesM = nodo("div", "ajustes__acciones");
			accionesM.append(boton(t("social.add"), "", async () => {
				await social.addMember(id, nuevo.input.value.trim());
				await pintar();
			}, avisoM));
			cm.append(nuevo.fila, avisoM, accionesM);
		}
		// Pistas
		const { s: st, cuerpo: ct } = seccion(t("social.tracks"));
		for (const pista of p.tracks) {
			const f = nodo("div", "social__fila social__fila--plana");
			const texto = nodo("div", "social__texto");
			texto.append(nodo("strong", "", pista.title), nodo("span", "social__sub", [pista.artist, pista.album].filter(Boolean).join(" · ")));
			f.append(texto);
			if (pista.available) f.append(nodo("span", "social__etiqueta", t("social.on_server")));
			f.append(nodo("span", "social__sub", duracion(pista.durationMs)));
			ct.append(f);
		}
		panel.append(enlaceA("social/playlists", t("social.back"), "social__volver"), s, sm, st);
	}
	// ── Pintado ─────────────────────────────────────────────────────────────
	async function pintar() {
		if (!estado || destruida) return;
		const panel = nodo("div", "ajustes__panel");
		try {
			if (!estado.host || !estado.reachable) panelConexion(panel);
			else if (!estado.loggedIn) panelLogin(panel);
			else if (ruta.params[0] === "user" && ruta.params[1]) await pintarUsuario(panel, ruta.params[1]);
			else if (ruta.params[0] === "playlist" && ruta.params[1]) await pintarPlaylist(panel, ruta.params[1]);
			else {
				if (ruta.params[0] === "playlists") pestaña = "playlists";
				if (avisoNube) panel.append(nodo("p", "ajustes__ayuda", avisoNube));
				pestañas(panel);
				if (pestaña === "users") await pintarUsuarios(panel);
				else if (pestaña === "playlists") await pintarPlaylists(panel);
				else await pintarPerfil(panel);
			}
		} catch (e) {
			panel.append(nodo("p", "ajustes__ayuda ajustes__ayuda--aviso", textoDeError(e)));
			// Sesión caducada: el backend ya olvidó el token, se vuelve a pedir
			if (e?.api?.code === "SERVER_AUTH") {
				estado = {
					...estado,
					loggedIn: false
				};
				panel.replaceChildren();
				panelLogin(panel);
			}
		}
		if (!destruida) el.replaceChildren(panel);
	}
	void cargar();
	const dejarIdioma = alCambiarIdioma(() => void pintar());
	return { destroy() {
		destruida = true;
		dejarIdioma();
		el.remove();
	} };
}

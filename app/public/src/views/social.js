/**
* Social: usuarios del servidor común y playlists compartidas.
*
* Todo lo que ve esta pestaña vive en el servidor (Serverify); la música y las
* playlists de verdad siguen en este equipo. Sin servidor o sin sesión la app
* funciona exactamente igual: aquí solo se pide conectar.
*/
import { player, social } from "../ipc/client.js";
import { alCambiarIdioma, t } from "../i18n/index.js";
import { textoDeError } from "../ui/publicar-playlist.js";
import { mostrarError } from "../ui/error-overlay.js";
import { botonIcono } from "../ui/icons.js";
import { nodo } from "../ui/dom.js";
import { crearPerfil } from "./social-perfil.js";
import { duracion, guardarCache, leerCache, SIN_CACHE } from "./social-cache.js";
export function mountSocialView(contenedor, ruta) {
	const el = nodo("section", "vista vista--scroll ajustes social");
	contenedor.replaceChildren(el);
	let estado = null;
	let pestaña = "users";
	let destruida = false;
	/** Resultado de la última descarga de la nube, para enseñarlo arriba. */
	let avisoNube = "";
	/**
	* Caché "viejo primero": cada dato del servidor se guarda al llegar y se
	* pinta de inmediato la copia anterior mientras llega la nueva. La clave
	* lleva servidor y usuario para no enseñar datos de otra cuenta.
	*/
	let cacheando = false;
	let pasada = 0;
	async function dato(clave, pedir) {
		const k = `${estado.host}:${estado.port}.${estado.username}.${clave}`;
		if (cacheando) {
			const v = leerCache(k);
			if (v === undefined) throw SIN_CACHE;
			return v;
		}
		const v = await pedir();
		guardarCache(k, v);
		return v;
	}
	async function cargar() {
		const previo = leerCache("estado");
		if (!estado && previo?.loggedIn) {
			estado = previo;
			void pintar();
		}
		try {
			estado = await social.status();
			if (estado.loggedIn) guardarCache("estado", estado);
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
	// ── Acceso: servidor, usuario y contraseña en un solo paso ──────────────
	function panelAcceso(panel) {
		panel.classList.add("social__acceso");
		const { s, cuerpo } = seccion(t("social.connect_title"));
		const servidor = campoTexto(t("social.server"), estado.host ? `${estado.host}:${estado.port}` : "");
		servidor.input.placeholder = "192.168.1.10:8787";
		servidor.input.autocapitalize = "none";
		const usuario = campoTexto(t("social.username"), estado.username);
		usuario.input.autocapitalize = "none";
		const clave = campoTexto(t("social.password"), "", "password");
		const aviso = nodo("p", "ajustes__ayuda ajustes__ayuda--aviso");
		const acceder = (registro) => async () => {
			// "ip" o "ip:puerto"; sin puerto, el de siempre
			const m = servidor.input.value.trim().match(/^(.*?)(?::(\d+))?$/);
			const host = m?.[1] ?? "";
			const puerto = m?.[2] ? Number(m[2]) : 8787;
			estado = await social.connect(host, puerto);
			if (!estado.reachable) {
				aviso.textContent = t("error.server_unreachable");
				return;
			}
			const u = usuario.input.value.trim();
			if (registro) await social.register(u, clave.input.value, u);
			else await social.login(u, clave.input.value);
			// Al entrar con mi usuario se trae lo que tengo guardado en el servidor
			avisoNube = t("social.pulling");
			await cargar();
			await descargarNube();
		};
		const entrar = acceder(false);
		const acciones = nodo("div", "ajustes__acciones");
		acciones.append(boton(t("social.login"), "", entrar, aviso), boton(t("social.register"), "boton--sutil", acceder(true), aviso));
		clave.input.addEventListener("keydown", (e) => {
			if (e.key === "Enter") void entrar().catch((x) => {
				aviso.textContent = textoDeError(x);
			});
		});
		cuerpo.append(servidor.fila, usuario.fila, clave.fila, aviso, acciones);
		panel.append(s);
	}
	// ── Pestañas con sesión ─────────────────────────────────────────────────
	function pestañas(panel) {
		const nav = nodo("div", "ajustes__tabs");
		nav.setAttribute("role", "tablist");
		for (const [clave, texto] of [
			["users", t("social.people")],
			["playlists", t("social.playlists")],
			["me", t("social.profile")]
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
	const claveDato = (clave) => `${estado.host}:${estado.port}.${estado.username}.${clave}`;
	const { avatar, tarjetaLista, cabeceraPerfil, escaparate, pintarPerfil } = crearPerfil({
		get estado() {
			return estado;
		},
		get avisoNube() {
			return avisoNube;
		},
		dato,
		boton,
		seccion,
		pintar,
		cargar,
		claveDato,
		textoNube
	});
	// ── Personas y playlists: rejillas a todo el ancho ──────────────────────
	async function pintarUsuarios(panel) {
		const lista = await dato("users", () => social.users());
		const { s, cuerpo } = seccion(t("social.people"));
		const rejilla = nodo("div", "social__rejilla");
		for (const u of lista) {
			const a = nodo("a", "social__tarjeta");
			// mi propia tarjeta lleva a Mi perfil (editable), no a la vista de cualquier usuario
			a.href = u.username === estado.username ? "#/social/me" : `#/social/user/${encodeURIComponent(u.username)}`;
			const cab = nodo("div", "social__tarjeta-cab");
			const quien = nodo("div", "social__texto");
			quien.append(nodo("strong", "", u.username === estado.username ? `${u.displayName} · ${t("social.you")}` : u.displayName), nodo("span", "social__sub", `@${u.username}`));
			cab.append(avatar(u, "media"), quien);
			a.append(cab);
			if (u.bio) a.append(nodo("p", "social__bio", u.bio));
			if (u.nowPlaying) {
				const ahora = nodo("div", "social__escucha");
				const barras = nodo("span", "perfil__barras");
				barras.append(nodo("i"), nodo("i"), nodo("i"));
				ahora.append(barras, nodo("span", "", `${u.nowPlaying.title}${u.nowPlaying.artist ? ` — ${u.nowPlaying.artist}` : ""}`));
				a.append(ahora);
			}
			const pie = nodo("div", "social__etiquetas");
			pie.append(nodo("span", "social__etiqueta", t("social.n_playlists", { count: u.nPlaylists })), nodo("span", "social__etiqueta", t("social.n_tracks", { count: u.nTracks })));
			a.append(pie);
			rejilla.append(a);
		}
		cuerpo.append(rejilla);
		panel.append(s);
	}
	function etiquetasDe(p) {
		const e = [p.visibility === "public" ? t("social.public") : t("social.private")];
		if (p.collaborative) e.push(t("social.collaborative"));
		if (p.localId) e.push(t("social.in_library"));
		return e;
	}
	async function pintarPlaylists(panel) {
		const lista = await dato("playlists", () => social.playlists());
		const { s, cuerpo } = seccion(t("social.playlists"));
		if (lista.length === 0) cuerpo.append(nodo("p", "vista__empty", t("social.no_playlists")));
		const rejilla = nodo("div", "perfil__rejilla perfil__rejilla--ancha");
		for (const p of lista) rejilla.append(tarjetaLista(p, `${p.ownerName} · ${t("social.n_tracks", { count: p.nTracks })}`, etiquetasDe(p)));
		cuerpo.append(rejilla);
		panel.append(s);
	}
	/** Invitaciones: un aviso arriba, en cualquier pestaña, mientras haya alguna. */
	function pintarInvitaciones(panel, lista) {
		if (lista.length === 0) return;
		const caja = nodo("section", "social__aviso");
		caja.append(nodo("h3", "social__aviso-titulo", t("social.invitations_banner", { count: lista.length })));
		const aviso = nodo("p", "ajustes__ayuda ajustes__ayuda--aviso");
		for (const p of lista) {
			const f = nodo("div", "social__fila social__fila--plana");
			const texto = nodo("div", "social__texto");
			texto.append(nodo("strong", "", p.name), nodo("span", "social__sub", `${t("social.invited_by")} ${p.ownerName} (@${p.owner})`));
			f.append(texto);
			f.append(boton(t("social.accept"), "", async () => {
				await social.accept(p.id);
				await pintar();
			}, aviso));
			f.append(boton(t("social.decline"), "boton--sutil", async () => {
				await social.decline(p.id);
				await pintar();
			}, aviso));
			caja.append(f);
		}
		caja.append(aviso);
		panel.append(caja);
	}
	// ── Perfil de otro usuario ──────────────────────────────────────────────
	async function pintarUsuario(panel, nombre) {
		const u = await dato(`user.${nombre}`, () => social.user(nombre));
		panel.append(enlaceA("social", t("social.back"), "social__volver"));
		cabeceraPerfil(panel, u);
		escaparate(panel, u);
	}
	// ── Detalle de una playlist del servidor ────────────────────────────────
	async function pintarPlaylist(panel, id) {
		const p = await dato(`playlist.${id}`, () => social.playlist(id));
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
			// Escuchar sin abrir: sincroniza la lista y lanza la primera pista.
			const escuchar = botonIcono("play", t("social.listen"), () => void (async () => {
				try {
					const r = await social.importPlaylist(id);
					if (r.firstTrackId) await player.playTrack(r.firstTrackId, { kind: "playlist", id: r.localId });
				} catch (e) {
					aviso.textContent = textoDeError(e);
				}
			})(), { tamano: 22 });
			escuchar.classList.add("bicono--primario");
			escuchar.title = t("social.listen");
			acciones.append(escuchar);
		} else {
			acciones.append(boton(t("social.add_to_library"), "", async () => {
				const r = await social.importPlaylist(id);
				abrir(r.localId);
			}, aviso));
			// Escuchar una playlist pública ajena: trae una copia local enlazada
			// (solo lectura en el servidor) y lanza la primera pista.
			const escuchar = botonIcono("play", t("social.listen"), () => void (async () => {
				try {
					const r = await social.importPlaylist(id);
					if (r.firstTrackId) await player.playTrack(r.firstTrackId, { kind: "playlist", id: r.localId });
					abrir(r.localId);
				} catch (e) {
					aviso.textContent = textoDeError(e);
				}
			})(), { tamano: 22 });
			escuchar.classList.add("bicono--primario");
			escuchar.title = t("social.listen");
			acciones.append(escuchar);
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
	/**
	* Primero la copia guardada (si la hay) y luego los datos vivos. Los pintados se encolan:
	* `cacheando` es compartido y dos pasadas a la vez se pisarían.
	*/
	let cola = Promise.resolve();
	function pintar() {
		cola = cola.then(pintarDoble, pintarDoble);
		return cola;
	}
	async function pintarDoble() {
		const mia = pasada += 1;
		cacheando = true;
		try {
			await pintarUna(mia);
		} finally {
			cacheando = false;
		}
		await pintarUna(mia);
	}
	async function pintarUna(mia) {
		if (!estado || destruida) return;
		const panel = nodo("div", "ajustes__panel");
		try {
			if (!estado.host || !estado.reachable || !estado.loggedIn) panelAcceso(panel);
			else if (ruta.params[0] === "user" && ruta.params[1] && decodeURIComponent(ruta.params[1]) !== estado.username) await pintarUsuario(panel, ruta.params[1]);
			else if (ruta.params[0] === "playlist" && ruta.params[1]) await pintarPlaylist(panel, ruta.params[1]);
			else {
				// entrar en mi propio usuario (desde cualquier enlace) abre Mi perfil
				if (ruta.params[0] === "playlists" || ruta.params[0] === "me") pestaña = ruta.params[0];
				else if (ruta.params[0] === "user") pestaña = "me";
				if (avisoNube) panel.append(nodo("p", "ajustes__ayuda", avisoNube));
				const invitaciones = await dato("invitations", () => social.invitations());
				pintarInvitaciones(panel, invitaciones);
				pestañas(panel);
				if (pestaña === "users") await pintarUsuarios(panel);
				else if (pestaña === "playlists") await pintarPlaylists(panel);
				else await pintarPerfil(panel);
			}
		} catch (e) {
			// Sin copia guardada: no hay nada viejo que enseñar, solo se espera
			if (e === SIN_CACHE) return;
			panel.append(nodo("p", "ajustes__ayuda ajustes__ayuda--aviso", textoDeError(e)));
			// Sesión caducada: el backend ya olvidó el token, se vuelve a pedir
			if (e?.api?.code === "SERVER_AUTH") {
				estado = {
					...estado,
					loggedIn: false
				};
				panel.replaceChildren();
				panelAcceso(panel);
			}
		}
		if (!destruida && mia === pasada) el.replaceChildren(panel);
	}
	void cargar();
	const dejarIdioma = alCambiarIdioma(() => void pintar());
	return { destroy() {
		destruida = true;
		dejarIdioma();
		el.remove();
	} };
}

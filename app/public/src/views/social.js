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
import { botonIcono, icono } from "../ui/icons.js";
import { ponerPortadaDePista } from "../ui/cards.js";
import { colorDePortada } from "../ui/color.js";
import { nodo } from "../ui/dom.js";
/** Tiempo escuchado: "2 h 05 min" o "12 min". */
function tiempo(ms) {
	const min = Math.round((ms || 0) / 6e4);
	return min >= 60 ? `${Math.floor(min / 60)} h ${String(min % 60).padStart(2, "0")} min` : `${min} min`;
}
function duracion(ms) {
	const s = Math.round((ms || 0) / 1e3);
	return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
}
const PREFIJO_CACHE = "localify.social.";
const SIN_CACHE = new Error("sin caché");
function leerCache(clave) {
	try {
		const v = localStorage.getItem(PREFIJO_CACHE + clave);
		return v === null ? undefined : JSON.parse(v);
	} catch {
		return undefined;
	}
}
function guardarCache(clave, valor) {
	try {
		localStorage.setItem(PREFIJO_CACHE + clave, JSON.stringify(valor));
	} catch {}
}
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
	function pestañas(panel, pendientes = 0) {
		const nav = nodo("div", "ajustes__tabs");
		nav.setAttribute("role", "tablist");
		for (const [clave, texto] of [
			["users", t("social.users")],
			["playlists", t("social.playlists")],
			["invitations", pendientes > 0 ? `${t("social.invitations")} (${pendientes})` : t("social.invitations")],
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
	// ── Perfil: cabecera con foto y escaparate (playlists + lo más escuchado) ──
	const version = Date.now();
	function avatar(u, grande, alCargar) {
		const a = nodo("span", `perfil__avatar${grande ? " perfil__avatar--grande" : ""}`, (u.displayName || u.username || "?").slice(0, 1).toUpperCase());
		if (u.hasAvatar) {
			const img = nodo("img");
			img.alt = "";
			img.src = `/api/social/img/avatar/${encodeURIComponent(u.username)}?v=${avatarV || version}`;
			img.addEventListener("load", () => {
				a.replaceChildren(img);
				alCargar?.(img);
			});
			img.addEventListener("error", () => img.remove());
		}
		return a;
	}
	let avatarV = 0;
	function cifra(n, etiqueta) {
		const c = nodo("div", "perfil__cifra");
		c.append(nodo("strong", "", String(n)), nodo("span", "", etiqueta));
		return c;
	}
	function cabeceraPerfil(panel, u, acciones) {
		const c = nodo("header", "perfil__cabecera");
		// el degradado se tiñe con el color dominante de la foto, como el reproductor
		const av = avatar(u, true, (img) => {
			const col = colorDePortada(img);
			if (col) c.style.setProperty("--perfil-color", col);
		});
		const datos = nodo("div", "perfil__datos");
		datos.append(nodo("h2", "perfil__nombre", u.displayName), nodo("span", "perfil__usuario", `@${u.username}`));
		if (u.bio) datos.append(nodo("p", "perfil__bio", u.bio));
		c.append(av, datos);
		if (acciones) {
			const b = nodo("button", "boton boton--sutil perfil__editar", t("social.edit_profile"));
			b.type = "button";
			b.addEventListener("click", acciones.alEditar);
			c.append(b);
			const cuenta = botonIcono("settings", t("social.account"), acciones.alCuenta, { tamano: 20 });
			cuenta.classList.add("perfil__cuenta");
			c.append(cuenta);
		}
		const cifras = nodo("div", "perfil__cifras");
		cifras.append(cifra(u.playlists?.length ?? 0, t("social.playlists")), cifra(tiempo(u.listenedMs ?? 0), t("social.plays_90")), cifra(u.topTracks?.length ?? 0, t("social.top_short")));
		c.append(cifras);
		if (u.nowPlaying) {
			const ahora = nodo("div", "perfil__ahora");
			const barras = nodo("span", "perfil__barras");
			barras.append(nodo("i"), nodo("i"), nodo("i"));
			const texto = nodo("span", "perfil__ahora-texto");
			texto.append(nodo("small", "", t("social.now_playing")), nodo("strong", "", u.nowPlaying.title), nodo("span", "", u.nowPlaying.artist ?? ""));
			ahora.append(barras, texto);
			c.append(ahora);
		}
		panel.append(c);
	}
	function escaparate(panel, u) {
		const listas = u.playlists ?? [];
		if (listas.length > 0) {
			const { s: sp, cuerpo: cp } = seccion(t("social.public_playlists"));
			const rejilla = nodo("div", "perfil__rejilla");
			for (const p of listas) {
				const a = nodo("a", "perfil__lista");
				a.href = `#/social/playlist/${p.id}`;
				const arte = nodo("span", "perfil__arte");
				arte.append(icono("music", 32));
				if (p.hasCover) {
					const img = nodo("img");
					img.alt = "";
					img.src = `/api/social/img/playlist/${p.id}?v=${version}`;
					img.addEventListener("load", () => arte.replaceChildren(img));
				}
				a.append(arte, nodo("strong", "", p.name), nodo("span", "social__sub", t("social.n_tracks", { count: p.nTracks })));
				rejilla.append(a);
			}
			cp.append(rejilla);
			panel.append(sp);
		}
		const top = u.topTracks ?? [];
		if (top.length > 0) {
			const { s: st, cuerpo: ct } = seccion(t("social.top_tracks"));
			const maximo = Math.max(...top.map((x) => x.ms), 1);
			top.forEach((x, i) => {
				const f = nodo("div", "perfil__pista");
				const arte = nodo("span", "perfil__miniatura");
				arte.append(icono("music", 20));
				ponerPortadaDePista(arte, x.id);
				const texto = nodo("div", "social__texto");
				const barra = nodo("span", "perfil__barra");
				barra.style.setProperty("--ancho", `${Math.round(x.ms / maximo * 100)}%`);
				texto.append(nodo("strong", "", x.title), nodo("span", "social__sub", x.artist), barra);
				f.append(nodo("span", "perfil__puesto", String(i + 1)), arte, texto, nodo("span", "social__etiqueta", tiempo(x.ms)));
				ct.append(f);
			});
			panel.append(st);
		}
	}
	async function pintarUsuarios(panel) {
		const lista = await dato("users", () => social.users());
		const { s, cuerpo } = seccion(t("social.users"));
		for (const u of lista) {
			const escucha = u.nowPlaying ? `♪ ${u.nowPlaying.title}${u.nowPlaying.artist ? ` — ${u.nowPlaying.artist}` : ""}` : "";
			const f = fila(u.displayName, escucha || `@${u.username}${u.bio ? ` · ${u.bio}` : ""}`, `social/user/${encodeURIComponent(u.username)}`, [t("social.n_playlists", { count: u.nPlaylists }), t("social.n_tracks", { count: u.nTracks })]);
			f.prepend(avatar(u, false));
			cuerpo.append(f);
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
		const lista = await dato("playlists", () => social.playlists());
		const { s, cuerpo } = seccion(t("social.playlists"));
		if (lista.length === 0) cuerpo.append(nodo("p", "vista__empty", t("social.no_playlists")));
		for (const p of lista) {
			cuerpo.append(fila(p.name, `${p.ownerName} · ${t("social.n_tracks", { count: p.nTracks })}`, `social/playlist/${p.id}`, etiquetasDe(p)));
		}
		panel.append(s);
	}
	/** Invitaciones: aceptar entra en la playlist; rechazar la quita de la lista. */
	function pintarInvitaciones(panel, lista) {
		const { s, cuerpo } = seccion(t("social.invitations"));
		if (lista.length === 0) cuerpo.append(nodo("p", "vista__empty", t("social.no_invitations")));
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
			cuerpo.append(f);
		}
		cuerpo.append(aviso);
		panel.append(s);
	}
	/** Ventana modal sencilla: título, contenido y cerrar. */
	function abrirDialogo(titulo, contenido) {
		const dlg = nodo("dialog", "dialogo perfil__dialogo");
		const form = nodo("form");
		form.method = "dialog";
		const cerrar = boton(t("common.close"), "boton--sutil", async () => dlg.close());
		const acciones = nodo("div", "dialogo__acciones");
		acciones.append(cerrar);
		form.append(nodo("h2", "dialogo__titulo", titulo), contenido, acciones);
		dlg.append(form);
		document.body.append(dlg);
		dlg.addEventListener("close", () => dlg.remove());
		dlg.showModal();
		return dlg;
	}
	/** Editar perfil: foto, nombre y bio. */
	function dialogoEditar(u) {
		const cuerpo = nodo("div", "ajustes__cuerpo perfil__editar-form");
		const foto = nodo("input");
		foto.type = "file";
		foto.accept = "image/*";
		foto.hidden = true;
		const aviso = nodo("p", "ajustes__ayuda ajustes__ayuda--aviso");
		const vistaFoto = nodo("button", "perfil__cambiar-foto");
		vistaFoto.type = "button";
		vistaFoto.setAttribute("aria-label", t("social.change_photo"));
		vistaFoto.append(avatar(u, true), nodo("span", "perfil__camara", t("social.change_photo")));
		vistaFoto.addEventListener("click", () => foto.click());
		let dlg;
		foto.addEventListener("change", () => void (async () => {
			if (!foto.files?.[0]) return;
			const fd = new FormData();
			fd.append("imagen", foto.files[0]);
			aviso.textContent = "…";
			try {
				const r = await fetch("/api/social/avatar", { method: "POST", body: fd });
				if (!r.ok) throw new Error(String(r.status));
				avatarV = Date.now();
				dlg.close();
				await pintar();
			} catch (e) {
				aviso.textContent = String(e);
			}
		})());
		const nombre = campoTexto(t("social.display_name"), u.displayName);
		const bio = campoTexto(t("social.bio"), u.bio);
		cuerpo.append(foto, vistaFoto, nombre.fila, bio.fila, aviso, boton(t("common.save"), "", async () => {
			await social.updateMe(nombre.input.value.trim(), bio.input.value);
			dlg.close();
			await pintar();
		}, aviso));
		dlg = abrirDialogo(t("social.edit_profile"), cuerpo);
	}
	/** Cuenta y nube: sincronización, privacidad y sesión. */
	function dialogoCuenta(u) {
		let dlg;
		const cuerpo2 = nodo("div", "ajustes__cuerpo");
		cuerpo2.append(nodo("p", "ajustes__ayuda", `@${u.username} · ${estado.host}:${estado.port}`));
		const filaNube = nodo("div", "ajustes__campo");
		const avisoPull = nodo("p", "ajustes__ayuda", avisoNube);
		filaNube.append(nodo("label", "ajustes__etiqueta", t("social.cloud")), boton(t("social.cloud_pull"), "boton--sutil", async () => {
			avisoPull.textContent = t("social.pulling");
			avisoPull.textContent = textoNube(await social.pull());
		}, avisoPull), avisoPull);
		cuerpo2.append(filaNube);
		const subida = nodo("input", "ajustes__check");
		subida.type = "checkbox";
		subida.checked = estado.upload;
		subida.addEventListener("change", () => void social.setUpload(subida.checked));
		const presencia = nodo("input", "ajustes__check");
		presencia.type = "checkbox";
		presencia.checked = estado.presence !== false;
		presencia.id = "so-presence";
		presencia.addEventListener("change", () => void social.setPresence(presencia.checked));
		const filaPresencia = nodo("div", "ajustes__campo");
		const etiquetaP = nodo("label", "ajustes__etiqueta", t("social.presence"));
		etiquetaP.htmlFor = "so-presence";
		filaPresencia.append(etiquetaP, presencia);
		cuerpo2.append(filaPresencia);
		const filaSubida = nodo("div", "ajustes__campo");
		const etiqueta = nodo("label", "ajustes__etiqueta", t("social.upload"));
		subida.id = "so-upload";
		etiqueta.htmlFor = "so-upload";
		filaSubida.append(etiqueta, subida);
		cuerpo2.append(filaSubida);
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
		}, progreso), progreso);
		cuerpo2.append(filaBiblioteca);
		const acciones = nodo("div", "ajustes__acciones");
		acciones.append(boton(t("social.logout"), "boton--sutil", async () => {
			await social.logout();
			dlg.close();
			await cargar();
		}), boton(t("social.disconnect"), "boton--peligro", async () => {
			await social.disconnect();
			dlg.close();
			await cargar();
		}));
		cuerpo2.append(acciones);
		dlg = abrirDialogo(t("social.account"), cuerpo2);
	}
	async function pintarPerfil(panel) {
		const u = await dato(`user.${estado.username}`, () => social.user(estado.username));
		cabeceraPerfil(panel, u, {
			alEditar: () => dialogoEditar(u),
			alCuenta: () => dialogoCuenta(u)
		});
		escaparate(panel, u);
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
			else if (ruta.params[0] === "user" && ruta.params[1]) await pintarUsuario(panel, ruta.params[1]);
			else if (ruta.params[0] === "playlist" && ruta.params[1]) await pintarPlaylist(panel, ruta.params[1]);
			else {
				if (ruta.params[0] === "playlists") pestaña = "playlists";
				if (avisoNube) panel.append(nodo("p", "ajustes__ayuda", avisoNube));
				const invitaciones = await dato("invitations", () => social.invitations());
				pestañas(panel, invitaciones.length);
				if (pestaña === "users") await pintarUsuarios(panel);
				else if (pestaña === "playlists") await pintarPlaylists(panel);
				else if (pestaña === "invitations") pintarInvitaciones(panel, invitaciones);
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

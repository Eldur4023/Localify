/**
* Social — el perfil: cabecera (editable en el sitio, con recorte de foto), escaparate (playlists
* públicas y lo más escuchado) y la columna de ajustes (sincronización, privacidad, nube, sesión).
*
* Vive aparte de social.js pero comparte su estado por `ctx`: `estado` y `avisoNube` son getters
* (cambian de valor en social.js) y el resto son las piezas que ya tiene (dato, boton, seccion…).
*/
import { player, social } from "../ipc/client.js";
import { t } from "../i18n/index.js";
import { textoDeError } from "../ui/publicar-playlist.js";
import { mostrarError } from "../ui/error-overlay.js";
import { botonIcono, icono } from "../ui/icons.js";
import { ponerPortadaDePista } from "../ui/cards.js";
import { colorDePortada } from "../ui/color.js";
import { recortarImagen } from "../ui/recortar-imagen.js";
import { nodo } from "../ui/dom.js";
import { guardarCache, SIN_CACHE, tiempo } from "./social-cache.js";
export function crearPerfil(ctx) {
	const { dato, boton, seccion, pintar, cargar, claveDato, textoNube } = ctx;
	// ── Piezas del perfil ───────────────────────────────────────────────────
	const version = Date.now();
	let avatarV = 0;
	/** `tam`: true = grande (cabecera), "media" = tarjetas de personas, nada = fila. */
	function avatar(u, tam, alCargar) {
		const clase = tam === true ? " perfil__avatar--grande" : tam === "media" ? " perfil__avatar--media" : "";
		const a = nodo("span", `perfil__avatar${clase}`, (u.displayName || u.username || "?").slice(0, 1).toUpperCase());
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
	function cifra(n, etiqueta) {
		const c = nodo("div", "perfil__cifra");
		c.append(nodo("strong", "", String(n)), nodo("span", "", etiqueta));
		return c;
	}
	/** Píldora "escuchando ahora" con el ecualizador animado. */
	function pildoraAhora(np) {
		const ahora = nodo("div", "perfil__ahora");
		const barras = nodo("span", "perfil__barras");
		barras.append(nodo("i"), nodo("i"), nodo("i"));
		const texto = nodo("span", "perfil__ahora-texto");
		texto.append(nodo("small", "", t("social.now_playing")), nodo("strong", "", np.title), nodo("span", "", np.artist ?? ""));
		ahora.append(barras, texto);
		return ahora;
	}
	/** Cabecera ancha con el color de la foto. Con `propio`, nombre, bio y foto se editan en el sitio. */
	function cabeceraPerfil(panel, u, propio = false) {
		const c = nodo("header", `perfil__cabecera${propio ? " perfil__cabecera--propio" : ""}`);
		// el degradado se tiñe con el color dominante de la foto, como el reproductor
		const colorear = (img) => {
			const col = colorDePortada(img);
			if (col) c.style.setProperty("--perfil-color", col);
		};
		const datos = nodo("div", "perfil__datos");
		let av;
		if (propio) {
			av = nodo("button", "perfil__cambiar-foto");
			av.type = "button";
			av.setAttribute("aria-label", t("social.change_photo"));
			av.title = t("social.change_photo");
			const camara = nodo("span", "perfil__camara");
			camara.append(icono("camera", 18));
			av.append(avatar(u, true, colorear), camara);
			const foto = nodo("input");
			foto.type = "file";
			foto.accept = "image/*";
			foto.hidden = true;
			av.addEventListener("click", () => foto.click());
			foto.addEventListener("change", () => void (async () => {
				const elegida = foto.files?.[0];
				foto.value = "";
				if (!elegida) return;
				// recortar y hacer zoom antes de subirla
				const recorte = await recortarImagen(elegida);
				if (!recorte) return;
				const fd = new FormData();
				fd.append("imagen", recorte, "avatar.jpg");
				try {
					const r = await fetch("/api/social/avatar", { method: "POST", body: fd });
					if (!r.ok) throw new Error(String(r.status));
					avatarV = Date.now();
					await pintar();
				} catch (e) {
					mostrarError(t("error.internal"), textoDeError(e));
				}
			})());
			av.append(foto);
			const nombre = nodo("input", "perfil__nombre perfil__nombre-input");
			nombre.value = u.displayName;
			nombre.maxLength = 40;
			nombre.setAttribute("aria-label", t("social.display_name"));
			const bio = nodo("input", "perfil__bio perfil__bio-input");
			bio.value = u.bio ?? "";
			bio.maxLength = 160;
			bio.placeholder = t("social.bio_placeholder");
			bio.setAttribute("aria-label", t("social.bio"));
			const guardadoTxt = nodo("span", "perfil__guardado");
			let guardado = {
				nombre: u.displayName,
				bio: u.bio ?? ""
			};
			// se guarda al salir del campo o con Enter; Esc deshace
			const guardar = async () => {
				const n = nombre.value.trim() || guardado.nombre;
				nombre.value = n;
				if (n === guardado.nombre && bio.value === guardado.bio) return;
				try {
					await social.updateMe(n, bio.value);
					guardado = {
						nombre: n,
						bio: bio.value
					};
					u.displayName = n;
					u.bio = bio.value;
					guardarCache(claveDato(`user.${ctx.estado.username}`), u);
					guardadoTxt.textContent = t("social.saved");
				} catch (e) {
					guardadoTxt.textContent = textoDeError(e);
				}
				globalThis.setTimeout(() => { guardadoTxt.textContent = ""; }, 2000);
			};
			for (const campo of [nombre, bio]) {
				campo.addEventListener("blur", () => void guardar());
				campo.addEventListener("keydown", (e) => {
					if (e.key === "Enter") campo.blur();
					else if (e.key === "Escape") {
						campo.value = campo === nombre ? guardado.nombre : guardado.bio;
						campo.blur();
					}
				});
			}
			datos.append(nombre, nodo("span", "perfil__usuario", `@${u.username}`), bio, guardadoTxt);
		} else {
			av = avatar(u, true, colorear);
			datos.append(nodo("h2", "perfil__nombre", u.displayName), nodo("span", "perfil__usuario", `@${u.username}`));
			if (u.bio) datos.append(nodo("p", "perfil__bio", u.bio));
		}
		c.append(av, datos);
		const resumen = nodo("div", "perfil__resumen");
		const cifras = nodo("div", "perfil__cifras");
		cifras.append(cifra(u.playlists?.length ?? 0, t("social.playlists")), cifra(tiempo(u.listenedMs ?? 0), t("social.plays_90")), cifra(u.topTracks?.length ?? 0, t("social.top_short")));
		resumen.append(cifras);
		if (u.nowPlaying) resumen.append(pildoraAhora(u.nowPlaying));
		c.append(resumen);
		panel.append(c);
	}
	function tarjetaLista(p, subtitulo, etiquetas = []) {
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
		a.append(arte, nodo("strong", "", p.name), nodo("span", "social__sub", subtitulo));
		if (etiquetas.length > 0) {
			const e = nodo("span", "social__etiquetas");
			for (const x of etiquetas) e.append(nodo("span", "social__etiqueta", x));
			a.append(e);
		}
		return a;
	}
	function escaparate(panel, u, propio = false) {
		const listas = u.playlists ?? [];
		const { s: sp, cuerpo: cp } = seccion(t("social.public_playlists"));
		if (listas.length > 0) {
			const rejilla = nodo("div", "perfil__rejilla");
			for (const p of listas) rejilla.append(tarjetaLista(p, t("social.n_tracks", { count: p.nTracks })));
			cp.append(rejilla);
		} else if (propio) cp.append(nodo("p", "vista__empty", t("social.no_public")));
		if (listas.length > 0 || propio) panel.append(sp);
		const top = u.topTracks ?? [];
		if (top.length > 0 || propio) {
			const { s: st, cuerpo: ct } = seccion(t("social.top_tracks"));
			if (top.length === 0) ct.append(nodo("p", "vista__empty", t("social.top_empty")));
			const maximo = Math.max(...top.map((x) => x.ms), 1);
			top.forEach((x, i) => {
				const f = nodo("div", `perfil__pista${propio ? " perfil__pista--pulsable" : ""}`);
				const arte = nodo("span", "perfil__miniatura");
				arte.append(icono("music", 20));
				ponerPortadaDePista(arte, x.id);
				const texto = nodo("div", "social__texto");
				const barra = nodo("span", "perfil__barra");
				barra.style.setProperty("--ancho", `${Math.round(x.ms / maximo * 100)}%`);
				texto.append(nodo("strong", "", x.title), nodo("span", "social__sub", x.artist), barra);
				f.append(nodo("span", "perfil__puesto", String(i + 1)), arte, texto, nodo("span", "social__etiqueta", tiempo(x.ms)));
				if (propio) {
					// mi top se reproduce con un clic (si la pista está en este equipo)
					f.tabIndex = 0;
					f.setAttribute("role", "button");
					const tocar = () => void player.playTrack(x.id, { kind: "single" }).catch(() => {});
					f.addEventListener("click", tocar);
					f.addEventListener("keydown", (e) => {
						if (e.key === "Enter") tocar();
					});
				}
				ct.append(f);
			});
			panel.append(st);
		}
	}

	// ── Mi perfil: cabecera editable + escaparate + columna de ajustes ──────
	function tarjeta(titulo, nombreIcono) {
		const s = nodo("section", "social__card");
		const cab = nodo("h3", "social__card-titulo");
		if (nombreIcono) cab.append(icono(nombreIcono, 18));
		cab.append(document.createTextNode(titulo));
		const cuerpo = nodo("div", "social__card-cuerpo");
		s.append(cab, cuerpo);
		return {
			s,
			cuerpo
		};
	}
	/** Interruptor con título y ayuda; guarda al momento. */
	function filaInterruptor(titulo, ayuda, valor, alCambiar) {
		const f = nodo("label", "social__ajuste");
		const texto = nodo("span", "social__texto");
		texto.append(nodo("strong", "", titulo));
		if (ayuda) texto.append(nodo("span", "social__sub", ayuda));
		const i = nodo("input", "interruptor");
		i.type = "checkbox";
		i.checked = valor;
		i.addEventListener("change", () => void Promise.resolve(alCambiar(i.checked)).catch((e) => {
			i.checked = !i.checked;
			mostrarError(t("error.internal"), textoDeError(e));
		}));
		f.append(texto, i);
		return f;
	}
	/** "visto hace 5 min" */
	function haceTiempo(s) {
		const n = s < 3600 ? Math.max(1, Math.round(s / 60)) : s < 86400 ? Math.round(s / 3600) : Math.round(s / 86400);
		return t("social.seen_ago", { time: `${n} ${s < 3600 ? "min" : s < 86400 ? "h" : "d"}` });
	}
	async function tarjetaSincronizacion() {
		const { s, cuerpo } = tarjeta(t("social.sync_title"), "refresh");
		let info;
		try {
			info = await dato("sync", () => social.syncStatus());
		} catch (e) {
			if (e === SIN_CACHE) throw e;
			info = {
				enabled: false,
				mode: "both",
				deviceId: "",
				devices: []
			};
		}
		const pintarCuerpo = () => {
			cuerpo.replaceChildren();
			cuerpo.append(filaInterruptor(t("social.sync_enable"), t("social.sync_help"), info.enabled, async (v) => {
				await social.setSync({ enabled: v });
				// el móvil necesita el servicio en primer plano para seguir sincronizado en segundo plano
				if (v) globalThis.AndroidBridge?.startPlayback?.();
				info = await social.syncStatus();
				guardarCache(claveDato("sync"), info);
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
						guardarCache(claveDato("sync"), info);
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
					const quitar = botonIcono("trash", t("social.remove_device"), () => void social.removeDevice(d.deviceId).then(() => {
						info.devices = info.devices.filter((x) => x.deviceId !== d.deviceId);
						guardarCache(claveDato("sync"), info);
						pintarCuerpo();
					}).catch((e) => mostrarError(t("error.internal"), textoDeError(e))), { tamano: 18 });
					f.append(quitar);
				}
				cuerpo.append(f);
			}
		};
		pintarCuerpo();
		return s;
	}
	function tarjetaPrivacidadYNube() {
		const { s: sp, cuerpo: cp } = tarjeta(t("social.privacy"), "users");
		cp.append(filaInterruptor(t("social.presence"), t("social.presence_help"), ctx.estado.presence !== false, async (v) => {
			await social.setPresence(v);
			ctx.estado.presence = v;
		}));
		const { s: sn, cuerpo: cn } = tarjeta(t("social.cloud_title"), "cloud-download");
		cn.append(filaInterruptor(t("social.upload"), t("social.upload_help"), ctx.estado.upload, async (v) => {
			await social.setUpload(v);
			ctx.estado.upload = v;
		}));
		const avisoPull = nodo("p", "ajustes__ayuda", ctx.avisoNube);
		const progreso = nodo("p", "ajustes__ayuda");
		const acciones = nodo("div", "social__acciones");
		acciones.append(boton(t("social.cloud_pull"), "boton--sutil", async () => {
			avisoPull.textContent = t("social.pulling");
			avisoPull.textContent = textoNube(await social.pull());
		}, avisoPull), boton(t("social.contribute_library_run"), "boton--sutil", async () => {
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
		}, progreso));
		cn.append(acciones, avisoPull, progreso);
		return [sp, sn];
	}
	function tarjetaSesion(u) {
		const { s, cuerpo } = tarjeta(t("social.session"), "settings");
		cuerpo.append(nodo("p", "social__sub", `@${u.username} · ${ctx.estado.host}:${ctx.estado.port}`));
		const acciones = nodo("div", "social__acciones");
		acciones.append(boton(t("social.logout"), "boton--sutil", async () => {
			await social.logout();
			await cargar();
		}), boton(t("social.disconnect"), "boton--peligro", async () => {
			await social.disconnect();
			await cargar();
		}));
		cuerpo.append(acciones);
		return s;
	}
	async function pintarPerfil(panel) {
		const u = await dato(`user.${ctx.estado.username}`, () => social.user(ctx.estado.username));
		cabeceraPerfil(panel, u, true);
		const rejilla = nodo("div", "perfil__layout");
		const principal = nodo("div", "perfil__principal");
		const lateral = nodo("aside", "perfil__lateral");
		escaparate(principal, u, true);
		lateral.append(await tarjetaSincronizacion(), ...tarjetaPrivacidadYNube(), tarjetaSesion(u));
		rejilla.append(principal, lateral);
		panel.append(rejilla);
	}

	return {
		avatar,
		tarjetaLista,
		cabeceraPerfil,
		escaparate,
		pintarPerfil
	};
}

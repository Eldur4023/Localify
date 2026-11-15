/**
* Social — el perfil: cabecera (editable en el sitio, con recorte de foto) y escaparate (playlists
* públicas y lo más escuchado). Los ajustes de cuenta viven en Ajustes › Cuenta (cuenta.js).
*
* Vive aparte de social.js pero comparte su estado por `ctx`: `estado` y `avisoNube` son getters
* (cambian de valor en social.js) y el resto son las piezas que ya tiene (dato, boton, seccion…).
*/
import { player, social } from "../ipc/client.js";
import { t } from "../i18n/index.js";
import { textoDeError } from "../ui/publicar-playlist.js";
import { mostrarError } from "../ui/error-overlay.js";
import { icono } from "../ui/icons.js";
import { ponerPortadaDePista } from "../ui/cards.js";
import { colorDePortada } from "../ui/color.js";
import { recortarImagen } from "../ui/recortar-imagen.js";
import { nodo } from "../ui/dom.js";
import { guardarCache, tiempo } from "./social-cache.js";
import { abrirAjustes } from "./settings.js";
export function crearPerfil(ctx) {
	const { dato, seccion, pintar, claveDato } = ctx;
	// ── Piezas del perfil ───────────────────────────────────────────────────
	const version = Date.now();
	let avatarV = 0;
	let fondoV = 0;
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
	/** "Cambiar fondo" (recorte ancho) y "Quitar fondo" en la esquina de mi cabecera. */
	function botonesFondo(u) {
		const caja = nodo("div", "perfil__fondo-acciones");
		const archivo = nodo("input");
		archivo.type = "file";
		archivo.accept = "image/*";
		archivo.hidden = true;
		const cambiar = nodo("button", "boton boton--sutil");
		cambiar.type = "button";
		cambiar.append(icono("camera", 16), document.createTextNode(t("social.change_background")));
		cambiar.addEventListener("click", () => archivo.click());
		archivo.addEventListener("change", () => void (async () => {
			const elegida = archivo.files?.[0];
			archivo.value = "";
			if (!elegida) return;
			const recorte = await recortarImagen(elegida, 1600, 3.2);
			if (!recorte) return;
			const fd = new FormData();
			fd.append("imagen", recorte, "banner.jpg");
			try {
				const r = await fetch("/api/social/banner", { method: "POST", body: fd });
				if (!r.ok) throw new Error(String(r.status));
				fondoV = Date.now();
				u.hasBanner = true;
				guardarCache(claveDato(`user.${ctx.estado.username}`), u);
				await pintar();
			} catch (e) {
				mostrarError(t("error.internal"), textoDeError(e));
			}
		})());
		caja.append(cambiar, archivo);
		if (u.hasBanner) {
			const quitar = nodo("button", "boton boton--sutil", t("social.remove_background"));
			quitar.type = "button";
			quitar.addEventListener("click", () => void (async () => {
				try {
					const r = await fetch("/api/social/banner", { method: "DELETE" });
					if (!r.ok) throw new Error(String(r.status));
					u.hasBanner = false;
					guardarCache(claveDato(`user.${ctx.estado.username}`), u);
					await pintar();
				} catch (e) {
					mostrarError(t("error.internal"), textoDeError(e));
				}
			})());
			caja.append(quitar);
		}
		return caja;
	}
	/** Cabecera ancha con el color de la foto. Con `propio`, nombre, bio y foto se editan en el sitio. */
	function cabeceraPerfil(panel, u, propio = false) {
		const c = nodo("header", `perfil__cabecera${propio ? " perfil__cabecera--propio" : ""}`);
		// el degradado se tiñe con el color dominante de la foto, como el reproductor
		const colorear = (img) => {
			const col = colorDePortada(img);
			if (col) c.style.setProperty("--perfil-color", col);
		};
		// imagen de fondo elegida por la persona (la ven todos); sin ella, el degradado de siempre
		if (u.hasBanner) {
			c.classList.add("perfil__cabecera--fondo");
			c.style.setProperty("--perfil-fondo", `url("/api/social/img/banner/${encodeURIComponent(u.username)}?v=${fondoV || version}")`);
		}
		if (propio) c.append(botonesFondo(u));
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
		cifras.append(cifra(u.playlists?.length ?? 0, t("social.playlists")));
		if (u.listenedMs !== undefined) cifras.append(cifra(tiempo(u.listenedMs), t("social.plays_90")));
		if (u.topTracks) cifras.append(cifra(u.topTracks.length, t("social.top_short")));
		resumen.append(cifras);
		if (u.nowPlaying) resumen.append(pildoraAhora(u.nowPlaying));
		if (propio) {
			const ajustes = nodo("a", "boton boton--sutil perfil__ajustes", t("account.settings"));
			ajustes.href = "#/settings";
			ajustes.addEventListener("click", () => abrirAjustes("cuenta"));
			resumen.append(ajustes);
		}
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

	async function pintarPerfil(panel) {
		const u = await dato(`user.${ctx.estado.username}`, () => social.user(ctx.estado.username));
		cabeceraPerfil(panel, u, true);
		const principal = nodo("div", "perfil__principal");
		escaparate(principal, u, true);
		panel.append(principal);
	}

	return {
		avatar,
		tarjetaLista,
		cabeceraPerfil,
		escaparate,
		pintarPerfil
	};
}

import { library, settings as api, stats as statsApi } from "../ipc/client.js";
import { alRecibir } from "../ipc/events.js";
import { alCambiarIdioma, cambiarIdioma, t } from "../i18n/index.js";
import { conEspera } from "../ui/cards.js";
import { confirmar } from "../ui/dialogo.js";
import { mountEqualizer } from "../ui/equalizer.js";
import { mostrarError } from "../ui/error-overlay.js";
/**
* Navegadores cuyas cookies sabe leer yt-dlp.
*
* La lista la valida el backend —`NAVEGADORES` en el dominio— y aquí solo se
* pinta. Duplicarla es el precio de no inventar un comando para pedir ocho
* cadenas que no cambian nunca; si divergieran, guardar el ajuste fallaría con
* un error claro en vez de romper las descargas en silencio.
*/
const NAVEGADORES = [
	"firefox",
	"chrome",
	"chromium",
	"edge",
	"brave",
	"opera",
	"vivaldi",
	"safari"
];
/** Nombres tal como los conoce la gente, no como los llama yt-dlp. */
const ETIQUETAS_NAVEGADOR = {
	firefox: "Firefox",
	chrome: "Chrome",
	chromium: "Chromium",
	edge: "Edge",
	brave: "Brave",
	opera: "Opera",
	vivaldi: "Vivaldi",
	safari: "Safari"
};
/**
* Valor centinela del desplegable para «un fichero».
*
* No puede chocar con ningún navegador porque yt-dlp no acepta espacios en ese
* argumento, así que ninguno se llamará nunca así.
*/
const FICHERO = "::fichero::";
/** Un patch vacío: todas las secciones a `null`. */
function patchVacio() {
	return {
		language: null,
		metadataProvider: null,
		audio: null,
		download: null,
		integrations: null,
		ui: null
	};
}
/** Convierte la configuración de audio vigente en algo que se pueda enviar. */
function audioEnviable(s) {
	return {
		crossfadeMs: s.audio.crossfadeMs,
		gapless: s.audio.gapless,
		eqProfile: { ...s.audio.eqProfile },
		normalizeVolume: s.audio.normalizeVolume,
		outputDeviceId: s.audio.outputDeviceId
	};
}
/**
* Milisegundos de escucha en algo legible: "3 d 4 h", "4 h 12 min" o "12 min".
*
* Tres formatos y no uno solo con todas las unidades: "0 d 3 h 20 min" para
* alguien que lleva un rato escuchando dice más ruido que información. Cada
* franja muestra solo las dos unidades que importan a esa escala.
*/
function duracionEscucha(ms) {
	const totalMinutos = Math.floor(Number(ms) / 6e4);
	const dias = Math.floor(totalMinutos / (60 * 24));
	const horas = Math.floor(totalMinutos % (60 * 24) / 60);
	const minutos = totalMinutos % 60;
	if (dias > 0) return t("stats.duration_days", {
		days: dias,
		hours: horas
	});
	if (horas > 0) return t("stats.duration_hours", {
		hours: horas,
		minutes: minutos
	});
	return t("stats.duration_minutes", { minutes: minutos });
}
/** Bytes en algo legible. Base 1024, que es lo que enseña el explorador. */
function tamano(bytes) {
	const unidades = [
		"B",
		"KB",
		"MB",
		"GB",
		"TB"
	];
	let valor = Number(bytes);
	let i = 0;
	while (valor >= 1024 && i < unidades.length - 1) {
		valor /= 1024;
		i += 1;
	}
	return `${valor.toFixed(valor < 10 && i > 0 ? 1 : 0)} ${unidades[i]}`;
}
/**
* Nombre visible de un perfil de ecualizador.
*
* Los de fábrica llevan una clave i18n (`eq.flat`); los del usuario, su nombre
* literal. Traducir el segundo devolvería el centinela de clave ausente
* —`[Mi curva]`— así que el prefijo decide cuál es cuál, igual que hace
* `EqProfile::es_predefinido` en Rust.
*/
function nombrePerfil(p) {
	return p.nameKey.startsWith("eq.") ? t(p.nameKey) : p.nameKey;
}
/** Una pestaña de Ajustes. El orden aquí es el orden en que se pintan. */
const PESTAÑAS = [
	"general",
	"audio",
	"descargas",
	"integraciones",
	"stats",
	"almacenamiento"
];
/** Clave i18n del rótulo de cada pestaña. Reutiliza las de sus secciones —son
* el mismo texto— salvo las dos que antes no tenían una sola sección propia. */
const ETIQUETA_PESTAÑA = {
	general: "settings.tab_general",
	audio: "settings.audio",
	descargas: "settings.downloads",
	integraciones: "settings.tab_integrations",
	stats: "settings.tab_stats",
	almacenamiento: "settings.storage"
};
export function mountSettingsView(contenedor) {
	const el = document.createElement("section");
	el.className = "vista vista--scroll ajustes";
	contenedor.replaceChildren(el);
	let actual = null;
	let dispositivos = [];
	let perfiles = [];
	let escaneando = false;
	/** Avance de la copia, o `null` si no hay ninguna en curso. */
	let migrando = null;
	let ecualizador = null;
	let pestañaActiva = "general";
	// ── Piezas de formulario ────────────────────────────────────────────────
	function seccion(titulo) {
		const s = document.createElement("section");
		s.className = "ajustes__seccion";
		const h = document.createElement("h3");
		h.className = "ajustes__titulo";
		h.textContent = titulo;
		const cuerpo = document.createElement("div");
		cuerpo.className = "ajustes__cuerpo";
		s.append(h, cuerpo);
		return {
			el: s,
			cuerpo
		};
	}
	function campo(etiqueta, control, ayuda) {
		const fila = document.createElement("div");
		fila.className = "ajustes__campo";
		const l = document.createElement("label");
		l.className = "ajustes__etiqueta";
		l.textContent = etiqueta;
		// El `for` tiene que apuntar a algo etiquetable. Algunos controles vienen
		// envueltos en un contenedor —el deslizador trae su lectura al lado—, así
		// que se busca dentro: apuntar al envoltorio dejaría la etiqueta muerta,
		// sin foco al pulsarla y sin nombre accesible para el lector de pantalla.
		const objetivo = control.querySelector("input, select, textarea, button") ?? control;
		if (!objetivo.id) objetivo.id = `aj-${Math.random().toString(36).slice(2, 9)}`;
		l.htmlFor = objetivo.id;
		fila.append(l, control);
		if (ayuda) {
			const p = document.createElement("p");
			p.className = "ajustes__ayuda";
			p.textContent = ayuda;
			fila.append(p);
		}
		return fila;
	}
	function selector(opciones, valor, alCambiar) {
		const s = document.createElement("select");
		s.className = "ajustes__select";
		for (const o of opciones) {
			const op = document.createElement("option");
			op.value = o.valor;
			op.textContent = o.texto;
			s.append(op);
		}
		s.value = valor;
		s.addEventListener("change", () => alCambiar(s.value));
		return s;
	}
	/**
	* Lista numerada de instrucciones.
	*
	* Va aquí y no en un enlace a la documentación por un motivo concreto: quien
	* está en esta pantalla ya ha decidido configurarlo, y mandarle a leer a otro
	* sitio es donde se abandona. Son tres pasos; caben.
	*
	* Recibe **claves** y no textos: así el cambio de idioma repinta la lista sin
	* que este helper tenga que saber nada del idioma.
	*/
	function pasos(claves) {
		const ol = document.createElement("ol");
		ol.className = "ajustes__pasos";
		for (const clave of claves) {
			const li = document.createElement("li");
			li.textContent = t(`settings.${clave}`);
			ol.append(li);
		}
		return ol;
	}
	function interruptor(valor, alCambiar) {
		const c = document.createElement("input");
		c.type = "checkbox";
		c.className = "ajustes__check";
		c.checked = valor;
		c.addEventListener("change", () => alCambiar(c.checked));
		return c;
	}
	function deslizador(min, max, paso, valor, alSoltar, formato) {
		const caja = document.createElement("div");
		caja.className = "ajustes__deslizador";
		const r = document.createElement("input");
		r.type = "range";
		r.min = String(min);
		r.max = String(max);
		r.step = String(paso);
		r.value = String(valor);
		const salida = document.createElement("span");
		salida.className = "ajustes__valor";
		salida.textContent = formato(valor);
		// `input` actualiza el texto —hay que ver el valor mientras se arrastra— y
		// `change` es el que guarda: sin esa separación se enviaría un patch por
		// cada píxel del recorrido.
		r.addEventListener("input", () => {
			salida.textContent = formato(Number(r.value));
		});
		r.addEventListener("change", () => alSoltar(Number(r.value)));
		caja.append(r, salida);
		return caja;
	}
	// ── Guardado ────────────────────────────────────────────────────────────
	async function aplicar(patch) {
		try {
			actual = await api.patch(patch);
			pintar();
		} catch (e) {
			mostrarError(t("error.internal"), String(e));
			// Se repinta con lo último confiable: dejar el control en el valor
			// rechazado haría creer que se guardó.
			pintar();
		}
	}
	function guardarAudio(cambio) {
		if (!actual) return;
		void aplicar({
			...patchVacio(),
			audio: {
				...audioEnviable(actual),
				...cambio
			}
		});
	}
	/**
	* Igual que `guardarAudio`, pero sin el repintado completo de `aplicar()`.
	*
	* La usa el ecualizador al asentarse (`alAsentarse`, con rebote de 400 ms
	* mientras se arrastra una banda — ver `equalizer.ts`). Ese guardado puede
	* llegar con el ratón todavía sobre el deslizador si el gesto dura más de
	* los 400 ms sin soltar, y un `pintar()` completo destruye y recrea la
	* pestaña de Audio entera, arrancando el arrastre en marcha de debajo del
	* cursor. No hace falta: el ecualizador ya mantiene su curva y el selector
	* de perfil sincronizados por su cuenta (ver el `mostrar()` de más abajo).
	*/
	async function guardarAudioSilencioso(cambio) {
		if (!actual) return;
		try {
			actual = await api.patch({
				...patchVacio(),
				audio: {
					...audioEnviable(actual),
					...cambio
				}
			});
		} catch (e) {
			mostrarError(t("error.internal"), String(e));
			pintar();
		}
	}
	/**
	* Igual que `guardarAudio`, para la sección de descargas.
	*
	* Devuelve la promesa porque quien elige un fichero de cookies tiene que
	* esperar a que esté guardado antes de repintar: si no, la pantalla se dibuja
	* con el ajuste anterior y parece que el selector no hizo nada.
	*/
	async function guardarDescargas(cambio) {
		if (!actual) return;
		await aplicar({
			...patchVacio(),
			download: {
				...actual.download,
				...cambio
			}
		});
	}
	/** Botón de acción con su estado de carga y su error a la vista. */
	function boton(etiqueta, accion) {
		const b = document.createElement("button");
		b.type = "button";
		b.className = "boton";
		b.textContent = etiqueta;
		b.addEventListener("click", () => {
			void (async () => {
				b.disabled = true;
				b.textContent = t("common.loading");
				try {
					await accion();
				} catch (e) {
					mostrarError(t("error.internal"), String(e));
				}
				b.disabled = false;
				b.textContent = etiqueta;
			})();
		});
		return b;
	}
	/**
	* Elige carpeta y pregunta qué hacer con lo que ya hay.
	*
	* La pregunta no se puede evitar: mover la música y dejarla donde está son
	* dos operaciones distintas con consecuencias distintas, y elegir por el
	* usuario significaría o copiar decenas de gigabytes que no pidió, o dejarle
	* media biblioteca inaccesible.
	*/
	async function pedirCarpeta() {
		const elegida = await api.pickFolder();
		// Cancelar no es un error ni merece mensaje.
		if (!elegida || elegida === actual?.libraryPath) return;
		const mover = await confirmar(t("settings.move_confirm", { path: elegida }));
		// El backend mueve los archivos de forma síncrona dentro de esta misma
		// petición (no hay job en segundo plano que reporte progreso real), así
		// que el indicador solo cubre lo que dura el propio await.
		if (mover) {
			migrando = true;
			pintar();
		}
		try {
			await api.changeLibraryPath(elegida, mover);
		} catch (e) {
			mostrarError(t("settings.move_failed"), String(e));
		} finally {
			migrando = null;
			if (pestañaActiva === "general") pintar();
		}
	}
	/** Fila de pestañas. Cambiar de pestaña no vuelve a pedir nada al backend
	* —los ajustes ya están en memoria—, salvo Estadísticas, que se refresca
	* cada vez que se entra: es la única pestaña cuyos números cambian sin que
	* el usuario haya tocado nada aquí. */
	function barraPestañas() {
		const nav = document.createElement("div");
		nav.className = "ajustes__tabs";
		nav.setAttribute("role", "tablist");
		for (const clave of PESTAÑAS) {
			const boton = document.createElement("button");
			boton.type = "button";
			boton.className = "ajustes__tab";
			boton.classList.toggle("is-activa", clave === pestañaActiva);
			boton.setAttribute("role", "tab");
			boton.setAttribute("aria-selected", String(clave === pestañaActiva));
			boton.textContent = t(ETIQUETA_PESTAÑA[clave]);
			boton.addEventListener("click", () => {
				if (pestañaActiva === clave) return;
				pestañaActiva = clave;
				pintar();
			});
			nav.append(boton);
		}
		return nav;
	}
	// ── Pintado ─────────────────────────────────────────────────────────────
	function pintar() {
		el.replaceChildren();
		if (!actual) return;
		const s = actual;
		// El ecualizador guarda lo que tenga pendiente al desmontarse (mover un
		// deslizador y cambiar de pestaña sin soltar no puede perder el ajuste),
		// así que se destruye aquí y no solo al salir de Ajustes entero.
		if (pestañaActiva !== "audio" && ecualizador) {
			ecualizador.destroy();
			ecualizador = null;
		}
		el.append(barraPestañas());
		const panel = document.createElement("div");
		panel.className = "ajustes__panel";
		el.append(panel);
		if (pestañaActiva === "stats") {
			pintarStats(panel);
			return;
		}
		// General
		if (pestañaActiva === "general") {
			const { el: bloque, cuerpo } = seccion(t("settings.tab_general"));
			cuerpo.append(campo(t("settings.language"), selector([{
				valor: "es",
				texto: t("settings.language.es")
			}, {
				valor: "en",
				texto: t("settings.language.en")
			}], s.language, (v) => {
				// El idioma se cambia en la interfaz de inmediato y se persiste
				// después: esperar a la respuesta dejaría la pantalla en el
				// idioma viejo durante el viaje de ida y vuelta.
				cambiarIdioma(v);
				void aplicar({
					...patchVacio(),
					language: v
				});
			})));
			cuerpo.append(campo(
				t("settings.provider"),
				selector([
					{
						valor: "combinado",
						texto: t("settings.provider.combinado")
					},
					{
						valor: "ytmusic",
						texto: t("settings.provider.ytmusic")
					},
					{
						valor: "musicbrainz",
						texto: t("settings.provider.musicbrainz")
					},
					{
						valor: "spotify",
						texto: t("settings.provider.spotify")
					}
				], s.metadataProvider, (v) => void aplicar({
					...patchVacio(),
					metadataProvider: v
				})),
				// Elegir Spotify sin credenciales deja la búsqueda muda, así que se
				// dice aquí y no cuando el usuario ya está buscando sin resultados.
				s.metadataProvider === "spotify" && !s.spotify.configured ? t("provider.not_configured") : t("settings.provider_help")
			));
			// El backend puede no saber todavía dónde está: una fila con la etiqueta
			// y nada al lado parece un fallo de pintado, no un dato ausente.
			const ruta = document.createElement("code");
			ruta.className = "ajustes__ruta";
			ruta.textContent = s.libraryPath.length > 0 ? s.libraryPath : t("settings.folder_unknown");
			const cambiar = document.createElement("button");
			cambiar.type = "button";
			cambiar.className = "boton boton--sutil";
			cambiar.textContent = t("settings.change_folder");
			cambiar.disabled = migrando !== null;
			cambiar.addEventListener("click", () => void pedirCarpeta());
			const caja = document.createElement("div");
			caja.className = "ajustes__carpeta";
			caja.append(ruta, cambiar);
			cuerpo.append(campo(t("settings.library_folder"), caja));
			// Mientras la copia corre se dice cuánto lleva. Cambiar de carpeta es la
			// única operación de la aplicación que puede durar minutos y bloquear
			// otra igual, así que es la única que muestra progreso explícito.
			if (migrando) {
				const aviso = document.createElement("p");
				aviso.className = "ajustes__ayuda";
				aviso.textContent = t("settings.moving");
				cuerpo.append(aviso);
			}
			panel.append(bloque);
		}
		// Audio
		if (pestañaActiva === "audio") {
			const { el: bloque, cuerpo } = seccion(t("settings.audio"));
			// Crossfade a cero significa reproducción sin huecos, no "sin nada": son
			// el mismo ajuste visto de dos maneras, y el backend espera que viajen
			// coherentes.
			const alSoltarFundido = (v) => {
				guardarAudio({
					crossfadeMs: v,
					gapless: v === 0
				});
			};
			const textoFundido = (v) => v === 0 ? t("settings.crossfade_off") : `${(v / 1e3).toFixed(1)} s`;
			cuerpo.append(campo(t("settings.crossfade"), deslizador(0, 12e3, 500, s.audio.crossfadeMs, alSoltarFundido, textoFundido)));
			const elegirPerfil = selector(perfiles.map((p) => ({
				valor: p.id,
				texto: nombrePerfil(p)
			})), s.audio.eqProfile.id, (v) => {
				const perfil = perfiles.find((p) => p.id === v);
				if (!perfil) return;
				guardarAudio({ eqProfile: { ...perfil } });
				ecualizador?.mostrar(perfil);
			});
			cuerpo.append(campo(t("settings.equalizer"), elegirPerfil));
			// El editor va debajo del selector y no dentro de un desplegable: la
			// curva es la explicación de lo que hace el perfil elegido, y esconderla
			// deja el selector como una lista de nombres sin significado.
			ecualizador?.destroy();
			ecualizador = mountEqualizer(cuerpo, {
				inicial: s.audio.eqProfile,
				// Se aplica en cada movimiento —el motor cambia coeficientes sin
				// cortar— y se persiste cuando la mano se detiene.
				alCambiar: (p) => {
					void api.previewEq(p).catch(() => {
						// Un fallo aquí no puede interrumpir el arrastre; el guardado
						// asentado lo reintentará y ahí sí se avisa.
					});
				},
				alAsentarse: (p) => {
					void guardarAudioSilencioso({ eqProfile: { ...p } });
					// La lista no tenía "personalizado" hasta ahora: si no se añade, el
					// selector se queda mostrando el perfil de fábrica que se acaba de
					// dejar de usar.
					if (!perfiles.some((x) => x.id === p.id)) {
						perfiles = [...perfiles, p];
						const op = document.createElement("option");
						op.value = p.id;
						op.textContent = nombrePerfil(p);
						elegirPerfil.append(op);
					}
					elegirPerfil.value = p.id;
				}
			});
			cuerpo.append(campo(t("settings.normalize"), interruptor(s.audio.normalizeVolume, (v) => guardarAudio({ normalizeVolume: v }))));
			cuerpo.append(campo(t("settings.device"), selector([{
				valor: "",
				texto: t("settings.device_default")
			}, ...dispositivos.map((d) => ({
				valor: d.id,
				texto: d.name
			}))], s.audio.outputDeviceId ?? "", (v) => guardarAudio({ outputDeviceId: v === "" ? null : v }))));
			panel.append(bloque);
		}
		// Descargas
		//
		// ## Por qué esta sección existe
		//
		// YouTube pide cada vez más «Sign in to confirm you're not a bot», y contra
		// eso no hay reintento que valga: es una puerta cerrada, no un fallo
		// pasajero. Las dos únicas cosas que la abren son unas cookies de sesión y
		// un yt-dlp reciente, y hasta ahora ninguna de las dos se podía tocar desde
		// la aplicación.
		if (pestañaActiva === "descargas") {
			const { el: bloque, cuerpo } = seccion(t("settings.downloads"));
			const ayuda = document.createElement("p");
			ayuda.className = "ajustes__ayuda";
			ayuda.textContent = t("settings.cookies_help");
			cuerpo.append(ayuda);
			// El desplegable lista los navegadores y una entrada más para el fichero.
			// Van en el mismo control porque son la misma decisión —de dónde salen
			// las cookies— y separarlos en dos obligaría a explicar cuál manda.
			const opciones = [
				{
					valor: "",
					texto: t("settings.cookies_none")
				},
				...NAVEGADORES.map((n) => ({
					valor: n,
					texto: ETIQUETAS_NAVEGADOR[n] ?? n
				})),
				{
					valor: FICHERO,
					texto: t("settings.cookies_file")
				}
			];
			const elegido = s.download.cookiesFile ? FICHERO : s.download.cookiesBrowser ?? "";
			const selectorCookies = selector(opciones, elegido, (v) => {
				if (v === FICHERO) {
					// Se pide el fichero **antes** de guardar: guardar «fichero» sin ruta
					// dejaría el ajuste en un estado que no significa nada.
					void (async () => {
						const ruta = await api.pickCookies();
						if (ruta) {
							await guardarDescargas({
								cookiesBrowser: null,
								cookiesFile: ruta
							});
						}
						pintar();
					})();
					return;
				}
				void guardarDescargas({
					cookiesBrowser: v === "" ? null : v,
					cookiesFile: null
				});
			});
			cuerpo.append(campo(t("settings.cookies"), selectorCookies));
			if (s.download.cookiesFile) {
				const ruta = document.createElement("p");
				ruta.className = "ajustes__ayuda";
				ruta.textContent = s.download.cookiesFile;
				cuerpo.append(ruta);
			}
			// El aviso solo aparece con el navegador elegido, que es cuando aplica:
			// `--cookies-from-browser` lee el almacén entero, no solo YouTube.
			if (s.download.cookiesBrowser) {
				const aviso = document.createElement("p");
				aviso.className = "ajustes__ayuda ajustes__ayuda--aviso";
				aviso.textContent = t("settings.cookies_warning");
				cuerpo.append(aviso);
			}
			const resultado = document.createElement("p");
			resultado.className = "ajustes__ayuda";
			const comprobar = boton(t("settings.cookies_test"), async () => {
				const r = await api.testCookies();
				resultado.textContent = r.detail ? `${t(r.messageKey)} — ${r.detail}` : t(r.messageKey);
				resultado.classList.toggle("ajustes__ayuda--aviso", !r.ok);
			});
			const actualizar = boton(t("settings.ytdlp_update"), async () => {
				const r = await api.updateYtdlp();
				resultado.textContent = r.detail ? `${t(r.messageKey)} — ${r.detail}` : t(r.messageKey);
				resultado.classList.toggle("ajustes__ayuda--aviso", !r.ok);
			});
			const acciones = document.createElement("div");
			acciones.className = "ajustes__acciones";
			acciones.append(comprobar, actualizar);
			cuerpo.append(acciones, resultado);
			const nota = document.createElement("p");
			nota.className = "ajustes__ayuda";
			nota.textContent = t("settings.ytdlp_help");
			cuerpo.append(nota);
			panel.append(bloque);
		}
		// Integraciones: Spotify y Discord comparten pestaña porque las dos son
		// "presta tus propias credenciales a un servicio externo" — la misma
		// decisión, con el mismo motivo (ADR: incrustarlas en el binario las
		// volvería compartidas por todo el mundo).
		// Spotify
		if (pestañaActiva === "integraciones") {
			const { el: bloque, cuerpo } = seccion(t("settings.spotify"));
			const ayuda = document.createElement("p");
			ayuda.className = "ajustes__ayuda";
			ayuda.textContent = t("settings.spotify_help");
			cuerpo.append(ayuda);
			const id = document.createElement("input");
			id.type = "text";
			id.className = "ajustes__input";
			id.autocomplete = "off";
			id.value = s.spotify.clientId ?? "";
			const secreto = document.createElement("input");
			secreto.type = "password";
			secreto.className = "ajustes__input";
			secreto.autocomplete = "off";
			// Nunca se rellena: el backend no lo devuelve y fingir que sí —con
			// puntos de relleno— haría creer que dejarlo así lo conserva.
			secreto.placeholder = s.spotify.configured ? "••••••••" : "";
			const estado = document.createElement("span");
			estado.className = "ajustes__estado";
			estado.textContent = s.spotify.configured ? t("settings.saved") : "";
			const guardar = document.createElement("button");
			guardar.type = "button";
			guardar.className = "boton";
			guardar.textContent = t("settings.save");
			guardar.addEventListener("click", () => {
				void (async () => {
					try {
						const r = await api.setSpotifyCredentials(id.value.trim(), secreto.value);
						secreto.value = "";
						estado.textContent = r.state === "ready" ? t("settings.saved") : r.state === "unavailable" ? t(r.reasonKey) : t("provider.not_configured");
						actual = await api.get();
					} catch (e) {
						estado.textContent = t("error.invalid");
						mostrarError(t("error.invalid"), String(e));
					}
				})();
			});
			cuerpo.append(campo(t("settings.client_id"), id), campo(t("settings.client_secret"), secreto));
			const acciones = document.createElement("div");
			acciones.className = "ajustes__acciones";
			acciones.append(guardar, estado);
			cuerpo.append(acciones);
			panel.append(bloque);
		}
		// Discord
		//
		// Pide credenciales de una aplicación registrada por el usuario, por el
		// mismo motivo que Spotify: incrustarlas en el binario las convertiría en
		// credenciales compartidas por todo el mundo, sacables del ejecutable con
		// un editor de texto.
		if (pestañaActiva === "integraciones") {
			const { el: bloque, cuerpo } = seccion(t("settings.discord_section"));
			cuerpo.append(pasos([
				"discord_step_1",
				"discord_step_2",
				"discord_step_3"
			]));
			const abrirDiscord = document.createElement("button");
			abrirDiscord.type = "button";
			abrirDiscord.className = "boton boton--sutil";
			abrirDiscord.textContent = t("settings.open_discord_apps");
			abrirDiscord.addEventListener("click", () => {
				void api.openExternal("discord_apps").catch((e) => {
					mostrarError(t("error.internal"), String(e));
				});
			});
			const irADiscord = document.createElement("div");
			irADiscord.className = "ajustes__acciones";
			irADiscord.append(abrirDiscord);
			cuerpo.append(irADiscord);
			const discordId = document.createElement("input");
			discordId.type = "text";
			discordId.className = "ajustes__input";
			discordId.autocomplete = "off";
			discordId.value = s.integrations.discordClientId ?? "";
			discordId.placeholder = "000000000000000000";
			// Al salir del campo y no en cada tecla: guardar por pulsación sería una
			// escritura en disco por carácter de un identificador de dieciocho.
			discordId.addEventListener("change", () => {
				const puesto = discordId.value.trim();
				void aplicar({
					...patchVacio(),
					integrations: {
						...s.integrations,
						discordClientId: puesto.length > 0 ? puesto : null
					}
				});
			});
			cuerpo.append(campo(t("settings.discord"), interruptor(s.integrations.discordEnabled, (v) => void aplicar({
				...patchVacio(),
				integrations: {
					...s.integrations,
					discordEnabled: v
				}
			})), t("settings.discord_help")), campo(t("settings.discord_client_id"), discordId, t("settings.discord_id_help")));
			panel.append(bloque);
		}
		// Almacenamiento
		//
		// Es el único sitio donde el disco es el tema, así que es el único donde
		// tiene sentido contar cuántas canciones están guardadas y cuánto ocupan.
		// En las listas no lo tiene: allí lo que importa es la canción.
		if (pestañaActiva === "almacenamiento") {
			const { el: bloque, cuerpo } = seccion(t("settings.storage"));
			const revisar = document.createElement("button");
			revisar.type = "button";
			revisar.className = "boton";
			revisar.disabled = escaneando;
			revisar.textContent = escaneando ? t("common.loading") : t("settings.scan");
			revisar.addEventListener("click", () => {
				escaneando = true;
				revisar.disabled = true;
				revisar.textContent = t("common.loading");
				void library.rescan().catch((e) => {
					// Si ni siquiera arrancó, el evento de fin no va a llegar nunca: hay
					// que devolver el botón a su sitio aquí o se queda muerto.
					escaneando = false;
					pintar();
					mostrarError(t("error.internal"), String(e));
				});
			});
			// ── Importar canciones propias ────────────────────────────────────
			//
			// A diferencia de «Revisar biblioteca» (que solo recupera ficheros de
			// pistas que el catálogo ya conoce), esto da de alta pistas nuevas a
			// partir de ficheros que no tienen ni tendrán equivalente remoto.
			const importar = document.createElement("button");
			importar.type = "button";
			importar.className = "boton";
			importar.textContent = t("settings.import");
			importar.addEventListener("click", () => {
				void (async () => {
					const elegidos = await library.pickImportFiles();
					if (elegidos.length === 0) return;
					importar.disabled = true;
					importar.textContent = t("common.loading");
					try {
						const informe = await library.importFiles(elegidos);
						mostrarError(t("settings.import_done", {
							imported: String(informe.imported),
							selected: String(informe.filesSelected)
						}), "");
						await refrescarUso();
					} catch (e) {
						mostrarError(t("settings.import_failed"), String(e));
					}
					importar.disabled = false;
					importar.textContent = t("settings.import");
				})();
			});
			// ── Borrar todo lo descargado ─────────────────────────────────────
			//
			// Es la única acción destructiva de la pantalla, y por eso es la única
			// roja. El color no decora: dice "esto no es como los demás botones".
			const vaciar = document.createElement("button");
			vaciar.type = "button";
			vaciar.className = "boton boton--peligro";
			vaciar.textContent = t("settings.wipe");
			vaciar.addEventListener("click", () => {
				void (async () => {
					// Confirmación con su propio botón, como pediste. El texto dice qué
					// se va **y qué se queda**: sin eso, "borrar todo" se lee como
					// "pierdo mis playlists" y nadie lo pulsa.
					const seguro = await confirmar(t("settings.wipe"), t("settings.wipe_do"), t("settings.wipe_confirm"));
					if (!seguro) return;
					try {
						const n = await library.wipeDownloads();
						mostrarError(t("settings.wipe_done", { count: String(n) }), "");
					} catch (e) {
						mostrarError(t("error.internal"), String(e));
					}
				})();
			});
			const uso = document.createElement("p");
			uso.className = "ajustes__ayuda";
			// ── Descargas fallidas ────────────────────────────────────────────
			//
			// Este es el único sitio de la aplicación donde un fallo de descarga se
			// ve. Las descargas son invisibles por diseño y las listas no dicen si
			// una canción está en disco, así que una que no se pudo emparejar
			// simplemente no sonaba, sin explicación y sin marcha atrás.
			//
			// Solo aparece si hay alguna. Un "0 canciones fallidas" permanente sería
			// invitar a preocuparse por algo que no pasa.
			const fallidas = document.createElement("p");
			fallidas.className = "ajustes__ayuda";
			fallidas.hidden = true;
			const reintentar = document.createElement("button");
			reintentar.type = "button";
			reintentar.className = "boton";
			reintentar.textContent = t("common.retry");
			reintentar.hidden = true;
			reintentar.addEventListener("click", () => {
				void (async () => {
					reintentar.disabled = true;
					reintentar.textContent = t("common.loading");
					try {
						const n = await library.retryFailed();
						mostrarError(t("settings.retry_done", { count: String(n) }), "");
						await refrescarUso();
					} catch (e) {
						mostrarError(t("error.internal"), String(e));
					}
					reintentar.disabled = false;
					reintentar.textContent = t("common.retry");
				})();
			});
			async function refrescarUso() {
				try {
					const st = await library.stats();
					uso.textContent = t("settings.storage_used", {
						tracks: String(st.localCount),
						size: tamano(st.totalBytes)
					});
					const hay = st.failedCount > 0;
					fallidas.hidden = !hay;
					reintentar.hidden = !hay;
					if (hay) {
						fallidas.textContent = t("settings.failed", { count: String(st.failedCount) });
					}
				} catch {
					uso.textContent = "";
				}
			}
			void refrescarUso();
			const acciones = document.createElement("div");
			acciones.className = "ajustes__acciones";
			acciones.append(revisar, importar, reintentar);
			// El destructivo va en su propia fila, separado del de revisar: pegados,
			// el rojo se convierte en "el botón de al lado" y se pulsa por inercia.
			const peligro = document.createElement("div");
			peligro.className = "ajustes__acciones ajustes__acciones--peligro";
			peligro.append(vaciar);
			cuerpo.append(acciones, uso, fallidas, peligro);
			panel.append(bloque);
		}
	}
	/**
	* Fila de un ranking (canción o artista más escuchado): puesto, nombre y un
	* detalle a la derecha con el tiempo y, si aplica, las reproducciones.
	*/
	function filaRanking(puesto, nombre, subtitulo, detalle) {
		const fila = document.createElement("div");
		fila.className = "stats__fila";
		const n = document.createElement("span");
		n.className = "stats__puesto";
		n.textContent = String(puesto);
		const centro = document.createElement("div");
		centro.className = "stats__nombre";
		const titulo = document.createElement("span");
		titulo.textContent = nombre;
		centro.append(titulo);
		if (subtitulo) {
			const sub = document.createElement("span");
			sub.className = "stats__subtitulo";
			sub.textContent = subtitulo;
			centro.append(sub);
		}
		const d = document.createElement("span");
		d.className = "stats__detalle";
		d.textContent = detalle;
		fila.append(n, centro, d);
		return fila;
	}
	/**
	* Pestaña de Estadísticas.
	*
	* Se pide siempre que se entra, a diferencia del resto de la pantalla: es la
	* única pestaña cuyos números cambian sin que el usuario haya tocado nada
	* aquí —escuchar música en otra vista los mueve— así que enseñar lo que se
	* pidió la última vez estaría, tarde o temprano, mintiendo.
	*/
	let cuerpoStats = null;
	let refrescandoStats = false;
	function pintarStats(destino) {
		const { el: bloque, cuerpo } = seccion(t("settings.tab_stats"));
		destino.append(bloque);
		cuerpoStats = cuerpo;
		void conEspera(cuerpo, statsApi.get()).then((r) => pintarContenidoStats(cuerpo, r)).catch((e) => {
			mostrarError(t("error.internal"), String(e));
		});
	}
	/**
	 * Mientras la pestaña está a la vista, cada escucha que avanza
	 * (`statsChanged`, cada ~5 s sonando) la repinta en su sitio: sin la
	 * espera de la primera carga, que haría parpadear la pantalla.
	 */
	function refrescarStats() {
		const cuerpo = cuerpoStats;
		if (pestañaActiva !== "stats" || !cuerpo?.isConnected || refrescandoStats) return;
		refrescandoStats = true;
		void statsApi.get().then((r) => {
			if (cuerpoStats !== cuerpo || !cuerpo.isConnected) return;
			cuerpo.replaceChildren();
			pintarContenidoStats(cuerpo, r);
		}).catch(() => {}).finally(() => {
			refrescandoStats = false;
		});
	}
	function pintarContenidoStats(cuerpo, r) {
		if (r.totalPlays === 0n) {
			const vacio = document.createElement("p");
			vacio.className = "ajustes__ayuda";
			vacio.textContent = t("stats.empty");
			cuerpo.append(vacio);
			return;
		}
		// ── Resumen ─────────────────────────────────────────────────────────
		const resumen = document.createElement("div");
		resumen.className = "stats__resumen";
		const destacar = (numero, etiqueta) => {
			const caja = document.createElement("div");
			caja.className = "stats__destacado";
			const n = document.createElement("span");
			n.className = "stats__numero";
			n.textContent = numero;
			const e = document.createElement("span");
			e.className = "stats__etiqueta";
			e.textContent = etiqueta;
			caja.append(n, e);
			return caja;
		};
		resumen.append(destacar(duracionEscucha(r.totalMsPlayed), t("stats.total_time")), destacar(String(r.totalPlays), t("stats.plays", { count: Number(r.totalPlays) })), destacar(String(r.distinctTracks), t("stats.distinct_tracks", { count: Number(r.distinctTracks) })));
		cuerpo.append(resumen);
		// ── Canciones más escuchadas ────────────────────────────────────────
		if (r.topTracks.length > 0) {
			const titulo = document.createElement("h4");
			titulo.className = "ajustes__titulo";
			titulo.textContent = t("stats.top_tracks");
			const lista = document.createElement("div");
			lista.className = "stats__lista";
			r.topTracks.forEach((fila, i) => {
				lista.append(filaRanking(i + 1, fila.track.title, fila.track.artistDisplay, duracionEscucha(fila.msPlayed)));
			});
			cuerpo.append(titulo, lista);
		}
		// ── Artistas más escuchados ─────────────────────────────────────────
		if (r.topArtists.length > 0) {
			const titulo = document.createElement("h4");
			titulo.className = "ajustes__titulo";
			titulo.textContent = t("stats.top_artists");
			const lista = document.createElement("div");
			lista.className = "stats__lista";
			r.topArtists.forEach((fila, i) => {
				lista.append(filaRanking(i + 1, fila.artist.name, "", duracionEscucha(fila.msPlayed)));
			});
			cuerpo.append(titulo, lista);
		}
	}
	// El escaneo lo lanza esta vista pero lo termina el backend: sin escuchar el
	// evento, el botón se quedaría deshabilitado para siempre.
	//
	// Se atiende también a `libraryChanged` porque un escaneo que no encuentra
	// nada que hacer puede no emitir un último `scanProgress`, y esperar a un
	// evento que no llega deja el botón muerto hasta recargar.
	const dejarEventos = alRecibir((evento) => {
		// La condición empieza por `escaneando` a propósito: sin ella,
		// `libraryChanged` —que también emite una descarga al terminar— repintaría
		// los ajustes enteros mientras el usuario los está tocando.
		const terminado = escaneando && (evento.type === "scanProgress" && evento.done >= evento.total || evento.type === "libraryChanged");
		if (terminado) {
			escaneando = false;
			pintar();
		}
		if (evento.type === "libraryPathChanged") {
			migrando = null;
			void api.get().then((s) => {
				actual = s;
				if (pestañaActiva === "general") pintar();
			});
			return;
		}
		if (evento.type === "statsChanged") {
			refrescarStats();
			return;
		}
		if (evento.type === "settingsChanged") {
			void api.get().then((s) => {
				actual = s;
				pintar();
			});
		}
	});
	void conEspera(el, Promise.all([
		api.get(),
		api.audioDevices(),
		api.eqProfiles()
	])).then(([s, d, p]) => {
		actual = s;
		dispositivos = d;
		perfiles = p;
		pintar();
	}).catch((e) => {
		mostrarError(t("error.internal"), String(e));
	});
	const dejarIdioma = alCambiarIdioma(pintar);
	return { destroy() {
		dejarIdioma();
		dejarEventos();
		// El ecualizador guarda lo que tenga pendiente al desmontarse: salir de
		// Ajustes justo tras mover un deslizador no puede tirar el ajuste.
		ecualizador?.destroy();
		ecualizador = null;
		el.remove();
	} };
}

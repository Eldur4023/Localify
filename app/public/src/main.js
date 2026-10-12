/**
* Punto de entrada del frontend: monta el armazón —barra lateral, barra superior y
* reproductor— y cede el contenido al router. Pinta datos y emite comandos; las
* decisiones viven en el backend. Módulos ES nativos, sin bundler.
*
* El armazón no se desmonta: solo cambia el contenido. La barra de reproducción vive
* fuera del router (desmontarla cortaría la música) y la lateral para no pedir las
* playlists en cada navegación.
*/
import { iniciar } from "./ipc/events.js";
import { arrancarAudioWeb } from "./shell/web-audio.js";
import { crearRouter } from "./router.js";
import { mountAnchoLateral } from "./shell/ancho-lateral.js";
import { mountSidebar } from "./shell/sidebar.js";
import { mountTopBar } from "./shell/topbar.js";
import { mountPlayerBar } from "./shell/player.js";
import { mountQueuePanel } from "./shell/queue-panel.js";
import { mountNowPlaying } from "./shell/now-playing.js";
import { mountTabs } from "./shell/tabs.js";
import { mountGestos } from "./shell/gestos.js";
import { mountAlbumView } from "./views/album.js";
import { mountArtistView } from "./views/artist.js";
import { mountHomeView } from "./views/home.js";
import { mountLibraryView } from "./views/library.js";
import { mountLikedView } from "./views/liked.js";
import { mountPlaylistView } from "./views/playlist.js";
import { mountSearchView } from "./views/search.js";
import { mountSettingsView } from "./views/settings.js";
import { mountSocialView } from "./views/social.js";
import { alCambiarIdioma, t } from "./i18n/index.js";
import { instalarCapturaDeErrores, mostrarError } from "./ui/error-overlay.js";
/**
* Quita el menú contextual del navegador donde no hay uno propio.
*
* Localify es una aplicación de escritorio; que un clic derecho en cualquier
* hueco saque "Recargar", "Ver código fuente" o "Inspeccionar" delata que
* debajo hay un WebView y ofrece acciones que no significan nada aquí. Además
* el texto no es seleccionable fuera de los campos (ver `base.css`), así que ni
* siquiera aparece "Copiar": el menú sale prácticamente vacío.
*
* ## Donde sí se deja
*
* En los campos de texto. Ahí el menú nativo trae cortar, copiar, pegar y las
* sugerencias del corrector, que son de verdad útiles y que reimplementar
* costaría mucho más de lo que valen.
*
* ## Por qué en la fase de burbuja
*
* Los menús propios llaman a `preventDefault` en su propio manejador. Este
* corre después, y volver a llamarlo sobre un evento ya cancelado no hace nada:
* no hay que distinguir entre "ya lo maneja alguien" y "no lo maneja nadie".
*/
function quitarMenuDelNavegador() {
	document.addEventListener("contextmenu", (e) => {
		const destino = e.target;
		if (!(destino instanceof HTMLElement)) {
			e.preventDefault();
			return;
		}
		const editable = destino.isContentEditable || destino instanceof HTMLInputElement || destino instanceof HTMLTextAreaElement;
		if (!editable) e.preventDefault();
	});
}
/** Busca un contenedor obligatorio del armazón. */
function contenedor(id) {
	const el = document.getElementById(id);
	if (!el) {
		// El HTML lo sirve el propio binario, así que esto solo puede ocurrir si
		// alguien edita index.html. Fallar en voz alta ahorra media hora.
		throw new Error(`falta el contenedor #${id} en index.html`);
	}
	return el;
}
function main() {
	// Lo primero: sin esto, un fallo de montaje deja la ventana a medio pintar y
	// sin decir por qué.
	instalarCapturaDeErrores();
	quitarMenuDelNavegador();
	// El ancho de la barra lateral se aplica antes de montar nada: hacerlo
	// después dejaría un fotograma con la barra en su ancho de fábrica.
	// No se guarda el resultado: el armazón no se desmonta nunca, así que no hay
	// nadie a quien devolvérselo.
	const armazon = document.querySelector(".app");
	if (armazon) mountAnchoLateral(armazon);
	const vista = contenedor("view");
	const barraLateral = contenedor("sidebar");
	const barraSuperior = contenedor("topbar");
	const reproductor = contenedor("player");
	// Los eventos se enganchan antes que nada: una descarga que termine mientras
	// se monta la interfaz debe llegar igual.
	void iniciar();
	// En el móvil quien suena es el WebView (no hay motor nativo): el shim se
	// activa solo si /api/capabilities lo dice.
	void arrancarAudioWeb();
	const router = crearRouter(vista, {
		home: (c) => mountHomeView(c),
		search: (c, r) => mountSearchView(c, r),
		library: (c) => mountLibraryView(c),
		liked: (c) => mountLikedView(c),
		playlist: (c, r) => mountPlaylistView(c, r),
		album: (c, r) => mountAlbumView(c, r),
		artist: (c, r) => mountArtistView(c, r),
		social: (c, r) => mountSocialView(c, r),
		settings: (c) => mountSettingsView(c)
	}, "home");
	const sidebar = mountSidebar(barraLateral, router);
	const topbar = mountTopBar(barraSuperior, router);
	const tabs = mountTabs(armazon, router);
	// Cola y vista ampliada cuelgan del armazón, no del router: se abren encima
	// de lo que haya y siguen abiertas al navegar (ver sus cabeceras).
	const cola = mountQueuePanel(document.body);
	const ampliada = mountNowPlaying(document.body);
	mountGestos(document.querySelector(".app"), ampliada);
	// Botón atrás de Android (MainActivity): primero cierra lo que esté encima —cola,
	// vista de la canción, cajón—; solo si no hay nada, navega hacia atrás de verdad.
	globalThis.__localifyAtras = () => {
		if (cola.abierto()) cola.cerrar();
		else if (ampliada.abierta()) ampliada.cerrar();
		else if (armazon?.classList.contains("is-drawer-abierto")) armazon.classList.remove("is-drawer-abierto");
		else return false;
		return true;
	};
	// El reproductor va en su propio `try`: si fallara, la aplicacion seguiria
	// siendo navegable, que es mejor que una ventana en blanco.
	try {
		mountPlayerBar(reproductor, {
			alAbrirCola: () => cola.alternar(),
			alAmpliar: () => ampliada.alternar()
		});
	} catch (e) {
		mostrarError("no se pudo montar el reproductor", String(e));
	}
	/** Título de la vista actual, para la barra superior. */
	function tituloDe(ruta) {
		switch (ruta.nombre) {
			case "home": return t("home.title");
			case "liked": return t("liked.title");
			case "playlist": return t("playlist.title");
			case "search": return t("search.title");
			case "settings": return t("settings.title");
			case "social": return t("nav.social");
			case "album": return t("album.title");
			case "artist": return t("artist.title");
			default: return t("library.title");
		}
	}
	function sincronizar(ruta) {
		sidebar.marcar(ruta.nombre, ruta.params[0]);
		tabs.marcar(ruta.nombre);
		topbar.titulo(tituloDe(ruta));
	}
	router.alNavegar(sincronizar);
	sincronizar(router.actual());
	// El título de la barra superior se calcula al navegar, así que cambiar de
	// idioma sin navegar lo dejaría en el anterior: es el único texto de la
	// interfaz que no se registra como traducible, porque depende de la ruta.
	alCambiarIdioma(() => sincronizar(router.actual()));
}
// El script es un módulo diferido: el DOM ya está listo cuando se ejecuta.
main();

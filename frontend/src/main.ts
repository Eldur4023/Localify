/**
 * Punto de entrada del frontend.
 *
 * El frontend de Localify pinta datos y emite comandos. No toma decisiones de
 * negocio: eso vive en Rust. Este archivo monta el armazón —barra lateral,
 * barra superior y reproductor— y cede el contenido al router.
 *
 * Los `import` llevan extensión `.js` aunque el fuente sea `.ts`: es un
 * requisito de los módulos ES nativos, que es lo que carga el WebView
 * directamente, sin bundler (ADR-019).
 *
 * ## El armazón no se desmonta
 *
 * Solo el contenido cambia al navegar. La barra de reproducción vive fuera del
 * router porque desmontarla al cambiar de vista cortaría la música, y la barra
 * lateral porque volver a pedir las playlists en cada navegación sería una
 * consulta por clic para datos que casi nunca cambian.
 */

import { updates } from "./ipc/client.js";
import { alRecibirTipo, iniciar } from "./ipc/events.js";
import { crearRouter, type Ruta, type Vista } from "./router.js";
import { mountAnchoLateral } from "./shell/ancho-lateral.js";
import { mountSidebar } from "./shell/sidebar.js";
import { mountTopBar } from "./shell/topbar.js";
import { mountPlayerBar } from "./shell/player.js";
import { mountQueuePanel } from "./shell/queue-panel.js";
import { mountNowPlaying } from "./shell/now-playing.js";
import { mountAlbumView } from "./views/album.js";
import { mountArtistView } from "./views/artist.js";
import { mountHomeView } from "./views/home.js";
import { mountLibraryView } from "./views/library.js";
import { mountLikedView } from "./views/liked.js";
import { mountPlaylistView } from "./views/playlist.js";
import { mountSearchView } from "./views/search.js";
import { mountSettingsView } from "./views/settings.js";
import { alCambiarIdioma, t } from "./i18n/index.js";
import { confirmar } from "./ui/dialogo.js";
import { instalarCapturaDeErrores, mostrarError } from "./ui/error-overlay.js";

/**
 * Avisa de una versión nueva y abre el navegador si el usuario acepta.
 *
 * Va en `main.ts` y no en una vista porque el aviso puede llegar en
 * cualquier pantalla, y el armazón —a diferencia de las vistas— nunca se
 * desmonta.
 */
function vigilarActualizaciones(): void {
  alRecibirTipo("updateAvailable", (evento) => {
    void (async () => {
      // No es destructivo: no hay nada que perder por aceptar de más, así
      // que el foco puede arrancar en "actualizar" en vez de en "cancelar".
      const aceptar = await confirmar(
        t("update.title"),
        t("update.accept"),
        t("update.message", { version: evento.version }),
        false,
      );
      if (!aceptar) return;

      try {
        await updates.openReleasePage();
      } catch (e) {
        mostrarError(t("error.internal"), String(e));
      }
    })();
  });
}

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
function quitarMenuDelNavegador(): void {
  document.addEventListener("contextmenu", (e) => {
    const destino = e.target;
    if (!(destino instanceof HTMLElement)) {
      e.preventDefault();
      return;
    }
    const editable =
      destino.isContentEditable ||
      destino instanceof HTMLInputElement ||
      destino instanceof HTMLTextAreaElement;
    if (!editable) e.preventDefault();
  });
}

/** Busca un contenedor obligatorio del armazón. */
function contenedor(id: string): HTMLElement {
  const el = document.getElementById(id);
  if (!el) {
    // El HTML lo sirve el propio binario, así que esto solo puede ocurrir si
    // alguien edita index.html. Fallar en voz alta ahorra media hora.
    throw new Error(`falta el contenedor #${id} en index.html`);
  }
  return el;
}

function main(): void {
  // Lo primero: sin esto, un fallo de montaje deja la ventana a medio pintar y
  // sin decir por qué.
  instalarCapturaDeErrores();
  quitarMenuDelNavegador();

  // El ancho de la barra lateral se aplica antes de montar nada: hacerlo
  // después dejaría un fotograma con la barra en su ancho de fábrica.
  // No se guarda el resultado: el armazón no se desmonta nunca, así que no hay
  // nadie a quien devolvérselo.
  const armazon = document.querySelector<HTMLElement>(".app");
  if (armazon) mountAnchoLateral(armazon);

  const vista = contenedor("view");
  const barraLateral = contenedor("sidebar");
  const barraSuperior = contenedor("topbar");
  const reproductor = contenedor("player");

  // Los eventos se enganchan antes que nada: una descarga que termine mientras
  // se monta la interfaz debe llegar igual.
  void iniciar();
  vigilarActualizaciones();

  const router = crearRouter(
    vista,
    {
      home: (c) => mountHomeView(c),
      search: (c, r: Ruta) => mountSearchView(c, r),
      library: (c) => mountLibraryView(c),
      liked: (c) => mountLikedView(c),
      playlist: (c, r: Ruta) => mountPlaylistView(c, r),
      album: (c, r: Ruta) => mountAlbumView(c, r),
      artist: (c, r: Ruta) => mountArtistView(c, r),
      settings: (c) => mountSettingsView(c),
    },
    "home",
  );

  const sidebar = mountSidebar(barraLateral, router);
  const topbar = mountTopBar(barraSuperior, router);

  // Cola y vista ampliada cuelgan del armazón, no del router: se abren encima
  // de lo que haya y siguen abiertas al navegar (ver sus cabeceras).
  const cola = mountQueuePanel(document.body);
  const ampliada = mountNowPlaying(document.body);

  // El reproductor va en su propio `try`: si fallara, la aplicacion seguiria
  // siendo navegable, que es mejor que una ventana en blanco.
  try {
    mountPlayerBar(reproductor, {
      alAbrirCola: () => cola.alternar(),
      alAmpliar: () => ampliada.alternar(),
    });
  } catch (e) {
    mostrarError("no se pudo montar el reproductor", String(e));
  }

  /** Título de la vista actual, para la barra superior. */
  function tituloDe(ruta: Ruta): string {
    switch (ruta.nombre) {
      case "home":
        return t("home.title");
      case "liked":
        return t("liked.title");
      case "playlist":
        return t("playlist.title");
      case "search":
        return t("search.title");
      case "settings":
        return t("settings.title");
      case "album":
        return t("album.title");
      case "artist":
        return t("artist.title");
      default:
        return t("library.title");
    }
  }

  function sincronizar(ruta: Ruta): void {
    sidebar.marcar(ruta.nombre, ruta.params[0]);
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

// Tipo reexportado para que las vistas no tengan que importarlo del router.
export type { Vista };

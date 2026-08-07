/**
 * Barra lateral: navegación y playlists.
 *
 * Persiste entre vistas —el router solo reemplaza el contenido— y es también
 * el destino de arrastre: soltar una canción sobre una playlist la añade, que
 * es como funciona en Spotify y lo que la gente intenta sin que nadie se lo
 * diga.
 */

import { playlists as api } from "../ipc/client.js";
import { alRecibir } from "../ipc/events.js";
import type { PlaylistSummaryDto } from "../ipc/types.gen.js";
import { alCambiarIdioma, t } from "../i18n/index.js";
import { icono } from "../ui/icons.js";
import { pedirTexto, pedirTextoConImagen } from "../ui/dialogo.js";
import { mostrarError } from "../ui/error-overlay.js";
import { ponerImagenDePlaylist } from "../ui/cards.js";
import { abrirMenu } from "../ui/menu.js";
import { opcionesDePlaylist } from "../ui/opciones-playlist.js";
import type { Router } from "../router.js";
import { arrastrable, zonaDeSoltado, TIPO_PISTAS } from "../ui/dnd.js";

export interface Sidebar {
  /** Vuelve a pedir las playlists. */
  refrescar(): void;
  /** Marca la entrada correspondiente a la ruta. */
  marcar(nombre: string, param?: string): void;
  destroy(): void;
}

export function mountSidebar(contenedor: HTMLElement, router: Router): Sidebar {
  contenedor.replaceChildren();

  // ── Navegación principal ────────────────────────────────────────────────
  const nav = document.createElement("ul");
  nav.className = "sidebar__nav";
  nav.setAttribute("role", "list");

  const entradas: Array<{ ruta: string; clave: string; icono: "home" | "search" | "library" }> = [
    { ruta: "home", clave: "nav.home", icono: "home" },
    { ruta: "search", clave: "nav.search", icono: "search" },
    { ruta: "library", clave: "nav.library", icono: "library" },
  ];

  const botones = new Map<string, HTMLAnchorElement>();
  for (const entrada of entradas) {
    const li = document.createElement("li");
    const a = document.createElement("a");
    a.className = "sidebar__item";
    a.href = `#/${entrada.ruta}`;
    a.append(icono(entrada.icono, 22));
    const texto = document.createElement("span");
    a.append(texto);
    li.append(a);
    nav.append(li);
    botones.set(entrada.ruta, a);

    const etiquetar = (): void => {
      texto.textContent = t(entrada.clave);
      // El globo cubre el caso de la barra colapsada, donde el texto está
      // oculto para la vista pero sigue ahí para quien lee la pantalla.
      a.title = texto.textContent;
    };
    etiquetar();
    a.dataset["clave"] = entrada.clave;
  }

  // ── Cabecera de playlists ───────────────────────────────────────────────
  const cabecera = document.createElement("div");
  cabecera.className = "sidebar__header";

  const titulo = document.createElement("span");
  const nueva = document.createElement("button");
  nueva.type = "button";
  nueva.className = "bicono";
  nueva.append(icono("plus", 18));
  // Un menú y no dos botones: crear e importar son la misma intención —quiero
  // una lista nueva— y poner dos iconos casi iguales obliga a acertar cuál es
  // cuál antes de saber que existen los dos.
  nueva.addEventListener("click", (e) => {
    const caja = nueva.getBoundingClientRect();
    abrirMenu(caja.left, caja.bottom, [
      {
        clave: "crear",
        etiqueta: t("playlist.create"),
        icono: "plus",
        ejecutar: () => void crear(),
      },
      {
        clave: "importar",
        etiqueta: t("playlist.import"),
        ejecutar: () => void importar(),
      },
    ]);
    e.stopPropagation();
  });
  cabecera.append(titulo, nueva);

  // ── Lista de playlists ──────────────────────────────────────────────────
  const lista = document.createElement("ul");
  lista.className = "sidebar__playlists";
  lista.setAttribute("role", "list");

  const liked = document.createElement("li");
  const likedLink = document.createElement("a");
  likedLink.className = "sidebar__item sidebar__item--liked";
  likedLink.href = "#/liked";
  likedLink.append(icono("heart-filled", 18));
  const likedTexto = document.createElement("span");
  likedLink.append(likedTexto);
  liked.append(likedLink);
  botones.set("liked", likedLink);

  const bloque = document.createElement("div");
  bloque.className = "sidebar__block";
  bloque.append(cabecera, lista);
  contenedor.append(nav, bloque);

  const soltados: Array<() => void> = [];

  function etiquetas(): void {
    titulo.textContent = t("library.playlists");
    nueva.setAttribute("aria-label", t("playlist.new"));
    nueva.title = t("playlist.new");
    likedTexto.textContent = t("nav.liked");
    likedLink.title = likedTexto.textContent;
    for (const [ruta, a] of botones) {
      const clave = a.dataset["clave"];
      if (clave) {
        const span = a.querySelector("span");
        if (span) span.textContent = t(clave);
      }
      void ruta;
    }
  }
  etiquetas();
  const dejarIdioma = alCambiarIdioma(etiquetas);

  async function crear(): Promise<void> {
    const respuesta = await pedirTextoConImagen({
      titulo: t("playlist.create"),
      etiqueta: t("playlist.name"),
      valor: t("playlist.new"),
      aceptar: t("playlist.create"),
      maxLength: 100,
      imagen: { etiqueta: t("playlist.cover_choose"), elegir: () => api.pickImage() },
    });
    if (!respuesta) return;

    try {
      const creada = await api.create(respuesta.texto);
      // La portada va después de crear porque necesita el identificador. Si
      // fallara —una imagen corrupta, un disco lleno— la playlist ya existe y
      // se puede volver a intentar desde su ficha: perderla por la foto sería
      // castigar el intento de decorarla.
      if (respuesta.imagen) {
        try {
          await api.setCover(creada.id, respuesta.imagen);
        } catch {
          // El aviso llega por el bus. La playlist queda con su mosaico.
        }
      }
      refrescar();
      router.ir(`playlist/${creada.id}`);
    } catch {
      // El error ya se comunica por el bus como aviso; aquí no hay nada que
      // añadir y romper la barra lateral sería peor.
    }
  }

  /**
   * Trae una lista pública pegando su enlace.
   *
   * El destino lo decide la URL —Spotify o YouTube Music—, así que aquí no hay
   * que elegir el origen: se pega y ya. De Spotify funciona sin credenciales
   * leyendo su página pública; con credenciales viene además la descripción.
   */
  async function importar(): Promise<void> {
    const enlace = await pedirTexto({
      titulo: t("playlist.import"),
      etiqueta: t("playlist.import_url"),
      aceptar: t("playlist.import"),
      maxLength: 300,
    });
    if (!enlace) return;

    try {
      await api.import(enlace);
      // La lista aparece por el evento `playlistImportFinished`, que ya está
      // escuchado más abajo: no hace falta refrescar a mano ni navegar, porque
      // una importación de cien canciones tarda y llevarse al usuario a una
      // ficha a medio llenar es peor que dejarle donde estaba.
    } catch (e) {
      mostrarError(t("playlist.import_failed"), String(e));
    }
  }

  function pintar(items: readonly PlaylistSummaryDto[]): void {
    for (const cancelar of soltados.splice(0)) cancelar();
    lista.replaceChildren(liked);

    for (const p of items) {
      const li = document.createElement("li");
      const a = document.createElement("a");
      a.className = "sidebar__item";
      a.href = `#/playlist/${p.id}`;
      a.dataset["playlistId"] = p.id;

      // Miniatura en vez del icono genérico: en una barra con diez playlists,
      // diez notas musicales iguales no distinguen ninguna, y la portada de lo
      // que hay dentro sí se reconoce de reojo.
      const arte = document.createElement("span");
      arte.className = "sidebar__arte";
      arte.append(icono("music", 20));
      ponerImagenDePlaylist(arte, p);

      const nombre = document.createElement("span");
      nombre.textContent = p.name;
      // Con la barra colapsada el texto no se ve: el globo del sistema es lo
      // único que queda para saber cuál es cuál sin abrirla.
      a.title = p.name;
      a.append(arte, nombre);

      // Soltar pistas encima las añade. Es lo que la gente intenta sin que
      // nadie se lo diga.
      soltados.push(
        zonaDeSoltado(a, TIPO_PISTAS, async (ids) => {
          await api.addTracks(p.id, ids, null);
          refrescar();
        }),
      );

      // Las mismas opciones que en su ficha. Renombrar una playlist obligaba a
      // entrar en ella, volver y comprobar que el cambio se veía; aquí está
      // donde se la está mirando.
      a.addEventListener("contextmenu", (e) => {
        e.preventDefault();
        abrirMenu(
          e.clientX,
          e.clientY,
          opcionesDePlaylist(p, {
            alCambiar: refrescar,
            alBorrar: () => {
              refrescar();
              // Si se borró la que estaba abierta, la pantalla se queda
              // enseñando algo que ya no existe.
              if (globalThis.location.hash === `#/playlist/${p.id}`) {
                router.ir("library");
              }
            },
          }),
        );
      });

      li.append(a);
      lista.append(li);
      botones.set(`playlist:${p.id}`, a);
    }
  }

  function refrescar(): void {
    void api
      .list()
      .then(pintar)
      .catch(() => {
        // Sin playlists la barra sigue siendo navegable.
      });
  }

  refrescar();

  /**
   * La lista se rehace cuando el dominio dice que cambió.
   *
   * Antes solo se refrescaba desde el botón de crear, que llamaba a
   * `refrescar()` a mano. Todo lo demás —borrar, renombrar, añadir canciones—
   * pasa en otras pantallas, y la barra se quedaba enseñando una playlist que
   * ya no existía hasta recargar la ventana. El backend ya publicaba el aviso;
   * aquí nadie lo escuchaba.
   */
  const dejarEventos = alRecibir((evento) => {
    if (evento.type === "playlistChanged" || evento.type === "playlistImportFinished") {
      refrescar();
    }
  });

  return {
    refrescar,

    marcar(nombre: string, param?: string): void {
      const clave = nombre === "playlist" && param ? `playlist:${param}` : nombre;
      for (const [k, a] of botones) {
        const activo = k === clave;
        a.classList.toggle("is-active", activo);
        if (activo) a.setAttribute("aria-current", "page");
        else a.removeAttribute("aria-current");
      }
    },

    destroy(): void {
      dejarIdioma();
      dejarEventos();
      for (const cancelar of soltados.splice(0)) cancelar();
      contenedor.replaceChildren();
    },
  };
}

// Reexportado para que las vistas puedan marcar pistas como arrastrables sin
// importar el módulo de arrastre por su cuenta.
export { arrastrable };

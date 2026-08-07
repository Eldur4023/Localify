/**
 * Barra superior: navegación y título de la vista.
 *
 * Los botones de atrás y adelante se deshabilitan cuando no hay a dónde ir. Un
 * botón que siempre parece pulsable y a veces no hace nada es peor que uno
 * apagado: obliga a probar para averiguar el estado.
 */

import { alCambiarIdioma, t } from "../i18n/index.js";
import { botonIcono } from "../ui/icons.js";
import { mountBusquedaRapida } from "./busqueda-rapida.js";
import type { Router } from "../router.js";

export interface TopBar {
  /** Cambia el título mostrado. */
  titulo(texto: string): void;
  /** Recalcula si los botones de navegación están activos. */
  sincronizar(): void;
  destroy(): void;
}

export function mountTopBar(contenedor: HTMLElement, router: Router): TopBar {
  contenedor.replaceChildren();

  const nav = document.createElement("div");
  nav.className = "topbar__nav";

  const atras = botonIcono("chevron-left", "", () => router.atras(), { tamano: 20 });
  const adelante = botonIcono("chevron-right", "", () => router.adelante(), {
    tamano: 20,
  });
  nav.append(atras, adelante);

  const titulo = document.createElement("h1");
  titulo.className = "topbar__title";

  // Ajustes vive aquí y no en la barra lateral: esa lista es de sitios donde
  // hay música, y meter la configuración entre las playlists la convierte en un
  // cajón de sastre.
  const ajustes = botonIcono("settings", "", () => router.ir("settings"), {
    tamano: 20,
    clase: "topbar__ajustes",
  });

  contenedor.append(nav, titulo);

  // La búsqueda rápida vive aquí porque tiene que estar siempre: querer poner
  // una canción concreta no debería obligar a navegar a otra pantalla primero.
  // Se monta entre el título y Ajustes, así que se añade en este orden.
  const rapida = mountBusquedaRapida(contenedor, router);

  contenedor.append(ajustes);

  function etiquetas(): void {
    atras.setAttribute("aria-label", t("nav.back"));
    atras.title = t("nav.back");
    adelante.setAttribute("aria-label", t("nav.forward"));
    adelante.title = t("nav.forward");
    ajustes.setAttribute("aria-label", t("nav.settings"));
    ajustes.title = t("nav.settings");
  }

  function sincronizar(): void {
    atras.disabled = !router.puedeIrAtras();
    adelante.disabled = !router.puedeIrAdelante();
  }

  etiquetas();
  sincronizar();
  const dejarIdioma = alCambiarIdioma(etiquetas);
  const dejarNav = router.alNavegar(sincronizar);

  return {
    titulo(texto: string): void {
      titulo.textContent = texto;
      document.title = texto === t("app.name") ? texto : `${texto} · ${t("app.name")}`;
    },
    sincronizar,
    destroy(): void {
      dejarIdioma();
      dejarNav();
      rapida.destroy();
      contenedor.replaceChildren();
    },
  };
}

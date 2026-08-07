/**
 * Tus me gusta.
 *
 * Es la misma lista que Biblioteca con otro origen y otro contexto de
 * reproducción. Que compartan componente es lo que hace que el menú
 * contextual, el arrastre y el teclado se comporten igual en las dos.
 */

import type { PageRequestDto, TrackRowDto } from "../ipc/types.gen.js";
import { library, page } from "../ipc/client.js";
import { alCambiarIdioma, t } from "../i18n/index.js";
import { mountTrackList } from "../ui/track-list.js";
import type { Pagina } from "../ui/virtual-list.js";
import type { Vista } from "../router.js";

const POR_PAGINA = 100;

export function mountLikedView(contenedor: HTMLElement): Vista {
  const el = document.createElement("section");
  el.className = "vista vista--lista";

  const cabecera = document.createElement("header");
  cabecera.className = "vista__header vista__header--destacada";
  const titulo = document.createElement("h2");
  const cuenta = document.createElement("span");
  cuenta.className = "vista__count";
  cabecera.append(titulo, cuenta);

  const cuerpo = document.createElement("div");
  cuerpo.className = "vista__body";

  const vacio = document.createElement("p");
  vacio.className = "vista__empty";
  vacio.hidden = true;

  el.append(cabecera, vacio, cuerpo);
  contenedor.replaceChildren(el);

  let offset = 0;
  let total: bigint | null = null;

  const trackList = mountTrackList(cuerpo, {
    contexto: () => ({ kind: "liked" }),
    numerar: true,
    conAlbum: true,
    conFecha: true,

    reiniciarOrigen(): void {
      offset = 0;
      total = null;
    },

    async cargar(): Promise<Pagina<TrackRowDto>> {
      const req: PageRequestDto = page({ offset, limit: POR_PAGINA });
      const respuesta = await library.favorites(req);
      offset += respuesta.items.length;
      total ??= respuesta.total;

      pintarCuenta();
      vacio.hidden = offset > 0;

      // Favoritos no expone cursor: una lista de me gusta rara vez pasa de unos
      // miles, y el desplazamiento por `offset` ahí no se nota.
      return {
        items: respuesta.items,
        hasMore: respuesta.items.length === POR_PAGINA,
      };
    },
  });

  function pintarCuenta(): void {
    const cuantas = total === null ? trackList.lista.items.length : Number(total);
    cuenta.textContent = t("library.count", { count: cuantas });
  }

  function etiquetas(): void {
    titulo.textContent = t("liked.title");
    vacio.textContent = t("liked.empty");
    pintarCuenta();
    trackList.lista.refresh();
  }

  etiquetas();
  const dejarIdioma = alCambiarIdioma(etiquetas);

  return {
    destroy(): void {
      dejarIdioma();
      trackList.destroy();
      el.remove();
    },
  };
}

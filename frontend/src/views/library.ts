/**
 * Canciones: todo el catálogo conocido.
 *
 * ## No hay filtro de "solo lo descargado"
 *
 * Lo hubo, y era un error de encuadre. Que una canción esté en disco o haya que
 * traerla es asunto de la aplicación, no del usuario: pulsar reproduce en los
 * dos casos y la diferencia son unos segundos. Ofrecerlo como filtro obligaba a
 * entender la mecánica de las descargas para usar la pantalla, y convertía "tus
 * canciones" en "las que resulta que hay en el disco duro" — dos cosas que no
 * tienen por qué parecerse.
 *
 * La paginación es por cursor. No se usa `offset`: en la página 500 obligaría a
 * SQLite a recorrer y descartar 25 000 filas antes de devolver 50. Con cursor,
 * la última página cuesta lo mismo que la primera.
 */

import type {
  PageRequestDto,
  TrackFilterDto,
  TrackRowDto,
  TrackSort,
} from "../ipc/types.gen.js";
import { library, page } from "../ipc/client.js";
import { alCambiarIdioma, t } from "../i18n/index.js";
import { alRecibirTipo } from "../ipc/events.js";
import { mountTrackList } from "../ui/track-list.js";
import type { Pagina } from "../ui/virtual-list.js";
import type { Vista } from "../router.js";

const POR_PAGINA = 100;

/** Ordenaciones ofrecidas, con su clave de traducción. */
const ORDENES: ReadonlyArray<{ valor: TrackSort; clave: string }> = [
  { valor: "titleAsc", clave: "library.sort.title" },
  { valor: "artistAsc", clave: "library.sort.artist" },
  { valor: "albumAsc", clave: "library.sort.album" },
  { valor: "addedDesc", clave: "library.sort.added" },
  { valor: "durationAsc", clave: "library.sort.duration" },
];

export function mountLibraryView(contenedor: HTMLElement): Vista {
  const el = document.createElement("section");
  el.className = "vista vista--lista";

  const cabecera = document.createElement("header");
  cabecera.className = "vista__header";

  const titulo = document.createElement("h2");
  const cuenta = document.createElement("span");
  cuenta.className = "vista__count";

  const controles = document.createElement("div");
  controles.className = "vista__controls";

  const orden = document.createElement("select");
  orden.className = "select";
  for (const o of ORDENES) {
    const opt = document.createElement("option");
    opt.value = o.valor;
    orden.append(opt);
  }

  controles.append(orden);
  cabecera.append(titulo, cuenta, controles);

  const cuerpo = document.createElement("div");
  cuerpo.className = "vista__body";

  // Recién instalada, esta es la primera pantalla que se ve, y era una lista
  // vacía con un "0 canciones" encima: indistinguible de un fallo de carga. Que
  // el texto diga qué hacer —buscar algo— importa más aquí que en ningún otro
  // sitio, porque es donde alguien decide si la aplicación funciona.
  const vacio = document.createElement("p");
  vacio.className = "vista__empty";
  vacio.hidden = true;

  el.append(cabecera, vacio, cuerpo);
  contenedor.replaceChildren(el);

  // ── Estado de la consulta ───────────────────────────────────────────────
  let cursor: string | null = null;
  let total: bigint | null = null;

  /** Sin acotar: el catálogo entero, esté o no en disco. */
  const FILTRO: TrackFilterDto = {
    favoritesOnly: false,
    localOnly: false,
    albumId: null,
    artistId: null,
    genreId: null,
    text: null,
  };

  const trackList = mountTrackList(cuerpo, {
    contexto: () => ({ kind: "library" }),
    conAlbum: true,
    conFecha: true,

    reiniciarOrigen(): void {
      cursor = null;
      total = null;
    },

    async cargar(): Promise<Pagina<TrackRowDto>> {
      const req: PageRequestDto = page({ limit: POR_PAGINA, cursor });
      const respuesta = await library.tracks(FILTRO, orden.value as TrackSort, req);

      cursor = respuesta.nextCursor;
      total ??= respuesta.total;
      pintarCuenta();

      // Los elementos van siempre, incluida la última página: descartarlos
      // perdería las últimas cien canciones de la biblioteca.
      return { items: respuesta.items, hasMore: cursor !== null };
    },
  });

  function pintarCuenta(): void {
    const cuantas = total === null ? trackList.lista.items.length : Number(total);
    cuenta.textContent = t("library.count", { count: cuantas });
    // Solo con el total ya contado. Mientras es `null` no se sabe si está vacía
    // o si la primera página aún no ha llegado, y enseñar "no hay nada" durante
    // la carga es peor que no enseñar nada.
    vacio.hidden = total === null || cuantas > 0;
  }

  function etiquetas(): void {
    titulo.textContent = t("library.title");
    vacio.textContent = t("library.empty");
    for (const [i, o] of ORDENES.entries()) {
      const opt = orden.options.item(i);
      if (opt) opt.textContent = t(o.clave);
    }
    pintarCuenta();
    trackList.lista.refresh();
  }

  const recargar = (): void => trackList.refrescar();
  orden.addEventListener("change", recargar);

  // Un rescan y una importación de ficheros propios cambian qué pistas hay
  // sin que el usuario haga nada en esta vista: sin escuchar el evento, las
  // canciones nuevas solo aparecerían al navegar fuera y volver.
  const dejarLibraryChanged = alRecibirTipo("libraryChanged", (evento) => {
    if (evento.scope === "tracks") recargar();
  });

  etiquetas();
  const dejarIdioma = alCambiarIdioma(etiquetas);

  return {
    destroy(): void {
      dejarIdioma();
      dejarLibraryChanged();
      orden.removeEventListener("change", recargar);
      trackList.destroy();
      el.remove();
    },
  };
}

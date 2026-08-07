/**
 * Lista de pistas: la fila que aparece en media aplicación.
 *
 * Canciones, Tus me gusta, álbum, artista, playlist y búsqueda muestran lo
 * mismo con variaciones pequeñas. Tenerla en un sitio hace que el menú
 * contextual, el arrastre y el teclado se comporten igual en todas, que es la
 * mitad de la sensación de que una aplicación está bien hecha.
 *
 * ## La fila no dice si la canción está descargada
 *
 * Hubo un punto de color que lo indicaba, y una llamada por ventana visible
 * para averiguarlo al desplazarse. Sobraban las dos: la descarga es invisible
 * por diseño, pulsar una fila reproduce en cualquier caso, y un indicador que
 * el usuario no puede accionar solo invita a preguntarse qué habría que hacer
 * con él. Quitarlo se llevó de paso una petición por scroll.
 *
 * ## Las carátulas se reciclan con la fila
 *
 * Cada fila lleva su miniatura, y la etiqueta `<img>` se reutiliza en lugar de
 * recrearse. Es lo que hace viable ponerlas en una lista de cincuenta mil
 * canciones: ver [`comienzoReciclable`].
 *
 * ## Sobre el teclado
 *
 * Una lista virtualizada no puede tener 50 000 elementos enfocables, así que el
 * contenedor es el que recibe el foco y `aria-activedescendant` señala la fila
 * activa. Es el patrón que los lectores de pantalla entienden para listas
 * largas, y el único compatible con reciclar nodos.
 */

import { player } from "../ipc/client.js";
import type { TrackRowDto } from "../ipc/types.gen.js";
import type { PlaybackContextDto } from "../ipc/types.gen.js";
import { idioma, t } from "../i18n/index.js";
import { duracion } from "../shell/player.js";
import { comienzoReciclable, type ComienzoReciclable } from "./cards.js";
import { abrirMenu } from "./menu.js";
import { opcionesDePista } from "./opciones-pista.js";
import { arrastrable, reordenable } from "./dnd.js";
import { icono } from "./icons.js";
import { mountVirtualList, type Pagina, type VirtualList } from "./virtual-list.js";

/** Altura de fila. Debe coincidir con `.track` en el CSS. */
export const ALTO_FILA = 56;

/**
 * Fecha de una celda: día, mes abreviado y año.
 *
 * Se construye un formateador por idioma y se guarda. `Intl.DateTimeFormat` es
 * caro de crear —compila reglas de localización— y aquí se llamaría una vez por
 * fila y por repintado: en una lista larga eso es miles de construcciones por
 * scroll.
 */
const FORMATOS = new Map<string, Intl.DateTimeFormat>();

function fechaCorta(segundos: number | null): string {
  if (segundos === null) return "";
  const loc = idioma();
  let formato = FORMATOS.get(loc);
  if (!formato) {
    formato = new Intl.DateTimeFormat(loc, {
      day: "numeric",
      month: "short",
      year: "numeric",
    });
    FORMATOS.set(loc, formato);
  }
  return formato.format(new Date(segundos * 1000));
}

export interface OpcionesLista {
  /** De dónde salen las páginas. */
  cargar(): Promise<Pagina<TrackRowDto>>;
  /**
   * Devuelve el origen a su primera página.
   *
   * Se llama antes de recargar. Sin esto, quien mantiene un cursor seguiría
   * pidiendo desde donde se quedó y la lista recargada empezaría por la mitad.
   */
  reiniciarOrigen?(): void;
  /** Contexto con el que reproducir al pulsar una fila. */
  contexto(): PlaybackContextDto;
  /** Si la lista pertenece a una playlist, para reordenar y quitar. */
  readonly playlistId?: string;
  /** Identificador de entrada de cada fila, si los hay. */
  entryIdDe?(pista: TrackRowDto, indice: number): string | undefined;
  /** Se llama tras quitar una pista de la playlist. */
  alQuitar?(): void;
  /** Muestra el número de orden. Un álbum sí; una búsqueda no. */
  readonly numerar?: boolean;
  /**
   * Columna de álbum.
   *
   * Fuera en la vista de un álbum, donde repetiría el mismo título en las doce
   * filas, y en la de búsqueda, donde el ancho se necesita para el título.
   */
  readonly conAlbum?: boolean;
  /**
   * Columna de fecha de alta.
   *
   * Solo tiene sentido donde la fila tiene fecha —biblioteca, favoritos,
   * playlist—. Si se pide donde no la hay, la celda queda vacía; ver
   * `TrackRow::added_at` en el backend.
   */
  readonly conFecha?: boolean;
}

export interface ListaDePistas {
  readonly el: HTMLElement;
  readonly lista: VirtualList<TrackRowDto>;
  refrescar(): void;
  destroy(): void;
}

export function mountTrackList(
  contenedor: HTMLElement,
  opciones: OpcionesLista,
): ListaDePistas {
  const desmontadores: Array<() => void> = [];
  let activa = -1;

  /**
   * Piezas de cada fila, por fila.
   *
   * Un `WeakMap` y no una búsqueda por índice entre `fila.children`: el orden
   * de los hijos ya cambió una vez —al quitar el punto de disponibilidad— y
   * desmontar la tupla al pintar es la clase de error que no da ningún aviso,
   * solo pone el título donde iba el artista.
   */
  const piezas = new WeakMap<
    HTMLElement,
    {
      numero: HTMLElement;
      titulo: HTMLElement;
      artista: HTMLElement;
      album: HTMLElement | null;
      fecha: HTMLElement | null;
      tiempo: HTMLElement;
      comienzo: ComienzoReciclable;
    }
  >();

  // Caja propia: la cabecera de columnas y la lista virtual son hermanas, y
  // `mountVirtualList` reemplaza los hijos del contenedor que recibe. Sin este
  // envoltorio, montar la lista se llevaría la cabecera por delante.
  const caja = document.createElement("div");
  caja.className = "lista";
  caja.append(cabeceraDeColumnas());

  const cuerpo = document.createElement("div");
  cuerpo.className = "lista__cuerpo";
  caja.append(cuerpo);
  contenedor.replaceChildren(caja);

  const lista = mountVirtualList<TrackRowDto>(cuerpo, {
    rowHeight: ALTO_FILA,

    createRow(): HTMLElement {
      const fila = document.createElement("div");
      fila.className = "track";
      fila.setAttribute("role", "option");

      // Los hijos se crean una vez; `renderRow` solo cambia su contenido.
      // Crear nodos al pintar anularía el reciclado.
      const trozo = (clase: string): HTMLElement => {
        const span = document.createElement("span");
        span.className = `track__${clase}`;
        return span;
      };
      const numero = trozo("index");
      const titulo = trozo("title");
      const artista = trozo("artist");
      const tiempo = trozo("time");
      const comienzo = comienzoReciclable();

      // Título y artista van apilados en una celda, no en dos columnas: es lo
      // que deja sitio para álbum y fecha sin comerse el título, y lo que hace
      // que la fila se lea de un vistazo —qué canción y de quién, juntos—.
      const principal = document.createElement("span");
      principal.className = "track__main";
      principal.append(titulo, artista);

      const album = opciones.conAlbum === true ? trozo("album") : null;
      const fecha = opciones.conFecha === true ? trozo("added") : null;

      fila.append(numero, ...comienzo.nodos, principal);
      if (album) fila.append(album);
      if (fecha) fila.append(fecha);
      fila.append(tiempo);
      piezas.set(fila, { numero, titulo, artista, album, fecha, tiempo, comienzo });

      const mas = document.createElement("button");
      mas.type = "button";
      mas.className = "track__more bicono";
      mas.tabIndex = -1;
      mas.append(icono("more", 16));
      fila.append(mas);

      desmontadores.push(arrastrable(fila, () => [fila.dataset["trackId"] ?? ""]));
      if (opciones.playlistId) {
        desmontadores.push(
          reordenable(fila, () => fila.dataset["entryId"] ?? ""),
        );
      }

      fila.addEventListener("contextmenu", (e) => {
        e.preventDefault();
        abrir(indiceDe(fila), e.clientX, e.clientY);
      });
      mas.addEventListener("click", (e) => {
        // Sin esto, abrir el menú de una fila también la reproduciría: el
        // clic del botón sube hasta la fila, que ahora sí reproduce.
        e.stopPropagation();
        const caja = mas.getBoundingClientRect();
        abrir(indiceDe(fila), caja.left, caja.bottom);
      });

      // Un clic reproduce. Spotify pide dos, pero aquí una fila no tiene otra
      // acción que competir por el clic simple: seleccionar por seleccionar no
      // sirve para nada, y obligar a dos pulsaciones para lo único que se hace
      // con una canción es cobrar un peaje sin contrapartida.
      fila.addEventListener("click", () => {
        const i = indiceDe(fila);
        marcar(i);
        reproducir(i);
      });

      return fila;
    },

    renderRow(fila, pista, indice): void {
      const p = piezas.get(fila);
      if (p) {
        p.numero.textContent = opciones.numerar === false ? "" : String(indice + 1);
        p.titulo.textContent = pista.title;
        p.artista.textContent = pista.artistDisplay;
        p.tiempo.textContent = duracion(pista.durationMs);
        p.comienzo.pintar(pista.albumId, pista.id);
        if (p.album) p.album.textContent = pista.albumTitle ?? "";
        if (p.fecha) p.fecha.textContent = fechaCorta(pista.addedAt);
      }

      fila.dataset["trackId"] = pista.id;
      fila.dataset["indice"] = String(indice);
      const entry = opciones.entryIdDe?.(pista, indice);
      if (entry) fila.dataset["entryId"] = entry;

      fila.id = `pista-${indice}`;
      fila.classList.toggle("is-active", indice === activa);
      fila.setAttribute("aria-selected", String(indice === activa));
    },

    loadMore: opciones.cargar,
  });

  lista.el.tabIndex = 0;
  lista.el.setAttribute("role", "listbox");
  lista.el.setAttribute("aria-label", t("library.tracks"));

  /**
   * Fila de títulos de columna.
   *
   * `aria-hidden`: el `listbox` de abajo ya se anuncia con su propia etiqueta y
   * cada fila con su contenido. Un lector de pantalla que leyera además esta
   * fila diría cinco palabras sueltas que no corresponden a nada accionable.
   */
  function cabeceraDeColumnas(): HTMLElement {
    const fila = document.createElement("div");
    fila.className = "lista__cabecera";
    fila.setAttribute("aria-hidden", "true");

    const celda = (clase: string, texto: string): HTMLElement => {
      const span = document.createElement("span");
      span.className = `track__${clase}`;
      span.textContent = texto;
      return span;
    };

    fila.append(
      celda("index", "#"),
      // El hueco de la carátula no se titula, pero tiene que ocupar: sin él,
      // "Título" empezaría cuarenta píxeles a la izquierda de los títulos.
      celda("hueco", ""),
      celda("main", t("tracks.col.title")),
    );
    if (opciones.conAlbum === true) fila.append(celda("album", t("tracks.col.album")));
    if (opciones.conFecha === true) fila.append(celda("added", t("tracks.col.added")));

    // La duración se titula con un reloj y no con la palabra: "Duración" no cabe
    // en las cinco cifras que ocupa la columna.
    const reloj = celda("time", "");
    reloj.title = t("tracks.col.duration");
    reloj.append(icono("clock", 16));
    fila.append(reloj);

    return fila;
  }

  function indiceDe(fila: HTMLElement): number {
    return Number(fila.dataset["indice"] ?? "-1");
  }

  function marcar(indice: number): void {
    activa = indice;
    lista.el.setAttribute("aria-activedescendant", `pista-${indice}`);
    lista.refresh();
  }

  function reproducir(indice: number): void {
    const pista = lista.items[indice];
    if (!pista) return;
    marcar(indice);
    void player.playTrack(pista.id, opciones.contexto());
  }

  function abrir(indice: number, x: number, y: number): void {
    const pista = lista.items[indice];
    if (!pista) return;
    marcar(indice);

    abrirMenu(
      x,
      y,
      opcionesDePista(pista, {
        contexto: opciones.contexto,
        playlistId: opciones.playlistId,
        entryId: opciones.entryIdDe?.(pista, indice),
        alCambiar: () => {
          refrescar();
          opciones.alQuitar?.();
        },
      }),
    );
  }

  // ── Teclado ─────────────────────────────────────────────────────────────
  const alTeclear = (e: KeyboardEvent): void => {
    const ultimo = lista.items.length - 1;
    if (ultimo < 0) return;

    switch (e.key) {
      case "ArrowDown":
        e.preventDefault();
        mover(Math.min(ultimo, activa + 1));
        break;
      case "ArrowUp":
        e.preventDefault();
        mover(Math.max(0, activa - 1));
        break;
      case "Home":
        e.preventDefault();
        mover(0);
        break;
      case "End":
        e.preventDefault();
        mover(ultimo);
        break;
      case "PageDown":
        e.preventDefault();
        mover(Math.min(ultimo, activa + porPantalla()));
        break;
      case "PageUp":
        e.preventDefault();
        mover(Math.max(0, activa - porPantalla()));
        break;
      case "Enter":
        e.preventDefault();
        reproducir(activa);
        break;
      case "ContextMenu": {
        e.preventDefault();
        const fila = lista.el.querySelector<HTMLElement>(`#pista-${activa}`);
        const caja = fila?.getBoundingClientRect();
        abrir(activa, caja?.left ?? 0, caja?.bottom ?? 0);
        break;
      }
      default:
        break;
    }
  };

  function porPantalla(): number {
    return Math.max(1, Math.floor(lista.el.clientHeight / ALTO_FILA) - 1);
  }

  /**
   * Mueve la selección y la trae a la vista.
   *
   * Sin el desplazamiento, bajar con las flechas saca la fila activa de la
   * pantalla y el foco se pierde de vista, que es peor que no tener teclado.
   */
  function mover(indice: number): void {
    if (indice < 0) return;
    marcar(indice);

    const arriba = indice * ALTO_FILA;
    const abajo = arriba + ALTO_FILA;
    const vistaArriba = lista.el.scrollTop;
    const vistaAbajo = vistaArriba + lista.el.clientHeight;

    if (arriba < vistaArriba) lista.el.scrollTop = arriba;
    else if (abajo > vistaAbajo) lista.el.scrollTop = abajo - lista.el.clientHeight;
  }

  lista.el.addEventListener("keydown", alTeclear);

  function refrescar(): void {
    activa = -1;
    opciones.reiniciarOrigen?.();
    lista.reset();
  }

  return {
    el: lista.el,
    lista,
    refrescar,
    destroy(): void {
      lista.el.removeEventListener("keydown", alTeclear);
      for (const quitar of desmontadores.splice(0)) quitar();
      lista.destroy();
    },
  };
}

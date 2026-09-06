/**
 * Lista de pistas: la fila que aparece en media aplicación.
 *
 * Canciones, Tus me gusta, álbum, artista, playlist y búsqueda muestran lo
 * mismo con variaciones pequeñas. Tenerla en un sitio hace que el menú
 * contextual, el arrastre y el teclado se comporten igual en todas, que es la
 * mitad de la sensación de que una aplicación está bien hecha.
 *
 * ## La fila sí dice si la canción está descargada
 *
 * Hubo un tiempo en que no lo decía: un punto de color se quitó junto con una
 * llamada por ventana visible al desplazarse, y con las dos se fue toda
 * indicación de estado. Pulsar una fila sigue reproduciendo en cualquier
 * caso —eso no ha cambiado—, pero dejar al usuario sin saber por qué una
 * canción no suena todavía es peor que un punto que no se puede accionar.
 *
 * El indicador vive por completo de eventos ya existentes
 * (`downloadProgress`, `availabilityChanged`): nunca se sondea una fila por su
 * cuenta, así que no se repite el error de antes.
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
import type { AvailabilityDto, TrackRowDto } from "../ipc/types.gen.js";
import type { PlaybackContextDto } from "../ipc/types.gen.js";
import { alRecibirTipo } from "../ipc/events.js";
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

/**
 * Pinta el indicador de estado de descarga de una fila.
 *
 * Un porcentaje mientras se descarga, la nube cuando ya está en disco, un
 * aviso si falló, y nada en el resto de casos: `Absent` no dice nada porque
 * pulsar reproduce igual, y decir "no descargada" en cada fila de una
 * biblioteca de cincuenta mil canciones sería ruido, no información.
 */
function pintarDisponibilidad(el: HTMLElement, a: AvailabilityDto): void {
  switch (a.kind) {
    case "downloading":
      pintarProgreso(el, a.progress);
      break;
    case "local":
      pintarDescargada(el);
      break;
    case "failed":
      pintarFallida(el, a.reasonKey);
      break;
    default:
      pintarAusente(el);
  }
}

function pintarProgreso(el: HTMLElement, percent: number): void {
  el.className = "track__state track__state--downloading";
  el.textContent = `${Math.round(percent * 100)}%`;
  el.title = "";
}

function pintarDescargada(el: HTMLElement): void {
  el.className = "track__state track__state--local";
  el.replaceChildren(icono("cloud-download", 16));
  el.title = t("tracks.downloaded");
}

function pintarFallida(el: HTMLElement, reasonKey: string): void {
  el.className = "track__state track__state--failed";
  el.replaceChildren(icono("alert", 16));
  el.title = t(reasonKey);
}

function pintarAusente(el: HTMLElement): void {
  el.className = "track__state";
  el.replaceChildren();
  el.title = "";
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
      estado: HTMLElement;
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
      const estado = trozo("state");
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
      fila.append(tiempo, estado);
      piezas.set(fila, { numero, titulo, artista, album, fecha, tiempo, estado, comienzo });

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
        // Se repinta siempre, incluso al reciclar: una fila reutilizada para
        // otra pista no debe arrastrar el estado de descarga de la anterior.
        pintarDisponibilidad(p.estado, pista.availability);
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

    // Mismo motivo que el hueco de la carátula: la columna de estado no tiene
    // texto que titular, pero tiene que ocupar su sitio para que "Duración" no
    // se desplace.
    fila.append(celda("state", ""));

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

  // ── Progreso de descarga en vivo ──────────────────────────────────────
  //
  // Solo estos dos eventos: `downloadProgress` para el porcentaje en marcha, y
  // `availabilityChanged` para los tres estados terminales (local, fallida,
  // ausente) que ya llegan acompañando a `downloadCompleted`/`downloadFailed`.
  // Nunca se sondea `library.availability` desde aquí: sería repetir la
  // petición por scroll que se quitó una vez.
  function filaDe(trackId: string): HTMLElement | null {
    return cuerpo.querySelector<HTMLElement>(
      `[data-track-id="${CSS.escape(trackId)}"]`,
    );
  }

  const dejarProgreso = alRecibirTipo("downloadProgress", (e) => {
    const fila = filaDe(e.trackId);
    const p = fila ? piezas.get(fila) : undefined;
    if (p) pintarProgreso(p.estado, e.percent);
  });
  const dejarDisponibilidad = alRecibirTipo("availabilityChanged", (e) => {
    const fila = filaDe(e.trackId);
    const p = fila ? piezas.get(fila) : undefined;
    if (p) pintarDisponibilidad(p.estado, e.availability);
  });

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
      dejarProgreso();
      dejarDisponibilidad();
      for (const quitar of desmontadores.splice(0)) quitar();
      lista.destroy();
    },
  };
}

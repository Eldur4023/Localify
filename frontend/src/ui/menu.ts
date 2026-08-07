/**
 * Menú contextual.
 *
 * ## Dos paneles, reutilizados
 *
 * Hay un panel principal y uno de submenú, ambos únicos en el documento, que se
 * mueven y se rellenan. Crear uno por fila dejaría 50 000 menús ocultos en una
 * biblioteca grande, que es exactamente lo que la virtualización venía a
 * evitar. Dos niveles bastan: un submenú dentro de un submenú es un menú que
 * nadie sabe recorrer.
 *
 * ## El submenú se abre al pasar por encima, con retardo
 *
 * Sin retardo, bajar el ratón hacia la última opción abre y cierra cada
 * submenú que se cruza por el camino. Con él, hay que pararse un momento sobre
 * la opción, que es justo lo que hace quien la quiere abrir.
 *
 * ## Se cierra con todo
 *
 * Clic fuera, `Escape`, scroll, cambio de tamaño y perder el foco de la
 * ventana. Un menú que sobrevive a un scroll queda flotando sobre el elemento
 * equivocado, y quien lo ve ya no sabe a qué fila se refiere.
 *
 * ## Cabe en la pantalla
 *
 * Se coloca en el cursor y, si se sale, se recoloca hacia dentro. El submenú
 * sale a la derecha de su opción y se pasa a la izquierda si no cabe. Sin eso,
 * un clic derecho cerca del borde abre un menú cuyas opciones son inalcanzables.
 */

import { icono, type Icono } from "./icons.js";

export interface OpcionMenu {
  readonly clave: string;
  readonly etiqueta: string;
  readonly icono?: Icono;
  /** Separador visual antes de esta opción. */
  readonly separar?: boolean;
  readonly peligrosa?: boolean;
  /**
   * Contenido del submenú.
   *
   * Puede ser asíncrono: la lista de playlists se pide al abrirlo y no al
   * construir el menú, para no consultar en cada clic derecho algo que casi
   * nunca se mira.
   */
  readonly submenu?: () => readonly OpcionMenu[] | Promise<readonly OpcionMenu[]>;
  ejecutar?(): void | Promise<void>;
}

/** Milisegundos de permanencia antes de abrir un submenú. */
const RETARDO_SUBMENU = 180;

/** Margen mínimo hasta el borde de la ventana. */
const MARGEN = 8;

let panel: HTMLElement | null = null;
let hijo: HTMLElement | null = null;
let cerrarActual: (() => void) | null = null;

function crearPanel(clase: string): HTMLElement {
  const el = document.createElement("div");
  el.className = clase;
  el.setAttribute("role", "menu");
  el.hidden = true;
  document.body.append(el);
  return el;
}

function panelPrincipal(): HTMLElement {
  panel ??= crearPanel("menu");
  return panel;
}

function panelHijo(): HTMLElement {
  hijo ??= crearPanel("menu menu--submenu");
  return hijo;
}

/** Cierra el menú abierto, si lo hay. */
export function cerrarMenu(): void {
  cerrarActual?.();
}

/** Cierra solo el submenú. */
function cerrarSubmenu(): void {
  if (!hijo) return;
  hijo.hidden = true;
  hijo.replaceChildren();
  panel
    ?.querySelectorAll('[aria-expanded="true"]')
    .forEach((b) => b.setAttribute("aria-expanded", "false"));
}

/** Coloca un panel dentro de la ventana, midiéndolo primero. */
function colocar(el: HTMLElement, x: number, y: number): DOMRect {
  el.hidden = false;
  el.style.left = "0px";
  el.style.top = "0px";
  const caja = el.getBoundingClientRect();

  const izq = Math.min(x, globalThis.innerWidth - caja.width - MARGEN);
  const arriba = Math.min(y, globalThis.innerHeight - caja.height - MARGEN);
  el.style.left = `${Math.max(MARGEN, izq)}px`;
  el.style.top = `${Math.max(MARGEN, arriba)}px`;
  return caja;
}

/** Construye los botones de una lista de opciones dentro de un panel. */
function rellenar(el: HTMLElement, opciones: readonly OpcionMenu[]): void {
  el.replaceChildren();

  for (const opcion of opciones) {
    if (opcion.separar) {
      const hr = document.createElement("div");
      hr.className = "menu__sep";
      hr.setAttribute("role", "separator");
      el.append(hr);
    }

    const boton = document.createElement("button");
    boton.type = "button";
    boton.className = "menu__item";
    boton.setAttribute("role", "menuitem");
    if (opcion.peligrosa) boton.classList.add("is-danger");

    if (opcion.icono) boton.append(icono(opcion.icono, 16));
    const texto = document.createElement("span");
    texto.className = "menu__texto";
    texto.textContent = opcion.etiqueta;
    boton.append(texto);

    if (opcion.submenu) {
      boton.classList.add("menu__item--padre");
      boton.setAttribute("aria-haspopup", "menu");
      boton.setAttribute("aria-expanded", "false");
      boton.append(icono("chevron-right", 16));
      atarSubmenu(boton, opcion.submenu);
      el.append(boton);
      continue;
    }

    boton.addEventListener("click", () => {
      cerrarMenu();
      void opcion.ejecutar?.();
    });
    el.append(boton);
  }
}

/** Engancha la apertura del submenú a una opción que lo tiene. */
function atarSubmenu(
  boton: HTMLButtonElement,
  contenido: NonNullable<OpcionMenu["submenu"]>,
): void {
  let temporizador: number | null = null;

  const cancelar = (): void => {
    if (temporizador !== null) {
      globalThis.clearTimeout(temporizador);
      temporizador = null;
    }
  };

  const abrir = async (): Promise<void> => {
    cancelar();
    if (boton.getAttribute("aria-expanded") === "true") return;

    cerrarSubmenu();
    boton.setAttribute("aria-expanded", "true");

    const el = panelHijo();
    // Algo visible desde el primer instante: pedir las playlists tarda poco,
    // pero un panel que aparece vacío y se rellena después parpadea.
    rellenar(el, [{ clave: "cargando", etiqueta: "…" }]);

    let opciones: readonly OpcionMenu[];
    try {
      opciones = await contenido();
    } catch {
      cerrarSubmenu();
      return;
    }
    // Puede haberse cerrado mientras se pedía.
    if (boton.getAttribute("aria-expanded") !== "true") return;

    rellenar(el, opciones);

    // A la derecha de la opción; si no cabe, al otro lado. El solape de un
    // píxel evita que el hueco entre paneles cuente como "salir del menú".
    const caja = boton.getBoundingClientRect();
    el.hidden = false;
    el.style.left = "0px";
    el.style.top = "0px";
    const propia = el.getBoundingClientRect();

    const derecha = caja.right - 1;
    const x =
      derecha + propia.width + MARGEN > globalThis.innerWidth
        ? caja.left - propia.width + 1
        : derecha;
    colocar(el, x, caja.top - 4);
  };

  boton.addEventListener("mouseenter", () => {
    cancelar();
    temporizador = globalThis.setTimeout(() => void abrir(), RETARDO_SUBMENU);
  });
  boton.addEventListener("mouseleave", cancelar);
  // Pulsar o pulsar Enter lo abre al instante: quien ya decidió no debe esperar.
  boton.addEventListener("click", (e) => {
    e.stopPropagation();
    void abrir();
  });
  boton.addEventListener("keydown", (e) => {
    if (e.key === "ArrowRight" || e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      void abrir().then(() =>
        hijo?.querySelector<HTMLButtonElement>(".menu__item")?.focus(),
      );
    }
  });
}

/** Abre el menú contextual en las coordenadas dadas. */
export function abrirMenu(x: number, y: number, opciones: readonly OpcionMenu[]): void {
  cerrarMenu();
  if (opciones.length === 0) return;

  const el = panelPrincipal();
  rellenar(el, opciones);
  colocar(el, x, y);

  // El primer elemento recibe el foco: sin esto el menú no es navegable con
  // teclado, y abrirlo desde la tecla de menú no serviría de nada.
  el.querySelector<HTMLButtonElement>(".menu__item")?.focus();

  const dentro = (destino: Node | null): boolean =>
    el.contains(destino) || hijo?.contains(destino) === true;

  const alPulsarFuera = (e: MouseEvent): void => {
    if (!dentro(e.target as Node)) cerrarMenu();
  };
  const alTeclear = (e: KeyboardEvent): void => {
    if (e.key !== "Escape") return;
    e.stopPropagation();
    // Escape cierra primero el submenú: quien lo abrió por error espera volver
    // al menú de antes, no perderlo todo y tener que repetir el clic derecho.
    if (hijo && !hijo.hidden) cerrarSubmenu();
    else cerrarMenu();
  };

  document.addEventListener("mousedown", alPulsarFuera, true);
  document.addEventListener("keydown", alTeclear, true);
  // `capture` para enterarse del scroll de cualquier contenedor, no solo del
  // documento: la lista virtualizada desplaza su propio elemento.
  document.addEventListener("scroll", cerrarMenu, true);
  globalThis.addEventListener("resize", cerrarMenu);
  globalThis.addEventListener("blur", cerrarMenu);

  cerrarActual = (): void => {
    cerrarSubmenu();
    el.hidden = true;
    el.replaceChildren();
    document.removeEventListener("mousedown", alPulsarFuera, true);
    document.removeEventListener("keydown", alTeclear, true);
    document.removeEventListener("scroll", cerrarMenu, true);
    globalThis.removeEventListener("resize", cerrarMenu);
    globalThis.removeEventListener("blur", cerrarMenu);
    cerrarActual = null;
  };
}

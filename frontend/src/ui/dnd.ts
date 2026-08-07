/**
 * Arrastrar y soltar.
 *
 * ## Requiere `dragDropEnabled: false` en `tauri.conf.json`
 *
 * Es lo contrario de lo que suena. Esa opción activa el manejo de arrastres
 * **del sistema operativo** —soltar ficheros del explorador sobre la ventana—,
 * y para hacerlo el host intercepta los eventos antes de que lleguen al
 * WebView. Con ella puesta, arrastrar dentro de la página no dispara `drop`
 * nunca: el HTML está bien, los manejadores están puestos, y no pasa nada.
 *
 * Localify no acepta ficheros sueltos, así que la opción va desactivada. Como
 * `tauri.conf.json` no admite comentarios, la razón vive aquí.
 *
 * ## El arrastre no puede ser el único camino
 *
 * Aunque funcione, arrastrar exige mantener el botón pulsado y cruzar la
 * ventana. Toda acción que se pueda hacer arrastrando tiene que poder hacerse
 * también desde el menú contextual: ver `opciones-pista.ts`.
 *
 * ## Por qué un tipo MIME propio
 *
 * `text/plain` funcionaría, pero entonces soltar texto arrastrado desde el
 * navegador o desde un editor activaría las zonas de la aplicación. Con un tipo
 * propio (`application/x-localify-tracks`), solo lo que sale de Localify puede
 * soltarse en Localify.
 *
 * ## El truco de `dragover`
 *
 * Una zona solo acepta un soltado si su manejador de `dragover` llama a
 * `preventDefault`. Es contraintuitivo —parece que se está cancelando algo— y
 * es la causa número uno de que un `drop` no llegue nunca. Aquí está en un solo
 * sitio para no tener que recordarlo en cada zona.
 *
 * ## Reordenar necesita saber el lado
 *
 * Al arrastrar dentro de una lista, importa si se suelta en la mitad de arriba
 * o de abajo de una fila: determina si la pista va antes o después. Se calcula
 * con el punto medio de la fila y se refleja con una línea, para que el usuario
 * vea dónde va a caer antes de soltar.
 */

/** Tipo MIME de un arrastre de pistas. El contenido es JSON con sus IDs. */
export const TIPO_PISTAS = "application/x-localify-tracks";

/** Tipo MIME de un arrastre de entradas de playlist, para reordenar. */
export const TIPO_ENTRADA = "application/x-localify-entry";

/** Marca un elemento como origen de arrastre de pistas. */
export function arrastrable(
  el: HTMLElement,
  ids: () => readonly string[],
): () => void {
  el.draggable = true;

  const alEmpezar = (e: DragEvent): void => {
    const lista = ids();
    if (lista.length === 0) return;
    e.dataTransfer?.setData(TIPO_PISTAS, JSON.stringify(lista));
    if (e.dataTransfer) e.dataTransfer.effectAllowed = "copyMove";
    el.classList.add("is-dragging");
  };
  const alTerminar = (): void => el.classList.remove("is-dragging");

  el.addEventListener("dragstart", alEmpezar);
  el.addEventListener("dragend", alTerminar);

  return () => {
    el.removeEventListener("dragstart", alEmpezar);
    el.removeEventListener("dragend", alTerminar);
    el.draggable = false;
  };
}

/** Marca un elemento como entrada reordenable de una playlist. */
export function reordenable(el: HTMLElement, entryId: () => string): () => void {
  el.draggable = true;

  const alEmpezar = (e: DragEvent): void => {
    e.dataTransfer?.setData(TIPO_ENTRADA, entryId());
    if (e.dataTransfer) e.dataTransfer.effectAllowed = "move";
    el.classList.add("is-dragging");
  };
  const alTerminar = (): void => el.classList.remove("is-dragging");

  el.addEventListener("dragstart", alEmpezar);
  el.addEventListener("dragend", alTerminar);

  return () => {
    el.removeEventListener("dragstart", alEmpezar);
    el.removeEventListener("dragend", alTerminar);
    el.draggable = false;
  };
}

/**
 * Convierte un elemento en zona de soltado para pistas.
 *
 * `manejar` recibe los identificadores. Si falla, la zona no se queda marcada:
 * un error no debe dejar la interfaz con un resaltado permanente.
 */
export function zonaDeSoltado(
  el: HTMLElement,
  tipo: string,
  manejar: (datos: string[]) => void | Promise<void>,
): () => void {
  const alPasar = (e: DragEvent): void => {
    if (!e.dataTransfer?.types.includes(tipo)) return;
    // Sin esto, `drop` no se dispara nunca.
    e.preventDefault();
    e.dataTransfer.dropEffect = "copy";
    el.classList.add("is-drop-target");
  };

  const alSalir = (): void => el.classList.remove("is-drop-target");

  const alSoltar = (e: DragEvent): void => {
    const crudo = e.dataTransfer?.getData(tipo);
    el.classList.remove("is-drop-target");
    if (!crudo) return;
    e.preventDefault();

    let datos: string[];
    try {
      const analizado: unknown = JSON.parse(crudo);
      datos = Array.isArray(analizado) ? analizado.map(String) : [crudo];
    } catch {
      // No era JSON: es un identificador suelto.
      datos = [crudo];
    }
    void manejar(datos);
  };

  el.addEventListener("dragover", alPasar);
  el.addEventListener("dragleave", alSalir);
  el.addEventListener("drop", alSoltar);

  return () => {
    el.removeEventListener("dragover", alPasar);
    el.removeEventListener("dragleave", alSalir);
    el.removeEventListener("drop", alSoltar);
    el.classList.remove("is-drop-target");
  };
}

/** Dónde caería lo que se está arrastrando, respecto a una fila. */
export type Lado = "antes" | "despues";

/** Mitad de la fila sobre la que está el cursor. */
export function ladoDeSoltado(fila: HTMLElement, e: DragEvent): Lado {
  const caja = fila.getBoundingClientRect();
  return e.clientY < caja.top + caja.height / 2 ? "antes" : "despues";
}

/**
 * Zona de reordenación sobre una lista.
 *
 * `filaDe` localiza la fila bajo el cursor; `indiceDe` dice qué posición ocupa.
 * `manejar` recibe la entrada arrastrada y el índice destino ya resuelto.
 */
export function zonaDeReordenacion(
  contenedor: HTMLElement,
  filaDe: (destino: EventTarget | null) => HTMLElement | null,
  indiceDe: (fila: HTMLElement) => number,
  manejar: (entryId: string, indice: number) => void | Promise<void>,
): () => void {
  let marcada: HTMLElement | null = null;

  const limpiar = (): void => {
    marcada?.classList.remove("is-drop-before", "is-drop-after");
    marcada = null;
  };

  const alPasar = (e: DragEvent): void => {
    if (!e.dataTransfer?.types.includes(TIPO_ENTRADA)) return;
    e.preventDefault();
    e.dataTransfer.dropEffect = "move";

    const fila = filaDe(e.target);
    if (!fila) return;
    if (marcada !== fila) limpiar();

    marcada = fila;
    const lado = ladoDeSoltado(fila, e);
    fila.classList.toggle("is-drop-before", lado === "antes");
    fila.classList.toggle("is-drop-after", lado === "despues");
  };

  const alSoltar = (e: DragEvent): void => {
    const entryId = e.dataTransfer?.getData(TIPO_ENTRADA);
    const fila = filaDe(e.target);
    const lado = fila ? ladoDeSoltado(fila, e) : "despues";
    limpiar();
    if (!entryId || !fila) return;
    e.preventDefault();

    const base = indiceDe(fila);
    void manejar(entryId, lado === "antes" ? base : base + 1);
  };

  contenedor.addEventListener("dragover", alPasar);
  contenedor.addEventListener("dragleave", limpiar);
  contenedor.addEventListener("drop", alSoltar);

  return () => {
    contenedor.removeEventListener("dragover", alPasar);
    contenedor.removeEventListener("dragleave", limpiar);
    contenedor.removeEventListener("drop", alSoltar);
    limpiar();
  };
}

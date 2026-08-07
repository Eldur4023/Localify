/**
 * Traducción.
 *
 * El backend nunca devuelve texto traducido: devuelve claves y parámetros
 * (ADR-012). Traducir es presentación, y mantenerlo aquí permite cambiar de
 * idioma sin volver a consultar nada ni reiniciar.
 *
 * ## Cambio en caliente
 *
 * Los elementos que muestran texto se registran con [`traducible`]. Al cambiar
 * de idioma se recorren y se vuelven a rellenar. No hay reconstrucción de
 * vistas ni pérdida de estado: la lista sigue por donde iba y el reproductor no
 * se entera.
 *
 * Se usa un `WeakRef` por elemento: un nodo que se va del DOM no debe
 * mantenerse vivo solo porque tradujo texto una vez.
 *
 * ## Una clave que falta se ve
 *
 * Devuelve la clave entre corchetes en vez de una cadena vacía. Un hueco en
 * blanco pasa desapercibido durante meses; `[playlist.rename]` en pantalla se
 * arregla el mismo día.
 */

import es from "./es.json" with { type: "json" };
import en from "./en.json" with { type: "json" };

export type Idioma = "es" | "en";

type Catalogo = Record<string, string>;

const CATALOGOS: Record<Idioma, Catalogo> = {
  es: es as Catalogo,
  en: en as Catalogo,
};

/** Clave del ajuste guardado en el navegador. */
const ALMACEN = "localify.idioma";

let actual: Idioma = detectar();

/**
 * Idioma inicial.
 *
 * Se prefiere lo que el usuario eligió; si no ha elegido, el del sistema. Que
 * la primera impresión esté en su idioma importa más que cualquier ajuste que
 * tenga que ir a buscar.
 */
function detectar(): Idioma {
  const guardado = globalThis.localStorage?.getItem(ALMACEN);
  if (guardado === "es" || guardado === "en") return guardado;
  return globalThis.navigator?.language?.startsWith("es") ? "es" : "en";
}

/** Elementos que hay que repintar al cambiar de idioma. */
interface Registro {
  readonly ref: WeakRef<HTMLElement>;
  readonly aplicar: (el: HTMLElement) => void;
}

const registrados: Registro[] = [];
const oyentes = new Set<() => void>();

/**
 * Texto de una clave, con sus parámetros sustituidos.
 *
 * Los parámetros van como `{nombre}` en el catálogo. Se sustituyen por texto
 * plano: el resultado siempre se asigna a `textContent`, nunca a `innerHTML`,
 * así que un nombre de playlist con `<script>` es solo texto.
 */
export function t(clave: string, params?: Record<string, string | number>): string {
  const plantilla = CATALOGOS[actual][clave] ?? CATALOGOS.en[clave];
  if (plantilla === undefined) return `[${clave}]`;
  if (!params) return plantilla;

  return plantilla.replace(/\{(\w+)\}/g, (coincidencia, nombre: string) => {
    const valor = params[nombre];
    return valor === undefined ? coincidencia : String(valor);
  });
}

/**
 * Registra un elemento para que se retraduzca al cambiar de idioma.
 *
 * Devuelve el propio elemento, para poder encadenarlo al construirlo.
 */
export function traducible(
  el: HTMLElement,
  aplicar: (el: HTMLElement) => void,
): HTMLElement {
  aplicar(el);
  registrados.push({ ref: new WeakRef(el), aplicar });
  return el;
}

/** Atajo: un elemento cuyo texto es una clave. */
export function textoTraducible(
  el: HTMLElement,
  clave: string,
  params?: Record<string, string | number>,
): HTMLElement {
  return traducible(el, (e) => {
    e.textContent = t(clave, params);
  });
}

/** Idioma en uso. */
export function idioma(): Idioma {
  return actual;
}

/** Cambia de idioma y repinta todo lo registrado. */
export function cambiarIdioma(nuevo: Idioma): void {
  if (nuevo === actual) return;
  actual = nuevo;
  globalThis.localStorage?.setItem(ALMACEN, nuevo);
  document.documentElement.lang = nuevo;

  // Se recorre de atrás adelante para poder quitar los nodos muertos sin
  // desordenar los índices que quedan por visitar.
  for (let i = registrados.length - 1; i >= 0; i -= 1) {
    const registro = registrados[i];
    const el = registro?.ref.deref();
    if (!registro || !el) {
      registrados.splice(i, 1);
      continue;
    }
    registro.aplicar(el);
  }

  for (const oyente of oyentes) oyente();
}

/**
 * Avisa cuando cambia el idioma.
 *
 * Para lo que no es un elemento con texto: una lista virtualizada tiene que
 * repintar sus filas, que se rellenan bajo demanda y no están registradas.
 */
export function alCambiarIdioma(oyente: () => void): () => void {
  oyentes.add(oyente);
  return () => oyentes.delete(oyente);
}

/** Cuántos elementos siguen registrados. Para diagnóstico. */
export function registradosVivos(): number {
  return registrados.filter((r) => r.ref.deref() !== undefined).length;
}

document.documentElement.lang = actual;

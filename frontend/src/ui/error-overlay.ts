/**
 * Errores no capturados, visibles.
 *
 * Sin esto, un fallo del frontend deja la ventana a medio pintar y en silencio:
 * no hay consola a la vista, y la única pista es "falta un trozo de la
 * interfaz". Diagnosticar eso cuesta media hora; leerlo en pantalla, un
 * segundo.
 *
 * Se muestra el mensaje y el origen, no la pila entera: la pila de un módulo
 * transpilado sin mapas de origen no aporta nada legible, y ocuparía la
 * pantalla.
 *
 * En un binario de publicación esto no debería mostrarse; hasta que exista la
 * distinción, es mejor verlo que no verlo.
 */

let capa: HTMLElement | null = null;

function contenedor(): HTMLElement {
  if (capa) return capa;
  const el = document.createElement("div");
  el.className = "errores";
  el.setAttribute("role", "alert");
  document.body.append(el);
  capa = el;
  return el;
}

/** Muestra un error en pantalla. */
export function mostrarError(titulo: string, detalle: string): void {
  const el = contenedor();

  const linea = document.createElement("div");
  linea.className = "errores__linea";

  const t = document.createElement("strong");
  t.textContent = titulo;
  const d = document.createElement("span");
  d.textContent = detalle;

  const cerrar = document.createElement("button");
  cerrar.type = "button";
  cerrar.textContent = "×";
  cerrar.className = "errores__cerrar";
  cerrar.addEventListener("click", () => linea.remove());

  linea.append(t, d, cerrar);
  el.prepend(linea);

  // Solo los últimos: un bucle que falla cada fotograma llenaría la memoria
  // con mensajes que nadie va a leer.
  while (el.childElementCount > 5) el.lastElementChild?.remove();
}

/** Engancha los manejadores globales. Devuelve cómo soltarlos. */
export function instalarCapturaDeErrores(): () => void {
  const alFallar = (e: ErrorEvent): void => {
    const donde = e.filename ? `${archivo(e.filename)}:${e.lineno}` : "";
    mostrarError(e.message, donde);
  };

  const alRechazar = (e: PromiseRejectionEvent): void => {
    const motivo: unknown = e.reason;
    const texto =
      motivo instanceof Error
        ? `${motivo.name}: ${motivo.message}`
        : String(motivo);
    mostrarError(texto, "promesa sin capturar");
  };

  globalThis.addEventListener("error", alFallar);
  globalThis.addEventListener("unhandledrejection", alRechazar);

  return () => {
    globalThis.removeEventListener("error", alFallar);
    globalThis.removeEventListener("unhandledrejection", alRechazar);
  };
}

/** Último tramo de una URL, que es lo único que aporta información. */
function archivo(url: string): string {
  return url.split("/").slice(-2).join("/");
}

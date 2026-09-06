/**
 * Iconos SVG en línea.
 *
 * ## Por qué en línea y no una fuente ni sprites
 *
 * Una fuente de iconos es un fichero más que descargar, que puede fallar y que
 * mientras tanto deja cuadrados en su sitio. Un sprite externo choca con la
 * política de contenido y añade una petición. En línea no hay petición, no hay
 * destello y el icono hereda `currentColor`, que es lo que hace que un botón
 * activo se tiña sin duplicar el icono.
 *
 * ## Todos en la misma rejilla
 *
 * `viewBox="0 0 24 24"` para todos. Sin eso, alinear un icono junto a otro
 * exige ajustes por icono, y siempre queda uno descolocado.
 *
 * Los trazos están escritos a mano para este proyecto y son deliberadamente
 * simples: la interfaz se lee mejor con formas sólidas que con contornos finos.
 */

/** Nombres disponibles. Que sea un tipo cerrado evita iconos fantasma. */
export type Icono =
  | "home"
  | "search"
  | "library"
  | "heart"
  | "heart-filled"
  | "play"
  | "pause"
  | "next"
  | "previous"
  | "shuffle"
  | "repeat"
  | "repeat-one"
  | "volume"
  | "volume-mute"
  | "queue"
  | "settings"
  | "plus"
  | "more"
  | "chevron-left"
  | "chevron-right"
  | "chevron-down"
  | "close"
  | "check"
  | "download"
  | "cloud-download"
  | "alert"
  | "music"
  | "expand"
  | "collapse"
  | "clock";

/** Trazos de cada icono, sobre una rejilla de 24×24. */
const TRAZOS: Record<Icono, string> = {
  home: "M12 3 2 11h3v9h5v-6h4v6h5v-9h3L12 3Z",
  search:
    "M10.5 3a7.5 7.5 0 1 0 4.55 13.46l4.24 4.25 1.42-1.42-4.25-4.24A7.5 7.5 0 0 0 10.5 3Zm0 2a5.5 5.5 0 1 1 0 11 5.5 5.5 0 0 1 0-11Z",
  library: "M4 3h2v18H4V3Zm4 0h2v18H8V3Zm5.4.5 1.9-.5 4.7 17.4-1.9.5L13.4 3.5Z",
  heart:
    "M12 20.3 4.6 13a4.9 4.9 0 0 1 0-7 4.9 4.9 0 0 1 7 0l.4.4.4-.4a4.9 4.9 0 0 1 7 7L12 20.3Zm-6-8.7 6 5.9 6-5.9a2.9 2.9 0 1 0-4.1-4.1L12 9.2l-1.9-1.7A2.9 2.9 0 1 0 6 11.6Z",
  "heart-filled":
    "M12 20.3 4.6 13a4.9 4.9 0 0 1 7-7l.4.4.4-.4a4.9 4.9 0 0 1 7 7L12 20.3Z",
  play: "M7 4v16l13-8L7 4Z",
  pause: "M6 4h4v16H6V4Zm8 0h4v16h-4V4Z",
  next: "M6 4l10 8-10 8V4Zm11 0h2v16h-2V4Z",
  previous: "M18 4v16L8 12l10-8ZM5 4h2v16H5V4Z",
  shuffle:
    "M17 3l4 4-4 4V8h-2.2l-2.3 3 2.3 3H17v-3l4 4-4 4v-3h-3.2l-2.9-3.8L7.8 18H3v-2h3.8l3-4-3-4H3V6h4.8l3 4 2.9-4H17V3Z",
  repeat: "M7 5h10v3l4-4-4-4v3H5v6h2V5Zm10 14H7v-3l-4 4 4 4v-3h12v-6h-2v4Z",
  "repeat-one":
    "M7 5h10v3l4-4-4-4v3H5v6h2V5Zm10 14H7v-3l-4 4 4 4v-3h12v-6h-2v4Zm-5-8h-1.5l-2 1v1.5l1.5-.7V17h2v-6Z",
  volume: "M4 9v6h4l5 5V4L8 9H4Zm12.5 3a4.5 4.5 0 0 0-2.5-4v8a4.5 4.5 0 0 0 2.5-4Z",
  "volume-mute":
    "M4 9v6h4l5 5V4L8 9H4Zm17.2-.6-1.4-1.4-2.3 2.3-2.3-2.3-1.4 1.4 2.3 2.3-2.3 2.3 1.4 1.4 2.3-2.3 2.3 2.3 1.4-1.4-2.3-2.3 2.3-2.3Z",
  queue: "M3 5h12v2H3V5Zm0 4h12v2H3V9Zm0 4h8v2H3v-2Zm14-8v8.2a3 3 0 1 0 2 2.8V7h3V5h-5Z",
  settings:
    "M12 8a4 4 0 1 0 0 8 4 4 0 0 0 0-8Zm0 6a2 2 0 1 1 0-4 2 2 0 0 1 0 4Zm9-2c0-.6 0-1.2-.1-1.7l2-1.6-2-3.4-2.4 1a8 8 0 0 0-3-1.7L15 2H9l-.5 2.6a8 8 0 0 0-3 1.7l-2.4-1-2 3.4 2 1.6a9.7 9.7 0 0 0 0 3.4l-2 1.6 2 3.4 2.4-1a8 8 0 0 0 3 1.7L9 22h6l.5-2.6a8 8 0 0 0 3-1.7l2.4 1 2-3.4-2-1.6c.1-.5.1-1.1.1-1.7Z",
  plus: "M11 5h2v6h6v2h-6v6h-2v-6H5v-2h6V5Z",
  more: "M6 10a2 2 0 1 0 0 4 2 2 0 0 0 0-4Zm6 0a2 2 0 1 0 0 4 2 2 0 0 0 0-4Zm6 0a2 2 0 1 0 0 4 2 2 0 0 0 0-4Z",
  "chevron-left": "M15.4 6.4 14 5l-7 7 7 7 1.4-1.4L9.8 12l5.6-5.6Z",
  "chevron-right": "M8.6 6.4 10 5l7 7-7 7-1.4-1.4L14.2 12 8.6 6.4Z",
  "chevron-down": "M6.4 8.6 5 10l7 7 7-7-1.4-1.4L12 14.2 6.4 8.6Z",
  close: "M19 6.4 17.6 5 12 10.6 6.4 5 5 6.4l5.6 5.6L5 17.6 6.4 19l5.6-5.6 5.6 5.6 1.4-1.4-5.6-5.6L19 6.4Z",
  check: "M9 16.2 4.8 12l-1.4 1.4L9 19 21 7l-1.4-1.4L9 16.2Z",
  download: "M11 3h2v9h4l-5 5-5-5h4V3ZM4 19h16v2H4v-2Z",
  // El indicador de "ya está en disco": la nube clásica con la flecha, para
  // que se distinga de un botón de acción como "download" de arriba.
  "cloud-download":
    "M19.35 10.04A7.49 7.49 0 0 0 12 4a7.49 7.99 0 0 0-7.35 6.04A5.5 5.5 0 0 0 6.5 20H19a5 5 0 0 0 .35-9.96ZM17 13l-5 5-5-5h3V9h4v4h3Z",
  alert: "M12 2 1 21h22L12 2Zm1 15h-2v-2h2v2Zm0-4h-2V9h2v4Z",
  music: "M12 3v10.6A4 4 0 1 0 14 17V7h4V3h-6Z",
  expand: "M4 4h7v2H6v5H4V4Zm9 0h7v7h-2V6h-5V4ZM4 13h2v5h5v2H4v-7Zm14 0h2v7h-7v-2h5v-5Z",
  collapse: "M9 4h2v7H4V9h5V4Zm4 0h2v5h5v2h-7V4ZM4 13h7v7H9v-5H4v-2Zm9 2h7v2h-5v5h-2v-7Z",
  // Titula la columna de duración. Es un aro y dos agujas, no un disco: relleno
  // sólido, a 16 px se ve como una mancha.
  clock:
    "M12 2a10 10 0 1 0 0 20 10 10 0 0 0 0-20Zm0 2a8 8 0 1 1 0 16 8 8 0 0 1 0-16Zm-1 3v6l5 3 1-1.7-4-2.3V7h-2Z",
};

/**
 * Crea un icono.
 *
 * `aria-hidden` porque el icono nunca es la etiqueta: el botón que lo contiene
 * lleva su `aria-label`. Un lector de pantalla que anunciara "play play"
 * sería peor que uno que anuncia "play".
 */
export function icono(nombre: Icono, tamano = 20): SVGSVGElement {
  const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  svg.setAttribute("viewBox", "0 0 24 24");
  svg.setAttribute("width", String(tamano));
  svg.setAttribute("height", String(tamano));
  svg.setAttribute("aria-hidden", "true");
  svg.setAttribute("focusable", "false");
  svg.classList.add("icono");

  const path = document.createElementNS("http://www.w3.org/2000/svg", "path");
  path.setAttribute("d", TRAZOS[nombre]);
  // `currentColor` es lo que permite que un botón activo tiña su icono sin
  // duplicarlo ni recolorear nada a mano.
  path.setAttribute("fill", "currentColor");
  svg.append(path);

  return svg;
}

/** Botón con icono y etiqueta accesible. */
export function botonIcono(
  nombre: Icono,
  etiqueta: string,
  alPulsar: () => void,
  opciones: { tamano?: number; clase?: string } = {},
): HTMLButtonElement {
  const boton = document.createElement("button");
  boton.type = "button";
  boton.className = `bicono ${opciones.clase ?? ""}`.trim();
  boton.setAttribute("aria-label", etiqueta);
  boton.title = etiqueta;
  boton.append(icono(nombre, opciones.tamano));
  boton.addEventListener("click", alPulsar);
  return boton;
}

/** Cambia el icono de un botón sin recrearlo. */
export function cambiarIcono(boton: HTMLElement, nombre: Icono, tamano = 20): void {
  boton.replaceChildren(icono(nombre, tamano));
}

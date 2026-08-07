/**
 * Barra de reproducción.
 *
 * Vive fuera del router y no se desmonta nunca: navegar no puede cortar la
 * música ni parpadear los controles.
 *
 * ## La posición se sondea; el resto llega por eventos
 *
 * Son dos ritmos distintos a propósito. La posición cambia sesenta veces por
 * segundo y no vale la pena mandarla por el bus: se pide con un temporizador a
 * 250 ms, que es un comando que solo lee atómicos. Cambiar de canción o pausar
 * ocurre cuando el usuario actúa, y eso sí llega por evento.
 *
 * Mandar la posición como evento saturaría el puente IPC para mover una barra
 * de progreso.
 *
 * ## Arrastrar la barra no salta hasta soltar
 *
 * Mientras se arrastra, la barra deja de seguir a la reproducción: si no, cada
 * sondeo tiraría del pulgar de vuelta y sería imposible apuntar.
 */

import { player } from "../ipc/client.js";
import type { PlayerStateDto } from "../ipc/types.gen.js";
import { alRecibir } from "../ipc/events.js";
import { alCambiarIdioma, t } from "../i18n/index.js";
import { botonIcono, cambiarIcono, icono } from "../ui/icons.js";
import { ponerPortadaDePista } from "../ui/cards.js";
import { abrirMenu } from "../ui/menu.js";
import { opcionesDePista } from "../ui/opciones-pista.js";

/** Cada cuánto se pide la posición. */
const SONDEO_MS = 250;

/** Formatea una duración como `m:ss`. */
export function duracion(ms: number): string {
  const total = Math.max(0, Math.floor(ms / 1000));
  const m = Math.floor(total / 60);
  const s = total % 60;
  return `${m}:${String(s).padStart(2, "0")}`;
}

export interface PlayerBar {
  /** Vuelve a pedir el estado completo. Es la resincronización. */
  refrescar(): void;
  destroy(): void;
}

export function mountPlayerBar(
  contenedor: HTMLElement,
  opciones: { alAbrirCola: () => void; alAmpliar: () => void },
): PlayerBar {
  contenedor.replaceChildren();

  // ── Izquierda: qué suena ────────────────────────────────────────────────
  const info = document.createElement("div");
  info.className = "pb__info";

  const portada = document.createElement("div");
  portada.className = "pb__cover";
  portada.append(icono("music", 22));

  const textos = document.createElement("div");
  textos.className = "pb__texts";
  const titulo = document.createElement("div");
  titulo.className = "pb__title";
  const artista = document.createElement("div");
  artista.className = "pb__artist";
  textos.append(titulo, artista);

  const megusta = botonIcono("heart", "", () => {}, { tamano: 18 });
  info.append(portada, textos, megusta);

  /**
   * La canción que suena acepta clic derecho, como cualquier otra fila.
   *
   * Es la que más a mano está de toda la aplicación —lleva ahí desde que
   * empezó a sonar— y era la única sin menú: para meterla en una playlist había
   * que ir a buscarla a otra pantalla.
   *
   * El contexto es `single`: lo que suena ya viene de algún sitio, y
   * reproducirla desde aquí no debería cambiar lo que va después.
   */
  const alMenuDeLaActual = (e: MouseEvent): void => {
    const pista = estado?.track;
    if (!pista) return;
    e.preventDefault();
    abrirMenu(
      e.clientX,
      e.clientY,
      opcionesDePista(pista, {
        contexto: () => ({ kind: "single" }),
        alCambiar: refrescar,
      }),
    );
  };
  info.addEventListener("contextmenu", alMenuDeLaActual);

  // ── Centro: controles y progreso ────────────────────────────────────────
  const centro = document.createElement("div");
  centro.className = "pb__center";

  const controles = document.createElement("div");
  controles.className = "pb__controls";

  const aleatorio = botonIcono("shuffle", "", () => void alternarAleatorio(), { tamano: 18 });
  const anterior = botonIcono("previous", "", () => void player.previous(), { tamano: 20 });
  const tocar = botonIcono("play", "", () => void player.toggle(), {
    tamano: 20,
    clase: "pb__play",
  });
  const siguiente = botonIcono("next", "", () => void player.next(), { tamano: 20 });
  const repetir = botonIcono("repeat", "", () => void rotarRepeticion(), { tamano: 18 });

  controles.append(aleatorio, anterior, tocar, siguiente, repetir);

  const barra = document.createElement("div");
  barra.className = "pb__progress";

  const transcurrido = document.createElement("span");
  transcurrido.className = "pb__time";
  const total = document.createElement("span");
  total.className = "pb__time";

  const rango = document.createElement("input");
  rango.type = "range";
  rango.min = "0";
  rango.max = "1000";
  rango.value = "0";
  rango.className = "pb__seek";

  barra.append(transcurrido, rango, total);
  centro.append(controles, barra);

  // ── Derecha: cola, letra, volumen ───────────────────────────────────────
  const derecha = document.createElement("div");
  derecha.className = "pb__right";

  const cola = botonIcono("queue", "", opciones.alAbrirCola, { tamano: 18 });
  const ampliar = botonIcono("expand", "", opciones.alAmpliar, { tamano: 18 });
  const silencio = botonIcono("volume", "", () => void alternarSilencio(), { tamano: 18 });

  const volumen = document.createElement("input");
  volumen.type = "range";
  volumen.min = "0";
  volumen.max = "100";
  volumen.value = "100";
  volumen.className = "pb__volume";

  derecha.append(cola, ampliar, silencio, volumen);
  contenedor.append(info, centro, derecha);

  // ── Estado local ────────────────────────────────────────────────────────
  let estado: PlayerStateDto | null = null;
  let arrastrando = false;
  let volumenPrevio = 1;
  /**
   * Pista cuya portada está puesta ahora mismo.
   *
   * Antes se guardaba el **álbum**, y las canciones sin álbum —lo importado de
   * una lista pública llega así— compartían todas la misma clave `null`: la
   * comparación las daba por iguales y el reproductor se quedaba sin carátula
   * para siempre.
   */
  let pistaPintada: string | null = null;

  function etiquetas(): void {
    aleatorio.setAttribute("aria-label", t("player.shuffle"));
    aleatorio.title = t("player.shuffle");
    anterior.setAttribute("aria-label", t("player.previous"));
    anterior.title = t("player.previous");
    siguiente.setAttribute("aria-label", t("player.next"));
    siguiente.title = t("player.next");
    cola.setAttribute("aria-label", t("player.queue"));
    cola.title = t("player.queue");
    ampliar.setAttribute("aria-label", t("player.expand"));
    ampliar.title = t("player.expand");
    volumen.setAttribute("aria-label", t("player.volume"));
    rango.setAttribute("aria-label", t("player.play"));
    pintarBoton();
    pintarRepeticion();
    if (!estado?.track) titulo.textContent = t("player.nothing");
  }

  function pintarBoton(): void {
    const sonando = estado?.status === "playing";
    cambiarIcono(tocar, sonando ? "pause" : "play", 20);
    const etiqueta = sonando ? t("player.pause") : t("player.play");
    tocar.setAttribute("aria-label", etiqueta);
    tocar.title = etiqueta;
  }

  function pintarRepeticion(): void {
    const modo = estado?.repeat ?? "off";
    cambiarIcono(repetir, modo === "track" ? "repeat-one" : "repeat", 18);
    repetir.classList.toggle("is-active", modo !== "off");
    const etiqueta =
      modo === "off"
        ? t("player.repeat_off")
        : modo === "queue"
          ? t("player.repeat_queue")
          : t("player.repeat_track");
    repetir.setAttribute("aria-label", etiqueta);
    repetir.title = etiqueta;
  }

  function pintar(nuevo: PlayerStateDto): void {
    estado = nuevo;

    titulo.textContent = nuevo.track?.title ?? t("player.nothing");
    artista.textContent = nuevo.track?.artistDisplay ?? "";
    cambiarIcono(megusta, nuevo.track?.isFavorite ? "heart-filled" : "heart", 18);
    megusta.classList.toggle("is-active", nuevo.track?.isFavorite === true);

    // La portada se rehace en cada cambio de pista: reutilizar la etiqueta
    // dejaría un instante la carátula de la canción anterior sobre el título
    // de la nueva.
    if (nuevo.track?.id !== pistaPintada) {
      pistaPintada = nuevo.track?.id ?? null;
      portada.querySelector(".portada")?.remove();
      if (nuevo.track) {
        ponerPortadaDePista(portada, nuevo.track.id);
      }
    }

    total.textContent = duracion(nuevo.durationMs);
    aleatorio.classList.toggle("is-active", nuevo.shuffle);
    volumen.value = String(Math.round(nuevo.volume * 100));
    pintarVolumen();

    pintarBoton();
    pintarRepeticion();
    pintarPosicion(nuevo.positionMs, nuevo.durationMs);
  }

  function pintarPosicion(pos: number, dur: number): void {
    transcurrido.textContent = duracion(pos);
    if (arrastrando) return;
    const fraccion = dur > 0 ? pos / dur : 0;
    rango.value = String(Math.round(fraccion * 1000));
    // La variable alimenta el degradado de la barra: sin ella habría que
    // pintar dos elementos superpuestos.
    rango.style.setProperty("--avance", `${fraccion * 100}%`);
  }

  /** Colorea la parte llena de la barra de volumen. */
  function pintarVolumen(): void {
    volumen.style.setProperty("--avance", `${volumen.value}%`);
  }

  async function alternarAleatorio(): Promise<void> {
    pintar(await player.setShuffle(!(estado?.shuffle ?? false)));
  }

  async function rotarRepeticion(): Promise<void> {
    const actual = estado?.repeat ?? "off";
    const siguiente = actual === "off" ? "queue" : actual === "queue" ? "track" : "off";
    pintar(await player.setRepeat(siguiente));
  }

  async function alternarSilencio(): Promise<void> {
    const actual = estado?.volume ?? 1;
    if (actual > 0) {
      volumenPrevio = actual;
      pintar(await player.setVolume(0));
      cambiarIcono(silencio, "volume-mute", 18);
    } else {
      pintar(await player.setVolume(volumenPrevio));
      cambiarIcono(silencio, "volume", 18);
    }
  }

  // ── Interacción ─────────────────────────────────────────────────────────
  rango.addEventListener("pointerdown", () => {
    arrastrando = true;
  });
  rango.addEventListener("input", () => {
    // Mientras se arrastra se muestra el destino, aunque el audio siga donde
    // estaba: sin esta respuesta inmediata, arrastrar parece que no hace nada.
    const dur = estado?.durationMs ?? 0;
    const destino = (Number(rango.value) / 1000) * dur;
    transcurrido.textContent = duracion(destino);
    rango.style.setProperty("--avance", `${Number(rango.value) / 10}%`);
  });
  const alSoltarBarra = (): void => {
    if (!arrastrando) return;
    arrastrando = false;
    const dur = estado?.durationMs ?? 0;
    void player.seek(Math.round((Number(rango.value) / 1000) * dur)).then(pintar);
  };
  rango.addEventListener("pointerup", alSoltarBarra);
  rango.addEventListener("change", alSoltarBarra);

  volumen.addEventListener("input", () => {
    // El relleno se pinta aquí y no al llegar la respuesta: esperar al backend
    // dejaría la barra un instante por detrás del pulgar en cada movimiento.
    pintarVolumen();
    void player.setVolume(Number(volumen.value) / 100);
  });

  megusta.addEventListener("click", () => {
    const pista = estado?.track;
    if (!pista) return;
    void import("../ipc/client.js").then(({ library }) =>
      library.setFavorite(pista.id, !pista.isFavorite).then(refrescar),
    );
  });

  // ── Sincronización ──────────────────────────────────────────────────────
  function refrescar(): void {
    void player.getState().then(pintar).catch(() => {});
  }

  const dejarEventos = alRecibir((evento) => {
    switch (evento.type) {
      case "trackChanged":
      case "playStatusChanged":
      case "shuffleChanged":
      case "repeatModeChanged":
      case "volumeChanged":
        refrescar();
        break;
      default:
        break;
    }
  });

  const temporizador = globalThis.setInterval(() => {
    if (estado?.status !== "playing" || arrastrando) return;
    void player.position().then((p) => {
      pintarPosicion(p.positionMs, estado?.durationMs ?? 0);
    });
  }, SONDEO_MS);

  /**
   * La barra espaciadora pausa y reanuda.
   *
   * ## Escribir manda
   *
   * Es el detalle que hace o rompe este atajo: si se captura sin mirar dónde
   * está el foco, la caja de búsqueda deja de admitir espacios y escribir
   * «bohemian rhapsody» se vuelve imposible mientras el atajo parece funcionar.
   *
   * Tampoco se toca cuando el foco está en un botón o un enlace: el navegador
   * ya los activa con espacio, y hacerlo también aquí alternaría dos veces.
   */
  const alEspacio = (e: KeyboardEvent): void => {
    if (e.code !== "Space" || e.repeat) return;

    const activo = document.activeElement;
    const escribiendo =
      activo instanceof HTMLInputElement ||
      activo instanceof HTMLTextAreaElement ||
      activo instanceof HTMLSelectElement ||
      (activo instanceof HTMLElement && activo.isContentEditable);
    const yaLoManeja =
      activo instanceof HTMLButtonElement || activo instanceof HTMLAnchorElement;

    if (escribiendo || yaLoManeja) return;

    // Sin esto la página se desplazaría, que es lo que hace el espacio por
    // defecto en cualquier documento.
    e.preventDefault();
    void player.toggle().then(pintar);
  };
  globalThis.addEventListener("keydown", alEspacio);

  const dejarIdioma = alCambiarIdioma(etiquetas);
  etiquetas();
  // El relleno inicial, antes de que llegue el primer estado: si no, la barra
  // arranca vacía aunque el pulgar esté al máximo.
  pintarVolumen();
  refrescar();

  return {
    refrescar,
    destroy(): void {
      globalThis.clearInterval(temporizador);
      globalThis.removeEventListener("keydown", alEspacio);
      dejarEventos();
      dejarIdioma();
      contenedor.replaceChildren();
    },
  };
}

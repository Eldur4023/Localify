/**
 * Vista ampliada: portada grande y letra sincronizada.
 *
 * ## La línea activa se busca hacia atrás desde la última conocida
 *
 * Una letra son unas cien líneas y la posición se sondea cuatro veces por
 * segundo. Recorrerlas enteras en cada sondeo es barato, pero no hace falta:
 * el tiempo casi siempre avanza, así que se parte de la línea anterior y solo
 * se retrocede cuando el usuario salta hacia atrás.
 *
 * ## El desplazamiento se hace con `scrollTo`, no con `scrollIntoView`
 *
 * `scrollIntoView` desplaza **todos** los antepasados desplazables hasta que el
 * elemento se vea, y en una ventana con la barra lateral eso mueve cosas que no
 * son la letra. Calcular el destino sobre el propio contenedor es una resta y
 * no toca nada más.
 *
 * ## Sin letra no hay hueco vacío
 *
 * La mayoría de las canciones no la tendrán —no hay un proveedor de letras
 * configurado por defecto— así que cuando falta, la portada ocupa el centro en
 * lugar de dejar media pantalla en blanco.
 */

import type { LyricLineDto, LyricsDto, PlayerStateDto } from "../ipc/types.gen.js";
import { lyrics as api, player } from "../ipc/client.js";
import { alRecibir } from "../ipc/events.js";
import { alCambiarIdioma, t } from "../i18n/index.js";
import { botonIcono, icono } from "../ui/icons.js";
import { ponerPortada } from "../ui/cards.js";

/** Cada cuánto se comprueba qué línea toca. El mismo ritmo que la barra. */
const SONDEO_MS = 250;

export interface VistaAmpliada {
  abrir(): void;
  cerrar(): void;
  alternar(): void;
  abierta(): boolean;
  destroy(): void;
}

export function mountNowPlaying(contenedor: HTMLElement): VistaAmpliada {
  const el = document.createElement("section");
  el.className = "ampliada";
  el.hidden = true;

  const cerrarBoton = botonIcono("chevron-down", "", () => cerrar(), { tamano: 22 });
  cerrarBoton.classList.add("ampliada__cerrar");

  const arte = document.createElement("div");
  arte.className = "ampliada__arte";
  arte.append(icono("music", 96));
  // Álbum cuya portada grande está puesta.
  let albumPintado: string | null = null;

  const titulo = document.createElement("h1");
  titulo.className = "ampliada__titulo";
  const artista = document.createElement("p");
  artista.className = "ampliada__artista";
  const album = document.createElement("p");
  album.className = "ampliada__album";

  // El aviso de "sin letra" va con la canción, no donde iría la letra: ese
  // panel desaparece cuando no hay nada que poner en él, y un mensaje dentro de
  // algo que no se muestra no lo lee nadie.
  const sinLetra = document.createElement("p");
  sinLetra.className = "ampliada__sin-letra";
  sinLetra.hidden = true;

  const izquierda = document.createElement("div");
  izquierda.className = "ampliada__izquierda";
  izquierda.append(arte, titulo, artista, album, sinLetra);

  const letra = document.createElement("div");
  letra.className = "ampliada__letra";

  el.append(cerrarBoton, izquierda, letra);
  contenedor.append(el);

  let pistaActual: string | null = null;
  let actual: LyricsDto | null = null;
  let lineas: LyricLineDto[] = [];
  let nodos: HTMLElement[] = [];
  let activa = -1;
  let temporizador: number | null = null;

  function pintarCabecera(estado: PlayerStateDto): void {
    titulo.textContent = estado.track?.title ?? t("player.nothing");
    if (estado.track?.albumId !== albumPintado) {
      albumPintado = estado.track?.albumId ?? null;
      arte.querySelector(".portada")?.remove();
      ponerPortada(arte, albumPintado);
    }
    artista.textContent = estado.track?.artistDisplay ?? "";
    album.textContent = estado.track?.albumTitle ?? "";
  }

  function pintarLetra(): void {
    letra.replaceChildren();
    nodos = [];
    activa = -1;

    if (!actual) {
      sinLetra.textContent = t("lyrics.none");
      sinLetra.hidden = false;
      el.classList.add("ampliada--sin-letra");
      return;
    }
    sinLetra.hidden = true;
    el.classList.remove("ampliada--sin-letra");

    if (actual.synced && actual.synced.length > 0) {
      lineas = [...actual.synced];
      for (const linea of lineas) {
        const p = document.createElement("p");
        p.className = "ampliada__linea";
        // Una línea vacía en un LRC es un silencio instrumental. Se pinta
        // igual, con un espacio duro, para que el desplazamiento siga siendo
        // regular en vez de dar un salto.
        p.textContent = linea.text.length > 0 ? linea.text : " ";
        letra.append(p);
        nodos.push(p);
      }
      return;
    }

    lineas = [];
    const p = document.createElement("p");
    p.className = "ampliada__plana";
    p.textContent = actual.plain ?? t("lyrics.none");
    letra.append(p);
  }

  /** Índice de la línea que suena, o -1 antes de la primera. */
  function lineaEn(posicionMs: number): number {
    // Se parte de la anterior porque el tiempo casi siempre avanza; solo se
    // retrocede cuando el usuario ha saltado hacia atrás.
    let i = activa;
    if (i >= lineas.length) i = lineas.length - 1;
    while (i >= 0 && (lineas[i]?.atMs ?? 0) > posicionMs) i -= 1;
    while (i + 1 < lineas.length && (lineas[i + 1]?.atMs ?? 0) <= posicionMs) i += 1;
    return i;
  }

  function resaltar(indice: number): void {
    if (indice === activa) return;
    nodos[activa]?.classList.remove("is-activa");
    activa = indice;
    const nodo = nodos[indice];
    if (!nodo) return;
    nodo.classList.add("is-activa");

    // Centrar en el contenedor de la letra, sin tocar ningún otro scroll.
    const destino = nodo.offsetTop - letra.clientHeight / 2 + nodo.offsetHeight / 2;
    letra.scrollTo({ top: Math.max(0, destino), behavior: "smooth" });
  }

  async function cargarLetra(trackId: string | null): Promise<void> {
    pistaActual = trackId;
    if (!trackId) {
      actual = null;
      pintarLetra();
      return;
    }
    try {
      actual = await api.get(trackId);
    } catch {
      // Sin letra no es un error que merezca una alerta: se muestra el estado
      // vacío y se sigue.
      actual = null;
    }
    // La canción puede haber cambiado mientras se pedía.
    if (pistaActual !== trackId) return;
    pintarLetra();
  }

  async function sincronizar(): Promise<void> {
    const estado = await player.getState();
    pintarCabecera(estado);
    if (estado.track?.id !== pistaActual) await cargarLetra(estado.track?.id ?? null);
  }

  function sondear(): void {
    if (lineas.length === 0) return;
    void player
      .position()
      .then((p) => resaltar(lineaEn(p.positionMs)))
      .catch(() => {
        // Un sondeo perdido no rompe nada: el siguiente lo corrige.
      });
  }

  const dejarEventos = alRecibir((evento) => {
    if (el.hidden) return;
    if (evento.type === "trackChanged" || evento.type === "playStatusChanged") {
      void sincronizar();
    }
  });

  function abrir(): void {
    el.hidden = false;
    void sincronizar();
    // El sondeo solo corre con la vista abierta: mantenerlo de fondo sería un
    // comando cuatro veces por segundo para mover algo que nadie ve.
    temporizador ??= globalThis.setInterval(sondear, SONDEO_MS);
  }

  function cerrar(): void {
    el.hidden = true;
    if (temporizador !== null) {
      globalThis.clearInterval(temporizador);
      temporizador = null;
    }
  }

  const alTeclado = (e: KeyboardEvent): void => {
    if (e.key === "Escape" && !el.hidden) cerrar();
  };
  globalThis.addEventListener("keydown", alTeclado);

  function etiquetas(): void {
    el.setAttribute("aria-label", t("player.expand"));
    cerrarBoton.setAttribute("aria-label", t("player.collapse"));
    cerrarBoton.title = t("player.collapse");
    if (!actual) pintarLetra();
  }

  etiquetas();
  const dejarIdioma = alCambiarIdioma(etiquetas);

  return {
    abrir,
    cerrar,
    alternar(): void {
      if (el.hidden) abrir();
      else cerrar();
    },
    abierta: () => !el.hidden,
    destroy(): void {
      cerrar();
      dejarIdioma();
      dejarEventos();
      globalThis.removeEventListener("keydown", alTeclado);
      el.remove();
    },
  };
}

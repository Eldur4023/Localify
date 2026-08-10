/**
 * La fila de pista que aparece fuera de una lista virtualizada.
 *
 * ## Por qué existe aparte de `track-list`
 *
 * `mountTrackList` monta una lista **completa**: pide páginas, recicla nodos y
 * gestiona el foco. Hay dos sitios donde no hay lista que montar sino un puñado
 * de filas ya en la mano —lo más escuchado de un artista, los resultados de
 * búsqueda— y allí esa maquinaria no aporta nada.
 *
 * ## Por qué no una copia en cada uno
 *
 * Porque ya se hizo y se notó. La vista de artista se quedó con un menú
 * contextual de una sola opción —«reproducir»— mientras la de búsqueda ya usaba
 * `opcionesDePista`: desde el artista no se podía encolar una canción ni meterla
 * en una playlist, y no por decisión de nadie, sino porque el menú se escribió
 * dos veces y solo se arregló uno. Es el mismo argumento que puso
 * `opciones-pista.ts` donde está.
 */

import type { PlaybackContextDto, TrackRowDto } from "../ipc/types.gen.js";
import { player } from "../ipc/client.js";
import { duracion } from "../shell/player.js";
import { comienzoDePista } from "./cards.js";
import { arrastrable } from "./dnd.js";
import { abrirMenu } from "./menu.js";
import { opcionesDePista } from "./opciones-pista.js";

export interface OpcionesFilaSuelta {
  /** Contexto con el que reproducir al pulsarla. */
  contexto(): PlaybackContextDto;
  /**
   * Número de orden, si la lista los muestra.
   *
   * Es la posición dentro de lo que se está enseñando, no la del disco: en «lo
   * más escuchado» significa el puesto, y ahí sí dice algo.
   */
  readonly indice?: number;
  /**
   * Texto de la segunda columna.
   *
   * Cambia con la pantalla: en la búsqueda interesa el artista, y en la ficha de
   * un artista —donde ya se sabe quién es— interesa el álbum.
   */
  readonly secundario: string;
}

export interface FilaSuelta {
  readonly el: HTMLElement;
  destroy(): void;
}

export function filaSuelta(pista: TrackRowDto, opciones: OpcionesFilaSuelta): FilaSuelta {
  const el = document.createElement("div");
  el.className = "track track--suelta";

  if (opciones.indice !== undefined) {
    const num = document.createElement("span");
    num.className = "track__index";
    num.textContent = String(opciones.indice + 1);
    el.append(num);
  }

  el.append(comienzoDePista(pista.albumId, pista.id));

  const titulo = document.createElement("span");
  titulo.className = "track__title";
  titulo.textContent = pista.title;

  const secundario = document.createElement("span");
  secundario.className = "track__artist";
  secundario.textContent = opciones.secundario;

  const tiempo = document.createElement("span");
  tiempo.className = "track__time";
  tiempo.textContent = duracion(pista.durationMs);

  el.append(titulo, secundario, tiempo);

  const reproducir = (): void => {
    void player.playTrack(pista.id, opciones.contexto());
  };
  el.addEventListener("click", reproducir);
  el.addEventListener("contextmenu", (e) => {
    e.preventDefault();
    abrirMenu(e.clientX, e.clientY, opcionesDePista(pista, { contexto: opciones.contexto }));
  });

  const soltarArrastre = arrastrable(el, () => [pista.id]);

  return {
    el,
    destroy(): void {
      soltarArrastre();
    },
  };
}

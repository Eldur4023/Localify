/**
 * Diálogo para reasignar los metadatos de una pista a mano.
 *
 * Busca en el proveedor activo y deja elegir entre varios candidatos, en vez
 * de asumir que el primer resultado es el correcto: media docena de
 * grabaciones pueden compartir título y artista, y solo quien mira la
 * canción sabe cuál es la suya.
 *
 * No persiste nada por su cuenta. Devuelve el candidato elegido (o `null` si
 * se cancela) para que quien llama decida cuándo escribirlo.
 */

import type { TrackCandidateDto } from "../ipc/types.gen.js";
import { library } from "../ipc/client.js";
import { t } from "../i18n/index.js";
import { duracion } from "../shell/player.js";

const LIMITE = 15;

function artistas(c: TrackCandidateDto): string {
  return c.artists.map((a) => a.name).join(", ");
}

/**
 * Abre el diálogo con `consultaInicial` ya escrita. Resuelve al candidato
 * elegido, o a `null` si se cancela sin elegir ninguno.
 */
export function elegirCandidato(
  consultaInicial: string,
): Promise<TrackCandidateDto | null> {
  return new Promise((resolver) => {
    const dlg = document.createElement("dialog");
    dlg.className = "dialogo dialogo--reasignar";

    const form = document.createElement("form");
    form.method = "dialog";

    const h = document.createElement("h2");
    h.className = "dialogo__titulo";
    h.textContent = t("reassign.title");

    const fila = document.createElement("div");
    fila.className = "reasignar__busqueda";

    const entrada = document.createElement("input");
    entrada.type = "text";
    entrada.className = "dialogo__input";
    entrada.value = consultaInicial;
    entrada.autocomplete = "off";
    entrada.spellcheck = false;

    const buscar = document.createElement("button");
    buscar.type = "button";
    buscar.className = "boton";
    buscar.textContent = t("reassign.search");

    fila.append(entrada, buscar);

    const resultados = document.createElement("ul");
    resultados.className = "reasignar__resultados";

    const estado = document.createElement("p");
    estado.className = "dialogo__mensaje";
    estado.hidden = true;

    let elegido: TrackCandidateDto | null = null;

    function cerrar(): void {
      dlg.close();
    }

    function pintarResultados(candidatos: TrackCandidateDto[]): void {
      resultados.replaceChildren();
      estado.hidden = candidatos.length > 0;
      if (candidatos.length === 0) {
        estado.textContent = t("reassign.empty");
        return;
      }

      for (const c of candidatos) {
        const li = document.createElement("li");
        li.className = "reasignar__opcion";
        li.tabIndex = 0;

        const titulo = document.createElement("span");
        titulo.className = "reasignar__opcion-titulo";
        titulo.textContent = c.title;

        const detalle = document.createElement("span");
        detalle.className = "reasignar__opcion-detalle";
        const partes = [artistas(c), c.album?.title, duracion(c.durationMs)].filter(
          (p): p is string => Boolean(p),
        );
        detalle.textContent = partes.join(" · ");

        li.append(titulo, detalle);

        const elegir = (): void => {
          elegido = c;
          cerrar();
        };
        li.addEventListener("click", elegir);
        li.addEventListener("keydown", (e) => {
          if (e.key === "Enter") elegir();
        });

        resultados.append(li);
      }
    }

    async function ejecutarBusqueda(): Promise<void> {
      const consulta = entrada.value.trim();
      if (!consulta) return;

      buscar.disabled = true;
      estado.hidden = false;
      estado.textContent = t("common.loading");
      try {
        const candidatos = await library.searchCandidates(consulta, LIMITE);
        pintarResultados(candidatos);
      } catch (e) {
        estado.hidden = false;
        estado.textContent = String(e);
      }
      buscar.disabled = false;
    }

    buscar.addEventListener("click", () => void ejecutarBusqueda());
    entrada.addEventListener("keydown", (e) => {
      if (e.key === "Enter") {
        e.preventDefault();
        void ejecutarBusqueda();
      }
    });

    const acciones = document.createElement("div");
    acciones.className = "dialogo__acciones";
    const cancelar = document.createElement("button");
    cancelar.type = "button";
    cancelar.className = "boton boton--sutil";
    cancelar.textContent = t("common.cancel");
    cancelar.addEventListener("click", cerrar);
    acciones.append(cancelar);

    form.append(h, fila, estado, resultados, acciones);
    dlg.append(form);
    document.body.append(dlg);

    dlg.addEventListener("click", (e) => {
      if (e.target === dlg) cerrar();
    });
    dlg.addEventListener("close", () => {
      dlg.remove();
      resolver(elegido);
    });

    dlg.showModal();
    entrada.select();
    // Búsqueda inicial con el título actual: quien abre esto casi siempre
    // quiere corregir lo que ya hay, no partir de cero.
    if (consultaInicial.trim()) void ejecutarBusqueda();
  });
}

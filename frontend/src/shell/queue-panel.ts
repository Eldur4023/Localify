/**
 * Panel lateral de la cola.
 *
 * ## Vive en el armazón, no en el router
 *
 * La cola no es un destino: se abre encima de lo que haya y sigue abierta al
 * navegar. Si fuera una ruta, abrirla obligaría a abandonar la vista actual y
 * cerrarla exigiría recordar de dónde se venía.
 *
 * ## Dos listas, no una
 *
 * "Siguiente en la cola" y "Siguiente desde: …" se ven parecidas y se comportan
 * distinto: la primera son elecciones explícitas del usuario, que se consumen
 * al sonar y se pueden reordenar y quitar; la segunda es la continuación del
 * contexto, que se recalcula sola. Mezclarlas haría que quitar una canción del
 * álbum que suena pareciera posible.
 *
 * ## No se virtualiza
 *
 * La cola de usuario son unas pocas canciones y la ventana de contexto la
 * acota el backend. Montar aquí una lista virtualizada sería pagar su
 * complejidad para pintar veinte filas.
 */

import type { QueueEntryDto, QueueSnapshotDto } from "../ipc/types.gen.js";
import { queue as api } from "../ipc/client.js";
import { alRecibir } from "../ipc/events.js";
import { alCambiarIdioma, t } from "../i18n/index.js";
import { botonIcono } from "../ui/icons.js";
import { reordenable, zonaDeReordenacion } from "../ui/dnd.js";
import { duracion } from "./player.js";

export interface PanelDeCola {
  abrir(): void;
  cerrar(): void;
  alternar(): void;
  abierto(): boolean;
  destroy(): void;
}

export function mountQueuePanel(contenedor: HTMLElement): PanelDeCola {
  const el = document.createElement("aside");
  el.className = "cola";
  el.hidden = true;
  el.setAttribute("aria-label", t("queue.title"));

  const cabecera = document.createElement("header");
  cabecera.className = "cola__header";
  const titulo = document.createElement("h2");
  titulo.className = "cola__titulo";
  // Vaciar la cola va con texto y no con otro icono: dos aspas seguidas en la
  // misma cabecera no dicen cuál cierra el panel y cuál borra lo que hay
  // dentro, y una de las dos no se deshace.
  const vaciar = document.createElement("button");
  vaciar.type = "button";
  vaciar.className = "cola__vaciar";
  vaciar.addEventListener("click", () => void api.clearUser());

  const cerrarBoton = botonIcono("chevron-right", "", () => cerrar(), { tamano: 18 });
  cabecera.append(titulo, vaciar, cerrarBoton);

  const cuerpo = document.createElement("div");
  cuerpo.className = "cola__cuerpo";

  el.append(cabecera, cuerpo);
  contenedor.append(el);

  let instantanea: QueueSnapshotDto | null = null;
  let revision = -1n;
  const desmontadores: Array<() => void> = [];

  function fila(entrada: QueueEntryDto, opciones: { usuario: boolean }): HTMLElement {
    const f = document.createElement("div");
    f.className = "cola__fila";
    f.dataset["entryId"] = entrada.entryId;

    const textos = document.createElement("div");
    textos.className = "cola__textos";
    const t1 = document.createElement("div");
    t1.className = "cola__title";
    t1.textContent = entrada.track.title;
    const t2 = document.createElement("div");
    t2.className = "cola__artist";
    t2.textContent = entrada.track.artistDisplay;
    textos.append(t1, t2);

    const tiempo = document.createElement("span");
    tiempo.className = "cola__time";
    tiempo.textContent = duracion(entrada.track.durationMs);

    f.append(textos, tiempo);

    // Solo la cola de usuario se puede tocar. La ventana de contexto la
    // recalcula el backend, y ofrecer un botón de quitar que no quita nada
    // sería peor que no ofrecerlo.
    if (opciones.usuario) {
      const quitar = botonIcono("close", t("common.close"), () => {
        void api.remove(entrada.entryId);
      }, { tamano: 14 });
      f.append(quitar);
      desmontadores.push(reordenable(f, () => entrada.entryId));
    }

    // Un clic salta a esa entrada: en la cola no hay nada más que hacer con
    // una fila, igual que en las listas de pistas.
    f.addEventListener("click", () => {
      void api.jumpTo(entrada.entryId);
    });

    return f;
  }

  function bloque(clave: string, entradas: readonly QueueEntryDto[], usuario: boolean): HTMLElement {
    const s = document.createElement("section");
    s.className = "cola__bloque";
    if (usuario) s.dataset["usuario"] = "1";

    const h = document.createElement("h3");
    h.className = "cola__subtitulo";
    h.textContent = clave;
    s.append(h);

    for (const entrada of entradas) s.append(fila(entrada, { usuario }));
    return s;
  }

  function pintar(): void {
    for (const quitar of desmontadores.splice(0)) quitar();
    cuerpo.replaceChildren();

    titulo.textContent = t("queue.title");
    vaciar.textContent = t("queue.clear");
    cerrarBoton.setAttribute("aria-label", t("common.close"));
    cerrarBoton.title = t("common.close");

    if (!instantanea) return;
    const q = instantanea;

    vaciar.hidden = q.userQueue.length === 0;

    if (q.current) {
      const s = document.createElement("section");
      s.className = "cola__bloque cola__bloque--actual";
      const h = document.createElement("h3");
      h.className = "cola__subtitulo";
      h.textContent = t("queue.now_playing");
      s.append(h, fila(q.current, { usuario: false }));
      cuerpo.append(s);
    }

    if (q.userQueue.length > 0) {
      cuerpo.append(bloque(t("queue.next_up"), q.userQueue, true));
    }

    if (q.contextQueue.length > 0) {
      // El backend manda la clave i18n del origen ("Siguiente desde: Álbum"),
      // no el texto: traducir es cosa del frontend (ADR-012).
      const desde = q.contextLabelKey
        ? `${t("queue.next_from")}: ${t(q.contextLabelKey)}`
        : t("queue.next_up");
      cuerpo.append(bloque(desde, q.contextQueue, false));
    }

    if (!q.current && q.userQueue.length === 0 && q.contextQueue.length === 0) {
      const p = document.createElement("p");
      p.className = "vista__empty";
      p.textContent = t("queue.empty");
      cuerpo.append(p);
    }
  }

  async function refrescar(): Promise<void> {
    try {
      const q = await api.get();
      // Las respuestas pueden llegar desordenadas si se encadenan varios
      // cambios; la revisión es monótona y descarta las viejas sin comparar
      // contenido.
      if (q.revision < revision) return;
      revision = q.revision;
      instantanea = q;
      pintar();
    } catch {
      // Un fallo puntual no debe vaciar el panel: se queda lo último bueno y
      // el siguiente evento reintenta.
    }
  }

  // Reordenar dentro de la cola de usuario. El índice es relativo a ese
  // bloque, que es lo que `queue_move` espera.
  const dejarReorden = zonaDeReordenacion(
    cuerpo,
    // El selector exige que la fila esté dentro del bloque de usuario. Aceptar
    // cualquier fila y filtrar después no bastaría: la zona ya habría marcado
    // el destino, prometiendo visualmente un movimiento que no va a ocurrir.
    (destino) =>
      destino instanceof Element
        ? destino.closest<HTMLElement>("[data-usuario] .cola__fila")
        : null,
    (f) => {
      const bloqueUsuario = f.closest<HTMLElement>("[data-usuario]");
      const filas = [...(bloqueUsuario?.querySelectorAll(".cola__fila") ?? [])];
      return filas.indexOf(f);
    },
    async (entryId, indice) => {
      if (indice < 0) return;
      await api.move(entryId, indice);
      await refrescar();
    },
  );

  const dejarEventos = alRecibir((evento) => {
    // Solo se consulta si el panel está abierto: mantenerlo al día mientras
    // está oculto sería una consulta por cada canción que suena, para pintar
    // algo que nadie ve.
    if (el.hidden) return;
    if (evento.type === "queueChanged" || evento.type === "trackChanged") {
      void refrescar();
    }
  });

  function abrir(): void {
    el.hidden = false;
    void refrescar();
  }

  function cerrar(): void {
    el.hidden = true;
  }

  const alTeclado = (e: KeyboardEvent): void => {
    if (e.key === "Escape" && !el.hidden) cerrar();
  };
  globalThis.addEventListener("keydown", alTeclado);

  const dejarIdioma = alCambiarIdioma(pintar);
  pintar();

  return {
    abrir,
    cerrar,
    alternar(): void {
      if (el.hidden) abrir();
      else cerrar();
    },
    abierto: () => !el.hidden,
    destroy(): void {
      dejarIdioma();
      dejarEventos();
      dejarReorden();
      globalThis.removeEventListener("keydown", alTeclado);
      for (const quitar of desmontadores.splice(0)) quitar();
      el.remove();
    },
  };
}

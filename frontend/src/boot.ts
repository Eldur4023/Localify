/**
 * Pantalla de arranque.
 *
 * Provisional de la Fase 4: comprueba de extremo a extremo que la cadena
 * completa funciona —TypeScript transpilado desde Rust, servido como ESM
 * nativo, hablando con los comandos de Tauri y recibiendo eventos del bus—
 * antes de que existan las vistas reales, que llegan en la Fase 10.
 */

import { library, LocalifyError, page, player, system } from "./ipc/client.js";
import { alRecibir, iniciar } from "./ipc/events.js";

/** Estructura de retorno común a todos los componentes. */
export interface Component {
  readonly el: HTMLElement;
  destroy(): void;
}

function fila(etiqueta: string, valor: string): HTMLElement {
  const el = document.createElement("div");
  el.className = "boot__row";

  const k = document.createElement("span");
  k.className = "boot__key";
  k.textContent = etiqueta;

  const v = document.createElement("span");
  v.className = "boot__value";
  v.textContent = valor;

  el.append(k, v);
  return el;
}

export function mountBootScreen(container: HTMLElement): Component {
  const el = document.createElement("div");
  el.className = "boot";

  const mark = document.createElement("div");
  mark.className = "boot__mark";
  mark.innerHTML = "Local<span>ify</span>";

  const hint = document.createElement("p");
  hint.className = "boot__hint";
  hint.textContent = "Fase 4 — API operativa";

  const datos = document.createElement("div");
  datos.className = "boot__data";

  const registro = document.createElement("div");
  registro.className = "boot__log";

  el.append(mark, hint, datos, registro);
  container.replaceChildren(el);

  const cancelar = alRecibir((evento) => {
    const linea = document.createElement("div");
    linea.textContent = `▸ ${evento.type}`;
    registro.prepend(linea);
    // El registro es una ayuda de desarrollo, no un historial: mantenerlo corto
    // evita que crezca sin límite durante una sesión larga.
    while (registro.childElementCount > 6) {
      registro.lastElementChild?.remove();
    }
  });

  void (async () => {
    try {
      await iniciar();

      const [version, stats, primera] = await Promise.all([
        system.apiVersion(),
        library.stats(),
        library.tracks({} as never, "titleAsc", page({ limit: 1 })),
      ]);

      datos.append(
        fila("API", `v${version}`),
        fila("Pistas", `${stats.trackCount} (${stats.localCount} en disco)`),
        fila("Álbumes", String(stats.albumCount)),
        fila("Artistas", String(stats.artistCount)),
      );

      // Reproduce la primera pista para provocar eventos reales y comprobar
      // que el puente los entrega.
      const pista = primera.items[0];
      if (pista) {
        await player.playTrack(pista.id, { kind: "library" });
        datos.append(fila("Sonando", `${pista.title} — ${pista.artistDisplay}`));
      }
    } catch (e) {
      const error = document.createElement("div");
      error.className = "boot__error";
      error.textContent =
        e instanceof LocalifyError
          ? `${e.api.code} · ${e.api.messageKey}`
          : String(e);
      datos.append(error);
    }
  })();

  return {
    el,
    destroy(): void {
      cancelar();
      el.remove();
    },
  };
}

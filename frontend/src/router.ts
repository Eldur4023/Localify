/**
 * Router propio, sobre el fragmento de la URL.
 *
 * ## Por qué el fragmento y no la History API
 *
 * La aplicación se sirve desde el protocolo de Tauri, no desde un servidor.
 * `pushState` cambiaría rutas que ningún servidor va a resolver, y recargar la
 * ventana daría un 404. El fragmento (`#/album/xyz`) nunca sale hacia el
 * origen: recargar siempre funciona.
 *
 * ## Por qué el historial es nuestro
 *
 * El navegador ya lleva uno, pero no distingue "volver" de "he pulsado el mismo
 * enlace otra vez", y no permite saber si hay algo delante sin trucos. Los
 * botones de atrás y adelante de la barra superior necesitan las dos cosas para
 * poder deshabilitarse, así que la pila se lleva aquí.
 *
 * ## La vista anterior se destruye
 *
 * Cada vista devuelve un `destroy()` y el router lo llama antes de montar la
 * siguiente. Sin eso, una lista virtualizada dejaría su `ResizeObserver` y su
 * oyente de scroll vivos, y tras diez navegaciones habría diez listas
 * recalculándose en cada píxel de desplazamiento.
 */

/** Lo que devuelve toda vista montada. */
export interface Vista {
  destroy(): void;
}

/** Una ruta reconocida. */
export interface Ruta {
  readonly nombre: string;
  /** Segmentos tras el nombre: `#/album/abc` da `["abc"]`. */
  readonly params: readonly string[];
  /** Lo que venga tras `?`, ya decodificado. */
  readonly query: URLSearchParams;
}

/** Monta una vista en el contenedor y devuelve cómo desmontarla. */
export type Montador = (contenedor: HTMLElement, ruta: Ruta) => Vista;

export interface Router {
  /** Navega. Añade una entrada al historial. */
  ir(destino: string): void;
  /** Reemplaza la entrada actual, sin añadir historial. */
  reemplazar(destino: string): void;
  atras(): void;
  adelante(): void;
  puedeIrAtras(): boolean;
  puedeIrAdelante(): boolean;
  /** Ruta vigente. */
  actual(): Ruta;
  /** Avisa tras cada navegación. Devuelve cómo dejar de escuchar. */
  alNavegar(oyente: (ruta: Ruta) => void): () => void;
  destroy(): void;
}

/** Analiza un fragmento como `#/album/abc?x=1`. */
export function analizar(fragmento: string): Ruta {
  const limpio = fragmento.replace(/^#\/?/, "");
  const [camino = "", consulta = ""] = limpio.split("?", 2);
  const partes = camino.split("/").filter((p) => p.length > 0).map(decodeURIComponent);

  return {
    nombre: partes[0] ?? "home",
    params: partes.slice(1),
    query: new URLSearchParams(consulta),
  };
}

export function crearRouter(
  contenedor: HTMLElement,
  rutas: Record<string, Montador>,
  porDefecto = "home",
): Router {
  // Pila propia: `indice` apunta a la entrada visible. Navegar recorta lo que
  // hubiera delante, igual que en un navegador.
  const pila: string[] = [];
  let indice = -1;
  let montada: Vista | null = null;
  let navegandoNosotros = false;
  const oyentes = new Set<(ruta: Ruta) => void>();

  function rutaActual(): Ruta {
    return analizar(globalThis.location.hash);
  }

  function montar(): void {
    const ruta = rutaActual();
    const montador = rutas[ruta.nombre] ?? rutas[porDefecto];
    if (!montador) return;

    // Desmontar antes de montar: si no, la vista saliente y la entrante
    // coexisten un instante y las dos escuchan los mismos eventos.
    montada?.destroy();
    montada = montador(contenedor, ruta);

    for (const oyente of oyentes) oyente(ruta);
  }

  function alCambiarFragmento(): void {
    if (!navegandoNosotros) {
      // El usuario editó la URL o usó los atajos del navegador: se trata como
      // una navegación nueva.
      pila.splice(indice + 1);
      pila.push(globalThis.location.hash);
      indice = pila.length - 1;
    }
    navegandoNosotros = false;
    montar();
  }

  globalThis.addEventListener("hashchange", alCambiarFragmento);

  function aplicar(destino: string): void {
    const fragmento = destino.startsWith("#") ? destino : `#/${destino}`;
    if (globalThis.location.hash === fragmento) {
      // Mismo destino: no hay evento de cambio, así que se remonta a mano.
      // Pulsar "Inicio" estando en Inicio debe refrescar, no quedarse quieto.
      montar();
      return;
    }
    navegandoNosotros = true;
    globalThis.location.hash = fragmento;
  }

  const router: Router = {
    ir(destino: string): void {
      pila.splice(indice + 1);
      pila.push(destino.startsWith("#") ? destino : `#/${destino}`);
      indice = pila.length - 1;
      aplicar(destino);
    },

    reemplazar(destino: string): void {
      const fragmento = destino.startsWith("#") ? destino : `#/${destino}`;
      if (indice >= 0) pila[indice] = fragmento;
      else {
        pila.push(fragmento);
        indice = 0;
      }
      aplicar(destino);
    },

    atras(): void {
      if (indice <= 0) return;
      indice -= 1;
      aplicar(pila[indice] ?? `#/${porDefecto}`);
    },

    adelante(): void {
      if (indice >= pila.length - 1) return;
      indice += 1;
      aplicar(pila[indice] ?? `#/${porDefecto}`);
    },

    puedeIrAtras: () => indice > 0,
    puedeIrAdelante: () => indice < pila.length - 1,
    actual: rutaActual,

    alNavegar(oyente): () => void {
      oyentes.add(oyente);
      return () => oyentes.delete(oyente);
    },

    destroy(): void {
      globalThis.removeEventListener("hashchange", alCambiarFragmento);
      montada?.destroy();
      montada = null;
      oyentes.clear();
    },
  };

  // Arranque: si no hay fragmento, se pone el de por defecto sin dejar una
  // entrada vacía en el historial detrás.
  if (!globalThis.location.hash || globalThis.location.hash === "#") {
    router.reemplazar(porDefecto);
  } else {
    pila.push(globalThis.location.hash);
    indice = 0;
    montar();
  }

  return router;
}

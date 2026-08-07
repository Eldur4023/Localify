/**
 * Lista virtualizada con reciclado de nodos.
 *
 * Es la pieza que hace posible una biblioteca de 50 000 pistas. Sin ella, el
 * navegador tendría que mantener 50 000 filas en el DOM: cientos de megabytes,
 * un `layout` de varios segundos al abrir, y un scroll a trompicones.
 *
 * ## El reciclado es lo importante, no la ventana
 *
 * Casi cualquier implementación calcula qué filas se ven. La diferencia está en
 * qué se hace con ellas: crear y destruir nodos en cada `scroll` genera basura
 * a ritmo de 60 Hz, y el recolector la recoge en pausas visibles justo mientras
 * el usuario arrastra.
 *
 * Aquí el número de nodos es **constante**. Se crea un grupo al montar y a
 * partir de ahí solo se mueven —`transform`— y se rellenan. Durante un scroll
 * de 50 000 filas no se crea ni un solo elemento.
 *
 * ## Por qué `transform` y no `top`
 *
 * Cambiar `top` obliga al navegador a recalcular el diseño de la fila.
 * `transform` solo la compone, que ocurre en el hilo de composición y no bloquea
 * al principal. Es la diferencia entre 60 fps y 20.
 *
 * ## Altura fija
 *
 * Todas las filas miden lo mismo. Es una limitación real y deliberada: con
 * alturas variables habría que medir cada fila para saber dónde empieza la
 * siguiente, lo que obliga a renderizarlas todas —justo lo que se quería
 * evitar— o a estimar y corregir, que produce saltos al desplazarse. Las listas
 * de Localify son homogéneas por diseño.
 */

/** Una página de resultados. */
export interface Pagina<T> {
  readonly items: readonly T[];
  /** `false` cuando esta era la última. */
  readonly hasMore: boolean;
}

/** Cómo pintar una fila y de dónde salen los datos. */
export interface VirtualListOptions<T> {
  /** Altura de cada fila en píxeles. Debe coincidir con la del CSS. */
  readonly rowHeight: number;
  /**
   * Filas de más que se mantienen fuera de la vista, arriba y abajo.
   *
   * Sin margen, cada píxel de scroll deja un hueco en blanco antes de que se
   * rellene la fila nueva. Con demasiadas, se pinta trabajo que nadie ve.
   */
  readonly overscan?: number;
  /** Crea un nodo vacío. Se llama una vez por elemento del grupo. */
  createRow(): HTMLElement;
  /** Rellena un nodo ya existente con los datos de `index`. */
  renderRow(el: HTMLElement, item: T, index: number): void;
  /**
   * Pide la siguiente página.
   *
   * Devuelve los elementos **y** si queda algo más, en vez de usar `null` para
   * las dos cosas. Con un solo valor, la última página obliga a elegir entre
   * devolver sus elementos —y hacer una consulta de más para descubrir que se
   * acabó— o señalar el final y perderlos. Separarlos hace imposible el
   * segundo error.
   *
   * La lista la llama sola al acercarse al final; quien la usa no gestiona
   * scroll ni umbrales.
   */
  loadMore(): Promise<Pagina<T>>;
  /**
   * Qué elementos están a la vista, tras cada cambio.
   *
   * Es el enganche para pedir la disponibilidad solo de lo visible: sin esto
   * habría una consulta por fila al hacer scroll.
   */
  onVisibleChange?(items: readonly T[], from: number, to: number): void;
}

/** Margen por defecto, en filas. */
const OVERSCAN_POR_DEFECTO = 6;

/**
 * A cuántas filas del final se pide la página siguiente.
 *
 * Con un margen holgado, la carga ocurre mientras todavía hay contenido que
 * mirar y el usuario no llega a ver el final de la lista.
 */
const UMBRAL_CARGA = 20;

/** Una lista virtualizada montada. */
export interface VirtualList<T> {
  readonly el: HTMLElement;
  /** Elementos cargados hasta ahora. */
  readonly items: readonly T[];
  /** Vuelve a pintar las filas visibles sin recargar datos. */
  refresh(): void;
  /**
   * Mueve un elemento, para reflejar una reordenación antes de confirmarla.
   *
   * Existe porque quien reordena tiene su propia copia de los datos, y mutar
   * solo la suya no cambiaría lo que la lista pinta: el elemento se quedaría
   * clavado en su sitio hasta que llegara la respuesta del backend, que es
   * justo lo que hace que un arrastre se sienta roto.
   */
  move(desde: number, hasta: number): void;
  /** Vacía la lista y empieza de cero. */
  reset(): void;
  /** Cuántos nodos hay vivos. Para diagnóstico y tests. */
  nodeCount(): number;
  destroy(): void;
}

export function mountVirtualList<T>(
  container: HTMLElement,
  options: VirtualListOptions<T>,
): VirtualList<T> {
  const overscan = options.overscan ?? OVERSCAN_POR_DEFECTO;

  const el = document.createElement("div");
  el.className = "vlist";

  // El espaciador solo existe para dar al contenedor la altura total y que la
  // barra de scroll sea la de verdad. Está vacío.
  const spacer = document.createElement("div");
  spacer.className = "vlist__spacer";
  spacer.setAttribute("aria-hidden", "true");

  const viewport = document.createElement("div");
  viewport.className = "vlist__viewport";

  el.append(spacer, viewport);
  container.replaceChildren(el);

  const items: T[] = [];
  const pool: HTMLElement[] = [];
  let agotada = false;
  let cargando = false;
  let destruida = false;

  // Qué se avisó la última vez. Se guardan los dos extremos, no solo el
  // primero: al llegar la primera página el índice inicial sigue siendo cero,
  // y comparando solo ese valor el aviso nunca llegaría. Quien lo usa para
  // precargar la disponibilidad se quedaría sin la primera pantalla entera.
  let avisadoDesde = -1;
  let avisadoHasta = -1;

  /** Cuántos nodos hacen falta para cubrir la vista más el margen. */
  function tamanoGrupo(): number {
    const visibles = Math.ceil(el.clientHeight / options.rowHeight);
    return visibles + overscan * 2;
  }

  /** Ajusta el grupo de nodos al tamaño necesario. */
  function ajustarGrupo(): void {
    const necesarios = tamanoGrupo();

    while (pool.length < necesarios) {
      const fila = options.createRow();
      fila.classList.add("vlist__row");
      fila.style.height = `${options.rowHeight}px`;
      viewport.append(fila);
      pool.push(fila);
    }
    // Al encoger la ventana sobran nodos. Se quitan de verdad: dejarlos
    // ocultos gastaría memoria sin dar nada a cambio.
    while (pool.length > necesarios) {
      pool.pop()?.remove();
    }
  }

  /** Coloca y rellena los nodos según la posición del scroll. */
  function pintar(): void {
    if (destruida) return;

    const primera = Math.max(
      0,
      Math.floor(el.scrollTop / options.rowHeight) - overscan,
    );

    ajustarGrupo();

    for (let i = 0; i < pool.length; i += 1) {
      const indice = primera + i;
      const fila = pool[i];
      if (!fila) continue;

      const item = items[indice];
      if (item === undefined) {
        // Más allá de lo cargado: se esconde en vez de eliminarse, para que el
        // grupo siga teniendo el mismo tamaño.
        fila.hidden = true;
        continue;
      }

      fila.hidden = false;
      fila.style.transform = `translateY(${indice * options.rowHeight}px)`;
      options.renderRow(fila, item, indice);
    }

    const hasta = Math.min(items.length, primera + pool.length);
    if (primera !== avisadoDesde || hasta !== avisadoHasta) {
      avisadoDesde = primera;
      avisadoHasta = hasta;
      options.onVisibleChange?.(items.slice(primera, hasta), primera, hasta);
    }

    // Se pide más cuando el final se acerca, no cuando se alcanza: así la
    // página siguiente llega antes de que haya nada que esperar.
    if (!agotada && primera + pool.length + UMBRAL_CARGA >= items.length) {
      void cargar();
    }
  }

  async function cargar(): Promise<void> {
    if (cargando || agotada || destruida) return;
    cargando = true;
    try {
      const pagina = await options.loadMore();
      if (destruida) return;

      if (!pagina.hasMore) agotada = true;
      if (pagina.items.length > 0) {
        items.push(...pagina.items);
        spacer.style.height = `${items.length * options.rowHeight}px`;
        pintar();
      }
    } catch {
      // Un fallo de red o de base de datos no debe dejar la lista en un estado
      // en el que no vuelva a intentarlo. Se libera el cerrojo y el siguiente
      // scroll reintenta.
    } finally {
      cargando = false;
    }
  }

  // `passive` porque nunca se llama a `preventDefault`: sin él, el navegador
  // tiene que esperar a que el manejador termine antes de desplazar.
  const alDesplazar = (): void => pintar();
  el.addEventListener("scroll", alDesplazar, { passive: true });

  // Al cambiar de tamaño la ventana cambia cuántas filas caben.
  const observador = new ResizeObserver(() => pintar());
  observador.observe(el);

  void cargar();

  return {
    el,
    get items(): readonly T[] {
      return items;
    },
    refresh: pintar,

    move(desde: number, hasta: number): void {
      if (desde < 0 || desde >= items.length) return;
      const [movido] = items.splice(desde, 1);
      if (movido === undefined) return;
      // Al sacar el elemento, todo lo que venía detrás baja una posición: un
      // destino posterior al origen hay que corregirlo o el elemento acaba una
      // fila más abajo de donde se soltó.
      const destino = hasta > desde ? hasta - 1 : hasta;
      items.splice(Math.max(0, Math.min(items.length, destino)), 0, movido);
      pintar();
    },

    reset(): void {
      items.length = 0;
      agotada = false;
      avisadoDesde = -1;
      avisadoHasta = -1;
      spacer.style.height = "0px";
      el.scrollTop = 0;
      for (const fila of pool) fila.hidden = true;
      void cargar();
    },
    nodeCount(): number {
      return pool.length;
    },
    destroy(): void {
      destruida = true;
      observador.disconnect();
      el.removeEventListener("scroll", alDesplazar);
      el.remove();
    },
  };
}

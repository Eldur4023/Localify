/**
 * Ancho ajustable de la barra lateral, con colapso a solo iconos.
 *
 * ## Por qué se guarda en `localStorage` y no en Ajustes
 *
 * Es una preferencia de presentación pura: el backend no la necesita para nada
 * y no cambia el comportamiento de la aplicación. El idioma se guarda igual y
 * por el mismo motivo. Meterla en la base de datos habría sido añadir un campo,
 * un DTO, un `patch` y una consulta al arrancar para mover un borde.
 *
 * ## Se arrastra un asa, no el borde del panel
 *
 * Un asa de seis píxeles con su propio cursor, en el hueco entre la barra y el
 * contenido. Poner el manejador en el borde del panel obliga a acertarle a un
 * píxel y compite con el desplazamiento de la lista de playlists.
 *
 * ## Estrechar mucho no la deja inservible: la colapsa
 *
 * Entre el mínimo abierto y el colapso hay un salto, no un continuo. Una barra
 * de 120 píxeles no sirve para nada —los nombres se cortan a la mitad— así que
 * al bajar del umbral se pasa de golpe a solo iconos, que sí es un estado útil.
 * Seguir empujando a la izquierda no hace nada más: ya está en su mínimo.
 *
 * Volver a abrirla es arrastrar a la derecha. El salto es simétrico, y por eso
 * el umbral de salida es más alto que el de entrada: con el mismo valor, un
 * temblor de dos píxeles en el punto justo haría parpadear la barra entre
 * abierta y cerrada.
 */

/** Clave en `localStorage`. Guarda un ancho en píxeles o `colapsada`. */
const ALMACEN = "localify.sidebar.width";

/** Valor que se guarda cuando está colapsada. */
const COLAPSADA = "colapsada";

/** Ancho en modo iconos: la miniatura de playlist más su holgura. */
const ANCHO_COLAPSADA = 72;

/** Ancho mínimo estando abierta: por debajo, un nombre normal ya no cabe. */
const MINIMO = 180;

/** Ancho máximo: por encima, la barra le quita sitio a lo que se está mirando. */
const MAXIMO = 420;

/** El que trae el diseño, y al que se vuelve con doble clic. */
const POR_DEFECTO = 240;

/** Arrastrando por debajo de esto, se colapsa. */
const UMBRAL_COLAPSO = 150;

/**
 * Y por encima de esto, se vuelve a abrir.
 *
 * Más alto que el de colapso a propósito: con un solo umbral, dejar el puntero
 * justo encima haría parpadear la barra con cada temblor de la mano.
 */
const UMBRAL_APERTURA = 190;

/** Clase que pone el armazón en modo iconos. */
const CLASE_COLAPSADA = "is-lateral-colapsada";

export interface AnchoLateral {
  destroy(): void;
}

function acotar(px: number): number {
  return Math.min(MAXIMO, Math.max(MINIMO, Math.round(px)));
}

export function mountAnchoLateral(contenedor: HTMLElement): AnchoLateral {
  /** Ancho al que volver al desplegar. */
  let anchoAbierta = POR_DEFECTO;
  let colapsada = false;

  function aplicar(): void {
    document.documentElement.style.setProperty(
      "--sidebar-width",
      `${colapsada ? ANCHO_COLAPSADA : anchoAbierta}px`,
    );
    contenedor.classList.toggle(CLASE_COLAPSADA, colapsada);
    asa.setAttribute("aria-valuenow", String(colapsada ? ANCHO_COLAPSADA : anchoAbierta));
  }

  function guardar(): void {
    globalThis.localStorage?.setItem(
      ALMACEN,
      colapsada ? COLAPSADA : String(anchoAbierta),
    );
  }

  function restaurar(): void {
    const crudo = globalThis.localStorage?.getItem(ALMACEN);
    if (crudo === COLAPSADA) {
      colapsada = true;
      return;
    }
    const px = Number(crudo);
    if (Number.isFinite(px) && px > 0) anchoAbierta = acotar(px);
  }

  const asa = document.createElement("div");
  asa.className = "asa-lateral";
  asa.setAttribute("role", "separator");
  asa.setAttribute("aria-orientation", "vertical");
  asa.setAttribute("aria-valuemin", String(ANCHO_COLAPSADA));
  asa.setAttribute("aria-valuemax", String(MAXIMO));
  asa.tabIndex = 0;
  contenedor.append(asa);

  restaurar();
  aplicar();

  let arrastrando = false;

  /** Decide el estado a partir de dónde está el puntero. */
  function segunPuntero(px: number): void {
    if (colapsada) {
      // Para reabrirla hay que pasar del umbral alto. Así, soltar el ratón un
      // poco a la derecha del borde no la despliega sin querer.
      if (px >= UMBRAL_APERTURA) {
        colapsada = false;
        anchoAbierta = acotar(px);
      }
    } else if (px < UMBRAL_COLAPSO) {
      colapsada = true;
    } else {
      anchoAbierta = acotar(px);
    }
    aplicar();
  }

  const alMover = (e: PointerEvent): void => {
    if (!arrastrando) return;
    // El ancho es la distancia desde el borde izquierdo del armazón, para no
    // repetir aquí el relleno que vive en el CSS.
    segunPuntero(e.clientX - contenedor.getBoundingClientRect().left);
  };

  const alSoltar = (e: PointerEvent): void => {
    if (!arrastrando) return;
    arrastrando = false;
    asa.releasePointerCapture(e.pointerId);
    document.body.classList.remove("is-redimensionando");
    guardar();
  };

  asa.addEventListener("pointerdown", (e) => {
    arrastrando = true;
    // La captura es lo que hace que el arrastre siga funcionando aunque el
    // puntero se salga del asa —que son seis píxeles— o de la ventana. Es
    // también lo que permite seguir empujando más allá del límite sin que el
    // gesto se rompa: el puntero se va, el arrastre se queda.
    asa.setPointerCapture(e.pointerId);
    document.body.classList.add("is-redimensionando");
    e.preventDefault();
  });
  asa.addEventListener("pointermove", alMover);
  asa.addEventListener("pointerup", alSoltar);
  asa.addEventListener("pointercancel", alSoltar);

  // Doble clic alterna entre colapsada y el ancho de fábrica. Es la salida para
  // quien la dejó inservible arrastrando y no sabe a qué anchura estaba.
  asa.addEventListener("dblclick", () => {
    colapsada = !colapsada;
    if (!colapsada) anchoAbierta = POR_DEFECTO;
    aplicar();
    guardar();
  });

  asa.addEventListener("keydown", (e) => {
    if (e.key !== "ArrowLeft" && e.key !== "ArrowRight") return;
    e.preventDefault();

    const izquierda = e.key === "ArrowLeft";
    const paso = e.shiftKey ? 40 : 10;

    // El teclado no reutiliza la lógica del puntero. Ahí los umbrales existen
    // porque el ratón se mueve solo; aquí cada pulsación es una intención, y
    // con los mismos umbrales abrir una barra colapsada costaría doce flechas
    // que no hacen nada visible.
    if (colapsada) {
      if (!izquierda) {
        colapsada = false;
        anchoAbierta = MINIMO;
      }
    } else if (izquierda && anchoAbierta <= MINIMO) {
      colapsada = true;
    } else {
      anchoAbierta = acotar(izquierda ? anchoAbierta - paso : anchoAbierta + paso);
    }

    aplicar();
    guardar();
  });

  return {
    destroy(): void {
      asa.remove();
      document.body.classList.remove("is-redimensionando");
      contenedor.classList.remove(CLASE_COLAPSADA);
    },
  };
}

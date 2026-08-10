/**
 * Buscar.
 *
 * ## Una lista, que puede crecer
 *
 * El backend contesta al instante con lo que ya tiene en el catálogo y consulta
 * al proveedor por detrás; cuando este responde, emite `searchRemoteReady` con
 * el mismo `queryId` y la vista repite la consulta para recoger la lista
 * completa, ya fundida y ordenada.
 *
 * **No hay dos bloques.** Los hubo —*En tu biblioteca* y *Más resultados*— y
 * eran una distinción interna disfrazada de información: como cada búsqueda
 * guarda sus resultados, el primer bloque era la respuesta del proveedor de la
 * vez anterior. Las mismas canciones, dos veces, con dos títulos distintos
 * encima. Quién guardó qué y cuándo no es asunto de quien busca una canción.
 *
 * ## Las respuestas viejas se descartan
 *
 * Escribir «bohemian» son ocho búsquedas, y no tienen por qué contestar en
 * orden. Cada respuesta trae su `queryId`: si no es el último que se pidió, se
 * tira. Sin eso, la respuesta de «boh» puede llegar después de la de
 * «bohemian» y dejar en pantalla resultados de hace tres pulsaciones.
 *
 * ## No hay temporizador de rebote aquí
 *
 * La búsqueda local es una consulta FTS5 sobre SQLite que tarda milisegundos.
 * Esperar 300 ms «por si acaso» solo añadiría latencia a lo que ya es
 * instantáneo.
 *
 * Quien limita la parte cara —la red— es el backend, que espera a que la mano
 * se detenga antes de consultar al proveedor. Hacerlo aquí obligaría a frenar
 * también lo local, que no lo necesita.
 */

import type {
  GrupoDeVersionesDto,
  SearchResultsDto,
  TrackRowDto,
} from "../ipc/types.gen.js";
import { player, search } from "../ipc/client.js";
import { alRecibir } from "../ipc/events.js";
import { alCambiarIdioma, t } from "../i18n/index.js";
import {
  carrusel,
  comienzoDePista,
  ponerFotoDeArtista,
  ponerPortada,
  ponerPortadaDePista,
  tarjeta,
  vacio,
} from "../ui/cards.js";
import { icono } from "../ui/icons.js";
import { arrastrable } from "../ui/dnd.js";
import { abrirMenu } from "../ui/menu.js";
import { opcionesDePista } from "../ui/opciones-pista.js";
import { duracion } from "../shell/player.js";
import type { Ruta, Vista } from "../router.js";

/** Pistas que se muestran. Pasado ese punto nadie sigue mirando. */
const TOPE = 20;

/**
 * Última consulta escrita, entre visitas a la pantalla.
 *
 * Vive en el módulo y no en la vista porque el router **desmonta** la vista al
 * navegar: sin esto, mirar la ficha de un artista y volver atrás dejaba la caja
 * en blanco y obligaba a escribirlo todo otra vez.
 *
 * Se guarda el texto y no los resultados. Repetir la consulta al volver cuesta
 * una lectura del índice local —milisegundos— y devuelve datos frescos, en vez
 * de resucitar una lista que puede llevar diez minutos parada.
 */
let ultimaConsulta = "";

export function mountSearchView(contenedor: HTMLElement, ruta?: Ruta): Vista {
  const el = document.createElement("section");
  el.className = "vista vista--scroll";

  const caja = document.createElement("div");
  caja.className = "buscador";
  const entrada = document.createElement("input");
  entrada.type = "search";
  entrada.className = "buscador__input";
  entrada.autocomplete = "off";
  entrada.spellcheck = false;
  caja.append(entrada);

  const resultados = document.createElement("div");
  resultados.className = "resultados";

  el.append(caja, resultados);
  contenedor.replaceChildren(el);

  let ultimo = 0n;
  /**
   * Identificador remoto ya atendido.
   *
   * Sin esto habría un bucle: repetir la consulta al recibir `searchRemoteReady`
   * hace que el backend vuelva a resolver la parte remota y a emitir el evento.
   * Atender cada identificador una sola vez lo corta.
   */
  let remotoAtendido = -1n;
  let vigente: SearchResultsDto | null = null;
  const desmontadores: Array<() => void> = [];

  /** Fila compacta de pista, con su menú y su arrastre. */
  function filaDePista(pista: TrackRowDto, contexto: string[]): HTMLElement {
    const fila = document.createElement("div");
    fila.className = "track track--suelta";

    const arte = comienzoDePista(pista.albumId, pista.id);

    const titulo = document.createElement("span");
    titulo.className = "track__title";
    titulo.textContent = pista.title;

    const artista = document.createElement("span");
    artista.className = "track__artist";
    artista.textContent = pista.artistDisplay;

    const tiempo = document.createElement("span");
    tiempo.className = "track__time";
    tiempo.textContent = duracion(pista.durationMs);

    fila.append(arte, titulo, artista, tiempo);

    const reproducir = (): void => {
      void player.playTrack(pista.id, {
        kind: "search",
        query: entrada.value,
        trackIds: contexto,
      });
    };
    fila.addEventListener("click", reproducir);
    fila.addEventListener("contextmenu", (e) => {
      e.preventDefault();
      // Las mismas opciones que en cualquier otra lista. Aquí tenía solo
      // "reproducir", y era la pantalla donde uno encuentra la canción que
      // quiere guardar en algún sitio.
      abrirMenu(
        e.clientX,
        e.clientY,
        opcionesDePista(pista, {
          contexto: () => ({ kind: "search", query: entrada.value, trackIds: contexto }),
        }),
      );
    });
    desmontadores.push(arrastrable(fila, () => [pista.id]));

    return fila;
  }

  /**
   * Punta de flecha que despliega las otras versiones de una canción.
   *
   * ## Gira, no se cambia
   *
   * El mismo icono rotado 180°, no dos iconos distintos. La rotación **es** la
   * animación: el usuario ve que la flecha que apuntaba abajo ahora apunta
   * arriba y entiende sin leer que eso mismo cierra lo que abrió. Cambiar el
   * icono daría un salto, y el salto no explica nada.
   */
  function desplegador(cuantas: number): HTMLButtonElement {
    const boton = document.createElement("button");
    boton.type = "button";
    boton.className = "versiones__mas bicono";
    boton.setAttribute("aria-expanded", "false");
    boton.append(icono("chevron-down", 16));

    const etiqueta = t("search.versions", { count: cuantas });
    boton.setAttribute("aria-label", etiqueta);
    boton.title = etiqueta;
    return boton;
  }

  /**
   * Una canción y, plegadas debajo, sus otras versiones.
   *
   * Las versiones se crean **al desplegar**, no al pintar: veinte grupos con
   * cinco versiones cada uno serían cien filas en el DOM para enseñar veinte.
   */
  function grupoDePistas(grupo: GrupoDeVersionesDto, contexto: string[]): HTMLElement {
    const bloque = document.createElement("div");
    bloque.className = "versiones";

    const cabecera = filaDePista(grupo.principal, contexto);
    bloque.append(cabecera);

    if (grupo.versiones.length === 0) return bloque;

    const lista = document.createElement("div");
    lista.className = "versiones__lista";
    lista.hidden = true;

    const boton = desplegador(grupo.versiones.length);
    boton.addEventListener("click", (e) => {
      // Sin esto, desplegar reproduce: el botón vive dentro de la fila, y la
      // fila entera es un gesto de reproducir.
      e.stopPropagation();

      const abierto = !lista.hidden;
      lista.hidden = abierto;
      boton.setAttribute("aria-expanded", String(!abierto));
      bloque.classList.toggle("is-abierto", !abierto);

      if (!abierto && lista.childElementCount === 0) {
        const ids = grupo.versiones.map((p) => p.id);
        for (const version of grupo.versiones) {
          lista.append(filaDePista(version, ids));
        }
      }
    });

    cabecera.append(boton);
    bloque.append(lista);
    return bloque;
  }

  function bloqueDePistas(clave: string, grupos: readonly GrupoDeVersionesDto[]): HTMLElement {
    const seccion = document.createElement("section");
    seccion.className = "resultados__bloque";

    const h = document.createElement("h3");
    h.className = "carrusel__titulo";
    h.textContent = t(clave);
    seccion.append(h);

    // El contexto de reproducción son las principales: al pulsar una fila, lo
    // que viene después es la siguiente canción, no otra versión de la misma.
    const ids = grupos.map((g) => g.principal.id);
    for (const grupo of grupos.slice(0, TOPE)) {
      seccion.append(grupoDePistas(grupo, ids));
    }
    return seccion;
  }

  /**
   * Tarjeta grande de la primera coincidencia.
   *
   * Ocupa mucho a propósito: casi siempre se busca **una cosa concreta**, y esa
   * cosa merece un objetivo grande en lugar de una fila más entre veinte
   * iguales. Que sea una canción, un disco o un artista lo decide el backend
   * (ver `primera_coincidencia`); aquí solo cambia a dónde lleva el clic.
   */
  function tarjetaDestacada(top: NonNullable<SearchResultsDto["top"]>): HTMLElement {
    const seccion = document.createElement("section");
    seccion.className = "resultados__bloque";

    const h = document.createElement("h3");
    h.className = "carrusel__titulo";
    h.textContent = t("search.top_result");
    seccion.append(h);

    const caja = document.createElement("div");
    caja.className = "destacado";

    const arte = document.createElement("div");
    arte.className = "destacado__arte";

    const titulo = document.createElement("div");
    titulo.className = "destacado__titulo";
    const tipo = document.createElement("div");
    tipo.className = "destacado__tipo";

    switch (top.kind) {
      case "track": {
        const pista = top.item;
        arte.append(icono("music", 48));
        ponerPortadaDePista(arte, pista.id);
        titulo.textContent = pista.title;
        tipo.textContent = `${t("search.kind_track")} · ${pista.artistDisplay}`;
        caja.addEventListener("click", () => {
          void player.playTrack(pista.id, {
            kind: "search",
            query: entrada.value,
            trackIds: [pista.id],
          });
        });
        // Ser la primera coincidencia no la convierte en otra cosa: sigue
        // siendo una canción, y con una canción se puede hacer lo mismo aquí
        // que en la lista de abajo.
        caja.addEventListener("contextmenu", (e) => {
          e.preventDefault();
          abrirMenu(
            e.clientX,
            e.clientY,
            opcionesDePista(pista, {
              contexto: () => ({
                kind: "search",
                query: entrada.value,
                trackIds: [pista.id],
              }),
            }),
          );
        });
        desmontadores.push(arrastrable(caja, () => [pista.id]));
        break;
      }
      case "album": {
        const album = top.item;
        arte.append(icono("music", 48));
        ponerPortada(arte, album.id);
        titulo.textContent = album.title;
        tipo.textContent = `${t("search.kind_album")} · ${album.artistDisplay}`;
        caja.addEventListener("click", () => {
          globalThis.location.hash = `#/album/${album.id}`;
        });
        caja.addEventListener("contextmenu", (e) => {
          e.preventDefault();
          abrirMenu(e.clientX, e.clientY, [
            {
              clave: "abrir",
              etiqueta: t("menu.go_to_album"),
              ejecutar: () => {
                globalThis.location.hash = `#/album/${album.id}`;
              },
            },
          ]);
        });
        break;
      }
      default: {
        const artista = top.item;
        arte.classList.add("destacado__arte--redonda");
        arte.append(icono("music", 48));
        ponerFotoDeArtista(arte, artista.id);
        titulo.textContent = artista.name;
        tipo.textContent = t("search.kind_artist");
        caja.addEventListener("click", () => {
          globalThis.location.hash = `#/artist/${artista.id}`;
        });
        break;
      }
    }

    const textos = document.createElement("div");
    textos.className = "destacado__textos";
    textos.append(titulo, tipo);

    // Un botón de reproducir solo tiene sentido sobre algo que suena. Un
    // artista lleva a su ficha, y poner ahí un play obligaría a decidir qué
    // canción suya empieza a sonar, que es una decisión que nadie ha pedido.
    caja.append(arte, textos);
    seccion.append(caja);
    return seccion;
  }

  function pintar(): void {
    for (const quitar of desmontadores.splice(0)) quitar();
    resultados.replaceChildren();

    if (entrada.value.trim().length === 0) {
      resultados.append(vacio(t("search.empty")));
      return;
    }
    if (!vigente) {
      resultados.append(vacio(t("search.searching")));
      return;
    }

    const { top, tracks, albums, artists, remote } = vigente;
    let algo = false;

    if (top) {
      resultados.append(tarjetaDestacada(top));
      algo = true;
    }

    if (tracks.length > 0) {
      resultados.append(bloqueDePistas("library.tracks", tracks));
      algo = true;
    }

    if (albums.length > 0) {
      const { el: bloque, cuerpo } = carrusel(t("library.albums"));
      for (const album of albums) {
        cuerpo.append(
          tarjeta({
            titulo: album.title,
            subtitulo: album.artistDisplay,
            destino: `#/album/${album.id}`,
            albumId: album.id,
          }),
        );
      }
      resultados.append(bloque);
      algo = true;
    }

    if (artists.length > 0) {
      const { el: bloque, cuerpo } = carrusel(t("library.artists"));
      for (const artista of artists) {
        cuerpo.append(
          tarjeta({
            titulo: artista.name,
            subtitulo: t("library.count", { count: artista.trackCount }),
            destino: `#/artist/${artista.id}`,
            redonda: true,
            artistId: artista.id,
          }),
        );
      }
      resultados.append(bloque);
      algo = true;
    }

    // El estado remoto ya no aporta canciones —vienen fundidas en `tracks`—,
    // solo dice si queda algo por llegar. "Sin credenciales" no es un error: es
    // accionable desde Ajustes, y callarlo dejaría al usuario preguntándose por
    // qué no encuentra nada.
    switch (remote.state) {
      case "loading":
        // Va al final y no en lugar de la lista: lo que ya hay es válido y se
        // puede pulsar. Vaciar la pantalla para decir "buscando" quitaría de en
        // medio resultados buenos que quizá eran justo el que se quería.
        resultados.append(vacio(t("search.searching")));
        break;
      case "unavailable":
        resultados.append(vacio(t(remote.reasonKey)));
        break;
      case "ready":
      case "notAttempted":
      default:
        break;
    }

    if (!algo && remote.state !== "loading") {
      resultados.append(vacio(t("search.no_results", { query: entrada.value })));
    }
  }

  function buscar(): void {
    const consulta = entrada.value.trim();
    ultimaConsulta = consulta;

    if (consulta.length === 0) {
      vigente = null;
      pintar();
      return;
    }

    void search
      .query(consulta)
      .then((r) => {
        // Una respuesta de una pulsación ya superada se tira.
        if (r.queryId < ultimo) return;
        ultimo = r.queryId;
        vigente = r;
        pintar();
      })
      .catch(() => {
        vigente = null;
        pintar();
      });
  }

  entrada.addEventListener("input", buscar);

  // Cuando el proveedor contesta, se repite la consulta: los resultados
  // remotos ya están en la base de datos local y vienen con la siguiente.
  const dejarEventos = alRecibir((evento) => {
    if (evento.type !== "searchRemoteReady") return;
    if (evento.queryId < ultimo || evento.queryId <= remotoAtendido) return;
    remotoAtendido = evento.queryId;
    buscar();
  });

  function etiquetas(): void {
    entrada.placeholder = t("search.placeholder");
    entrada.setAttribute("aria-label", t("search.title"));
    pintar();
  }

  etiquetas();
  const dejarIdioma = alCambiarIdioma(etiquetas);

  // Lo que trae la ruta manda —viene de la búsqueda rápida, con lo que el
  // usuario acaba de escribir— y si no, se recupera lo último de esta sesión.
  const desdeRuta = ruta?.params[0];
  const inicial = desdeRuta ? decodeURIComponent(desdeRuta) : ultimaConsulta;
  if (inicial.length > 0) {
    entrada.value = inicial;
    buscar();
  }

  entrada.focus();
  // El cursor al final y no seleccionando todo: quien vuelve suele querer
  // afinar lo que escribió, y con el texto seleccionado la primera tecla lo
  // borra entero.
  entrada.setSelectionRange(entrada.value.length, entrada.value.length);

  return {
    destroy(): void {
      dejarIdioma();
      dejarEventos();
      entrada.removeEventListener("input", buscar);
      for (const quitar of desmontadores.splice(0)) quitar();
      el.remove();
    },
  };
}

/**
 * Las opciones del menú contextual de una canción.
 *
 * ## Por qué vive aquí y no en cada lista
 *
 * La misma canción aparece en Canciones, Tus me gusta, un álbum, un artista,
 * una playlist y en los resultados de búsqueda. Cada una de esas pantallas
 * pinta sus filas a su manera, pero **lo que se puede hacer con una canción es
 * lo mismo en todas**. Cuando cada vista tenía su lista, la de búsqueda se
 * quedó con una sola opción —reproducir— y era justo la pantalla donde uno
 * encuentra algo que quiere guardar.
 *
 * ## Las playlists se piden al abrir el submenú
 *
 * No al construir el menú. Un clic derecho no debe costar una consulta que casi
 * nunca se mira, y si se pidieran al montar la lista estarían caducadas en
 * cuanto se creara una playlist nueva.
 */

import type { TrackRowDto, PlaybackContextDto } from "../ipc/types.gen.js";
import { library, playlists, player, queue } from "../ipc/client.js";
import { t } from "../i18n/index.js";
import { confirmar, pedirTexto } from "./dialogo.js";
import { elegirCandidato } from "./reasignar-metadatos.js";
import type { OpcionMenu } from "./menu.js";

/** Longitud máxima del nombre de una playlist, la misma que acepta el backend. */
const MAX_NOMBRE = 100;

export interface ContextoDePista {
  /** Con qué contexto reproducir al elegir "Reproducir". */
  contexto(): PlaybackContextDto;
  /** Se llama cuando algo cambió y la lista debería repintarse. */
  alCambiar?(): void;
  /** Si la fila pertenece a una playlist, para poder quitarla de ella. */
  readonly playlistId?: string;
  /** Identificador de la entrada dentro de la playlist. */
  readonly entryId?: string;
}

/**
 * Crea una playlist y mete la canción dentro, en un solo gesto.
 *
 * Es el camino que más se usa: la primera vez que quieres guardar algo todavía
 * no tienes dónde. Obligar a ir a la barra lateral, crearla, volver y repetir
 * el clic derecho sería tres pasos para lo que aquí es uno.
 */
async function nuevaCon(trackId: string, alCambiar?: () => void): Promise<void> {
  const nombre = await pedirTexto({
    titulo: t("playlist.create"),
    etiqueta: t("playlist.name"),
    valor: t("playlist.new"),
    aceptar: t("playlist.create"),
    maxLength: MAX_NOMBRE,
  });
  if (!nombre) return;

  const creada = await playlists.create(nombre);
  await playlists.addTracks(creada.id, [trackId], null);
  alCambiar?.();
}

/** Opciones del submenú "Añadir a playlist". */
async function destinos(pista: TrackRowDto, ctx: ContextoDePista): Promise<OpcionMenu[]> {
  const listas = await playlists.list();

  const opciones: OpcionMenu[] = [
    {
      clave: "nueva",
      etiqueta: t("playlist.new"),
      icono: "plus",
      ejecutar: () => nuevaCon(pista.id, ctx.alCambiar),
    },
  ];

  for (const [i, lista] of listas.entries()) {
    opciones.push({
      clave: `pl:${lista.id}`,
      // El separador va solo antes de la primera: divide "crear una nueva" de
      // "meterla en una que ya existe", que son dos intenciones distintas.
      separar: i === 0,
      etiqueta: lista.name,
      icono: "music",
      ejecutar: async () => {
        // `null` como posición: al final, que es donde se espera que caiga algo
        // que acabas de añadir.
        await playlists.addTracks(lista.id, [pista.id], null);
        ctx.alCambiar?.();
      },
    });
  }

  return opciones;
}

/** Lo que se puede hacer con una canción, en cualquier pantalla. */
export function opcionesDePista(pista: TrackRowDto, ctx: ContextoDePista): OpcionMenu[] {
  const menu: OpcionMenu[] = [
    {
      clave: "play",
      etiqueta: t("menu.play"),
      icono: "play",
      ejecutar: () => void player.playTrack(pista.id, ctx.contexto()),
    },
    {
      clave: "next",
      etiqueta: t("menu.play_next"),
      ejecutar: () => void queue.addNext([pista.id]),
    },
    {
      clave: "queue",
      etiqueta: t("menu.add_to_queue"),
      icono: "queue",
      ejecutar: () => void queue.addLast([pista.id]),
    },
    {
      clave: "playlist",
      separar: true,
      etiqueta: t("menu.add_to_playlist"),
      icono: "plus",
      submenu: () => destinos(pista, ctx),
    },
    {
      clave: "like",
      etiqueta: pista.isFavorite ? t("menu.unlike") : t("menu.like"),
      icono: pista.isFavorite ? "heart-filled" : "heart",
      ejecutar: () =>
        void library.setFavorite(pista.id, !pista.isFavorite).then(() => ctx.alCambiar?.()),
    },
  ];

  // Ir al álbum y al artista. Van juntos y con un separador delante porque son
  // las dos que **navegan** en vez de hacer algo con la canción.
  //
  // Cada una aparece solo si tiene destino. `artistId` es el principal: la fila
  // no trae la lista entera de artistas —sería una consulta por fila— y para
  // esto basta, que es a donde llevaría un clic en su nombre.
  if (pista.albumId) {
    const albumId = pista.albumId;
    menu.push({
      clave: "album",
      separar: true,
      etiqueta: t("menu.go_to_album"),
      ejecutar: () => {
        globalThis.location.hash = `#/album/${albumId}`;
      },
    });
  }

  if (pista.artistId) {
    const artistId = pista.artistId;
    menu.push({
      clave: "artist",
      separar: !pista.albumId,
      etiqueta: t("menu.go_to_artist"),
      ejecutar: () => {
        globalThis.location.hash = `#/artist/${artistId}`;
      },
    });
  }

  if (ctx.playlistId && ctx.entryId) {
    const { playlistId, entryId } = ctx;
    menu.push({
      clave: "remove",
      separar: true,
      peligrosa: true,
      etiqueta: t("menu.remove"),
      icono: "close",
      ejecutar: async () => {
        await playlists.removeEntries(playlistId, [entryId]);
        ctx.alCambiar?.();
      },
    });
  }

  // Solo si hay algo que borrar. Una opción que no hace nada la mitad de las
  // veces enseña a desconfiar del menú entero.
  if (pista.availability.kind === "local") {
    menu.push({
      clave: "delete_download",
      separar: true,
      peligrosa: true,
      etiqueta: t("menu.delete_download"),
      icono: "close",
      ejecutar: async () => {
        // Sin confirmación: no se pierde nada que no se recupere solo. La
        // canción sigue en sus playlists y vuelve a bajarse al reproducirla,
        // así que preguntar sería un paso de más para algo reversible.
        await library.deleteDownload(pista.id);
        ctx.alCambiar?.();
      },
    });
  }

  // ── Gestión de metadatos ──────────────────────────────────────────────
  //
  // A diferencia de todo lo de arriba, esto no depende de si la pista está
  // descargada: un fichero importado sin etiquetas o un emparejamiento
  // equivocado son casos igual de válidos para una pista que nunca se ha
  // bajado.
  menu.push({
    clave: "reassign_metadata",
    separar: true,
    etiqueta: t("menu.reassign_metadata"),
    ejecutar: async () => {
      const elegido = await elegirCandidato(pista.title);
      if (!elegido) return;
      await library.assignMetadata(pista.id, elegido);
      ctx.alCambiar?.();
    },
  });
  menu.push({
    clave: "reset_metadata",
    etiqueta: t("menu.reset_metadata"),
    ejecutar: async () => {
      await library.resetMetadata(pista.id);
      ctx.alCambiar?.();
    },
  });

  // La única acción que se lleva playlists, favoritos e historial por
  // delante: a diferencia de "borrar descarga", esto sí necesita que el
  // usuario lo confirme antes de que pase.
  menu.push({
    clave: "delete_track",
    peligrosa: true,
    etiqueta: t("menu.delete_track"),
    icono: "close",
    ejecutar: async () => {
      const seguro = await confirmar(
        t("menu.delete_track"),
        t("menu.delete_track_do"),
        t("menu.delete_track_confirm"),
      );
      if (!seguro) return;
      await library.deleteTrack(pista.id);
      ctx.alCambiar?.();
    },
  });

  return menu;
}

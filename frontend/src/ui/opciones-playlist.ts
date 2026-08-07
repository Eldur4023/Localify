/**
 * Lo que se puede hacer con una playlist.
 *
 * ## Por qué vive aquí y no en cada pantalla
 *
 * Una playlist se maneja desde dos sitios: su ficha y la barra lateral. Cuando
 * cada uno tenía sus botones, la barra lateral no tenía ninguno —había que
 * entrar en la playlist para renombrarla— y la ficha acabó con dos botones
 * distintos que usaban el mismo icono de cerrar.
 *
 * Con las acciones en un sitio, el clic derecho ofrece lo mismo esté donde
 * esté, y añadir una nueva la pone en los dos a la vez.
 */

import { playlists } from "../ipc/client.js";
import { t } from "../i18n/index.js";
import { confirmar, pedirTexto } from "./dialogo.js";
import { mostrarError } from "./error-overlay.js";
import type { OpcionMenu } from "./menu.js";

/** Lo mínimo que hace falta para ofrecer las acciones. */
export interface PlaylistAccionable {
  readonly id: string;
  readonly name: string;
  readonly hasOwnCover: boolean;
  /**
   * Ausente donde no se conoce —la barra lateral solo tiene el resumen—, y
   * entonces el diálogo abre en blanco. Es correcto para la mayoría, que no
   * tienen ninguna.
   */
  readonly description?: string | null;
}

export interface ReaccionesPlaylist {
  /** Tras renombrar o cambiar la foto. */
  alCambiar?(): void;
  /** Tras borrarla. La pantalla que la mostraba ya no tiene qué mostrar. */
  alBorrar?(): void;
}

/** Pide el nombre nuevo y lo aplica. */
export async function renombrar(
  p: PlaylistAccionable,
  reacciones: ReaccionesPlaylist,
): Promise<void> {
  const nuevo = await pedirTexto({
    titulo: t("playlist.rename"),
    etiqueta: t("playlist.name"),
    valor: p.name,
    maxLength: 100,
  });
  if (!nuevo || nuevo === p.name) return;
  await playlists.rename(p.id, nuevo);
  reacciones.alCambiar?.();
}

/**
 * Cambia la descripción.
 *
 * ## Va en su propio diálogo, no junto al nombre
 *
 * Son dos gestos distintos: el nombre se corrige al vuelo y la descripción se
 * escribe. Un diálogo con los dos campos obligaría a pasar por el nombre cada
 * vez que alguien solo quiere retocar una frase, y a decidir qué pasa si se
 * cambian los dos y uno falla.
 *
 * Vaciar el campo **quita** la descripción: es lo que espera quien la borra, y
 * evita tener que ofrecer un "quitar descripción" aparte.
 */
export async function describir(
  p: PlaylistAccionable,
  reacciones: ReaccionesPlaylist,
): Promise<void> {
  const nueva = await pedirTexto({
    titulo: t("playlist.describe"),
    etiqueta: t("playlist.description"),
    valor: p.description ?? "",
    maxLength: 500,
    // Una descripción es un párrafo. Con un campo de una línea, releer lo
    // escrito obliga a recorrerlo con el cursor.
    multilinea: true,
    // Sin esto, borrarla del todo sería imposible: el diálogo trata la cadena
    // vacía como "cancelar" y no habría forma de quitar lo que se puso.
    permitirVacio: true,
  });
  if (nueva === null) return;

  const limpia = nueva.trim();
  await playlists.setDescription(p.id, limpia.length > 0 ? limpia : null);
  reacciones.alCambiar?.();
}

/**
 * Abre el selector del sistema y aplica la imagen elegida.
 *
 * El backend la **copia** a la biblioteca: guardar la ruta original dejaría la
 * foto rota en cuanto el usuario vaciara Descargas o desconectara el disco de
 * donde salió.
 */
export async function cambiarFoto(
  p: PlaylistAccionable,
  reacciones: ReaccionesPlaylist,
): Promise<void> {
  const ruta = await playlists.pickImage();
  if (!ruta) return;
  try {
    await playlists.setCover(p.id, ruta);
    reacciones.alCambiar?.();
  } catch (e) {
    mostrarError(t("playlist.cover_failed"), String(e));
  }
}

/** Quita la foto propia y devuelve la playlist al mosaico. */
export async function quitarFoto(
  p: PlaylistAccionable,
  reacciones: ReaccionesPlaylist,
): Promise<void> {
  await playlists.clearCover(p.id);
  reacciones.alCambiar?.();
}

/** Pide confirmación y borra. */
export async function borrar(
  p: PlaylistAccionable,
  reacciones: ReaccionesPlaylist,
): Promise<void> {
  const seguro = await confirmar(
    t("playlist.delete_confirm", { name: p.name }),
    t("playlist.delete"),
  );
  if (!seguro) return;
  await playlists.remove(p.id);
  reacciones.alBorrar?.();
}

/** Menú contextual de una playlist. */
export function opcionesDePlaylist(
  p: PlaylistAccionable,
  reacciones: ReaccionesPlaylist = {},
): OpcionMenu[] {
  const menu: OpcionMenu[] = [
    {
      clave: "rename",
      etiqueta: t("playlist.rename"),
      ejecutar: () => renombrar(p, reacciones),
    },
    {
      clave: "describe",
      etiqueta: t("playlist.describe"),
      ejecutar: () => describir(p, reacciones),
    },
    {
      clave: "cover",
      etiqueta: t("playlist.cover"),
      icono: "plus",
      ejecutar: () => cambiarFoto(p, reacciones),
    },
  ];

  // Solo si hay algo que quitar: una opción permanentemente inerte enseña a
  // desconfiar del menú entero.
  if (p.hasOwnCover) {
    menu.push({
      clave: "cover_remove",
      etiqueta: t("playlist.cover_remove"),
      icono: "close",
      ejecutar: () => quitarFoto(p, reacciones),
    });
  }

  menu.push({
    clave: "delete",
    separar: true,
    peligrosa: true,
    etiqueta: t("playlist.delete"),
    icono: "close",
    ejecutar: () => borrar(p, reacciones),
  });

  return menu;
}

/**
 * Inicio.
 *
 * Las secciones las decide el backend, no esta vista: qué merece mostrarse
 * depende del historial y de la biblioteca, y eso es negocio. Aquí solo se
 * pinta lo que llega, en el orden que llega.
 *
 * Si no llega nada —biblioteca recién estrenada, sin historial— se dice, en vez
 * de rellenar con lo que sea. Una pantalla que finge conocerte acierta menos
 * que una que no dice nada.
 */

import type { HomeSectionDto } from "../ipc/types.gen.js";
import { home, player } from "../ipc/client.js";
import { alCambiarIdioma, t } from "../i18n/index.js";
import { carrusel, conEspera, tarjeta, vacio } from "../ui/cards.js";
import { arrastrable } from "../ui/dnd.js";
import type { Vista } from "../router.js";

export function mountHomeView(contenedor: HTMLElement): Vista {
  const el = document.createElement("section");
  el.className = "vista vista--scroll";
  contenedor.replaceChildren(el);

  let secciones: HomeSectionDto[] = [];
  const desmontadores: Array<() => void> = [];

  function pintar(): void {
    for (const quitar of desmontadores.splice(0)) quitar();
    el.replaceChildren();

    if (secciones.length === 0) {
      el.append(vacio(t("home.empty")));
      return;
    }

    for (const seccion of secciones) {
      // Los parámetros llegan como pares y se sustituyen en la clave: la
      // sección "Porque escuchaste {artist}" necesita el nombre.
      const params = Object.fromEntries(seccion.params);
      const { el: bloque, cuerpo } = carrusel(t(seccion.key, params));

      switch (seccion.items.kind) {
        case "tracks":
          for (const pista of seccion.items.items) {
            const card = tarjeta({
              titulo: pista.title,
              subtitulo: pista.artistDisplay,
              destino: pista.albumId ? `#/album/${pista.albumId}` : "#/library",
              albumId: pista.albumId,
            });
            // Una tarjeta de canción **reproduce**, no navega. Sigue siendo un
            // enlace al álbum para que el clic central y el teclado funcionen,
            // pero la acción principal de una canción es sonar, y repartirla
            // entre un clic y dos obligaría a recordar cuál hace qué.
            card.addEventListener("click", (e) => {
              e.preventDefault();
              void player.playTrack(pista.id, { kind: "library" });
            });
            desmontadores.push(arrastrable(card, () => [pista.id]));
            cuerpo.append(card);
          }
          break;

        case "albums":
          for (const album of seccion.items.items) {
            cuerpo.append(
              tarjeta({
                titulo: album.title,
                subtitulo: album.year ? String(album.year) : album.artistDisplay,
                destino: `#/album/${album.id}`,
                albumId: album.id,
              }),
            );
          }
          break;

        case "artists":
          for (const artista of seccion.items.items) {
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
          break;

        case "playlists":
          for (const lista of seccion.items.items) {
            cuerpo.append(
              tarjeta({
                titulo: lista.name,
                subtitulo: t("library.count", { count: lista.trackCount }),
                destino: `#/playlist/${lista.id}`,
                playlist: lista,
              }),
            );
          }
          break;

        default:
          break;
      }

      el.append(bloque);
    }
  }

  void conEspera(el, home.sections())
    .then((s) => {
      secciones = s;
      pintar();
    })
    .catch(() => {
      secciones = [];
      pintar();
    });

  const dejarIdioma = alCambiarIdioma(pintar);

  return {
    destroy(): void {
      dejarIdioma();
      for (const quitar of desmontadores.splice(0)) quitar();
      el.remove();
    },
  };
}

/**
* Tarjetas: la unidad visual de Inicio, Buscar y las fichas.
*
* ## El hueco de la portada se reserva siempre
*
* La imagen ocupa su sitio antes de existir, con la misma proporción. Sin eso,
* cada portada que carga empuja el texto hacia abajo y la página baila mientras
* se llena, que es de las cosas que más delatan una interfaz mal hecha.
*
* Mientras no hay portada se muestra un icono sobre el fondo de la tarjeta: es
* el mismo espacio, no un hueco vacío.
*/
import { icono } from "./icons.js";
/**
* Pone `ruta` como fuente de una portada, probando las dos formas en que
* Tauri sirve un esquema propio según la plataforma.
*
* Windows y Android lo sirven como `http://<esquema>.localhost/<ruta>`;
* macOS y Linux, como `<esquema>://localhost/<ruta>` — está documentado así
* en el propio código fuente de Tauri, en el aviso de
* `register_uri_scheme_protocol`. Detectar el sistema operativo desde JS
* para elegir una sola forma añadiría una dependencia solo para esto; probar
* la de Windows primero y caer a la otra al fallar cubre las dos sin
* necesitarla, porque la que no es la de la plataforma actual falla limpio
* y al instante, no a medias ni con contenido equivocado.
*
* `siFallaDelTodo` solo se llama si **ninguna** de las dos formas cargó: ni
* hay portada, ni es un problema de plataforma.
*/
function asignarSrcConRespaldo(img, ruta, siFallaDelTodo) {
	img.onerror = () => {
		img.onerror = () => {
			img.onerror = null;
			siFallaDelTodo();
		};
		img.src = `/api/cover/${ruta}`;
	};

}
/** Filas de tarjetas, con desplazamiento horizontal. */
export function carrusel(titulo) {
	const el = document.createElement("section");
	el.className = "carrusel";
	const h = document.createElement("h3");
	h.className = "carrusel__titulo";
	h.textContent = titulo;
	const cuerpo = document.createElement("div");
	cuerpo.className = "carrusel__cuerpo";
	cuerpo.setAttribute("role", "list");
	el.append(h, cuerpo);
	return {
		el,
		cuerpo
	};
}
/**
* Coloca la portada de un álbum dentro de un hueco ya reservado.
*
* El icono se queda debajo y solo se ve si la imagen no llega: no hay portada,
* o el álbum no la tiene. Quitarlo al empezar la carga dejaría un rectángulo
* vacío parpadeando en cada tarjeta.
*/
function ponerImagen(hueco, ruta) {
	const img = document.createElement("img");
	img.className = "portada";
	img.alt = "";
	img.decoding = "async";
	// Antes llevaba `loading = "lazy"` para no pedir de golpe las tarjetas
	// fuera de pantalla. En Linux, con la ventana sin maximizar, WebKitGTK a
	// veces calcula mal el tamaño del contenedor en el primer pintado y estas
	// imágenes nunca llegan a considerarse visibles: el `load` no se dispara
	// jamás y la portada se queda con el icono para siempre. Las imágenes las
	// sirve el propio backend en disco, no la red, así que pedirlas todas no
	// sale caro.
	// Hasta que carga no se muestra, para que el icono no salte a la imagen con
	// un parpadeo intermedio. Si falla de verdad —ni portada, ni problema de
	// plataforma—, se queda el icono y ya está.
	img.addEventListener("load", () => img.classList.add("is-lista"));
	asignarSrcConRespaldo(img, ruta, () => img.remove());
	hueco.append(img);
}
export function ponerPortada(hueco, albumId) {
	if (!albumId) return;
	ponerImagen(hueco, encodeURIComponent(albumId));
}
/**
* Portada de una canción.
*
* ## Una canción, una URL
*
* Antes esto elegía: con álbum pedía el del álbum y sin álbum el de la pista.
* Dos caminos para la misma imagen son dos formas de que difieran, y difirieron:
* la misma canción salía con una carátula en la lista y con otra en el
* reproductor, y cambiaba al volver a entrar.
*
* Ahora siempre se pide por la pista. **Quién responde lo decide el backend**,
* que sí puede: usa la miniatura que el catálogo da para esa canción y, si no
* hay, cae a la portada de su álbum. Aquí no hay nada que elegir.
*/
export function ponerPortadaDePista(hueco, trackId) {
	ponerImagen(hueco, `track/${encodeURIComponent(trackId)}`);
}
/**
* Coloca la foto de un artista, con las mismas reglas que una portada.
*
* ## Por qué no se usa `imageUrl` directamente
*
* `ArtistRowDto` trae la URL que dio el proveedor, y es tentador ponerla en el
* `src`. Sería un fallo por partida doble: el CSP no permite cargar imágenes de
* otro origen —quedaría el hueco vacío sin más explicación que un aviso en la
* consola—, y cada pintado saldría a la red de Spotify o de YouTube, así que la
* ficha de un artista no se vería sin conexión. Cacheadas en disco se piden una
* vez y se ven siempre, igual que las portadas.
*
* ## El prefijo `artist/` no es decoración
*
* Los identificadores de álbum y de artista tienen la misma forma. Sin el
* prefijo, el backend no podría saber cuál le están pidiendo.
*/
export function ponerFotoDeArtista(hueco, artistId) {
	if (!artistId) return;
	ponerImagen(hueco, `artist/${encodeURIComponent(artistId)}`);
}
/**
* Comienzo de una fila de pista: botón de reproducir y portada.
*
* ## El botón va aparte de la carátula, no encima
*
* Superpuesto tapaba justo lo que el usuario está mirando para reconocer la
* canción. Como círculo a la izquierda ocupa su propio sitio y las dos cosas se
* ven a la vez.
*
* ## Y no es un control, es una señal
*
* La fila entera ya reproduce al pulsarla, así que el botón no añade ninguna
* acción: añade la **pista visual** de que ahí se puede pulsar. Por eso no
* captura el ratón (`pointer-events: none`) y está oculto para los lectores de
* pantalla: un segundo control que hace exactamente lo mismo que su contenedor
* sería un obstáculo al tabular, no una ayuda.
*
* Reserva su hueco siempre, aunque esté invisible: aparecer al pasar el ratón
* no puede desplazar el título medio centímetro a la derecha.
*/
export function comienzoDePista(albumId, trackId) {
	const frag = document.createDocumentFragment();
	const play = document.createElement("span");
	play.className = "track__play";
	play.setAttribute("aria-hidden", "true");
	play.append(icono("play", 14));
	const arte = document.createElement("span");
	arte.className = "track__arte";
	arte.append(icono("music", 16));
	// Con identificador de pista, la portada la resuelve el backend por la
	// canción. Sin él —los sitios que aún no lo pasan— se cae al álbum.
	if (trackId) ponerPortadaDePista(arte, trackId);
	else ponerPortada(arte, albumId);
	frag.append(play, arte);
	return frag;
}
/**
* Rellena un hueco con la imagen de una playlist.
*
* ## Una portada o un mosaico, según lo que haya
*
* Una playlist no tiene portada propia: la hereda de lo que contiene. Con un
* solo álbum se usa esa portada entera; con dos o más se compone una rejilla
* 2×2, que es lo que hace que se reconozca de un vistazo como "esa lista", sin
* que nadie haya tenido que elegir una imagen.
*
* ## Con tres, se repite la primera
*
* Una rejilla 2×2 con tres imágenes deja un cuadrante vacío, y ese hueco se lee
* como un error de carga. Repetir la primera lo cierra y nadie nota nada.
*
* ## Vacía no es un fallo
*
* Una playlist recién creada no tiene canciones, así que no tiene portadas.
* Queda el icono del hueco, que es la respuesta correcta: todavía no hay nada.
*/
export function ponerMosaico(hueco, albumes) {
	if (albumes.length === 0) return;
	if (albumes.length === 1) {
		ponerPortada(hueco, albumes[0]);
		return;
	}
	const rejilla = document.createElement("div");
	rejilla.className = "mosaico";
	const cuatro = albumes.length === 3 ? [...albumes, albumes[0] ?? ""] : albumes.slice(0, 4);
	for (const album of cuatro) {
		const celda = document.createElement("span");
		celda.className = "mosaico__celda";
		ponerPortada(celda, album);
		rejilla.append(celda);
	}
	hueco.append(rejilla);
}
/**
* Pinta la imagen de una playlist: la del usuario si la eligió, o el mosaico.
*
* Está en un solo sitio porque aparece en tres —barra lateral, ficha e Inicio—
* y la regla de cuál gana no puede estar escrita tres veces: la primera que se
* olvide de actualizar enseñará el mosaico de una playlist que ya tiene foto.
*
* ## La URL lleva la marca de tiempo
*
* Las portadas se sirven con `immutable`, que es lo correcto porque el
* contenido de una URL no cambia... salvo que el usuario cambie su imagen. La
* marca de tiempo de la playlist hace que sea otra URL, y el WebView la pide de
* nuevo en vez de enseñar la anterior hasta reiniciar.
*/
export function ponerImagenDePlaylist(hueco, p) {
	if (p.hasOwnCover) {
		const img = document.createElement("img");
		img.className = "portada";
		img.alt = "";
		img.decoding = "async";
		img.addEventListener("load", () => img.classList.add("is-lista"));
		asignarSrcConRespaldo(img, `playlist/${encodeURIComponent(p.id)}?v=${p.updatedAt}`, () => img.remove());
		hueco.append(img);
		return;
	}
	ponerMosaico(hueco, p.coverAlbums);
}
/**
* Igual que [`comienzoDePista`], pero para listas virtualizadas.
*
* ## Por qué hace falta una versión aparte
*
* Una lista virtualizada **recicla** sus filas: veinte nodos sirven para
* cincuenta mil canciones. Crear una etiqueta `<img>` en cada pintado dejaría
* miles de imágenes a medio cargar en vuelo durante un desplazamiento rápido,
* cada una pidiendo su portada al backend. Reutilizando la misma etiqueta,
* cambiar `src` **cancela** la petición anterior: como mucho hay tantas en
* curso como filas visibles, unas quince, se desplace lo que se desplace.
*
* ## La imagen se oculta antes de cambiar de fuente
*
* Sin eso, la fila reciclada enseña la carátula de la canción que ocupaba ese
* nodo hace un momento mientras carga la nueva. Es el error clásico de las
* listas recicladas y se nota muchísimo: la portada equivocada junto al título
* correcto.
*/
export function comienzoReciclable() {
	const play = document.createElement("span");
	play.className = "track__play";
	play.setAttribute("aria-hidden", "true");
	play.append(icono("play", 14));
	const arte = document.createElement("span");
	arte.className = "track__arte";
	arte.append(icono("music", 16));
	const img = document.createElement("img");
	img.className = "portada";
	img.alt = "";
	img.decoding = "async";
	img.addEventListener("load", () => img.classList.add("is-lista"));
	arte.append(img);
	let puesto = null;
	return {
		nodos: [play, arte],
		pintar(albumId, trackId) {
			// Siempre por la pista cuando se conoce: es lo que hace que la fila y el
			// reproductor enseñen lo mismo. El álbum solo queda como respaldo para
			// quien todavía no pasa el identificador.
			const ruta = trackId ? `track/${encodeURIComponent(trackId)}` : albumId ? encodeURIComponent(albumId) : null;
			if (ruta === puesto) return;
			puesto = ruta;
			img.classList.remove("is-lista");
			if (ruta === null) {
				img.onerror = null;
				img.removeAttribute("src");
				return;
			}
			// Al fallar se queda el icono debajo, que para eso está. La etiqueta
			// no se quita: este nodo tiene que seguir sirviendo para la siguiente
			// canción. `asignarSrcConRespaldo` reemplaza `onerror` en cada llamada,
			// así que reciclar el nodo para otra pista no arrastra el fallo del
			// anterior.
			asignarSrcConRespaldo(img, ruta, () => img.classList.remove("is-lista"));
		}
	};
}
/** Una tarjeta con portada, título y subtítulo. */
export function tarjeta(datos) {
	const a = document.createElement("a");
	a.className = "tarjeta";
	a.href = datos.destino;
	a.setAttribute("role", "listitem");
	const arte = document.createElement("div");
	arte.className = "tarjeta__arte";
	if (datos.redonda) arte.classList.add("tarjeta__arte--redonda");
	arte.append(icono(datos.marcador ?? "music", 32));
	if (datos.playlist) ponerImagenDePlaylist(arte, datos.playlist);
	else if (datos.artistId) ponerFotoDeArtista(arte, datos.artistId);
	else if (datos.trackId) ponerPortadaDePista(arte, datos.trackId);
	else ponerPortada(arte, datos.albumId);
	const t = document.createElement("div");
	t.className = "tarjeta__titulo";
	t.textContent = datos.titulo;
	const s = document.createElement("div");
	s.className = "tarjeta__sub";
	s.textContent = datos.subtitulo;
	a.append(arte, t, s);
	return a;
}
/** Bloque de texto para cuando una vista no tiene nada que mostrar. */
export function vacio(texto) {
	const p = document.createElement("p");
	p.className = "vista__empty";
	p.textContent = texto;
	return p;
}
/**
* Esqueleto de carga.
*
* Aparece solo si la espera pasa de 150 ms: por debajo, un destello gris es
* más molesto que la propia espera.
*/
export function esqueleto(filas) {
	const el = document.createElement("div");
	el.className = "esqueleto";
	for (let i = 0; i < filas; i += 1) {
		const linea = document.createElement("div");
		linea.className = "esqueleto__linea";
		el.append(linea);
	}
	return el;
}
/**
* Muestra un indicador de carga solo si la promesa tarda.
*
* Devuelve la promesa original para poder encadenarla.
*/
export async function conEspera(contenedor, promesa, umbralMs = 150) {
	let indicador = null;
	const temporizador = globalThis.setTimeout(() => {
		indicador = esqueleto(6);
		contenedor.append(indicador);
	}, umbralMs);
	try {
		return await promesa;
	} finally {
		globalThis.clearTimeout(temporizador);
		indicador?.remove();
	}
}

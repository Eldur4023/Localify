/**
* Diálogos de la aplicación.
*
* ## Por qué no `prompt()`
*
* `globalThis.prompt` funciona, pero abre el cuadro del sistema: tipografía,
* colores y botones del navegador en medio de una aplicación oscura. Delata que
* debajo hay un WebView, que es justo lo que no debe notarse. Y no se puede
* traducir: los botones salen en el idioma de Windows, no en el de Localify.
*
* ## Se apoya en `<dialog>`, no en un `div` con `position: fixed`
*
* El elemento nativo trae hechas las tres cosas que se olvidan al reimplementarlo:
* el foco se queda dentro mientras está abierto, `Escape` cierra, y al cerrarse
* el foco vuelve a donde estaba. Además se pinta en la capa superior del
* navegador, así que ningún `z-index` de la aplicación puede taparlo.
*
* ## Devuelve una promesa
*
* Quien lo abre espera una respuesta, igual que con `prompt`, y así el código
* que llama se lee en el mismo orden en que pasan las cosas.
*/
import { t } from "../i18n/index.js";
/**
* Pide un texto. Resuelve a `null` si se cancela.
*
* El valor devuelto viene recortado, y un texto en blanco cuenta como cancelar:
* una playlist llamada "   " no es lo que nadie quería crear.
*/
export function pedirTexto(opciones) {
	return pedirTextoConImagen(opciones).then((r) => r?.texto ?? null);
}
/**
* Igual que [`pedirTexto`], pero devuelve también la imagen elegida.
*
* ## La imagen es opcional y va después del nombre
*
* Crear una playlist tiene que poder hacerse escribiendo un nombre y pulsando
* Enter. Poner la foto por delante, o hacerla obligatoria, convertiría un gesto
* de dos segundos en una sesión de decoración. Quien la quiera, la elige; quien
* no, ni se entera de que estaba.
*/
export function pedirTextoConImagen(opciones) {
	return new Promise((resolver) => {
		const dlg = document.createElement("dialog");
		dlg.className = "dialogo";
		const form = document.createElement("form");
		// `method="dialog"` hace que enviar cierre el diálogo y guarde el botón
		// pulsado en `returnValue`. Sin esto habría que interceptar el submit para
		// que Enter no recargara la página.
		form.method = "dialog";
		const h = document.createElement("h2");
		h.className = "dialogo__titulo";
		h.textContent = opciones.titulo;
		const etiqueta = document.createElement("label");
		etiqueta.className = "dialogo__campo";
		const texto = document.createElement("span");
		texto.textContent = opciones.etiqueta;
		// `<textarea>` o `<input>` según haga falta. Los dos comparten `value`,
		// `maxLength` y `select()`, así que el resto del diálogo no distingue.
		const entrada = opciones.multilinea ? document.createElement("textarea") : document.createElement("input");
		if (entrada instanceof HTMLInputElement) entrada.type = "text";
		else entrada.rows = 4;
		entrada.className = "dialogo__input";
		entrada.value = opciones.valor ?? "";
		entrada.autocomplete = "off";
		entrada.spellcheck = false;
		if (opciones.maxLength) entrada.maxLength = opciones.maxLength;
		etiqueta.append(texto, entrada);
		// ── Imagen opcional ──────────────────────────────────────────────────
		let imagenElegida = null;
		let previsualizacion = null;
		if (opciones.imagen) {
			const { etiqueta: rotulo, elegir } = opciones.imagen;
			const fila = document.createElement("div");
			fila.className = "dialogo__imagen";
			const muestra = document.createElement("div");
			muestra.className = "dialogo__muestra";
			previsualizacion = muestra;
			const boton = document.createElement("button");
			boton.type = "button";
			boton.className = "boton boton--sutil";
			boton.textContent = rotulo;
			boton.addEventListener("click", () => {
				void elegir().then((ruta) => {
					if (!ruta) return;
					imagenElegida = ruta;
					// Solo el nombre del fichero: la ruta completa no cabe y no dice
					// nada que el usuario no sepa ya. No se previsualiza la imagen
					// porque el WebView no puede leer ficheros sueltos del disco, y
					// abrirle esa puerta por una miniatura no compensa.
					muestra.textContent = ruta.split(/[\\/]/).pop() ?? "";
				});
			});
			fila.append(boton, muestra);
			form.append(h, etiqueta, fila);
		}
		const acciones = document.createElement("div");
		acciones.className = "dialogo__acciones";
		const cancelar = document.createElement("button");
		cancelar.type = "button";
		cancelar.className = "boton boton--sutil";
		cancelar.textContent = t("common.cancel");
		const aceptar = document.createElement("button");
		aceptar.type = "submit";
		aceptar.className = "boton";
		aceptar.textContent = opciones.aceptar ?? t("common.confirm");
		acciones.append(cancelar, aceptar);
		if (!previsualizacion) form.append(h, etiqueta);
		form.append(acciones);
		dlg.append(form);
		document.body.append(dlg);
		let respuesta = null;
		cancelar.addEventListener("click", () => dlg.close());
		form.addEventListener("submit", () => {
			const valor = entrada.value.trim();
			const vale = valor.length > 0 || opciones.permitirVacio === true;
			respuesta = vale ? {
				texto: valor,
				imagen: imagenElegida
			} : null;
		});
		// Un clic en el fondo oscuro cancela, que es lo que espera quien lo hace.
		// El objetivo es el propio `<dialog>` solo cuando se pulsa fuera del
		// formulario, porque el contenido ocupa toda su caja.
		dlg.addEventListener("click", (e) => {
			if (e.target === dlg) dlg.close();
		});
		dlg.addEventListener("close", () => {
			dlg.remove();
			resolver(respuesta);
		});
		dlg.showModal();
		entrada.select();
	});
}
/**
* Pregunta sí o no. Resuelve a `true` solo si se confirma.
*
* `mensaje` es para las acciones destructivas: un título solo puede decir qué
* se va a hacer, pero no **qué se conserva**, y esa es la mitad que decide.
* "Borrar todo lo descargado" sin más se lee como "pierdo mis playlists".
*
* `destructivo` (por defecto `true`) decide dos cosas: el color del botón de
* aceptar y dónde arranca el foco. Para "borrar todo" el foco debe empezar en
* "cancelar" —no puede resolverse dándole a Enter por inercia—, pero esa misma
* cautela en "¿quieres actualizar?" solo estorbaría: ahí no hay nada que
* perder por aceptar de más.
*/
export function confirmar(titulo, aceptar, mensaje, destructivo = true) {
	return new Promise((resolver) => {
		const dlg = document.createElement("dialog");
		dlg.className = "dialogo";
		const form = document.createElement("form");
		form.method = "dialog";
		const h = document.createElement("h2");
		h.className = "dialogo__titulo";
		h.textContent = titulo;
		const acciones = document.createElement("div");
		acciones.className = "dialogo__acciones";
		const no = document.createElement("button");
		no.type = "button";
		no.className = "boton boton--sutil";
		no.textContent = t("common.cancel");
		const si = document.createElement("button");
		si.type = "submit";
		si.className = destructivo ? "boton boton--peligro" : "boton";
		si.textContent = aceptar ?? t("common.confirm");
		acciones.append(no, si);
		if (mensaje) {
			const p = document.createElement("p");
			p.className = "dialogo__mensaje";
			p.textContent = mensaje;
			form.append(h, p, acciones);
		} else {
			form.append(h, acciones);
		}
		dlg.append(form);
		document.body.append(dlg);
		let confirmado = false;
		no.addEventListener("click", () => dlg.close());
		form.addEventListener("submit", () => {
			confirmado = true;
		});
		dlg.addEventListener("click", (e) => {
			if (e.target === dlg) dlg.close();
		});
		dlg.addEventListener("close", () => {
			dlg.remove();
			resolver(confirmado);
		});
		dlg.showModal();
		(destructivo ? no : si).focus();
	});
}

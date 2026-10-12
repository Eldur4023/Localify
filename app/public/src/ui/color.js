/**
* Color dominante de una portada, para teñir el reproductor como hace Spotify.
*
* Se promedia el cuadrado central (lo que enseña la portada) en una cuadrícula
* de 16×16, dando más peso a los píxeles saturados: así las bandas negras o
* grises no apagan el resultado. Devuelve `rgb(...)` ya oscurecido para que el
* texto blanco se lea encima, o null si la imagen no se puede leer.
*/
export function colorDePortada(img) {
	try {
		const lado = 16;
		const lienzo = document.createElement("canvas");
		lienzo.width = lienzo.height = lado;
		const ctx = lienzo.getContext("2d", { willReadFrequently: true });
		const w = img.naturalWidth;
		const h = img.naturalHeight;
		if (!w || !h) return null;
		const s = Math.min(w, h);
		ctx.drawImage(img, (w - s) / 2, (h - s) / 2, s, s, 0, 0, lado, lado);
		const datos = ctx.getImageData(0, 0, lado, lado).data;
		let r = 0, g = 0, b = 0, peso = 0;
		for (let i = 0; i < datos.length; i += 4) {
			const sat = Math.max(datos[i], datos[i + 1], datos[i + 2]) - Math.min(datos[i], datos[i + 1], datos[i + 2]);
			const luz = (datos[i] + datos[i + 1] + datos[i + 2]) / 3;
			const p = (1 + (sat * sat) / 64) * (luz < 24 ? 0.1 : 1);
			r += datos[i] * p; g += datos[i + 1] * p; b += datos[i + 2] * p; peso += p;
		}
		const k = 0.5 / peso;
		return `rgb(${Math.round(r * k)}, ${Math.round(g * k)}, ${Math.round(b * k)})`;
	} catch {
		return null; // lienzo contaminado o imagen sin cargar: se queda el color por defecto
	}
}

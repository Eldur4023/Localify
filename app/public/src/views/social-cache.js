/** Utilidades de Social: formato de tiempos y caché "viejo primero" en localStorage. */
/** Tiempo escuchado: "2 h 05 min" o "12 min". */
export function tiempo(ms) {
	const min = Math.round((ms || 0) / 6e4);
	return min >= 60 ? `${Math.floor(min / 60)} h ${String(min % 60).padStart(2, "0")} min` : `${min} min`;
}
export function duracion(ms) {
	const s = Math.round((ms || 0) / 1e3);
	return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
}
export const PREFIJO_CACHE = "localify.social.";
export const SIN_CACHE = new Error("sin caché");
export function leerCache(clave) {
	try {
		const v = localStorage.getItem(PREFIJO_CACHE + clave);
		return v === null ? undefined : JSON.parse(v);
	} catch {
		return undefined;
	}
}
export function guardarCache(clave, valor) {
	try {
		localStorage.setItem(PREFIJO_CACHE + clave, JSON.stringify(valor));
	} catch {}
}

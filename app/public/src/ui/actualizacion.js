import { actualizaciones } from "../ipc/client.js";
import { t } from "../i18n/index.js";
import { nodo } from "./dom.js";
/**
* Aviso de versión nueva. El backend (actualizar.lux) pregunta a GitHub al abrir la app y cada 6 h; aquí solo se pinta.
* "Más tarde" lo oculta hasta que salga OTRA versión. En Android el APK lo baja e instala el puente nativo
* (AndroidBridge.installApk); en escritorio lo hace el backend y reinicia la app.
*/
const SALTADA = "localify.update.saltada";
function leer() {
	try { return globalThis.localStorage.getItem(SALTADA); } catch { return null; }
}
function guardar(v) {
	try { globalThis.localStorage.setItem(SALTADA, v); } catch { /* sin almacenamiento: reaparecerá */ }
}
function mostrar(s) {
	if (document.querySelector(".actualizacion")) return;
	const caja = nodo("div", "actualizacion");
	caja.setAttribute("role", "status");
	const texto = nodo("span", "actualizacion__texto", t("update.available", { version: s.latest }));
	const acciones = nodo("div", "actualizacion__acciones");
	const instalar = nodo("button", "boton", s.canInstall ? t("update.install") : t("update.view"));
	instalar.type = "button";
	instalar.addEventListener("click", async () => {
		if (!s.canInstall) {
			globalThis.open(s.page, "_blank");
			return;
		}
		instalar.disabled = true;
		texto.textContent = t("update.installing");
		try {
			if (globalThis.AndroidBridge?.installApk) globalThis.AndroidBridge.installApk(s.url);
			else {
				const r = await actualizaciones.install();
				if (!r.ok) throw new Error(r.message);
				// el script reinicia la app al acabar; si en 5 minutos no ha pasado, algo falló: se vuelve a ofrecer
				globalThis.setTimeout(() => {
					texto.textContent = t("update.failed", { error: t("update.timeout") });
					instalar.disabled = false;
				}, 5 * 60 * 1000);
			}
		} catch (e) {
			texto.textContent = t("update.failed", { error: String(e?.message ?? e) });
			instalar.disabled = false;
		}
	});
	const luego = nodo("button", "boton boton--sutil", t("update.later"));
	luego.type = "button";
	luego.addEventListener("click", () => {
		guardar(s.latest);
		caja.remove();
	});
	acciones.append(instalar, luego);
	caja.append(texto, acciones);
	document.body.append(caja);
}
async function mirar() {
	try {
		const s = await actualizaciones.status();
		if (s.available && leer() !== s.latest) mostrar(s);
	} catch { /* sin red o sin releases: nada que avisar */ }
}
/** Mira al poco de abrir y cada 30 min; el backend consulta a GitHub al abrir la app y luego cada 6 h. */
export function vigilarActualizaciones() {
	globalThis.setTimeout(() => void mirar(), 8000);
	globalThis.setInterval(() => void mirar(), 30 * 60 * 1000);
}

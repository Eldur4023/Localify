/**
* Enlaces localify:// que abren la app (el sistema se los pasa al binario o a la
* actividad de Android, y estos al backend: POST /api/deeplink).
*
* Hoy solo hay uno: localify://invite?server=host:puerto&code=…, de la página de
* invitación del servidor. Abre Social con el servidor y el código ya puestos.
*/
import { social } from "../ipc/client.js";
import { alRecibirTipo, emitirLocal } from "../ipc/events.js";
/** Invitación por usar: la recoge el panel de acceso de Social (`tomarInvitacion`). */
let invitacion = null;
export function tomarInvitacion() {
	const i = invitacion;
	invitacion = null;
	return i;
}
async function recoger() {
	const { url } = await social.takeDeepLink();
	if (!url) return;
	let u;
	try {
		u = new URL(url);
	} catch {
		return;
	}
	const server = u.searchParams.get("server") ?? "";
	const code = u.searchParams.get("code") ?? "";
	if (u.hostname !== "invite" || !server || !code) return;
	invitacion = { server, code };
	globalThis.location.hash = "#/social";
	// si Social ya estaba abierta, el cambio de ruta no la vuelve a montar
	emitirLocal("inviteLink");
}
/** Arranque: el enlace pudo llegar antes de que cargara la página; luego, cada aviso del backend. */
export function vigilarEnlaces() {
	void recoger().catch(() => {});
	alRecibirTipo("deepLink", () => void recoger().catch(() => {}));
}

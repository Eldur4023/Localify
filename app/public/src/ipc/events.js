const CANAL_EVENTOS = "localify://event";
const CANAL_RESYNC = "localify://resync";
const manejadores = new Set();
const manejadoresResync = new Set();
let iniciado = false;
/**
* Arranca la escucha. Es idempotente: llamarla dos veces no duplica los
* manejadores del puente.
*
* En este port no hay puente de eventos de Tauri: el transporte sondea el
* estado del backend (`/api/events/poll`) y sintetiza los MISMOS eventos con
* los MISMOS nombres que emitía el bus de Rust, de modo que ninguna vista
* cambia. Ver `ipc/transport.js`.
*/
export async function iniciar() {
	if (iniciado) return;
	iniciado = true;
	const { arrancarEventos } = await import("./transport.js");
	arrancarEventos((evento) => {
		for (const m of manejadores) {
			// Un manejador que falle no debe impedir que los demás reciban el
			// evento: son independientes entre sí.
			try {
				m(evento);
			} catch (e) {
				console.error("manejador de evento falló", evento.type, e);
			}
		}
	}, () => {
		for (const m of manejadoresResync) {
			try {
				m();
			} catch (e) {
				console.error("manejador de resincronización falló", e);
			}
		}
	});
}
/** Se suscribe a todos los eventos. */
export function alRecibir(manejador) {
	manejadores.add(manejador);
	return () => manejadores.delete(manejador);
}
/**
* Se suscribe a un tipo concreto de evento, con el payload ya estrechado.
*/
export function alRecibirTipo(tipo, manejador) {
	return alRecibir((evento) => {
		if (evento.type === tipo) {
			manejador(evento);
		}
	});
}
/**
* Se suscribe a la señal de resincronización.
*
* Quien la reciba debe **recargar su estado** con los comandos de consulta, no
* intentar reconstruirlo a partir de los eventos que sí llegaron.
*/
export function alResincronizar(manejador) {
	manejadoresResync.add(manejador);
	return () => manejadoresResync.delete(manejador);
}

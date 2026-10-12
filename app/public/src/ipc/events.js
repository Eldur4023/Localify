const CANAL_EVENTOS = "localify://event";
const manejadores = new Set();
let iniciado = false;
/**
* Arranca la escucha. Es idempotente: llamarla dos veces no duplica los
* manejadores del puente.
*
* El backend empuja una foto de su estado por SSE (`/api/events/stream`) y el transporte sintetiza
* los eventos por diferencias. Ver `ipc/transport.js`.
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

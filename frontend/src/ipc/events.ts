/**
 * Escucha de eventos del backend.
 *
 * ## Por qué existe la resincronización
 *
 * El bus del backend descarta mensajes si el puente se retrasa. En lugar de
 * dejar que la interfaz quede desincronizada **en silencio** —que es el peor
 * modo de fallo: todo parece funcionar y los datos están mal—, el backend emite
 * `localify://resync` y aquí se avisa a quien tenga que recargar su estado.
 *
 * Por eso los eventos llevan identificadores y no objetos completos: son una
 * optimización, no la fuente de verdad. Perder uno degrada la reactividad
 * durante un instante, nunca la corrección.
 */

import type { LocalifyEvent } from "./types.gen.js";

const CANAL_EVENTOS = "localify://event";
const CANAL_RESYNC = "localify://resync";

type Manejador = (evento: LocalifyEvent) => void;
type ManejadorResync = () => void;
/** Función que cancela una suscripción. */
export type Cancelar = () => void;

const manejadores = new Set<Manejador>();
const manejadoresResync = new Set<ManejadorResync>();
let iniciado = false;

/**
 * Arranca la escucha. Es idempotente: llamarla dos veces no duplica los
 * manejadores del puente.
 */
export async function iniciar(): Promise<void> {
  if (iniciado) return;
  iniciado = true;

  const tauri = window.__TAURI__;
  if (!tauri) {
    throw new Error("el puente de Tauri no está disponible");
  }

  await tauri.event.listen<LocalifyEvent>(CANAL_EVENTOS, ({ payload }) => {
    for (const m of manejadores) {
      // Un manejador que falle no debe impedir que los demás reciban el
      // evento: son independientes entre sí.
      try {
        m(payload);
      } catch (e) {
        console.error("manejador de evento falló", payload.type, e);
      }
    }
  });

  await tauri.event.listen<null>(CANAL_RESYNC, () => {
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
export function alRecibir(manejador: Manejador): Cancelar {
  manejadores.add(manejador);
  return () => manejadores.delete(manejador);
}

/**
 * Se suscribe a un tipo concreto de evento, con el payload ya estrechado.
 */
export function alRecibirTipo<T extends LocalifyEvent["type"]>(
  tipo: T,
  manejador: (evento: Extract<LocalifyEvent, { type: T }>) => void,
): Cancelar {
  return alRecibir((evento) => {
    if (evento.type === tipo) {
      manejador(evento as Extract<LocalifyEvent, { type: T }>);
    }
  });
}

/**
 * Se suscribe a la señal de resincronización.
 *
 * Quien la reciba debe **recargar su estado** con los comandos de consulta, no
 * intentar reconstruirlo a partir de los eventos que sí llegaron.
 */
export function alResincronizar(manejador: ManejadorResync): Cancelar {
  manejadoresResync.add(manejador);
  return () => manejadoresResync.delete(manejador);
}

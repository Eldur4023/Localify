-- Quita Last.fm: la cola de scrobbles pendientes deja de tener sentido sin
-- nadie que la vacíe.
--
-- Se borra la tabla y no se deja vacía a propósito: unas filas huérfanas de
-- una función que ya no existe no son un dato que conservar, son basura que
-- alguien tendría que explicar dentro de un año.

DROP INDEX IF EXISTS idx_scrobble_pending;
DROP TABLE IF EXISTS scrobble_queue;

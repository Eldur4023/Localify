-- Marca las pistas cuyos metadatos el usuario fijó a mano, para que el
-- refresco automático (`MetadataService::refresh_stale`, caducidad de 30
-- días) no los sobreescriba silenciosamente.
--
-- Hace falta para dos gestos nuevos: resetear una pista a "sin identificar" y
-- reasignarle metadatos buscando en el proveedor. Los dos dejan la fila con
-- datos que el usuario eligió, no el proveedor; sin este candado, la próxima
-- pasada de refresco los pisaría con los originales en cuanto `metadata_at`
-- volviera a caducar.
ALTER TABLE tracks ADD COLUMN metadata_locked INTEGER NOT NULL DEFAULT 0;

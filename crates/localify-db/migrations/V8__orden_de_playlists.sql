-- Orden manual de las playlists en la barra lateral (arrastrar para
-- reordenar).
--
-- Antes se ordenaban por `updated_at DESC`: la lista saltaba de sitio cada vez
-- que se tocaba una playlist -añadirle una canción la subía al principio-, que
-- es justo lo contrario de un orden que el usuario pueda fijar a mano.
--
-- Clave fraccionaria (ADR-009), igual que `playlist_items.position`: mover una
-- playlist entre sus hermanas es un único `UPDATE`, sea cual sea el tamaño de
-- la lista.
ALTER TABLE playlists ADD COLUMN position REAL NOT NULL DEFAULT 0;

-- El id es un uuid-v7 (ordenable por tiempo de creación): usarlo para el orden
-- inicial deja "las últimas que creé, al final", lo más parecido al orden que
-- ya se veía antes de esta migración.
UPDATE playlists SET position = (
    SELECT (COUNT(*) - 1) * 1024.0
    FROM playlists p2
    WHERE p2.id <= playlists.id
);

CREATE INDEX idx_playlists_position ON playlists (position ASC);

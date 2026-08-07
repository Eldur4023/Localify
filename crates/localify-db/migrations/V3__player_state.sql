-- ═══════════════════════════════════════════════════════════════════════════
-- V3 — Estado del reproductor
--
-- Fila única. Restaura la sesión exactamente donde se dejó: pista, segundo,
-- cola completa, modos y permutación de aleatorio.
--
-- La cola va como JSON en una columna y no en una tabla normalizada. Es una
-- decisión deliberada: se lee y se escribe SIEMPRE entera, nunca se consulta
-- por partes y se actualiza con frecuencia. Normalizarla solo añadiría
-- escrituras. La normalización sirve para consultar; esto no se consulta.
-- ═══════════════════════════════════════════════════════════════════════════

CREATE TABLE player_state (
    id            INTEGER PRIMARY KEY CHECK (id = 1),
    track_id      TEXT    REFERENCES tracks (id) ON DELETE SET NULL,
    position_ms   INTEGER NOT NULL DEFAULT 0,
    volume        REAL    NOT NULL DEFAULT 1.0 CHECK (volume BETWEEN 0.0 AND 1.0),
    repeat_mode   TEXT    NOT NULL DEFAULT 'off'
                  CHECK (repeat_mode IN ('off', 'queue', 'track')),
    shuffle       INTEGER NOT NULL DEFAULT 0 CHECK (shuffle IN (0, 1)),
    -- Reproduce la MISMA permutación tras reiniciar, en lugar de rebarajar.
    shuffle_seed  INTEGER,
    context       TEXT,                       -- JSON: PlaybackContext
    context_queue TEXT,                       -- JSON: [TrackId] en orden efectivo
    user_queue    TEXT,                       -- JSON: [QueueEntry]
    queue_index   INTEGER NOT NULL DEFAULT 0,
    updated_at    INTEGER NOT NULL DEFAULT (unixepoch())
) STRICT;

INSERT INTO player_state (id) VALUES (1);

-- Informe del último escaneo de biblioteca. Se guarda para poder mostrarlo en
-- Ajustes sin volver a escanear.
CREATE TABLE scan_reports (
    id            INTEGER PRIMARY KEY,
    files_scanned INTEGER NOT NULL,
    recovered     INTEGER NOT NULL,
    missing       INTEGER NOT NULL,
    unreadable    INTEGER NOT NULL,
    duration_ms   INTEGER NOT NULL,
    finished_at   INTEGER NOT NULL DEFAULT (unixepoch())
) STRICT;

CREATE INDEX idx_scan_reports_finished ON scan_reports (finished_at DESC);

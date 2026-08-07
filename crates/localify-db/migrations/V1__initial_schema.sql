-- ═══════════════════════════════════════════════════════════════════════════
-- V1 — Esquema inicial
--
-- Principio rector: el identificador principal de una pista es su ID de
-- Spotify. El ID de YouTube nunca es clave de dominio; vive en `youtube_matches`
-- como caché de la capa de descarga.
--
-- `STRICT` en todas las tablas: SQLite obliga a respetar los tipos declarados.
-- Es gratis y elimina una clase entera de errores.
-- ═══════════════════════════════════════════════════════════════════════════

-- ─── CATÁLOGO ──────────────────────────────────────────────────────────────

CREATE TABLE artists (
    id          TEXT    PRIMARY KEY,          -- ID de Spotify, o 'local:<uuid>'
    name        TEXT    NOT NULL,
    name_norm   TEXT    NOT NULL,             -- normalización canónica (core::text)
    image_url   TEXT,
    popularity  INTEGER,
    followers   INTEGER,
    metadata_at INTEGER,                      -- NULL = solo referencia, sin detalle
    created_at  INTEGER NOT NULL DEFAULT (unixepoch())
) STRICT;

CREATE INDEX idx_artists_name_norm ON artists (name_norm);

CREATE TABLE albums (
    id           TEXT    PRIMARY KEY,
    title        TEXT    NOT NULL,
    title_norm   TEXT    NOT NULL,
    album_type   TEXT    NOT NULL DEFAULT 'album'
                 CHECK (album_type IN ('album', 'single', 'compilation')),
    release_date TEXT,                        -- ISO-8601, precisión variable
    total_tracks INTEGER,
    cover_url    TEXT,
    cover_cached INTEGER NOT NULL DEFAULT 0 CHECK (cover_cached IN (0, 1)),
    label        TEXT,
    metadata_at  INTEGER,
    created_at   INTEGER NOT NULL DEFAULT (unixepoch())
) STRICT;

CREATE INDEX idx_albums_title_norm ON albums (title_norm);

CREATE TABLE tracks (
    id           TEXT    PRIMARY KEY,
    title        TEXT    NOT NULL,
    title_norm   TEXT    NOT NULL,
    album_id     TEXT    REFERENCES albums (id) ON DELETE SET NULL,
    duration_ms  INTEGER NOT NULL CHECK (duration_ms > 0),
    track_number INTEGER,
    disc_number  INTEGER DEFAULT 1,
    explicit     INTEGER NOT NULL DEFAULT 0 CHECK (explicit IN (0, 1)),
    isrc         TEXT,                        -- clave de oro para el matching
    popularity   INTEGER,

    -- Denormalización deliberada (ADR-011): sin esto, cada fila de una lista de
    -- 50 000 pistas necesitaría un JOIN con GROUP_CONCAT sobre track_artists.
    -- Solo hay un camino de escritura, así que no puede desincronizarse.
    artist_display TEXT NOT NULL DEFAULT '',
    artist_norm    TEXT NOT NULL DEFAULT '',

    metadata_at  INTEGER,
    added_at     INTEGER NOT NULL DEFAULT (unixepoch())
) STRICT;

CREATE INDEX idx_tracks_album   ON tracks (album_id, disc_number, track_number);
CREATE INDEX idx_tracks_added   ON tracks (added_at DESC, id DESC);
CREATE INDEX idx_tracks_isrc    ON tracks (isrc) WHERE isrc IS NOT NULL;
CREATE INDEX idx_tracks_title_n ON tracks (title_norm);
CREATE INDEX idx_tracks_stale   ON tracks (metadata_at);

CREATE TABLE track_artists (
    track_id  TEXT    NOT NULL REFERENCES tracks (id)  ON DELETE CASCADE,
    artist_id TEXT    NOT NULL REFERENCES artists (id) ON DELETE CASCADE,
    position  INTEGER NOT NULL,               -- 0 = artista principal
    PRIMARY KEY (track_id, artist_id)
) STRICT;

CREATE INDEX idx_track_artists_artist ON track_artists (artist_id);

CREATE TABLE album_artists (
    album_id  TEXT    NOT NULL REFERENCES albums (id)  ON DELETE CASCADE,
    artist_id TEXT    NOT NULL REFERENCES artists (id) ON DELETE CASCADE,
    position  INTEGER NOT NULL,
    PRIMARY KEY (album_id, artist_id)
) STRICT;

CREATE INDEX idx_album_artists_artist ON album_artists (artist_id);

-- Spotify asigna géneros a artistas, no a pistas. El género de una pista se
-- hereda de su artista, y es la señal principal del motor de recomendaciones.
CREATE TABLE genres (
    id   INTEGER PRIMARY KEY,
    name TEXT    NOT NULL UNIQUE
) STRICT;

CREATE TABLE artist_genres (
    artist_id TEXT    NOT NULL REFERENCES artists (id) ON DELETE CASCADE,
    genre_id  INTEGER NOT NULL REFERENCES genres (id)  ON DELETE CASCADE,
    PRIMARY KEY (artist_id, genre_id)
) STRICT;

CREATE INDEX idx_artist_genres_genre ON artist_genres (genre_id);

-- ─── MATERIALIZACIÓN LOCAL ─────────────────────────────────────────────────

-- La existencia de una fila aquí ES la definición de "la pista está en mi
-- biblioteca". Si hay fila, hay fichero completo y verificado. Los ficheros a
-- medias viven en .tmp/ y jamás se registran.
CREATE TABLE audio_files (
    track_id     TEXT    PRIMARY KEY REFERENCES tracks (id) ON DELETE CASCADE,
    rel_path     TEXT    NOT NULL UNIQUE,     -- RELATIVA a la biblioteca (ADR-018)
    format       TEXT    NOT NULL,
    codec        TEXT    NOT NULL,
    bitrate_kbps INTEGER,
    sample_rate  INTEGER,
    channels     INTEGER,
    size_bytes   INTEGER NOT NULL,
    duration_ms  INTEGER NOT NULL,            -- medida real, no la de Spotify
    source       TEXT    NOT NULL DEFAULT 'youtube'
                 CHECK (source IN ('youtube', 'imported')),
    youtube_id   TEXT,                        -- procedencia, informativo
    verified_at  INTEGER NOT NULL,
    created_at   INTEGER NOT NULL DEFAULT (unixepoch())
) STRICT;

CREATE INDEX idx_audio_files_created ON audio_files (created_at DESC, track_id DESC);

-- ─── DESCARGAS Y EMPAREJAMIENTO ────────────────────────────────────────────

-- Caché de la decisión del matcher. Borrarla solo cuesta tiempo, nunca datos.
CREATE TABLE youtube_matches (
    id         INTEGER PRIMARY KEY,
    track_id   TEXT    NOT NULL REFERENCES tracks (id) ON DELETE CASCADE,
    video_id   TEXT    NOT NULL,
    title      TEXT    NOT NULL,
    channel    TEXT,
    duration_s INTEGER,
    view_count INTEGER,
    from_music INTEGER NOT NULL DEFAULT 0 CHECK (from_music IN (0, 1)),
    score      REAL    NOT NULL,
    confidence TEXT    NOT NULL CHECK (confidence IN ('high', 'medium', 'low')),
    breakdown  TEXT    NOT NULL,              -- JSON: por qué ganó (trazabilidad)
    rejected   INTEGER NOT NULL DEFAULT 0 CHECK (rejected IN (0, 1)),
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    UNIQUE (track_id, video_id)
) STRICT;

CREATE INDEX idx_ytm_track ON youtube_matches (track_id, rejected, score DESC);

-- Nótese la ausencia de 'paused' y 'cancelled': no existen en el diseño
-- (ADR-016). Un trabajo solo termina completándose o fallando.
CREATE TABLE download_jobs (
    track_id    TEXT    PRIMARY KEY REFERENCES tracks (id) ON DELETE CASCADE,
    state       TEXT    NOT NULL
                CHECK (state IN ('queued', 'matching', 'downloading',
                                 'finalizing', 'done', 'failed')),
    priority    TEXT    NOT NULL DEFAULT 'prefetch'
                CHECK (priority IN ('immediate', 'prefetch')),
    video_id    TEXT,
    tmp_path    TEXT,
    bytes_done  INTEGER NOT NULL DEFAULT 0,
    bytes_total INTEGER,
    attempts    INTEGER NOT NULL DEFAULT 0,
    last_error  TEXT,                         -- clave i18n, no texto de usuario
    started_at  INTEGER,
    updated_at  INTEGER NOT NULL DEFAULT (unixepoch())
) STRICT;

CREATE INDEX idx_dl_state ON download_jobs (state, priority, updated_at);

-- ─── COLECCIÓN DEL USUARIO ─────────────────────────────────────────────────

CREATE TABLE favorites (
    track_id TEXT    PRIMARY KEY REFERENCES tracks (id) ON DELETE CASCADE,
    added_at INTEGER NOT NULL DEFAULT (unixepoch())
) STRICT;

CREATE INDEX idx_favorites_added ON favorites (added_at DESC, track_id DESC);

CREATE TABLE playlists (
    id          TEXT    PRIMARY KEY,          -- uuid-v7: ordenable por tiempo
    name        TEXT    NOT NULL,
    name_norm   TEXT    NOT NULL,
    description TEXT,
    cover_path  TEXT,                         -- si NULL, la UI compone un mosaico
    source      TEXT    NOT NULL DEFAULT 'local'
                CHECK (source IN ('local', 'spotify_import')),
    source_id   TEXT,
    created_at  INTEGER NOT NULL DEFAULT (unixepoch()),
    updated_at  INTEGER NOT NULL DEFAULT (unixepoch())
) STRICT;

CREATE INDEX idx_playlists_updated ON playlists (updated_at DESC);

-- La entrada tiene ID propio: la misma pista puede aparecer varias veces en la
-- misma playlist, y "elimina esta fila" debe ser inequívoco.
CREATE TABLE playlist_items (
    id          TEXT    PRIMARY KEY,
    playlist_id TEXT    NOT NULL REFERENCES playlists (id) ON DELETE CASCADE,
    track_id    TEXT    NOT NULL REFERENCES tracks (id)    ON DELETE CASCADE,
    -- Clave fraccionaria (ADR-009): reordenar es UN update, no N.
    position    REAL    NOT NULL,
    added_at    INTEGER NOT NULL DEFAULT (unixepoch())
) STRICT;

CREATE INDEX idx_pli_playlist ON playlist_items (playlist_id, position);
CREATE INDEX idx_pli_track    ON playlist_items (track_id);

-- Materia prima del motor de recomendaciones. `completed` distingue una
-- escucha real de un salto: la señal negativa vale tanto como la positiva.
CREATE TABLE play_history (
    id        INTEGER PRIMARY KEY,
    track_id  TEXT    NOT NULL REFERENCES tracks (id) ON DELETE CASCADE,
    played_at INTEGER NOT NULL DEFAULT (unixepoch()),
    ms_played INTEGER NOT NULL,
    completed INTEGER NOT NULL DEFAULT 0 CHECK (completed IN (0, 1)),
    context   TEXT
) STRICT;

CREATE INDEX idx_history_track  ON play_history (track_id, played_at DESC);
CREATE INDEX idx_history_recent ON play_history (played_at DESC);

-- ─── SISTEMA ───────────────────────────────────────────────────────────────

CREATE TABLE settings (
    key        TEXT    PRIMARY KEY,
    value      TEXT    NOT NULL,              -- JSON
    updated_at INTEGER NOT NULL DEFAULT (unixepoch())
) STRICT;

CREATE TABLE cache_entries (
    namespace  TEXT    NOT NULL,
    key        TEXT    NOT NULL,
    value      BLOB    NOT NULL,
    expires_at INTEGER NOT NULL,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    PRIMARY KEY (namespace, key)
) STRICT;

CREATE INDEX idx_cache_expiry ON cache_entries (expires_at);

CREATE TABLE lyrics (
    track_id   TEXT    PRIMARY KEY REFERENCES tracks (id) ON DELETE CASCADE,
    synced     TEXT,                          -- JSON [{atMs, text}]
    plain      TEXT,
    source     TEXT,
    -- Caché negativa: recordar que no existe evita preguntar sin fin.
    not_found  INTEGER NOT NULL DEFAULT 0 CHECK (not_found IN (0, 1)),
    fetched_at INTEGER NOT NULL DEFAULT (unixepoch())
) STRICT;

-- Un scrobble generado sin conexión se envía al recuperarla, en vez de
-- perderse.
CREATE TABLE scrobble_queue (
    id         INTEGER PRIMARY KEY,
    track_id   TEXT    NOT NULL REFERENCES tracks (id) ON DELETE CASCADE,
    timestamp  INTEGER NOT NULL,
    attempts   INTEGER NOT NULL DEFAULT 0,
    last_error TEXT
) STRICT;

CREATE INDEX idx_scrobble_pending ON scrobble_queue (attempts, timestamp);

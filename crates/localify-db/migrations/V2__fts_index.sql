-- ═══════════════════════════════════════════════════════════════════════════
-- V2 — Búsqueda de texto completo
--
-- `content=''` mantiene el índice pequeño: FTS5 guarda solo el índice
-- invertido y los datos se resuelven con un JOIN por rowid. Para 50 000 pistas
-- son unos pocos MB.
--
-- `prefix='2 3'` indexa prefijos de 2 y 3 caracteres, que es lo que hace
-- instantánea la búsqueda mientras se teclea.
--
-- `remove_diacritics 2` iguala "Bjork" y "Björk" en el propio tokenizador, en
-- coherencia con la normalización de `core::text`.
-- ═══════════════════════════════════════════════════════════════════════════

CREATE VIRTUAL TABLE tracks_fts USING fts5 (
    title,
    artist,
    album,
    content  = '',
    tokenize = "unicode61 remove_diacritics 2",
    prefix   = '2 3'
);

-- La sincronización va por triggers para que sea IMPOSIBLE que el índice se
-- desincronice del catálogo. Dejarlo en manos del código de aplicación
-- garantizaría que, tarde o temprano, algún camino de escritura lo olvide.
--
-- IMPORTANTE sobre `content=''`: el índice externo no guarda el texto original,
-- así que una fila 'delete' debe repetir EXACTAMENTE los valores con los que se
-- indexó. Si no coinciden, FTS5 no borra los términos y quedan residuos que
-- devuelven resultados fantasma. De ahí que todos los triggers de borrado
-- reconstruyan los tres campos igual que el de inserción.

CREATE TRIGGER tracks_fts_ai AFTER INSERT ON tracks BEGIN
    INSERT INTO tracks_fts (rowid, title, artist, album)
    VALUES (
        new.rowid,
        new.title,
        new.artist_display,
        COALESCE((SELECT title FROM albums WHERE id = new.album_id), '')
    );
END;

CREATE TRIGGER tracks_fts_ad AFTER DELETE ON tracks BEGIN
    INSERT INTO tracks_fts (tracks_fts, rowid, title, artist, album)
    VALUES (
        'delete',
        old.rowid,
        old.title,
        old.artist_display,
        COALESCE((SELECT title FROM albums WHERE id = old.album_id), '')
    );
END;

CREATE TRIGGER tracks_fts_au
AFTER UPDATE OF title, artist_display, album_id ON tracks BEGIN
    INSERT INTO tracks_fts (tracks_fts, rowid, title, artist, album)
    VALUES (
        'delete',
        old.rowid,
        old.title,
        old.artist_display,
        COALESCE((SELECT title FROM albums WHERE id = old.album_id), '')
    );
    INSERT INTO tracks_fts (rowid, title, artist, album)
    VALUES (
        new.rowid,
        new.title,
        new.artist_display,
        COALESCE((SELECT title FROM albums WHERE id = new.album_id), '')
    );
END;

-- Renombrar un álbum deja obsoleto el campo `album` de todas sus pistas. Sin
-- este trigger, buscar por el título nuevo no las encontraría y buscar por el
-- viejo seguiría encontrándolas.
CREATE TRIGGER albums_fts_au AFTER UPDATE OF title ON albums
WHEN old.title <> new.title
BEGIN
    INSERT INTO tracks_fts (tracks_fts, rowid, title, artist, album)
    SELECT 'delete', t.rowid, t.title, t.artist_display, old.title
    FROM tracks t
    WHERE t.album_id = old.id;

    INSERT INTO tracks_fts (rowid, title, artist, album)
    SELECT t.rowid, t.title, t.artist_display, new.title
    FROM tracks t
    WHERE t.album_id = new.id;
END;

-- Borrar un álbum pone `tracks.album_id` a NULL (ON DELETE SET NULL), pero eso
-- NO dispara `tracks_fts_au`: las acciones referenciales no cuentan como un
-- UPDATE con lista de columnas. Hay que reindexar aquí.
CREATE TRIGGER albums_fts_bd BEFORE DELETE ON albums BEGIN
    INSERT INTO tracks_fts (tracks_fts, rowid, title, artist, album)
    SELECT 'delete', t.rowid, t.title, t.artist_display, old.title
    FROM tracks t
    WHERE t.album_id = old.id;

    INSERT INTO tracks_fts (rowid, title, artist, album)
    SELECT t.rowid, t.title, t.artist_display, ''
    FROM tracks t
    WHERE t.album_id = old.id;
END;

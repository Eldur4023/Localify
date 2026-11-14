-- Una sola vez, en servidores creados antes de que users.id llevara AUTOINCREMENT (sin él, borrar al último
-- usuario hacía que el siguiente heredara su id).
--   systemctl stop serverify ; cp data/serverify.db data/serverify.antes-autoincrement.db
--   sqlite3 data/serverify.db < migrar-users-autoincrement.sql ; systemctl start serverify
-- NO se hace al arrancar el servidor: dentro de Lux las claves foráneas están activas y DROP TABLE users
-- dispararía los ON DELETE CASCADE (invitaciones, playlists, historial...). Aquí se desactivan.
PRAGMA foreign_keys = OFF;
BEGIN;
CREATE TABLE users_nueva(
    id integer primary key autoincrement,
    username text not null unique collate nocase,
    display_name text not null,
    bio text not null default '',
    pass_hash text not null,
    created_at integer not null default (unixepoch())
);
INSERT INTO users_nueva(id, username, display_name, bio, pass_hash, created_at)
    SELECT id, username, display_name, bio, pass_hash, created_at FROM users;
DROP TABLE users;
ALTER TABLE users_nueva RENAME TO users;
COMMIT;
PRAGMA foreign_key_check;

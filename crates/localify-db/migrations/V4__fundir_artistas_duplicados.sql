-- Funde los artistas que son la misma persona guardada varias veces.
--
-- Un artista no tiene identificador universal: es un canal en YouTube Music, un
-- UUID en MusicBrainz y **nada** cuando llega de la página de incrustación de
-- Spotify, que solo publica nombres. Para ese último caso se inventaba uno
-- local por cada canción, así que importar treinta temas de un grupo creaba
-- treinta artistas idénticos, todos con una canción y sin foto.
--
-- El origen ya está arreglado (`id_canonico` en el repositorio de pistas), pero
-- eso solo vale para lo que se escriba a partir de ahora. Esto limpia lo que ya
-- estaba.
--
-- ## Qué se funde y qué no
--
-- **Solo desaparecen los locales.** Dos artistas con identidad de catálogo y el
-- mismo nombre pueden ser dos personas distintas, y unirlos sería inventarse un
-- dato. Un local es literalmente "no sabemos quién es", así que absorberlo en
-- uno que sí lo sabe no pierde nada.
--
-- El superviviente se elige por nombre normalizado: primero uno no local; entre
-- varios, el primero por identificador, que es estable entre ejecuciones.

CREATE TEMP TABLE fusion AS
SELECT
    a.id AS viejo,
    (SELECT b.id
       FROM artists b
      WHERE b.name_norm = a.name_norm
      ORDER BY (b.id LIKE 'local:%'), b.id
      LIMIT 1) AS bueno
FROM artists a
WHERE a.id LIKE 'local:%';

DELETE FROM fusion WHERE viejo = bueno;

-- `OR IGNORE` en los tres: la pista puede estar ya asociada al superviviente
-- —pasa cuando una canción acredita al artista dos veces, una por catálogo y
-- otra por importación— y la clave primaria lo rechazaría. Ahí no hay nada que
-- mover: la fila buena ya existe y la vieja sobra.
UPDATE OR IGNORE track_artists
   SET artist_id = (SELECT bueno FROM fusion WHERE viejo = artist_id)
 WHERE artist_id IN (SELECT viejo FROM fusion);

UPDATE OR IGNORE album_artists
   SET artist_id = (SELECT bueno FROM fusion WHERE viejo = artist_id)
 WHERE artist_id IN (SELECT viejo FROM fusion);

UPDATE OR IGNORE artist_genres
   SET artist_id = (SELECT bueno FROM fusion WHERE viejo = artist_id)
 WHERE artist_id IN (SELECT viejo FROM fusion);

-- Lo que el `OR IGNORE` no pudo mover queda apuntando al artista viejo, que
-- ahora se borra. El `ON DELETE CASCADE` de esas tres tablas se lleva las
-- sobras, que son exactamente las duplicadas.
DELETE FROM artists WHERE id IN (SELECT viejo FROM fusion);

DROP TABLE fusion;

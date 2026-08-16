-- Funde el mismo artista visto por dos catálogos distintos.
--
-- La V4 absorbió los artistas `local:` —los que la importación de Spotify se
-- inventaba por canción— y dejó fuera, a propósito, los que tenían identidad de
-- catálogo: «dos artistas con identidad y el mismo nombre pueden ser dos
-- personas distintas, y unirlos sería inventarse un dato».
--
-- Eso sigue siendo cierto **dentro** de un catálogo. Entre catálogos no lo es:
-- un canal de YouTube y un UUID de MusicBrainz son dos formas de nombrar al
-- mismo artista, y tener los dos es un residuo de haber cambiado de proveedor,
-- no dos personas. Los seis casos que quedaban en la biblioteca real eran
-- exactamente eso, sin una sola excepción:
--
--     foto     2 pistas  UCLlchLQvkIB_QWxH6J2tLIA  Casey Edwards
--     -        1 pistas  dd6aeb09-60b7-...         Casey Edwards
--
-- ## Qué sobrevive
--
-- **El canal de YouTube.** No es una preferencia estética: es el que trae la
-- foto, el que responde a `browse` —a un UUID de MusicBrainz el InnerTube
-- contesta 400— y por tanto el único con el que la ficha de artista funciona.
--
-- ## La salvaguarda
--
-- Solo se funde cuando de cada lado hay **exactamente uno**. Dos UUIDs con el
-- mismo nombre son la señal de que sí son dos personas distintas, y ahí no se
-- toca nada.

CREATE TEMP TABLE fusion AS
WITH clasificados AS (
    SELECT
        id,
        name_norm,
        -- Un canal de YouTube: 'UC' y 22 caracteres más.
        (id LIKE 'UC%' AND length(id) = 24) AS es_canal
    FROM artists
    WHERE id NOT LIKE 'local:%'
)
SELECT
    m.id AS viejo,
    c.id AS bueno
FROM clasificados c
JOIN clasificados m
  ON m.name_norm = c.name_norm
 AND m.es_canal = 0
WHERE c.es_canal = 1
  -- Exactamente uno de cada lado, o no se toca.
  AND (SELECT COUNT(*) FROM clasificados x
        WHERE x.name_norm = c.name_norm AND x.es_canal = 1) = 1
  AND (SELECT COUNT(*) FROM clasificados x
        WHERE x.name_norm = c.name_norm AND x.es_canal = 0) = 1;

-- `OR IGNORE` por lo mismo que en la V4: la pista puede acreditar ya a los dos
-- —pasa cuando una canción llegó por un catálogo y se refrescó con el otro— y
-- la clave primaria rechazaría el movimiento. Ahí no hay nada que mover.
UPDATE OR IGNORE track_artists
   SET artist_id = (SELECT bueno FROM fusion WHERE viejo = artist_id)
 WHERE artist_id IN (SELECT viejo FROM fusion);

UPDATE OR IGNORE album_artists
   SET artist_id = (SELECT bueno FROM fusion WHERE viejo = artist_id)
 WHERE artist_id IN (SELECT viejo FROM fusion);

UPDATE OR IGNORE artist_genres
   SET artist_id = (SELECT bueno FROM fusion WHERE viejo = artist_id)
 WHERE artist_id IN (SELECT viejo FROM fusion);

DELETE FROM artists WHERE id IN (SELECT viejo FROM fusion);

-- El nombre visible de la pista se recompone porque la grafía superviviente
-- puede no ser la que había: «kittydog» y «Kittydog» comparten `name_norm` y son
-- dos filas distintas, así que fundirlas cambia lo que se lee en la lista.
--
-- `artist_norm` no hace falta tocarlo: los dos fundidos tienen el mismo
-- `name_norm` por construcción, así que la forma normalizada del conjunto no
-- cambia. Es lo que permite recomponer esto en SQL, donde no existe
-- `text::normalize`.
UPDATE tracks
   SET artist_display = COALESCE((
           SELECT GROUP_CONCAT(nombre, ', ')
             FROM (SELECT ar.name AS nombre
                     FROM track_artists ta
                     JOIN artists ar ON ar.id = ta.artist_id
                    WHERE ta.track_id = tracks.id
                    ORDER BY ta.position)), '')
 WHERE id IN (
     SELECT ta.track_id FROM track_artists ta
      WHERE ta.artist_id IN (SELECT bueno FROM fusion));

DROP TABLE fusion;

#!/usr/bin/env bash
# Extremo a extremo del cliente + servidor (sin ventana): arranca un servidor y la app
# con un HOME y datos temporales y ejercita búsqueda, descarga desde el servidor,
# playlists compartidas, duplicados y el uso sin sesión.
#   tests/serverify-e2e.sh [ruta-al-binario-lux]     (por defecto build/vendor/lux/lux)
REPO=$(cd "$(dirname "$0")/.." && pwd); LUX=$(realpath "${1:-$REPO/build/vendor/lux/lux}")
W=$(mktemp -d); trap 'kill $P1 $P2 $P3 2>/dev/null; rm -rf "$W"' EXIT
mkdir -p $W/srv $W/home; cp $REPO/server/server.lux $W/srv/
(cd $W/srv && JWT_SECRET=e2e-secret-0123456789 exec $LUX server.lux --port 8794 >$W/srv.log 2>&1) & P1=$!
cp -r $REPO/app $W/app; rm -rf $W/app/data
(cd $W/app && HOME=$W/home exec $LUX . --port 8795 --no-watch >$W/cl.log 2>&1) & P2=$!
for i in $(seq 60); do curl -sf localhost:8794/api/info >/dev/null && curl -s -X POST -H 'content-type: application/json' -d '{}' localhost:8795/api/events/poll -o /dev/null -f && break; sleep 0.3; done
inv() { curl -s -X POST -H 'content-type: application/json' -d "$(python3 -c 'import json,sys;print(json.dumps({"cmd":sys.argv[1],"args":sys.argv[2]}))' "$1" "${2:-{\}}")" localhost:8795/api/invoke; }
fail=0; ok() { if [[ "$2" == *"$3"* ]]; then echo "  ok    $1"; else echo "  FAIL  $1: got [${2:0:300}] want [$3]"; fail=1; fi; }
B=localhost:8794/api
# bob siembra una pista en el servidor
TB=$(curl -s -X POST -H 'content-type: application/json' -d '{"username":"bob","password":"secreto123"}' $B/register | sed 's/.*"token":"\([^"]*\)".*/\1/')
head -c 40000 /dev/urandom > $W/a.opus
curl -s -X PUT -H "Authorization: Bearer $TB" -F audio=@$W/a.opus -F ext=opus -F title=Yellow -F artist=Coldplay -F album=Parachutes -F duration_ms=266000 -F search_norm="yellow coldplay parachutes" $B/tracks/dQw4w9WgXcQ >/dev/null
ok "sin servidor: búsqueda local vacía" "$(inv search_query '{"q":"yellow"}')" '"tracks":[]'
ok "sin sesión: comando social" "$(inv social_users)" SERVER_AUTH
ok "conectar" "$(inv server_connect '{"host":"127.0.0.1","port":8794}')" '"reachable":true'
ok "conectado sin sesión: estado" "$(inv server_status)" '"loggedIn":false'
ok "sin sesión: búsqueda funciona" "$(inv search_query '{"q":"yellow"}')" '"queryId"'
ok "sin sesión: no trae del servidor" "$(sqlite3 $W/app/data/localify.db "select count(*) from tracks where id='dQw4w9WgXcQ'")" 0
ok "sin sesión: social pide login" "$(inv social_playlists)" SERVER_AUTH
ok "sin sesión: aportar biblioteca pide login" "$(inv server_sync_library)" SERVER_AUTH
ok "registro" "$(inv server_register '{"username":"ana","password":"secreto123","displayName":"Ana"}')" '"username":"ana"'
ok "estado" "$(inv server_status)" '"loggedIn":true'
ok "búsqueda trae del servidor" "$(inv search_query '{"q":"yellow"}')" 'dQw4w9WgXcQ'
ok "candidatos del servidor" "$(inv library_search_candidates '{"query":"coldplay yellow","limit":3}')" '"title":"Yellow"'
# descarga: debe venir del servidor (no hay yt-dlp que valga: la red está fuera)
sqlite3 $W/app/data/localify.db "select id,title from tracks limit 5" 2>&1 | head -5; inv player_play_track '{"trackId":"dQw4w9WgXcQ"}' | head -c 300; echo
for i in $(seq 25); do curl -s -X POST -d '{}' -H 'content-type: application/json' localhost:8795/api/events/poll >/dev/null; [ -n "$(find $W/home -name 'dQw4w9WgXcQ.*' -path '*audio*' 2>/dev/null)" ] && break; sleep 0.5; done
F=$(find $W/home -name 'dQw4w9WgXcQ.*' -path '*audio*' 2>/dev/null | head -1); ok "audio descargado del servidor" "$F" dQw4w9WgXcQ
[ -n "$F" ] && cmp -s "$F" $W/a.opus && ok "contenido idéntico" 1 1 || ok "contenido idéntico (remux/ext puede variar)" "$(stat -c%s "$F" 2>/dev/null)" 40000
# playlist: publicar, ver, importar con otro usuario
PL=$(inv playlist_create '{"name":"Mi mix"}' | tr -d '"')
inv playlist_add_tracks "{\"playlistId\":\"$PL\",\"trackIds\":[\"dQw4w9WgXcQ\"]}" >/dev/null
ok "publicar" "$(inv social_publish "{\"playlistId\":\"$PL\",\"visibility\":\"public\",\"collaborative\":true}")" '"serverId":"1"'
ok "listada" "$(inv social_playlists)" '"name":"Mi mix"'
ok "detalle disponible" "$(inv social_playlist '{"id":"1"}')" '"available":true'
ok "perfil público" "$(inv social_user '{"username":"ana"}')" '"name":"Mi mix"'
ok "miembro" "$(inv social_add_member '{"id":"1","username":"bob"}'; echo fin)" fin
ok "miembro inexistente" "$(inv social_add_member '{"id":"1","username":"nadie"}')" USER_NOT_FOUND
# bob añade pista en servidor; ana sincroniza y la recibe
curl -s -X POST -H "Authorization: Bearer $TB" $B/playlists/1/accept -o /dev/null
curl -s -X POST -H "Authorization: Bearer $TB" -H 'content-type: application/json' -d '{"tracks":[{"id":"bbbbbbbbbbb","title":"De Bob","artist":"Bob","durationMs":1000}]}' $B/playlists/1/tracks -o /dev/null
inv social_sync "{\"playlistId\":\"$PL\"}" >/dev/null
ok "sync trae pista de bob" "$(inv playlist_detail "{\"playlistId\":\"$PL\"}")" 'De Bob'
ok "importar playlist existente enlaza" "$(inv social_import '{"id":"1"}')" "\"localId\":\"$PL\""

# ── duplicados locales ──
DB=$W/app/data/localify.db
mkdir -p $W/home/Music/Localify/audio/zz
head -c 30000 /dev/urandom > $W/home/Music/Localify/audio/zz/A1.opus; head -c 30000 /dev/urandom > $W/home/Music/Localify/audio/zz/B1.opus
sqlite3 $DB "
insert into tracks(id,title,title_norm,duration_ms,artist_display,artist_norm) values
 ('A1','Bloody Power Fame','bloody power fame',200000,'Coldrain','coldrain'),
 ('B1','Bloody Power Fame','bloody power fame',201500,'Coldrain','coldrain'),
 ('C1','Bloody Power Fame (Live)','bloody power fame live',200000,'Coldrain','coldrain'),
 ('D1','Bloody Power Fame','bloody power fame',260000,'Coldrain','coldrain');
insert into audio_files(track_id,rel_path,format,source) values ('A1','audio/zz/A1.opus','opus','youtube'),('B1','audio/zz/B1.opus','opus','youtube');
insert into favorites(track_id) values ('B1');
insert into playlists(id,name,name_norm,position) values ('PLX','x','x',1);
insert into playlist_items(id,playlist_id,track_id,position) values ('i1','PLX','A1',1),('i2','PLX','B1',2),('i3','PLX','C1',3);"
ok "fusiona" "$(inv library_merge_duplicates)" '"merged":'
ok "queda una" "$(sqlite3 $DB "select count(*) from tracks where title_norm='bloody power fame' and duration_ms < 210000 and id in ('A1','B1')")" 1
ok "la lista no repite la canción" "$(sqlite3 $DB "select count(*) from playlist_items where playlist_id='PLX' and track_id in ('A1','B1')")" 1
ok "favorito heredado" "$(sqlite3 $DB "select count(*) from favorites where track_id in ('A1','B1')")" 1
ok "un solo fichero de audio" "$(ls $W/home/Music/Localify/audio/zz | wc -l)" 1
ok "sigue habiendo la live y la otra duración" "$(sqlite3 $DB "select count(*) from tracks where id in ('C1','D1')")" 2
ok "sin duplicados después" "$(inv library_merge_duplicates)" '"merged":0'
# ── identidad contra el servidor ──
TA=$(curl -s -X POST -H 'content-type: application/json' -d '{"username":"ana","password":"secreto123"}' $B/login | sed 's/.*"token":"\([^"]*\)".*/\1/')
inv server_login '{"username":"ana","password":"secreto123"}' >/dev/null
K=$(sqlite3 $DB "select id from tracks where id in ('A1','B1')")
ok "aportar biblioteca: sube lo que falta" "$(inv server_sync_library '{"offset":0}')" '"uploaded":'
ok "servidor: ahora la tiene" "$(curl -s -H "Authorization: Bearer $TB" "$B/tracks/search?q=bloody")" 'Bloody Power Fame'
ok "aportar otra vez: ya está (por identidad)" "$(inv server_sync_library '{"offset":0}')" '"uploaded":0'
# misma canción con otro id en el servidor: no se duplica en local al buscar ni importar
curl -s -X PUT -H "Authorization: Bearer $TB" -F audio=@$W/a.opus -F ext=opus -F title="Faint" -F artist="Linkin Park" -F duration_ms=162000 -F search_norm="faint linkin park" -F title_norm=faint -F artist_norm="linkin park" $B/tracks/FAINTSRV0001 >/dev/null
sqlite3 $DB "insert into tracks(id,title,title_norm,duration_ms,artist_display,artist_norm) values ('FAINTLOC001','Faint','faint',162500,'Linkin Park','linkin park')"
N0=$(sqlite3 $DB "select count(*) from tracks")
inv search_query '{"q":"faint linkin"}' >/dev/null
ok "búsqueda no crea una copia de la que ya tengo" "$(sqlite3 $DB "select count(*) from tracks where title_norm='faint'")" 1
curl -s -X POST -H "Authorization: Bearer $TB" -H 'content-type: application/json' -d '{"name":"De Bob","visibility":"public","tracks":[{"id":"FAINTSRV0001","title":"Faint","artist":"Linkin Park","durationMs":162000,"titleNorm":"faint","artistNorm":"linkin park"}]}' $B/playlists -o /dev/null
SPL=$(curl -s -H "Authorization: Bearer $TB" $B/playlists | sed 's/.*"id":\([0-9]*\),"name":"De Bob".*/\1/')
R=$(inv social_import "{\"id\":\"$SPL\"}")
LP=$(echo "$R" | sed 's/.*"localId":"\([^"]*\)".*/\1/')
ok "importar reutiliza mi pista" "$(sqlite3 $DB "select track_id from playlist_items where playlist_id='$LP'")" FAINTLOC001
ok "importar no crea duplicados" "$(sqlite3 $DB "select count(*) from tracks where title_norm='faint'")" 1

# ── la nube: playlists automáticas, segundo equipo, y nada de descargar lo que ya hay ──
poll() { curl -s -X POST -H 'content-type: application/json' -d '{}' localhost:${1:-8795}/api/events/poll >/dev/null; }
TA=$(curl -s -X POST -H 'content-type: application/json' -d '{"username":"ana","password":"secreto123"}' $B/login | sed 's/.*"token":"\([^"]*\)".*/\1/')
PA=$(inv playlist_create '{"name":"Auto"}' | tr -d '"')
inv playlist_add_tracks "{\"playlistId\":\"$PA\",\"trackIds\":[\"dQw4w9WgXcQ\"]}" >/dev/null
poll
ok "una playlist nueva sube sola (privada)" "$(curl -s -H "Authorization: Bearer $TA" $B/playlists)" '"name":"Auto"'
SID=$(curl -s -H "Authorization: Bearer $TA" $B/playlists | sed 's/.*"id":\([0-9]*\),"name":"Auto".*/\1/')
ok "con sus canciones" "$(curl -s -H "Authorization: Bearer $TA" $B/playlists/$SID)" '"title":"Yellow"'
ok "y es privada" "$(curl -s -H "Authorization: Bearer $TA" $B/playlists/$SID)" '"visibility":"private"'
inv playlist_rename "{\"playlistId\":\"$PA\",\"name\":\"Auto 2\"}" >/dev/null; poll
ok "renombrar se propaga" "$(curl -s -H "Authorization: Bearer $TA" $B/playlists/$SID)" '"name":"Auto 2"'
EID=$(sqlite3 $DB "select id from playlist_items where playlist_id='$PA'")
# segundo equipo: otro HOME, otra base de datos
mkdir -p $W/home2/Music/Localify/audio/zz; cp -r $REPO/app $W/app2; rm -rf $W/app2/data
(cd $W/app2 && HOME=$W/home2 exec $LUX . --port 8796 --no-watch >$W/cl2.log 2>&1) & P3=$!
for i in $(seq 40); do curl -s -f -X POST -d '{}' -H 'content-type: application/json' localhost:8796/api/events/poll -o /dev/null && break; sleep 0.3; done
inv2() { curl -s -X POST -H 'content-type: application/json' -d "$(python3 -c 'import json,sys;print(json.dumps({"cmd":sys.argv[1],"args":sys.argv[2]}))' "$1" "${2:-{\}}")" localhost:8796/api/invoke; }
DB2=$W/app2/data/localify.db
head -c 30000 /dev/urandom > $W/home2/Music/Localify/audio/zz/LOC1.opus
# aquí ya tengo "Bloody Power Fame" descargada con OTRO id (y duración parecida)
sqlite3 $DB2 "insert into tracks(id,title,title_norm,duration_ms,artist_display,artist_norm) values('LOC1','Bloody Power Fame','bloody power fame',200400,'Coldrain','coldrain');
insert into audio_files(track_id,rel_path,format,source) values('LOC1','audio/zz/LOC1.opus','opus','youtube')"
inv2 server_connect '{"host":"127.0.0.1","port":8794}' >/dev/null
ok "equipo 2: entra con el mismo usuario" "$(inv2 server_login '{"username":"ana","password":"secreto123"}')" '"username":"ana"'
PULL=$(inv2 server_pull)
ok "tira de la nube" "$PULL" '"total":'
ok "cuenta lo que ya tenía" "$PULL" '"alreadyHad":1'
ok "NO encola lo que ya tengo (Bloody)" "$(sqlite3 $DB2 "select count(*) from download_jobs j join tracks t on t.id=j.track_id where t.title_norm='bloody power fame'")" 0
ok "NO crea otra copia de lo que ya tengo" "$(sqlite3 $DB2 "select count(*) from tracks where title_norm='bloody power fame'")" 1
ok "sí encola lo que falta (Yellow)" "$(sqlite3 $DB2 "select count(*) from download_jobs j join tracks t on t.id=j.track_id where t.title_norm='yellow'")" 1
ok "el favorito llega a mi pista" "$(sqlite3 $DB2 "select count(*) from favorites where track_id='LOC1'")" 1
ok "trae mis playlists" "$(sqlite3 $DB2 "select count(*) from playlists where name='Auto 2'")" 1
ok "con sus canciones" "$(sqlite3 $DB2 "select count(*) from playlist_items pi join playlists p on p.id=pi.playlist_id where p.name='Auto 2'")" 1
# repetir la descarga de la nube no cambia nada
inv2 server_pull >/dev/null
ok "repetir no duplica" "$(sqlite3 $DB2 "select count(*) from tracks where title_norm='yellow'")" 1
# quitar una canción en el equipo 1 -> llega al 2
inv playlist_remove_entries "{\"playlistId\":\"$PA\",\"entryIds\":[\"$EID\"]}" >/dev/null; poll
ok "quitar canción se propaga al servidor" "$(curl -s -H "Authorization: Bearer $TA" $B/playlists/$SID | grep -o '"tracks":\[[^]]*\]')" '"tracks":[]'
poll 8796; sleep 0.5; poll 8796
ok "y llega al equipo 2" "$(sqlite3 $DB2 "select count(*) from playlist_items pi join playlists p on p.id=pi.playlist_id where p.name='Auto 2'")" 0
# borrar la playlist en el equipo 1 -> desaparece en el 2
inv playlist_delete "{\"playlistId\":\"$PA\"}" >/dev/null; poll
ok "borrar se propaga al servidor" "$(curl -s -H "Authorization: Bearer $TA" $B/playlists)" '"name":"Mi mix"'
[[ "$(curl -s -H "Authorization: Bearer $TA" $B/playlists)" != *'"name":"Auto 2"'* ]] && ok "ya no está en el servidor" 1 1 || ok "ya no está en el servidor" 0 1
state2=$(curl -s -H "Authorization: Bearer $TA" $B/playlist-tombstones); ok "queda constancia del borrado" "$state2" "$SID"
inv2 server_pull >/dev/null
ok "y se borra en el equipo 2 (no se vuelve a crear)" "$(sqlite3 $DB2 "select count(*) from playlists where name='Auto 2'")" 0
ok "ni resucita en el servidor" "$(curl -s -H "Authorization: Bearer $TA" $B/playlists | grep -c '"name":"Auto 2"')" 0

# ── playlist conjunta: la invitada la recibe sola, puede editarla, pero no es suya ──
CJ=$(inv playlist_create '{"name":"Conjunta"}' | tr -d '"')
inv playlist_add_tracks "{\"playlistId\":\"$CJ\",\"trackIds\":[\"dQw4w9WgXcQ\"]}" >/dev/null
CSID=$(inv social_ensure "{\"playlistId\":\"$CJ\"}" | sed 's/.*"id":\([0-9]*\),"name".*/\1/')
ok "invitar a bob" "$(inv social_add_member "{\"id\":\"$CSID\",\"username\":\"bob\"}"; echo fin)" fin
# el equipo 2 pasa a ser de bob (otro usuario: no arrastra los enlaces de ana)
inv2 server_logout >/dev/null
BOBLOGIN=$(inv2 server_login '{"username":"bob","password":"secreto123"}'); ok "bob entra en el equipo 2" "$BOBLOGIN" '"username":"bob"'
inv2 server_pull >/dev/null
ok "bob ve la invitación" "$(inv2 social_invitations)" '"name":"Conjunta"'
ok "sin aceptar, la conjunta no entra" "$(sqlite3 $DB2 "select count(*) from playlists where name='Conjunta'")" 0
CJ2=$(inv2 social_invitations | sed 's/.*"id":\([0-9]*\),"name":"Conjunta".*/\1/')
ok "bob rechaza una prueba: no entra" "$(inv2 social_decline "{\"id\":\"$CJ2\"}" >/dev/null; sqlite3 $DB2 "select count(*) from playlists where name='Conjunta'")" 0
ok "y la invitación a la conjunta desaparece" "$(inv2 social_invitations | grep -c Conjunta)" 0
ok "la vuelvo a invitar a bob" "$(inv social_add_member "{\"id\":\"$CSID\",\"username\":\"bob\"}" >/dev/null; inv2 server_pull >/dev/null; inv2 social_invitations)" '"name":"Conjunta"'
ok "bob acepta" "$(inv2 social_accept "{\"id\":\"$CSID\"}" | head -c 40)" '"localId"'
ok "al aceptar entra en su biblioteca" "$(sqlite3 $DB2 "select count(*) from playlists where name='Conjunta'")" 1
ok "con la canción de ana" "$(sqlite3 $DB2 "select count(*) from playlist_items pi join playlists p on p.id=pi.playlist_id where p.name='Conjunta'")" 1
ok "y no se llevó las playlists de ana por enlaces viejos" "$(curl -s -H "Authorization: Bearer $TB" $B/playlists | grep -c '"name":"Auto 2"')" 0
BP=$(sqlite3 $DB2 "select id from playlists where name='Conjunta'")
sqlite3 $DB2 "insert or ignore into tracks(id,title,title_norm,duration_ms,artist_display,artist_norm) values('BOBTRK1','De Bob','de bob',100000,'Bob','bob')"
inv2 playlist_add_tracks "{\"playlistId\":\"$BP\",\"trackIds\":[\"BOBTRK1\"]}" >/dev/null
poll 8796
ok "bob añade una canción y ana la ve en el servidor" "$(curl -s -H "Authorization: Bearer $TA" $B/playlists/$CSID)" '"title":"De Bob"'
inv2 playlist_rename "{\"playlistId\":\"$BP\",\"name\":\"Renombrada por bob\"}" >/dev/null; poll 8796
ok "bob no puede renombrarla en el servidor" "$(curl -s -H "Authorization: Bearer $TA" $B/playlists/$CSID)" '"name":"Conjunta"'
ok "servidor caído no rompe la búsqueda" "$(kill $P1; sleep 0.5; inv search_query '{"q":"yellow"}')" '"queryId"'
((fail)) && { tail -15 $W/cl.log | grep -v warning; echo "--- equipo 2:"; grep -v warning $W/cl2.log | tail -8; exit 1; }; echo "todo ok"

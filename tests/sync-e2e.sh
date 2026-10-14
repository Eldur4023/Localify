#!/usr/bin/env bash
# Reproducción sincronizada: dos clientes (A y B) del mismo usuario contra un servidor.
#   tests/sync-e2e.sh [ruta-al-binario-lux]
# Dos clientes (A y B) contra un servidor: A reproduce, B debe seguirlo.
REPO=$(cd "$(dirname "$0")/.." && pwd); LUX=$(realpath "${1:-$REPO/build/vendor/lux/lux}")
W=$(mktemp -d); trap 'kill $P1 $P2 $P3 2>/dev/null; rm -rf "$W"' EXIT
mkdir -p $W/srv $W/ha $W/hb; cp $REPO/server/server.lux $W/srv/
(cd $W/srv && JWT_SECRET=e2e-secret-0123456789 exec $LUX server.lux --port 8794 >$W/srv.log 2>&1) & P1=$!
for X in a b; do cp -r $REPO/app $W/app$X; rm -rf $W/app$X/data; done
(cd $W/appa && HOME=$W/ha exec $LUX . --port 8795 --no-watch >$W/a.log 2>&1) & P2=$!
(cd $W/appb && HOME=$W/hb exec $LUX . --port 8796 --no-watch >$W/b.log 2>&1) & P3=$!
for i in $(seq 80); do curl -sf localhost:8794/api/info >/dev/null && curl -sf -X POST -H 'content-type: application/json' -d '{}' localhost:8795/api/events/poll -o /dev/null && curl -sf -X POST -H 'content-type: application/json' -d '{}' localhost:8796/api/events/poll -o /dev/null && break; sleep 0.3; done
inv() { curl -s -X POST -H 'content-type: application/json' -d "$(python3 -c 'import json,sys;print(json.dumps({"cmd":sys.argv[2],"args":sys.argv[3]}))' x "$2" "${3:-{\}}")" localhost:$1/api/invoke; }
fail=0; ok() { if [[ "$2" == *"$3"* ]]; then echo "  ok    $1"; else echo "  FAIL  $1: got [${2:0:400}] want [$3]"; fail=1; fi; }
B=localhost:8794/api
for P in 8795 8796; do inv $P server_connect '{"host":"127.0.0.1","port":8794}' >/dev/null; done
inv 8795 server_register '{"username":"ana","password":"secreto123","displayName":"Ana"}' >/dev/null
inv 8796 server_login '{"username":"ana","password":"secreto123"}' >/dev/null
# pista sembrada en el servidor (audio falso) y en A
TA=$(curl -s -X POST -H 'content-type: application/json' -d '{"username":"ana","password":"secreto123"}' $B/login | sed 's/.*"token":"\([^"]*\)".*/\1/')
head -c 40000 /dev/urandom > $W/a.opus
q() { python3 -c 'import sqlite3,sys;c=sqlite3.connect(sys.argv[1]);c.execute("pragma busy_timeout=5000");r=c.execute(sys.argv[2]).fetchall();c.commit();print(r)' "$@"; }
q $W/appa/data/localify.db "insert into tracks(id,title,title_norm,duration_ms,artist_display,artist_norm) values ('TRK1','Yellow','yellow',266000,'Coldplay','coldplay'),('TRK2','Clocks','clocks',250000,'Coldplay','coldplay')" >/dev/null
ok "estado A" "$(inv 8795 social_sync_status)" '"enabled":false'
ok "activar A" "x$(inv 8795 social_sync_set '{"enabled":true}')" x
ok "activar B" "x$(inv 8796 social_sync_set '{"enabled":true}')" x
sleep 2.5
inv 8795 player_play_track '{"trackId":"TRK1","context":{"kind":"search","query":"","trackIds":["TRK1","TRK2"]}}' >/dev/null
sleep 3
ok "B sigue la pista" "$(inv 8796 player_get_state)" '"title":"Yellow"'
ok "B suena" "$(inv 8796 player_get_state)" '"status":"playing"'
echo "-- B pausa --"
inv 8796 player_pause >/dev/null; sleep 2.5
ok "A pausado por B" "$(inv 8795 player_get_state)" '"status":"paused"'
echo "-- A siguiente --"
inv 8795 player_next >/dev/null; sleep 2.5
ok "B en Clocks" "$(inv 8796 player_get_state)" '"title":"Clocks"'
echo "-- A seek 100s --"
inv 8795 player_seek '{"positionMs":100000}' >/dev/null; sleep 2.5
ok "B posición ~100s" "$(inv 8796 player_get_state | python3 -c 'import json,sys;p=json.load(sys.stdin)["positionMs"];print("POS_OK" if 99000<=p<=106000 else "POS_BAD %d"%p)')" POS_OK
echo "-- fin de canción en ambos a la vez (A tiene cola, B no) --"
inv 8795 player_play_track '{"trackId":"TRK1","context":{"kind":"search","query":"","trackIds":["TRK1","TRK2"]}}' >/dev/null; sleep 2.5
CA=$(inv 8795 player_get_state | python3 -c 'import json,sys;print(json.load(sys.stdin)["cue"])'); CB=$(inv 8796 player_get_state | python3 -c 'import json,sys;print(json.load(sys.stdin)["cue"])')
inv 8796 player_ended "{\"cue\":$CB}" >/dev/null; inv 8795 player_ended "{\"cue\":$CA}" >/dev/null; sleep 2.5
ok "A avanzó a Clocks" "$(inv 8795 player_get_state)" '"title":"Clocks"'
ok "B sigue a A (Clocks, sonando)" "$(inv 8796 player_get_state)" '"title":"Clocks"'
echo "-- modo solo aquí en B --"
inv 8796 social_sync_set '{"mode":"here"}' >/dev/null
inv 8795 player_play_track '{"trackId":"TRK1"}' >/dev/null; sleep 2.5
ok "B muestra Yellow" "$(inv 8796 player_get_state)" '"title":"Yellow"'
ok "B en pausa (solo aquí)" "$(inv 8796 player_get_state)" '"status":"paused"'
ok "A sigue sonando" "$(inv 8795 player_get_state)" '"status":"playing"'
echo "-- desactivar B y A cambia --"
inv 8796 social_sync_set '{"enabled":false}' >/dev/null
inv 8795 player_previous >/dev/null; sleep 2.5
ok "B ya no sigue" "$(inv 8796 player_get_state)" '"title":"Yellow"'
ok "equipos listados" "$(inv 8795 social_sync_status)" '"devices":[{'
echo "-- logs de error --"; grep -i "error\|panic\|fail" $W/a.log $W/b.log | grep -v "lux: warning" | head
[ $fail = 0 ] && echo TODO-OK || echo HAY-FALLOS

#!/usr/bin/env bash
# Servidor solo (sin cliente): permisos de playlists, tope de /playback y reintento de /relay.
#   tests/server-permisos.sh [ruta-al-binario-lux]     (necesita ffmpeg para la parte de relay)
REPO=$(cd "$(dirname "$0")/.." && pwd); LUX=$(realpath "${1:-$REPO/build/vendor/lux/lux}")
W=$(mktemp -d); trap 'kill $P 2>/dev/null; rm -rf "$W"' EXIT; cp $REPO/server/server.lux $W/; cd $W
(JWT_SECRET=e2e-secret-0123456789 exec $LUX server.lux --port 8797 >$W/log 2>&1) & P=$!
for i in $(seq 40); do curl -sf localhost:8797/api/info >/dev/null && break; sleep 0.3; done
B=localhost:8797/api; J='content-type: application/json'; fail=0
chk() { if [ "$2" = "$3" ]; then echo "  ok    $1"; else echo "  FAIL  $1: got $2 want $3"; fail=1; fi; }
tok() { curl -s -X POST -H "$J" -d "{\"username\":\"$1\",\"password\":\"secreto123\"}" $B/register | sed 's/.*"token":"\([^"]*\)".*/\1/'; }
code() { curl -s -o /dev/null -w "%{http_code}" "$@"; }
TA=$(tok ana); TB=$(tok bob)
PID=$(curl -s -X POST -H "Authorization: Bearer $TA" -H "$J" -d '{"name":"P","visibility":"public"}' $B/playlists | sed 's/.*"id":\([0-9]*\).*/\1/')
TR='{"tracks":[{"id":"aaaaaaaaaaa","title":"T","artist":"A","titleNorm":"t","artistNorm":"a","durationMs":1000}]}'
chk "lector no añade pistas" "$(code -X POST -H "Authorization: Bearer $TB" -H "$J" -d "$TR" $B/playlists/$PID/tracks)" 403
chk "lector no reescribe la lista" "$(code -X PUT -H "Authorization: Bearer $TB" -H "$J" -d "$TR" $B/playlists/$PID)" 403
chk "lector no borra pistas" "$(code -X DELETE -H "Authorization: Bearer $TB" $B/playlists/$PID/tracks/aaaaaaaaaaa)" 403
curl -s -o /dev/null -X POST -H "Authorization: Bearer $TA" -H "$J" -d '{"username":"bob"}' $B/playlists/$PID/members
curl -s -o /dev/null -X POST -H "Authorization: Bearer $TB" $B/playlists/$PID/accept
chk "miembro añade pistas" "$(code -X POST -H "Authorization: Bearer $TB" -H "$J" -d "$TR" $B/playlists/$PID/tracks)" 204
chk "miembro reescribe pistas" "$(code -X PUT -H "Authorization: Bearer $TB" -H "$J" -d '{"tracks":[]}' $B/playlists/$PID)" 204
chk "el miembro no cambia la visibilidad" "$(curl -s -H "Authorization: Bearer $TA" $B/playlists/$PID | grep -o '"visibility":"[a-z]*"')" '"visibility":"public"'
BIG=$(python3 -c 'import json;print(json.dumps({"deviceId":"d1","trackKey":"k","state":{"x":"y"*30000}}))')
chk "/playback tiene tope de tamaño" "$(echo "$BIG" | curl -s -o /dev/null -w "%{http_code}" -X PUT -H "Authorization: Bearer $TA" -H "$J" -d @- $B/playback)" 413
if command -v ffmpeg >/dev/null; then
  head -c 40000 /dev/urandom > basura.bin; ffmpeg -v error -f lavfi -i "sine=frequency=440:duration=3" -c:a libopus bueno.opus -y
  chk "relay: audio corrupto falla" "$(curl -s -o /dev/null -w '%{http_code}' -X PUT -H "Authorization: Bearer $TA" -F audio=@basura.bin -F title=C -F artist=A -F title_norm=c -F artist_norm=a $B/relay/RELAYTEST01)" 502
  chk "relay: el reintento usa el audio nuevo" "$(curl -s -o /dev/null -w '%{http_code}' -X PUT -H "Authorization: Bearer $TA" -F audio=@bueno.opus -F title=C -F artist=A -F title_norm=c -F artist_norm=a $B/relay/RELAYTEST01)" 201
fi
[ $fail = 0 ] && echo TODO-OK || echo HAY-FALLOS; exit $fail

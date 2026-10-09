#!/usr/bin/env bash
# E2E del servidor: arranca uno limpio en un directorio temporal y ejercita la API.
#   server/test.sh [ruta-al-binario-lux]
cd "$(dirname "$0")"
LUX=${1:-../vendor/lux/build/lux}; PORT=${PORT:-8792}; B=http://127.0.0.1:$PORT/api
W=$(mktemp -d); cp server.lux "$W/"; trap 'kill $PID 2>/dev/null; rm -rf "$W"' EXIT
LUXBIN=$(realpath "$LUX"); (cd "$W" && JWT_SECRET=test-secret-0123456789 exec "$LUXBIN" server.lux --port $PORT >"$W/log" 2>&1) & PID=$!
for i in $(seq 50); do curl -sf $B/info >/dev/null && break; sleep 0.2; done
fail=0; ok() { if [[ "$2" == *"$3"* ]]; then echo "  ok    $1"; else echo "  FAIL  $1: got [$2] want [$3]"; fail=1; fi; }
j() { curl -s -X "$1" -H 'Content-Type: application/json' ${T:+-H "Authorization: Bearer $T"} "${@:3}" -d "$2" -w ' %{http_code}'; }
ok info "$(curl -s $B/info)" Serverify
ok "sin token" "$(curl -s -o /dev/null -w %{http_code} $B/me)" 401
ok "pass corta" "$(j POST '{"username":"ana","password":"x"}' $B/register)" 400
R=$(j POST '{"username":"ana","password":"secreto123","displayName":"Ana"}' $B/register); ok registro "$R" '201'
TA=$(sed 's/.*"token":"\([^"]*\)".*/\1/' <<<"$R")
ok "usuario repetido" "$(j POST '{"username":"ANA","password":"secreto123"}' $B/register)" 409
ok "login mal" "$(j POST '{"username":"ana","password":"nope"}' $B/login)" 401
TB=$(sed 's/.*"token":"\([^"]*\)".*/\1/' <<<"$(j POST '{"username":"bob","password":"secreto123"}' $B/register)")
T=$TA; ok me "$(curl -s -H "Authorization: Bearer $T" $B/me)" '"username":"ana"'
ok "editar perfil" "$(j PUT '{"displayName":"Ana G","bio":"hola"}' $B/me)" 'Ana G'
head -c 20000 /dev/urandom > "$W/a.opus"
up() { curl -s -X PUT -H "Authorization: Bearer $T" -F audio=@"$W/a.opus" -F ext=opus -F title="Yellow" -F artist=Coldplay -F album=Parachutes -F duration_ms=266000 -F search_norm="yellow coldplay parachutes" -w ' %{http_code}' $B/tracks/dQw4w9WgXcQ; }
ok "subida" "$(up)" '201'
ok "subida repetida" "$(up)" '"stored":false'
ok "id inválido" "$(curl -s -o /dev/null -w %{http_code} -X PUT -H "Authorization: Bearer $T" -F audio=@"$W/a.opus" -F title=x $B/tracks/..%2Fx)" 4
ok "búsqueda" "$(curl -s -H "Authorization: Bearer $T" "$B/tracks/search?q=coldplay%20yell")" dQw4w9WgXcQ
ok "búsqueda vacía" "$(curl -s -H "Authorization: Bearer $T" "$B/tracks/search?q=zzz")" '[]'
ok "have" "$(j POST '{"ids":["dQw4w9WgXcQ","nope"]}' $B/tracks/have)" '["dQw4w9WgXcQ"]'
curl -s -H "Authorization: Bearer $T" $B/tracks/dQw4w9WgXcQ/audio | cmp -s - "$W/a.opus" && ok "descarga idéntica" 1 1 || ok "descarga idéntica" 0 1
# playlists
P='{"name":"Mix","visibility":"public","collaborative":true,"tracks":[{"id":"dQw4w9WgXcQ","title":"Yellow","artist":"Coldplay"},{"id":"zzzzzzzzzzz","title":"Otra"}]}'
ok "crear playlist" "$(j POST "$P" $B/playlists)" '"id":1'
D=$(curl -s -H "Authorization: Bearer $T" $B/playlists/1); ok "detalle" "$D" '"available":true'; ok "no disponible" "$D" '"available":false'
T=$TB; ok "pública legible" "$(curl -s -H "Authorization: Bearer $T" $B/playlists/1)" '"access":"reader"'
ok "lector no edita" "$(j POST '{"tracks":[{"id":"x","title":"t"}]}' $B/playlists/1/tracks)" 403
T=$TA; ok "añadir miembro" "$(j POST '{"username":"bob"}' $B/playlists/1/members)" 204
T=$TB; ok "miembro ve la suya" "$(curl -s -H "Authorization: Bearer $T" $B/playlists)" '"name":"Mix"'
ok "miembro añade" "$(j POST '{"tracks":[{"id":"bbbbbbbbbbb","title":"De Bob"}]}' $B/playlists/1/tracks)" 204
ok "miembro no borra" "$(j DELETE '' $B/playlists/1)" 404
ok "perfil con playlists" "$(curl -s -H "Authorization: Bearer $T" $B/users/ana)" '"name":"Mix"'
ok "social" "$(curl -s -H "Authorization: Bearer $T" $B/users)" '"username":"bob"'
ok "salirse" "$(j DELETE '' $B/playlists/1/members/bob)" 204
ok "ya no edita" "$(j POST '{"tracks":[{"id":"x","title":"t"}]}' $B/playlists/1/tracks)" 403
T=$TA; ok "dueño borra" "$(j DELETE '' $B/playlists/1)" 204
((fail)) && { echo; tail -5 "$W/log"; exit 1; }; echo "todo ok"

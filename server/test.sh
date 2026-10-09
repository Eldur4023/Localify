#!/usr/bin/env bash
# E2E del servidor: arranca uno limpio en un directorio temporal y ejercita la API.
#   server/test.sh [ruta-al-binario-lux]
cd "$(dirname "$0")"
LUX=${1:-../build/vendor/lux/lux}; PORT=${PORT:-8792}; B=http://127.0.0.1:$PORT/api
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
up() { curl -s -X PUT -H "Authorization: Bearer $T" -F audio=@"$W/a.opus" -F ext=opus -F title="Yellow" -F artist=Coldplay -F album=Parachutes -F duration_ms=266000 -F search_norm="yellow coldplay parachutes" -F title_norm=yellow -F artist_norm=coldplay -w ' %{http_code}' $B/tracks/dQw4w9WgXcQ; }
ok "subida" "$(up)" '201'
ok "subida repetida" "$(up)" '"stored":false'
# duplicados: misma canción (título + artista + duración ±3 s) con otro id no se guarda dos veces
dup() { curl -s -X PUT -H "Authorization: Bearer $T" -F audio=@"$W/a.opus" -F ext=opus -F title="Yellow" -F artist=Coldplay -F duration_ms=$1 -F search_norm="yellow coldplay" -F title_norm=yellow -F artist_norm=coldplay $B/tracks/otroIdDistinto; }
ok "duplicado por identidad" "$(dup 267500)" '"duplicate":true'
ok "otra versión (duración distinta) sí" "$(dup 200000)" '"stored":true'
ok "match por identidad" "$(j POST '{"items":[{"id":"x1","titleNorm":"yellow","artistNorm":"coldplay","durationMs":266000},{"id":"x2","titleNorm":"nada","artistNorm":"nadie","durationMs":1}]}' $B/tracks/match)" '"id":"x1","serverId":"dQw4w9WgXcQ"'
ok "match sin coincidencia" "$(j POST '{"items":[{"id":"x2","titleNorm":"nada","artistNorm":"nadie","durationMs":1}]}' $B/tracks/match)" '"serverId":""'
ok "id inválido" "$(curl -s -o /dev/null -w %{http_code} -X PUT -H "Authorization: Bearer $T" -F audio=@"$W/a.opus" -F title=x $B/tracks/..%2Fx)" 4
ok "búsqueda sin repetidas" "$(curl -s -H "Authorization: Bearer $T" "$B/tracks/search?q=yellow" | grep -o '"title":"Yellow"' | wc -l)" 2
ok "búsqueda" "$(curl -s -H "Authorization: Bearer $T" "$B/tracks/search?q=coldplay%20yell")" dQw4w9WgXcQ
ok "búsqueda vacía" "$(curl -s -H "Authorization: Bearer $T" "$B/tracks/search?q=zzz")" '[]'
ok "have" "$(j POST '{"ids":["dQw4w9WgXcQ","nope"]}' $B/tracks/have)" '["dQw4w9WgXcQ"]'
curl -s -H "Authorization: Bearer $T" $B/tracks/dQw4w9WgXcQ/audio | cmp -s - "$W/a.opus" && ok "descarga idéntica" 1 1 || ok "descarga idéntica" 0 1
# playlists
P='{"name":"Mix","visibility":"public","collaborative":true,"tracks":[{"id":"dQw4w9WgXcQ","title":"Yellow","artist":"Coldplay"},{"id":"OTRO","title":"Yellow","artist":"Coldplay","titleNorm":"yellow","artistNorm":"coldplay","durationMs":266100},{"id":"zzzzzzzzzzz","title":"Otra"}]}'
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
# ── permisos: nadie toca mi playlist salvo a quien yo invite ──
T=$TA; PV=$(j POST '{"name":"Privada","tracks":[{"id":"zzzzzzzzzzz","title":"Secreta"}]}' $B/playlists | sed 's/.*"id":\([0-9]*\).*/\1/')
PP=$(j POST '{"name":"Publica","visibility":"public","tracks":[{"id":"zzzzzzzzzzz","title":"Abierta"}]}' $B/playlists | sed 's/.*"id":\([0-9]*\).*/\1/')
T=$TB
ok "privada: otro usuario no la ve" "$(curl -s -o /dev/null -w %{http_code} -H "Authorization: Bearer $T" $B/playlists/$PV)" 404
ok "privada: no aparece en su lista" "$(curl -s -H "Authorization: Bearer $T" $B/playlists | grep -c Privada)" 0
ok "privada: no puede añadir" "$(j POST '{"tracks":[{"id":"x","title":"t"}]}' $B/playlists/$PV/tracks)" 403
ok "privada: no puede borrarla" "$(j DELETE '' $B/playlists/$PV)" 404
ok "privada: no puede invitarse" "$(j POST '{"username":"bob"}' $B/playlists/$PV/members)" 404
ok "pública: la ve pero solo lectura" "$(curl -s -H "Authorization: Bearer $T" $B/playlists/$PP)" '"access":"reader"'
ok "pública: no puede añadir" "$(j POST '{"tracks":[{"id":"x","title":"t"}]}' $B/playlists/$PP/tracks)" 403
ok "pública: no puede quitar" "$(j DELETE '' $B/playlists/$PP/tracks/zzzzzzzzzzz)" 403
ok "pública: no puede reescribirla" "$(j PUT '{"name":"Hackeada","tracks":[]}' $B/playlists/$PP)" 403
ok "pública: ni cambiarle el nombre" "$(j PUT '{"name":"Hackeada"}' $B/playlists/$PP)" 204
T=$TA; ok "pública: sigue intacta" "$(curl -s -H "Authorization: Bearer $T" $B/playlists/$PP)" '"name":"Publica"'
ok "pública: sin conjunta todavía" "$(curl -s -H "Authorization: Bearer $T" $B/playlists/$PP)" '"collaborative":false'
# hacerla conjunta = invitar a alguien (sin casillas)
ok "invitar a bob" "$(j POST '{"username":"bob"}' $B/playlists/$PV/members)" 204
ok "y ya es conjunta" "$(curl -s -H "Authorization: Bearer $T" $B/playlists/$PV)" '"collaborative":true'
ok "no puede invitarse a sí misma" "$(j POST '{"username":"ana"}' $B/playlists/$PV/members)" 400
T=$TB
ok "invitado: la ve" "$(curl -s -H "Authorization: Bearer $T" $B/playlists/$PV)" '"access":"member"'
ok "invitado: puede añadir" "$(j POST '{"tracks":[{"id":"inv1","title":"Del invitado"}]}' $B/playlists/$PV/tracks)" 204
ok "invitado: puede quitar" "$(j DELETE '' $B/playlists/$PV/tracks/inv1)" 204
ok "invitado: no la renombra" "$(j PUT '{"name":"Mia ahora"}' $B/playlists/$PV)" 204
ok "invitado: no la borra" "$(j DELETE '' $B/playlists/$PV)" 404
ok "invitado: no invita a otros" "$(j POST '{"username":"bob"}' $B/playlists/$PV/members)" 404
ok "invitado: no la hace pública" "$(j PUT '{"visibility":"public"}' $B/playlists/$PV)" 204
T=$TA; D=$(curl -s -H "Authorization: Bearer $T" $B/playlists/$PV)
ok "sigue llamándose igual" "$D" '"name":"Privada"'
ok "sigue siendo privada" "$D" '"visibility":"private"'
ok "la dueña la expulsa" "$(j DELETE '' $B/playlists/$PV/members/bob)" 204
T=$TB; ok "expulsado: ya no la ve" "$(curl -s -o /dev/null -w %{http_code} -H "Authorization: Bearer $T" $B/playlists/$PV)" 404
ok "expulsado: no puede añadir" "$(j POST '{"tracks":[{"id":"x","title":"t"}]}' $B/playlists/$PV/tracks)" 403
T=$TA; ok "dueña: puede borrarla" "$(j DELETE '' $B/playlists/$PV)" 204
T=$TA; ok "dueño borra" "$(j DELETE '' $B/playlists/1)" 204
((fail)) && { echo; tail -5 "$W/log"; exit 1; }; echo "todo ok"

#!/usr/bin/env bash
# Genera dist/installer.exe (instalador NSIS) desde la compilación cruzada con MinGW (build-win/).
# Todo se hace en local: nada de CI. Necesita: x86_64-w64-mingw32-g++, makensis (apt install nsis),
# libcurl para MinGW en ~/win-deps/x64 y el respack del host (build/respack: compila antes la versión Linux).
#   tools/package-windows.sh [versión]        (por defecto, la de CMake / "0.0.0")
# ffmpeg.exe y ffprobe.exe se toman de ../Homeflix-Local/build-win (o de FFMPEG_DIR); yt-dlp y el
# bootstrapper de WebView2 se descargan una vez a build-win/redist.
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
B=$ROOT/build-win
VERSION=${1:-$(tr -d "[:space:]" < "$ROOT/app/version.txt")}
FFMPEG_DIR=${FFMPEG_DIR:-$ROOT/../Homeflix-Local/build-win}
command -v makensis >/dev/null || { echo "falta makensis: sudo apt install nsis" >&2; exit 1; }
[ -x "$ROOT/build/respack" ] || { echo "falta build/respack: compila la versión Linux primero (cmake --build build)" >&2; exit 1; }

echo "== compilando localify.exe"
cmake -S "$ROOT" -B "$B" -G Ninja -DCMAKE_BUILD_TYPE=Release -DCMAKE_SYSTEM_NAME=Windows \
  -DCMAKE_C_COMPILER=x86_64-w64-mingw32-gcc -DCMAKE_CXX_COMPILER=x86_64-w64-mingw32-g++ \
  -DCMAKE_RC_COMPILER=x86_64-w64-mingw32-windres -DLUX_HOST_RESPACK="$ROOT/build/respack" \
  -DCMAKE_PREFIX_PATH="$HOME/win-deps/x64" -DCMAKE_FIND_ROOT_PATH="/usr/x86_64-w64-mingw32;$HOME/win-deps/x64" >/dev/null
cmake --build "$B" --target localify -j"$(nproc)"
x86_64-w64-mingw32-strip -o "$B/localify-stripped.exe" "$B/localify.exe"

echo "== dependencias"
mkdir -p "$B/redist"
[ -f "$B/redist/yt-dlp.exe" ] || curl -fsSL -o "$B/redist/yt-dlp.exe" https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe
[ -f "$B/redist/MicrosoftEdgeWebview2Setup.exe" ] || curl -fsSL -o "$B/redist/MicrosoftEdgeWebview2Setup.exe" "https://go.microsoft.com/fwlink/p/?LinkId=2124703"
for f in ffmpeg.exe ffprobe.exe; do [ -f "$FFMPEG_DIR/$f" ] || { echo "falta $FFMPEG_DIR/$f (FFMPEG_DIR=...)" >&2; exit 1; }; done
[ -f "$B/redist/localify.ico" ] || python3 -c "
from PIL import Image
Image.open('$ROOT/app/icon.png').convert('RGBA').save('$B/redist/localify.ico', sizes=[(16,16),(32,32),(48,48),(64,64),(128,128),(256,256)])"

STAGE=$B/stage; rm -rf "$STAGE"; mkdir -p "$STAGE"
cp "$B/localify-stripped.exe" "$STAGE/localify.exe"
cp "$(x86_64-w64-mingw32-g++ -print-file-name=libwinpthread-1.dll)" "$STAGE/"
cp "$FFMPEG_DIR/ffmpeg.exe" "$FFMPEG_DIR/ffprobe.exe" "$B/redist/yt-dlp.exe" "$B/redist/MicrosoftEdgeWebview2Setup.exe" "$B/redist/localify.ico" "$STAGE/"
cp "$ROOT/LICENSE" "$STAGE/LICENSE.txt"

echo "== instalador"
mkdir -p "$ROOT/dist"
makensis -V2 -DSRC="$STAGE" -DOUT="$ROOT/dist/installer.exe" -DVERSION="$VERSION" "$ROOT/deploy/windows/installer.nsi"
echo "$ROOT/dist/installer.exe ($(du -h "$ROOT/dist/installer.exe" | cut -f1))"

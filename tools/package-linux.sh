#!/usr/bin/env bash
# Genera en dist/ el binario suelto (localify-<versión>-linux-x86_64) y el paquete .deb. Todo en local.
#   tools/package-linux.sh [versión]        (por defecto app/version.txt; compila antes: cmake --build build)
# Necesita dpkg-deb. El icono sale de app/icon.png (128x128).
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
VERSION=${1:-$(tr -d "[:space:]" < "$ROOT/app/version.txt")}
BIN=$ROOT/build/localify
[ -x "$BIN" ] || { echo "falta $BIN: compila antes (cmake --build build --target localify)" >&2; exit 1; }
command -v dpkg-deb >/dev/null || { echo "falta dpkg-deb" >&2; exit 1; }
OUT=$ROOT/dist; mkdir -p "$OUT"
strip -o "$OUT/localify-$VERSION-linux-x86_64" "$BIN"
chmod 755 "$OUT/localify-$VERSION-linux-x86_64"

P=$(mktemp -d); trap 'rm -rf "$P"' EXIT
install -Dm755 "$OUT/localify-$VERSION-linux-x86_64" "$P/usr/bin/localify"
install -Dm644 /dev/stdin "$P/usr/share/applications/Localify.desktop" <<DESK
[Desktop Entry]
Type=Application
Name=Localify
Comment=Reproductor de musica local con la experiencia de Spotify
Exec=localify
Icon=localify
Terminal=false
Categories=AudioVideo;Audio;Music;
StartupWMClass=Localify
DESK
python3 -c "
from PIL import Image
Image.open('$ROOT/app/icon.png').convert('RGBA').resize((128, 128), Image.LANCZOS).save('$P/icon.png')"
install -Dm644 "$P/icon.png" "$P/usr/share/icons/hicolor/128x128/apps/localify.png"; rm "$P/icon.png"
install -d "$P/DEBIAN"
cat > "$P/DEBIAN/control" <<CTL
Package: localify
Version: $VERSION
Section: sound
Priority: optional
Architecture: amd64
Depends: libgtk-3-0, libwebkit2gtk-4.1-0, libcurl4
Recommends: yt-dlp, ffmpeg
Maintainer: Burnt <burnt@burnt.ovh>
Homepage: https://github.com/Eldur4023/Localify
Description: Reproductor de musica local con la experiencia de Spotify
 Localify es un reproductor de musica local que resuelve tu biblioteca
 contra YouTube Music para descargar y reproducir sin conexion. Este
 paquete instala el binario nativo (GTK3 + WebKitGTK, sobre Lux), sin
 dependencias de un runtime aparte.
 .
 yt-dlp y ffmpeg/ffprobe deben estar en el PATH para el catalogo remoto
 y las descargas.
CTL
dpkg-deb --root-owner-group --build "$P" "$OUT/localify_${VERSION}_amd64.deb" >/dev/null
ls -la "$OUT" | grep -E "localify.*(linux|deb)"

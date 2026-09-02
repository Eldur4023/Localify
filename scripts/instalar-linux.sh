#!/usr/bin/env bash
#
# Instalador de Localify para Linux.
#
# El paquete trae **todas** sus bibliotecas —WebKitGTK, GTK y sus 160 y pico
# dependencias— así que no hace falta instalar nada del sistema. Es lo que lo
# distingue del `.deb`, que depende de que la distribución traiga las suyas.
#
# ## Por qué un árbol y no un AppImage a secas
#
# El AppImage del que sale esto necesita FUSE para montarse, y libfuse2 no viene
# instalado ni en Ubuntu 22.04 ni en las que le siguen. Un instalador que empieza
# pidiendo que instales otra cosa no es un instalador. Aquí el árbol va ya
# extraído y se copia tal cual.
#
#   sudo ./instalar.sh              # para todo el sistema, en /opt
#   ./instalar.sh --user            # solo para ti, en ~/.local
#   sudo ./instalar.sh --desinstalar
set -euo pipefail

NOMBRE="localify"
VERSION="1.2.3"
AQUI="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

MODO_USUARIO=0
DESINSTALAR=0
for arg in "$@"; do
  case "$arg" in
    --user|--usuario)      MODO_USUARIO=1 ;;
    --uninstall|--desinstalar) DESINSTALAR=1 ;;
    -h|--help|--ayuda)
      sed -n '3,20p' "${BASH_SOURCE[0]}" | sed 's/^# \?//'
      exit 0 ;;
    *) echo "argumento desconocido: $arg" >&2; exit 2 ;;
  esac
done

# Sin --user, hace falta ser root: se escribe en /opt y en /usr/local.
if [ "$MODO_USUARIO" -eq 0 ] && [ "$(id -u)" -ne 0 ]; then
  echo "Esto escribe en /opt y en /usr/local, así que necesita sudo." >&2
  echo "  sudo $0            (para todo el sistema)" >&2
  echo "  $0 --user          (solo para tu usuario, sin sudo)" >&2
  exit 1
fi

if [ "$MODO_USUARIO" -eq 1 ]; then
  DESTINO="$HOME/.local/lib/$NOMBRE"
  BIN="$HOME/.local/bin"
  APPS="$HOME/.local/share/applications"
  ICONOS="$HOME/.local/share/icons/hicolor"
else
  DESTINO="/opt/$NOMBRE"
  BIN="/usr/local/bin"
  APPS="/usr/share/applications"
  ICONOS="/usr/share/icons/hicolor"
fi

paso() { printf '\033[36m==> %s\033[0m\n' "$1"; }
ok()   { printf '\033[32m    %s\033[0m\n' "$1"; }
avisa(){ printf '\033[33m    %s\033[0m\n' "$1"; }

# ─── Desinstalar ─────────────────────────────────────────────────────────────
if [ "$DESINSTALAR" -eq 1 ]; then
  paso "Desinstalando Localify"
  rm -rf "$DESTINO"
  rm -f "$BIN/$NOMBRE" "$APPS/$NOMBRE.desktop"
  find "$ICONOS" -name "$NOMBRE.png" -delete 2>/dev/null || true
  ok "Quitado."
  echo
  avisa "Tu música y tus ajustes NO se han tocado. Están en:"
  avisa "  ~/.config/Localify   (base de datos, ajustes, binarios de yt-dlp)"
  avisa "  ~/Music/Localify     (el audio descargado)"
  exit 0
fi

# ─── Instalar ────────────────────────────────────────────────────────────────
[ -d "$AQUI/app" ] || { echo "falta la carpeta 'app' junto al instalador" >&2; exit 1; }

paso "Instalando Localify $VERSION en $DESTINO"
rm -rf "$DESTINO"
mkdir -p "$DESTINO"
cp -a "$AQUI/app/." "$DESTINO/"
ok "$(du -sh "$DESTINO" | cut -f1) copiados, bibliotecas incluidas."

# El lanzador es un script y no un enlace simbólico **a propósito**: `AppRun`
# localiza sus bibliotecas con `readlink -f "$(dirname "$0")"`, y a través de un
# enlace eso resolvería al directorio del enlace —/usr/local/bin— donde no hay
# ninguna. Con `exec` sobre la ruta real, resuelve donde debe.
paso "Poniendo '$NOMBRE' en el PATH ($BIN)"
mkdir -p "$BIN"
cat > "$BIN/$NOMBRE" <<LANZADOR
#!/bin/sh
exec "$DESTINO/AppRun" "\$@"
LANZADOR
chmod +x "$BIN/$NOMBRE"
ok "$BIN/$NOMBRE"

paso "Registrando en el menú de aplicaciones"
mkdir -p "$APPS"
cat > "$APPS/$NOMBRE.desktop" <<ESCRITORIO
[Desktop Entry]
Type=Application
Name=Localify
Comment=Reproductor de musica local con la experiencia de Spotify
Exec=$BIN/$NOMBRE
Icon=$NOMBRE
Terminal=false
Categories=AudioVideo;Audio;Music;
StartupWMClass=localify
ESCRITORIO

for tam in 32 128 256; do
  origen="$AQUI/app/usr/share/icons/hicolor/${tam}x${tam}/apps/$NOMBRE.png"
  [ -f "$origen" ] || continue
  mkdir -p "$ICONOS/${tam}x${tam}/apps"
  cp "$origen" "$ICONOS/${tam}x${tam}/apps/$NOMBRE.png"
done
command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -f -t "$ICONOS" 2>/dev/null || true
command -v update-desktop-database >/dev/null && update-desktop-database "$APPS" 2>/dev/null || true
ok "Listo."

# ~/.local/bin no está en el PATH de todas las distribuciones. Decirlo ahora
# ahorra el "lo instalé y no existe el comando" de dentro de cinco minutos.
case ":$PATH:" in
  *":$BIN:"*) ;;
  *) echo; avisa "$BIN no está en tu PATH. Añádelo a ~/.profile:";
     avisa "  export PATH=\"\$PATH:$BIN\"" ;;
esac

# El icono de bandeja es la única pieza que este árbol no trae consigo: la
# usan muchos escritorios distintos y cada uno con su propia implementación,
# así que se busca en el sistema en vez de empaquetarla. Sin ella Localify
# arranca y suena igual —solo falta el icono—, así que no se para el
# instalador por esto, solo se avisa.
if ! ldconfig -p 2>/dev/null | grep -q "libayatana-appindicator3.so.1\|libappindicator3.so.1"; then
  echo
  avisa "Sin libayatana-appindicator3, Localify arrancará sin icono de bandeja."
  avisa "  (el resto funciona igual; instálala si la quieres)"
fi

# ─── Sidecars ────────────────────────────────────────────────────────────────
# yt-dlp y FFmpeg no van dentro: yt-dlp se rompe cuando YouTube cambia y tiene
# que poder actualizarse solo, sin reinstalar Localify (ADR-006).
echo
if [ -x "${XDG_CONFIG_HOME:-$HOME/.config}/Localify/bin/yt-dlp" ]; then
  ok "yt-dlp ya está instalado; Localify lo actualizará solo al arrancar."
else
  avisa "Falta yt-dlp, que es de donde sale el audio. Instálalo con:"
  avisa "  ./fetch-sidecars.sh"
fi

echo
paso "Arranca con 'localify' o desde el menú de aplicaciones."

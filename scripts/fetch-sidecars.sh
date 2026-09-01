#!/usr/bin/env bash
#
# Descarga yt-dlp y FFmpeg en la carpeta de binarios de Localify.
#
# Equivalente de `fetch-sidecars.ps1` para Linux. Los sidecars no se empaquetan
# con la aplicación (ADR-006): yt-dlp se rompe cuando YouTube cambia su
# ofuscación, cada pocas semanas, y poder actualizarlo sin publicar una versión
# de Localify es la diferencia entre "se arregla solo" y "la app está rota hasta
# la siguiente release".
#
# Una vez instalado, la propia aplicación lo mantiene al día al arrancar; ver
# `actualizar_yt_dlp` en localify-platform.
#
#   ./scripts/fetch-sidecars.sh            # instala lo que falte
#   ./scripts/fetch-sidecars.sh --forzar   # reinstala aunque ya esté
set -euo pipefail

DESTINO="${XDG_CONFIG_HOME:-$HOME/.config}/Localify/bin"
FORZAR=0
[ "${1:-}" = "--forzar" ] && FORZAR=1

paso() { printf '\033[36m==> %s\033[0m\n' "$1"; }
ok()   { printf '\033[32m    %s\033[0m\n' "$1"; }

mkdir -p "$DESTINO"
paso "Destino: $DESTINO"

# ─── yt-dlp ──────────────────────────────────────────────────────────────────
# Siempre la última publicación: fijar una versión sería contraproducente,
# porque el valor de yt-dlp está justo en ir por delante de YouTube.
YTDLP="$DESTINO/yt-dlp"
if [ -x "$YTDLP" ] && [ "$FORZAR" -eq 0 ]; then
  ok "yt-dlp ya presente (versión $("$YTDLP" --version 2>/dev/null || echo '?')). Usa --forzar para reinstalar."
else
  paso 'Descargando yt-dlp...'
  curl -fsSL -o "$YTDLP.download" \
    https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp_linux
  chmod +x "$YTDLP.download"
  # Comprobación mínima: que responda. Una descarga truncada fallaría aquí y no
  # a mitad de una descarga de audio.
  "$YTDLP.download" --version >/dev/null
  mv "$YTDLP.download" "$YTDLP"
  ok "yt-dlp $("$YTDLP" --version) instalado."
fi

# ─── FFmpeg ──────────────────────────────────────────────────────────────────
# Compilaciones estáticas de johnvansickle.com, las que recomienda el propio
# proyecto FFmpeg para Linux. Estáticas a propósito: así no dependen de las
# bibliotecas de la distribución y el binario vale en cualquiera.
FFMPEG="$DESTINO/ffmpeg"
FFPROBE="$DESTINO/ffprobe"
if [ -x "$FFMPEG" ] && [ -x "$FFPROBE" ] && [ "$FORZAR" -eq 0 ]; then
  ok "FFmpeg ya presente ($("$FFMPEG" -version 2>/dev/null | head -1)). Usa --forzar para reinstalar."
else
  paso 'Descargando FFmpeg (~30 MB)...'
  TMP="$(mktemp -d)"
  trap 'rm -rf "$TMP"' EXIT
  curl -fsSL -o "$TMP/ffmpeg.tar.xz" \
    https://johnvansickle.com/ffmpeg/releases/ffmpeg-release-amd64-static.tar.xz
  tar -xJf "$TMP/ffmpeg.tar.xz" -C "$TMP"
  for nombre in ffmpeg ffprobe; do
    origen="$(find "$TMP" -type f -name "$nombre" -perm -u+x | head -1)"
    [ -n "$origen" ] || { echo "el archivo de FFmpeg no contiene $nombre" >&2; exit 1; }
    install -m 755 "$origen" "$DESTINO/$nombre"
  done
  ok "$("$FFMPEG" -version | head -1) instalado."
fi

echo
paso 'Sidecars listos.'
ls -lh "$DESTINO" | tail -n +2 | awk '{ printf "  %-12s %s\n", $9, $5 }'

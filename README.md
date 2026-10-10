# Lux Desktop — Localify

**Localify**, el reproductor de música local (port de
[Localify](https://github.com/), originalmente Rust + Tauri), construido
sobre [Lux](vendor/lux): ventana nativa GTK3 + WebKitGTK, LuxScript en vez
de Node, y un binario nativo único. El audio lo reproduce un **motor nativo**
(miniaudio + ffmpeg, `src/audio_engine.cpp`) que el backend LuxScript manda
por el módulo `audio`, la biblioteca vive en SQLite, y la resolución de
canciones (YouTube Music + MusicBrainz + yt-dlp) está reescrita íntegramente
en LuxScript.

## Qué incluye el port

- **Biblioteca** catálogo con pistas/álbumes/artistas/favoritos/historial y
  estadísticas (`app/library.lux`, esquema en `app/db.lux`, adaptado de las
  migraciones V1-V8 del original).
- **Reproducción completa** — cola doble estilo Spotify (cola de usuario con
  prioridad absoluta + cola de contexto), aleatorio con permutación estable
  y semilla persistida, repetición off/cola/pista, regla de 3 s del botón
  "anterior", sesión restaurada al arrancar (`app/queue.lux`,
  `app/player.lux`). El backend es el único dueño del reproductor
  (`app/motor.lux`): aplica el estado al motor de audio y reacciona a sus
  avisos (relevo gapless/crossfade, fin de pista).
- **Resolución de canciones** — cliente InnerTube de YouTube Music
  (`app/ytmusic.lux`), MusicBrainz (`app/musicbrainz.lux`), el scorer
  completo del emparejador (Jaro-Winkler, factor de duración multiplicativo,
  vocabulario de versiones con la excepción central: un término que la pista
  pide anula la penalización) en `app/text.lux` + `app/scoring.lux`.
- **Descargas** — yt-dlp como proceso de larga duración vía el módulo `proc`
  (handles persistentes entre peticiones), máquina de estados que avanza por
  ticks, plan de consultas por orden de fiabilidad, reintentos con backoff,
  rechazo de vídeos fallidos, verificación de duración con ffprobe y
  remux+etiquetas con ffmpeg en un solo pase (`app/downloads.lux`).
- **Playlists** — CRUD, reordenado, importación de listas públicas de
  YouTube Music y de Spotify (página embed, sin credenciales)
  (`app/playlists.lux`).
- **Letras** vía LRCLIB con caché positiva y negativa (`app/lyrics.lux`).
- **Frontend** — SPA vanilla ES modules en `app/public/src/` + `app/templates/`,
  tema oscuro heredado de los tokens del original. La capa IPC es `fetch`
  contra `/api/invoke` para las órdenes y **SSE** (`/api/events/stream`) para
  los eventos: el backend empuja una foto de su estado cuando cambia.

## Cómo está montado el audio

Como en el original (`crates/localify-audio`), el audio no pasa por el
WebView: el camino de audio de WebKitGTK (GStreamer) fallaba con salidas
Bluetooth y obligaba a medio frontend a existir solo para sincronizar dos
dueños del estado.

- **Motor** (`src/audio_engine.cpp`): ffmpeg decodifica a PCM f32 por una
  tubería a un anillo SPSC de 4 s por voz; el hilo de audio mezcla dos voces
  (gapless, crossfade equal-power), aplica un ecualizador de 10 bandas y
  volumen (v³), y lee solo atómicos y anillos: no asigna, no toma locks, no
  hace I/O. La posición es la que se oye. El motor **no conoce la cola**: el
  backend le da la pista actual (`audio.load`) y la siguiente
  (`audio.preload`), y recibe avisos (`handoff` / `ended` / `error`).
- **Salida**: miniaudio sobre PulseAudio/PipeWire o ALSA; la salida se elige
  en Ajustes por nombre (`audio.devices`, `audio.set_device`).
- **Tareas de fondo** (`app/events.lux`, bloques `every` de Lux): el tick del
  reproductor (200 ms), las descargas (1 s) y la búsqueda remota. No dependen
  de que la ventana esté a la vista.
- **Migraciones** versionadas (`settings.schema_version`) en `on start:`.

## Tests

```bash
cmake --build build --target audio_engine_test app-headless
./build/audio_engine_test          # el mezclador sin tarjeta de sonido (render() a mano)
python3 tests/e2e_audio.py         # backend + motor + SSE, sin ventana ni sonido
ctest --test-dir build             # las dos
```

`app-headless <dir-de-la-app> [puerto]` levanta el servidor y el motor sin
ventana (con `LUX_AUDIO_NULL=1` no suena nada). `LUXDESKTOP_APP_DIR=<dir>
./build/app-dev` arranca el modo dev contra otra copia de la app.

## Requisitos

- `libgtk-3-dev`, `libwebkit2gtk-4.1-dev` (o `-4.0-dev`) para compilar.
- **ffmpeg** en el PATH: decodifica todo el audio que se reproduce.
- **yt-dlp** en el PATH para descargar (obligatorio para el catálogo remoto).
- **ffprobe** para verificar y etiquetar lo descargado (opcional: sin él el
  audio se guarda tal cual).
- Si YouTube pide verificación, configura cookies en Ajustes.

## Dónde viven los datos

Todo bajo `$XDG_DATA_HOME/lux-desktop/localify/` (normalmente
`~/.local/share/lux-desktop/localify/`): el binario empaquetado extrae ahí
sus recursos en cada arranque (sobrescribiendo solo los de solo lectura) y
trabaja desde ese directorio — la base de datos en `./data/localify.db`, el
audio descargado en la carpeta de biblioteca (por defecto `~/Music/Localify`,
configurable en Ajustes). En dev (`app-dev`) el cwd es `app/` directamente y
la base de datos queda en `app/data/` (ignorada por git y excluida de
respack: los datos de runtime nunca van dentro del binario).

## Build y ejecución

```bash
cmake -S . -B build -DCMAKE_BUILD_TYPE=Release
cmake --build build -j$(nproc)
./build/app        # empaquetado: respack incrusta app/ en el binario
./build/app-dev    # dev: lee app/ del disco, hot-reload al guardar
```

---

# Documentación del esqueleto (Lux Desktop)

An Electron/Tauri equivalent built on [Lux](vendor/lux) — a native GTK3 +
WebKitGTK window instead of a bundled Chromium, LuxScript instead of Node,
and a build that produces **one native binary**: compile it, run it, the
window opens. Nothing to install alongside it, no runtime to ship
separately, no crate tree to download.

Lux is vendored as plain source under `vendor/lux/` (copied in, not a git
submodule — no network access needed to build this project once it's
checked out) and used as a library, with one small, documented addition: a
`window` module (`vendor/lux/src/lux_script/modules/window.cpp`) using
Lux's own drop-in module mechanism, purely so window title/size can be
declared from LuxScript itself instead of from CMake or C++. Nothing about
how Lux itself works is changed.

## Where you work

**`app/`** — that's it. `app/app.lux`, `app/templates/`, `app/public/`, and
any other `.lux` file you want; Lux compiles a whole directory tree, order
does not matter. `CMakeLists.txt` never needs to change for anything that
happens inside `app/`.

## How it works

1. **Write the app** in `app/` — LuxScript, the same language and template
   engine as any other Lux project. Window title/size go in `app.lux`'s own
   `window:` block (see below), not in CMake.
2. **Build**: `cmake --build build`. At build time, `respack`
   (`tools/respack/`) embeds `app/`'s whole contents into the binary as byte
   arrays — no external files, no assets folder to keep next to it.
3. **Run `./build/app`.** At startup it extracts its own embedded files into
   a throwaway temp directory, compiles them through Lux's own compiler
   (`lux_script::compile`, the exact same one `lux` itself uses), starts an
   HTTP server bound to `127.0.0.1` on a random free port — nothing outside
   the machine can ever reach it — and opens a native window pointed at it.
   The page just does `fetch()` against the app's own routes; that loopback
   HTTP call *is* the whole "IPC bridge", nothing extra to learn. Closing
   the window (or `CTRL+C`) shuts the server down and deletes the temp
   directory.

## The window: block

```lux
import window

app:
    name      "My App"
    templates "./templates"
    static "/static" -> "./public"

    window:
        title     "My App"
        width     800
        height    600
        resizable true
        devtools  false     # true opens WebKit's inspector
        icon      "./icon.png"   # path relative to the app's own directory
```

Exactly the same mechanism `sqlite: { file, pool }` already uses for its own
config — `import window` marks it active, and the block's keys get
validated and read by the module itself. If the block (or the whole
`import`) is missing, sensible defaults apply (1024x768, resizable, no
devtools, no icon) and the window falls back to `app: name` for its title.

## Controlling the window from a route

Same `import window`, now called from inside a handler — the window is the
same process, so this is a plain function call:

```lux
post endpoint("/minimize"):
    window.minimize()   # also: .restore()
    return status(204)

get endpoint("/pick-file"):
    string path = await window.open_file()      # "" if the user canceled
    return { "path": path }
```

`open_file`/`save_file` open a native GTK file chooser and need `await`:
showing a dialog and waiting on the user is an unbounded wait, so they run
on Lux's shared worker pool instead of the event loop thread.

Anything else in the `window` module (title, maximize, fullscreen,
notifications, clipboard, menu, tray) is a no-op here: this app never used
it, so the runtime doesn't wire it.

## Build

Needs `libgtk-3-dev` and `libwebkit2gtk-4.1-dev` (`-4.0-dev` on an older
distribution) — the only things not already vendored. Everything Lux itself
needs (llhttp) is vendored inside `vendor/lux/third_party/`.

```bash
cmake -S . -B build -DCMAKE_BUILD_TYPE=Release
cmake --build build -j$(nproc)
./build/app        # the packaged binary -- respack embeds a snapshot of app/
./build/app-dev     # dev mode -- reads app/ live, hot-reloads on save
```

## Installing it as a real app

```bash
./build/app --install-desktop
```

Writes a `.desktop` launcher into `~/.local/share/applications/` (and the
window's icon, if it has one, into the standard
`~/.local/share/icons/hicolor/256x256/apps/` theme location) pointing at
this exact binary's own path — the app then shows up in the normal
application menu/launcher like anything installed through a package
manager. Never run automatically: it's the one thing in this whole project
that writes outside its own directory, into the desktop environment
itself, so it only happens when asked for by name.

## Window size is remembered

Resizing the window and reopening the app later restores that size —
saved to `~/.cache/lux-desktop/<app-id>.geometry` (a plain `WIDTH HEIGHT`,
nothing fancier) right as the window closes, and read back before the next
one opens. The `window:` block's `width`/`height` are the first-launch
default, not something the window snaps back to on every run.

## Dev mode

`./build/app-dev` reads `app/` straight off disk instead of an embedded
snapshot: no `respack`, no rebuilding this binary after editing a `.lux`
file, a template or a static asset. A background thread watches `app/` and
recompiles through the same `lux_script::compile()` the packaged binary
uses — LuxScript compiles to bytecode in milliseconds, no `g++` involved —
and the window's page reloads on its own once it succeeds. A syntax error
is printed and the previous, working version keeps serving; the window
never goes down over a typo. Devtools are always on in this mode.

## What this is not (yet)

- **Linux only**, same as Lux itself (epoll, `sendfile(2)`, `SO_REUSEPORT`).

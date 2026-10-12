# Android shell (Localify)

Una `Activity` Kotlin con un `WebView` que carga `libluxapp.so` — el mismo
servidor Lux del build de escritorio, sin GTK. No hay nada específico de la
app aquí: se edita `../app/` y se recompila.

Paquete `es.localify.app` (el código Kotlin sigue en `dev.lux.desktop`). La release
(`assembleRelease`) se firma con `~/android-build/localify-release.jks`, cuyas
claves están en `~/android-build/signing.properties` (fuera del repo: **hay que
guardar una copia**, sin ella no se pueden publicar actualizaciones). Los builds
debug activan la depuración remota del WebView (`chrome://inspect`).

```bash
export ANDROID_HOME=~/Android/Sdk      # SDK + NDK 27
export JAVA_HOME=~/android-build/jdk   # JDK 17
cd android && ~/android-build/gradle-8.7/bin/gradle assembleDebug --no-daemon
```

(El `gradle` del sistema es demasiado viejo; usar el 8.7 de `~/android-build`.)

Los datos (`data/localify.db`, el audio descargado) viven en el
`filesDir/lux` privado de la app.

## Dependencias cruzadas (~/android-deps)

Los módulos `sqlite` y `http` de Lux enlazan librerías que el NDK no trae.
Ya están cross-compiladas para arm64 en `~/android-deps/arm64` (sqlite3,
openssl, libcurl estático) y `build.gradle.kts` las pasa por
`-DSQLITE3_INC/-DSQLITE3_LIB` y un wrapper de pkg-config
(`~/android-deps/bin/pkg-config-arm64`) que confina las rutas: el pkg-config
del host metería `-I/usr/include` en la compilación cruzada y la revienta.
Para regenerarlas: amalgamation de sqlite.org, openssl-3.3 (`./Configure
android-arm64`, con `ANDROID_NDK_ROOT` exportado) y curl-8.10 (cmake,
`CURL_USE_OPENSSL=ON`, estático).

En el CMake raíz hay un parche de una línea respecto al vendor limpio
(`AND NOT ANDROID` en el guard de "Lux requires Linux"); el resto del vendor
tiene dos ajustes de compatibilidad con clang/libc++ (`luxp_serve.cpp`:
mutex en vez de `std::atomic<shared_ptr>`; `template.cpp`: `Value kNull{}`
value-initialized).

## Qué funciona distinto en el móvil

- **Descargas**: no hay `yt-dlp` en el teléfono. Las URLs de googlevideo van
  firmadas con la IP que las pidió, así que resolver en otra máquina y bajar
  desde el móvil da 403 con datos móviles. El móvil lo hace todo con su IP
  (`../app/ytmovil.lux`): resuelve con InnerTube (cliente `ANDROID_VR`, URLs
  directas sin cifrar) y baja con `range=0-N` (sin él googlevideo estrangula).
  Si hay servidor común, le sube el crudo (`PUT /relay/:id`), que lo
  transcodifica a MP3 etiquetado, y el móvil se lo trae con `srv_obtener`; si
  no, el crudo (webm/m4a) se queda tal cual. El servidor nunca toca YouTube.
- **Red nativa**: Android no trae `cacert.pem` y su almacén no sirve de
  `CAPATH` a OpenSSL 3 (hashes antiguos): `src/boot.cpp` junta los certificados
  del sistema en `filesDir/lux/cacert.pem` y exporta `LUX_CA_BUNDLE`, que usa el
  módulo `http` de Lux (`CURLOPT_CAINFO`). Con la app en segundo plano y la
  pantalla apagada Android corta la red a la app: cualquier prueba por ADB
  necesita pantalla encendida y desbloqueada (`adb shell svc power stayon usb`).
- **Audio**: no hay motor nativo (los hooks de `audio` quedan vacíos); quien
  suena es un elemento `<audio>` del WebView alimentado por `GET /audio/:id`
  (con Range). El shim es `../app/public/src/shell/web-audio.js`, activado por
  `GET /api/capabilities` (`mobile: true`, que `src/android.cpp` fija con la
  variable de entorno `LUX_LOCALIFY_MOBILE=1`). El WebView reporta "ended" y
  su posición al backend (`/api/player/media-event`) para que la cola avance.
- **Servicio en primer plano + MediaSession nativa** (`PlaybackService.kt`): la
  `Activity` lo arranca al abrirse y se para al quitar la app de recientes. Sostiene
  el proceso (servidor, descargas, `<audio>`) con la pantalla apagada y publica al
  sistema título, portada y posición: notificación con anterior/play/siguiente,
  pantalla de bloqueo, auriculares y coche. El WebView **no** publica su
  `MediaSession` (`navigator.mediaSession` ni existe aquí), así que `web-audio.js`
  alimenta el servicio por `AndroidBridge.setMedia` y los botones vuelven por
  `window.__localifyNative.cmd`. Pausa al desenchufar auriculares
  (`ACTION_AUDIO_BECOMING_NOISY`). El audio focus lo gestiona Chromium; no está
  verificado con una llamada real.
- **Interfaz** (`app/public/src/styles/mobile.css`, `shell/gestos.js`): la misma
  que en escritorio con ≤700 px. Cajón de navegación (☰ o deslizar desde el
  cuarto izquierdo), mini-reproductor (portada, título, play, siguiente) y vista
  de la canción (portada, datos, letra, controles completos) al tocarlo o
  deslizando desde el cuarto derecho.
- `window.*` (menú, bandeja, diálogos nativos) no hace nada o falla sin ruido.

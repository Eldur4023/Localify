# 08 — Decisiones técnicas (ADRs)

El prompt exige: *"Si alguna decisión técnica admite varias soluciones, explica
cuál eliges y por qué"*. Aquí están todas las que lo admiten, con sus
alternativas y el coste que asumimos al elegir.

Formato: **Contexto → Opciones → Decisión → Consecuencias**.

---

## ADR-001 — Frontend sin framework

**Contexto.** El prompt pide HTML/CSS/TypeScript y evitar React, Vue o Svelte
salvo razón técnica de peso. Toda la lógica debe residir en Rust.

**Opciones.**
1. Vanilla TS.
2. Svelte (compilado, sin runtime pesado).
3. Lit (Web Components).

**Decisión.** **Vanilla TS.**

**Razones.** Si el frontend solo pinta datos y emite comandos (P1), el problema
que resuelven los frameworks —gestión de estado derivado complejo— no existe
aquí. La pieza técnicamente difícil de esta UI es la **lista virtualizada de
50 000 filas**, y ahí un virtual DOM genérico es coste añadido: se implementa
mejor con reciclado directo de nodos. Además evitamos ~40 KB de runtime y una
dependencia menos que actualizar.

**Consecuencias.** Escribimos a mano un router (~80 líneas), un store (~150
líneas) y un helper de plantillas. Es código que entendemos por completo. El
riesgo real —que la UI crezca hasta necesitar un framework— se acota con la
regla "un componente = un archivo = una función `create*`".

**Se revisaría si.** Aparecen vistas con estado derivado profundamente anidado,
o el store supera las ~400 líneas.

---

## ADR-019 — Sin Node.js, sin bundler: TypeScript transpilado desde Rust

**Contexto.** El frontend es TypeScript, que hay que convertir a JavaScript
ejecutable. La vía habitual (Vite + npm) introduce un segundo ecosistema de
paquetes, un segundo gestor de dependencias y un runtime completo (Node.js)
como requisito para compilar el proyecto.

**Opciones.**
1. Vite + npm (lo estándar en Tauri).
2. `esbuild` como binario suelto, sin npm.
3. Transpilar con **oxc** (transpilador TypeScript en Rust) desde el `build.rs`
   de `localify-app`, y servir **módulos ES nativos** sin empaquetar.
4. Escribir JavaScript con anotaciones JSDoc y no transpilar nada.

**Decisión.** **Opción 3.**

**Razones.**
- **El bundler no aporta nada aquí.** Empaquetar existe para reducir peticiones
  de red en Internet. Nuestros assets se sirven desde el propio proceso, por
  `tauri://localhost`, con latencia efectivamente nula. WebView2 es Chromium:
  `<script type="module">` e `import` nativos funcionan sin más.
- **Eliminar Node elimina un ecosistema entero**: sin `node_modules`, sin
  `package-lock.json`, sin `npm audit`, sin la clase de fallos por versiones
  de dependencias transitivas. `cargo build` compila el proyecto completo,
  backend y frontend, con una sola cadena de herramientas.
- Convertir TypeScript a JavaScript es, para nuestro caso, **borrado de tipos**:
  una transformación puramente sintáctica que oxc hace en milisegundos y que se
  integra en `build.rs` con `rerun-if-changed`.
- La opción 4 renunciaría a TypeScript, que el prompt pide explícitamente.
- La opción 2 evita npm pero mantiene la descarga y gestión de un binario
  externo para algo que Rust ya sabe hacer.

**Consecuencias.**
- **La comprobación de tipos deja de ser parte del build.** oxc borra tipos, no
  los verifica. Se mitiga así: `tsc --noEmit` queda como verificación opcional
  para quien tenga Node instalado, y se ejecuta en CI en un job aparte que
  **no bloquea la compilación**. Es un linter, no una puerta. El contrato con
  el backend lo sigue garantizando `ts-rs`, que genera los tipos desde Rust.
- Sin *tree shaking* ni minificación. Irrelevante: el frontend previsto ronda
  los ~150 KB de fuentes y se carga desde disco local.
- Sin HMR. Se sustituye por recarga automática: `build.rs` observa
  `frontend/src` y `cargo tauri dev` recarga la ventana. Para una app cuya
  lógica vive en Rust —donde cualquier cambio real recompila Rust de todos
  modos— el HMR aportaba poco.
- Los `import` deben llevar la extensión `.js` explícita (requisito de ESM
  nativo), aunque el fichero fuente sea `.ts`. Es la convención de
  TypeScript con `moduleResolution: "nodenext"`.

**Se revisaría si.** El número de módulos crece hasta que la cascada de
peticiones ESM sea medible en el arranque (> 50 ms), o si hiciera falta una
dependencia de terceros que solo se distribuya por npm.

---

## ADR-002 — Motor de audio en Rust, no Web Audio API

**Contexto.** Hacen falta crossfade, ecualizador de 10 bandas, gapless,
múltiples formatos y **reproducir un archivo mientras se descarga**.

**Opciones.**
1. Web Audio API en el WebView (`AudioContext`, `BiquadFilterNode`).
2. `rodio` (abstracción de alto nivel sobre cpal).
3. `cpal` + `symphonia` con mezclador propio.

**Decisión.** **Opción 3: motor propio sobre cpal + symphonia.**

**Razones.**
- Web Audio viola el principio P1 (la lógica de reproducción acabaría en TS) y,
  sobre todo, **no puede leer un archivo que está creciendo**: `decodeAudioData`
  requiere el buffer completo, y `MediaSource Extensions` exige segmentar el
  stream nosotros, lo que sería reimplementar el problema en el peor sitio.
- `rodio` no expone control de sample-accurate crossfade entre dos fuentes ni
  una cadena de DSP insertable, y su modelo de `Sink` no encaja con precargar la
  siguiente pista.
- El motor propio da control total: dos voces, rampas equal-power, biquads,
  limitador y una fuente personalizada (`GrowingFileSource`) que es la clave de
  toda la UX de descarga transparente.

**Consecuencias.** Es la parte más costosa del proyecto (Fase 7 completa) y la
que concentra el riesgo técnico. A cambio, es la que hace posible el producto:
sin `GrowingFileSource` no hay "pulsa play y suena en 2 segundos".

**Plan B documentado.** Si el motor propio se atasca, `rodio` con espera a
buffer mínimo: se pierde crossfade real y se degrada el time-to-first-audio,
pero el producto sigue en pie.

---

## ADR-003 — Decodificación de Opus

**Contexto.** El mejor audio que sirve YouTube es Opus en WebM (~160 kbps VBR).
**Symphonia no incluye decodificador Opus.**

**Opciones.**
1. Preferir m4a/AAC (itag 140, 128 kbps), que symphonia decodifica nativamente.
2. Transcodificar Opus → FLAC con FFmpeg tras descargar.
3. Registrar un decodificador Opus propio, basado en `libopus`, en el
   `CodecRegistry` de symphonia.

**Decisión.** **Opción 3**, con la opción 1 como fallback configurable.

**Razones.** La opción 1 sacrifica calidad de forma permanente, y el prompt
exige "siempre la máxima calidad disponible". La opción 2 es peor: transcodificar
un códec con pérdida a otro formato no recupera nada, infla el fichero ~4× y
añade minutos de CPU por canción. La opción 3 mantiene el fichero original
intacto y solo aporta la pieza que falta: seguimos usando el demuxer Matroska
de symphonia y enchufamos el decodificador. Es una integración limpia por el
punto de extensión previsto para ello.

**Consecuencias.** Dependencia C: `opusic-sys` compila libopus y necesita cmake
más un compilador de C. Ambos están disponibles en el entorno de desarrollo, y
la compilación se verificó de principio a fin.

**Lo que cambió al implementarlo.** No hizo falta escribir el envoltorio: existe
`symphonia-adapter-libopus`, que ya implementa `RegisterableAudioDecoder` y
maneja el `pre-skip` de OggOpus. La decisión —libopus en el registro de códecs
de symphonia— se mantiene; la integración se reduce a una línea en
`localify-audio/src/decode/mod.rs`.

**Por qué no un decodificador en Rust puro.** Los hay (`opus-decoder`,
`moosicbox_opus_native`), pero son de versión 0.x reciente y con poco uso. Un
artefacto sutil de decodificación no rompe ningún test —el audio "funciona"— y
se manifiesta como "esta canción suena rara", que es de las cosas más difíciles
de atribuir. libopus es la implementación de referencia contra la que se define
el formato.

**Un detalle de compilación que costó un rato.** La primera prueba falló con
`C1041: no se puede abrir la base de datos de programa`. No era incompatibilidad
del toolchain: la ruta de compilación superaba `MAX_PATH`. Compila sin problemas
desde una ruta normal.

---

## ADR-004 — `rusqlite` en lugar de `sqlx`

**Contexto.** SQLite local, con FTS5, keyset pagination y un pool que respete
el modelo N-lectores/1-escritor de WAL.

**Opciones.**
1. `sqlx` con `sqlite` (async, consultas verificadas en compilación).
2. `rusqlite` con `bundled` (síncrono, envuelto en `spawn_blocking`).
3. `sea-orm`/`diesel` (ORM).

**Decisión.** **`rusqlite` con `bundled-full`** (incluye FTS5).

**Razones.**
- SQLite es un fichero local: no hay latencia de red que amortizar con async.
  El async de `sqlx` sobre SQLite acaba, internamente, en un pool de hilos
  bloqueantes de todos modos. Nos ahorramos la capa.
- FTS5 y los PRAGMAs se manejan con naturalidad en `rusqlite`; en `sqlx`
  requieren rodeos.
- Un ORM es contraproducente aquí: las consultas críticas (búsqueda con `bm25`,
  keyset pagination, similitud con CTEs) se escriben mejor a mano, y el ORM
  añadiría abstracción sobre lo que más nos importa optimizar.
- Binario más pequeño y compilación más rápida.

**Consecuencias.** Escribimos SQL a mano y mappers fila → entidad. No hay
verificación de consultas en tiempo de compilación; se compensa con tests de
repositorio contra una base de datos temporal (criterio de la Fase 3).

---

## ADR-005 — Spotify: client credentials con credenciales del usuario

**Contexto.** El prompt exige que el usuario **no inicie sesión**, pero la Web
API requiere autenticación de aplicación. El proyecto es open source, así que
no puede haber un secret en el repositorio.

**Opciones.**
1. Incrustar `client_id`/`client_secret` en el código.
2. Proxy propio que guarde el secret en un servidor.
3. El usuario pega sus credenciales una vez en Ajustes.
4. Usar el endpoint anónimo del reproductor web de Spotify.

**Decisión.** **Opción 3**, con inyección en tiempo de compilación para los
builds oficiales.

**Razones.**
- (1) filtra el secret a cualquiera que clone el repositorio y lleva a su
  revocación.
- (2) contradice "completamente local" e introduce infraestructura, coste y un
  punto único de fallo.
- (4) es una API interna no documentada: puede romperse cualquier día y es
  jurídicamente más discutible que usar la API oficial.
- (3) es explícito, honesto y sostenible. El usuario no vincula su cuenta de
  Spotify: solo aporta credenciales de aplicación, obtenibles gratis en un
  minuto. **No hay login de usuario**, que es exactamente lo que pide el prompt.

**Consecuencias.** Fricción en el primer arranque, mitigada con un asistente
guiado y con el hecho de que **la app funciona por completo sin Spotify** sobre
la biblioteca ya existente. `SpotifyProvider` es un trait, así que añadir
MusicBrainz o Deezer como alternativa no toca ningún servicio.

---

## ADR-006 — yt-dlp como sidecar, no como biblioteca

**Contexto.** Hay que obtener audio de YouTube.

**Opciones.**
1. Reimplementar la extracción en Rust.
2. Bindings a Python + yt-dlp embebido.
3. Binario sidecar invocado como proceso, con salida JSON.

**Decisión.** **Opción 3.**

**Razones.** YouTube cambia su ofuscación cada pocas semanas; yt-dlp publica
correcciones en días. Reimplementarlo sería una carrera perdida a tiempo
completo. Embeber Python añade ~40 MB y un intérprete completo al bundle.
Un proceso hijo aísla los fallos (un crash de yt-dlp no tumba la app), permite
actualizar el extractor sin publicar versión nueva de Localify, y su
`--dump-json` / `--progress-template` da una interfaz estable y parseable.

**Consecuencias.** Dependencia de un binario externo. Se mitiga con:
verificación de presencia y versión al arrancar, auto-actualización, y un
mensaje accionable si falta. `ffmpeg` sigue el mismo patrón.

---

## ADR-007 — Reproducción progresiva con `GrowingFileSource`

**Contexto.** *"Comenzar la descarga, crear archivo temporal, comenzar a
reproducir dicho archivo, continuar descargando en segundo plano."*

**Opciones.**
1. Esperar a la descarga completa.
2. Esperar a un buffer de N segundos y luego reproducir el fichero normal.
3. `MediaSource` personalizado que bloquea al llegar al final del fichero y
   espera más bytes.
4. Pipe de yt-dlp a memoria.

**Decisión.** **Opción 3.**

**Razones.** (1) rompe la UX exigida. (2) es frágil: symphonia devolvería EOF
en cuanto el decodificador se adelante al descargador, y habría que reabrir el
fichero. (4) impide hacer seek hacia atrás y no deja fichero para reutilizar.
(3) es la solución correcta y encaja en el punto de extensión que symphonia
ofrece: implementar `MediaSource` con un `read` que, ante EOF prematuro, espera
en un `Condvar` a que el descargador señale nuevos bytes.

**Consecuencias.** Requiere que el contenedor sea decodificable en streaming.
WebM/Matroska lo es por diseño (clusters secuenciales), y es el contenedor de
Opus en YouTube, así que la preferencia de formato y esta decisión se refuerzan
mutuamente. Para m4a se comprueba que el `moov` esté al principio; si no, se
espera a la descarga completa (degradación silenciosa y correcta).

**Detalle crítico en Windows.** El fichero se abre con `FILE_SHARE_DELETE` para
que el rename final funcione con el handle abierto. Sin eso, la finalización
fallaría siempre.

---

## ADR-008 — Actores para el estado concurrente

**Contexto.** `Playback`, `Queue` y `Download` tienen estado mutable con
invariantes temporales, accedido desde comandos IPC, el motor de audio y tareas
de fondo.

**Opciones.**
1. `Arc<Mutex<State>>`.
2. `Arc<RwLock<State>>`.
3. Actores con `mpsc` + `oneshot`.

**Decisión.** **Opción 3.**

**Razones.** Con locks, cualquier operación que necesite tocar cola y
reproducción a la vez introduce riesgo de deadlock por orden de adquisición, y
el estado puede observarse a medio actualizar. Con actor, el estado es de un
único propietario, las transiciones se serializan naturalmente, el actor puede
hacer trabajo entre mensajes (precargar la siguiente pista) y los tests son
deterministas: se le envían mensajes y se comprueban las respuestas.

**Consecuencias.** Más código de andamiaje (enum de comandos + handle + impl
del trait). Se acota con una macro interna para los casos petición/respuesta.
Regla estricta: **un actor nunca espera a otro actor dentro de su bucle
principal** — esas esperas se delegan a tareas hijas.

---

## ADR-009 — Claves fraccionarias para ordenar playlists

**Contexto.** Drag & drop en playlists de miles de pistas.

**Opciones.**
1. `position INTEGER` con renumeración.
2. Lista enlazada (`prev_id`/`next_id`).
3. `position REAL` con punto medio.

**Decisión.** **Opción 3.**

**Razones.** (1) implica actualizar hasta N filas por cada arrastre: en una
playlist de 5 000 pistas es inaceptable. (2) hace que leer la playlist en orden
requiera recorrer la lista, imposible de paginar con SQL. (3) reordena con un
`UPDATE` de una sola fila y mantiene `ORDER BY position` trivial e indexable.

**Consecuencias.** Los `f64` pierden precisión tras ~50 inserciones sucesivas
en el mismo hueco. Se mitiga con un rebalanceo en segundo plano cuando la
separación mínima baja de `1e-6`, operación que ocurre rarísimamente y no es
visible.

---

## ADR-010 — Eventos con IDs, no con estado

**Contexto.** Hay que mantener la UI sincronizada con el backend.

**Opciones.**
1. Cada evento lleva el estado completo afectado.
2. Cada evento lleva IDs y el consumidor consulta.
3. Sincronización total periódica.

**Decisión.** **Opción 2.**

**Razones.** (1) satura el puente IPC (todo se serializa a JSON) y duplica la
fuente de verdad. (3) desperdicia trabajo y añade latencia. (2) mantiene los
eventos diminutos y, sobre todo, hace que **perder un evento sea inofensivo**:
el consumidor consulta el estado real cuando lo necesita.

**Consecuencias.** Una llamada adicional tras algunos eventos. Es despreciable
(IPC local, consultas indexadas) y a cambio obtenemos el manejo de `Lagged` →
`resync`, que hace la UI robusta ante ráfagas de eventos.

---

## ADR-011 — Denormalizar `artist_display` en `tracks`

**Contexto.** Toda lista de pistas necesita mostrar "Queen, David Bowie".

**Opciones.**
1. `JOIN` + `GROUP_CONCAT` sobre `track_artists` en cada consulta.
2. Vista materializada.
3. Columna denormalizada mantenida en escritura.

**Decisión.** **Opción 3.**

**Razones.** El agregado por fila multiplica el coste de la consulta más
frecuente de toda la app. SQLite no tiene vistas materializadas nativas. La
columna denormalizada convierte la consulta de lista en un `SELECT` plano sobre
una tabla, que es lo que permite el objetivo de 60 fps con 50 000 filas.

**Consecuencias.** Riesgo de desincronización, acotado porque **solo hay un
camino de escritura** (`MetadataService`, dentro de la misma transacción que
escribe `track_artists`). La relación normalizada se conserva íntegra para
consultas por artista; la columna es solo una caché de presentación.

---

## ADR-012 — El backend no traduce

**Contexto.** Español e inglés, con cambio en caliente.

**Opciones.**
1. El backend devuelve mensajes ya traducidos.
2. El backend devuelve claves i18n y parámetros; el frontend traduce.
3. Catálogos duplicados en ambos lados.

**Decisión.** **Opción 2.**

**Razones.** Traducir es presentación, y el backend debe ser agnóstico de
presentación para que la API sirva a otros frontends (requisito explícito del
prompt). Con claves, cambiar de idioma no requiere consultar de nuevo al
backend ni reiniciar nada.

**Consecuencias.** Hay que mantener sincronizadas las claves emitidas por Rust
con los catálogos JSON. Se cubre con un test que extrae todas las claves
literales del código Rust y verifica que existen en `es.json` y `en.json`.

---

## ADR-013 — Un solo crate con código específico de SO

**Contexto.** SMTC y la thumbnail toolbar son Win32 puro, pero el proyecto debe
poder portarse.

**Opciones.**
1. `#[cfg(windows)]` repartido por donde haga falta.
2. Todo el código de plataforma tras traits en `localify-platform`.
3. Depender de un crate multiplataforma de terceros (p. ej. `souvlaki`).

**Decisión.** **Opción 2.**

**Razones.** (1) esparce condicionales por toda la base de código y hace que
portar sea una excavación. (3) añade una dependencia que cubre solo parte de lo
necesario (no cubre la thumbnail toolbar) y limita el control sobre la portada.
(2) concentra todo `unsafe` y todo `#[cfg]` en un único crate con superficie
pública mínima: portar a Linux es escribir `linux/mpris.rs` y nada más.

**Consecuencias.** Una capa de indirección extra en operaciones que ocurren
como mucho una vez por canción. Coste nulo en la práctica.

---

## ADR-014 — Tipos TypeScript generados, no escritos

**Contexto.** Los DTOs cruzan la frontera Rust ↔ TS y deben mantenerse
sincronizados.

**Opciones.**
1. Escribir las interfaces TS a mano.
2. Generarlas con `ts-rs` desde los DTOs de Rust.
3. Definir un esquema (OpenAPI/JSON Schema) y generar ambos lados.

**Decisión.** **Opción 2.**

**Razones.** (1) se desincroniza el primer día que alguien añade un campo, y el
fallo aparece en tiempo de ejecución. (3) añade una fuente de verdad extra y
herramienta de generación para Rust, que es innecesaria cuando Rust ya es la
autoridad. (2) hace que Rust sea la única fuente de verdad de los tipos que
cruzan la frontera IPC.

**Consecuencias.** Un paso de generación en el build. La CI regenera y exige
diff vacío, de modo que es imposible mezclar un cambio que desincronice los
tipos.

**Matiz introducido por [ADR-019](#adr-019--sin-nodejs-sin-bundler-typescript-transpilado-desde-rust).**
Como el build de producción no ejecuta `tsc`, un desajuste de tipos no rompe la
compilación: lo detecta el job opcional de `tsc --noEmit` en CI. Sigue siendo
detección automática y temprana, pero es un aviso de CI y no un error de
compilación. Es el precio de eliminar Node del camino crítico, y se considera
aceptable porque el frontend es deliberadamente delgado.

---

## ADR-015 — El motor de audio no conoce la cola

**Contexto.** ¿Quién decide cuándo empieza el crossfade?

**Opciones.**
1. El motor gestiona la cola y encadena solo.
2. El motor solo mezcla voces; `PlaybackService` decide.

**Decisión.** **Opción 2** (separación mecanismo/política).

**Razones.** La lógica de "qué suena después" depende de shuffle, repetición,
cola de usuario, contexto y disponibilidad de descarga: es negocio puro, y no
puede vivir en un componente cuyo código corre parcialmente en un hilo de
tiempo real. El motor expone `load`, `crossfade_to` y eventos; el servicio
orquesta.

**Consecuencias.** El servicio necesita saber cuándo se acerca el final
(`ApproachingEnd`) y actuar a tiempo. Es un evento más y un `select!` en el
actor. A cambio, la lógica de cola es testeable sin tarjeta de sonido.

---

## ADR-016 — Sin cancelación de descargas

**Contexto.** El prompt lo dice literalmente: *"Si el usuario cambia de canción,
la descarga anterior NO se cancela. No existe pausa. No existe cancelación."*

**Decisión.** El trait `DownloadService` **no expone** ningún método de
cancelación o pausa. No es una funcionalidad pendiente: no existe en el diseño.

**Razones.** Codificar la regla en el tipo, y no en un comentario, hace
imposible violarla por accidente más adelante. Además simplifica el actor: un
job solo termina completándose o fallando, lo que elimina toda una clase de
estados intermedios y de ficheros a medio escribir.

**Consecuencias.** Reproducir muchas canciones seguidas genera descargas
concurrentes. Se acota con el límite de concurrencia por carril (2 + 2) y una
cola FIFO; nada se descarta, solo se ordena. El consumo de red se mantiene
razonable y ninguna descarga se pierde.

---

## ADR-017 — Confianza baja no descarga

**Contexto.** El scorer puede no encontrar una coincidencia fiable.

**Opciones.**
1. Descargar el mejor candidato pase lo que pase.
2. No descargar y marcar la pista como fallida.
3. Preguntar al usuario cuál es el correcto.

**Decisión.** **Opción 2**, con la (3) disponible como acción manual opcional
desde el menú contextual.

**Razones.** (1) contamina la biblioteca con karaokes y covers, y una vez
descargado el fichero **nunca se vuelve a descargar** (invariante del prompt),
así que el error queda grabado de forma permanente. (3) como comportamiento por
defecto rompe la transparencia exigida.

**Consecuencias.** Algunas pistas de nicho no se descargarán automáticamente.
Es el compromiso correcto: una biblioteca más pequeña pero limpia vale más que
una grande con basura. La UI lo indica de forma discreta en la fila, y ofrece
"elegir versión manualmente" a quien lo quiera.

---

## ADR-018 — Rutas relativas en la base de datos

**Contexto.** El usuario puede cambiar la carpeta de biblioteca.

**Decisión.** `audio_files.rel_path` es relativa a la raíz de la biblioteca. La
raíz vive en `settings.json`.

**Razones.** Cambiar la carpeta pasa a ser mover ficheros y actualizar **un
único valor**, en lugar de reescribir 50 000 filas dentro de una transacción
que podría fallar a mitad. Como efecto secundario, la base de datos se vuelve
portable entre máquinas y perfiles de usuario.

**Consecuencias.** Toda resolución de ruta pasa por un único helper que
concatena raíz + relativa. Ninguna ruta absoluta se persiste jamás.

---

## ADR-020 — Remuxear a Ogg tras la descarga

**Contexto.** YouTube sirve el audio Opus dentro de un contenedor **WebM**, no
Ogg. El diseño original guardaba ese fichero con extensión `.opus`, lo que es
sencillamente falso: un `.opus` de verdad es Ogg.

Se descubrió al integrar el etiquetado, y trae dos problemas concretos: los
reproductores que confían en la extensión fallan al abrirlo, y las etiquetas de
Matroska tienen soporte pobre —mientras que los Vorbis comments de Ogg admiten
claves arbitrarias, que es justo lo que necesita `LOCALIFY_SPOTIFY_ID`.

**Opciones.**
1. Guardar como `.webm`. Honesto, pero deja el problema de las etiquetas.
2. Recodificar a otro formato. Descartado de plano: perdería calidad.
3. **Remuxear** WebM → Ogg con `ffmpeg -c copy`.

**Decisión.** **Opción 3.**

**Razones.** Un remux cambia el envoltorio sin tocar un solo bit del audio
codificado. Cuesta milisegundos, no degrada nada, y produce un `.opus`
auténtico con etiquetado completo. Es exactamente lo que quería decir el diseño
con "FFmpeg se usa para remuxear e inspeccionar, no para recodificar"; lo que
faltaba era aplicarlo.

**Consecuencias.** El pipeline de finalización gana un paso entre la descarga y
el etiquetado. El fichero original **no se borra hasta después de verificar el
remuxeado**, para que un fallo no deje la descarga sin nada. La reproducción
progresiva sigue ocurriendo sobre el `.part` en WebM, que también es de flujo:
el remux solo afecta al fichero definitivo.

---

## ADR-021 — La identidad de un fichero se recupera por dos vías

**Contexto.** `rescan` necesita saber a qué pista corresponde cada fichero. El
diseño confiaba en una etiqueta `LOCALIFY_SPOTIFY_ID`.

Al implementarlo apareció una limitación: **ID3v2 no admite claves
personalizadas a través de la API genérica de `lofty`**, que las descarta porque
un identificador de marco ID3v2 tiene exactamente cuatro caracteres. Afecta a
los MP3, que llegan de ficheros importados por el usuario.

**Opciones.**
1. Escribir marcos `TXXX` de ID3v2 a mano, saltándose la API genérica.
2. Usar una etiqueta estándar existente con otro significado.
3. Añadir el **nombre del fichero** como segunda vía.

**Decisión.** **Opción 3**, manteniendo la etiqueta donde el formato la admite.

**Razones.** El nombre ya contiene el identificador por construcción
(`audio/<shard>/<track_id>.<ext>`), así que la información está ahí sin coste.
La opción 1 ataría el código a los detalles internos de un formato para un caso
secundario; la 2 corrompería el significado de un campo estándar y confundiría a
otros reproductores.

Las dos vías se complementan: el nombre falla si el usuario renombra el fichero,
y ahí es donde la etiqueta sigue funcionando (en Ogg y MP4, que son los formatos
que produce Localify).

**Consecuencias.** Un MP3 importado y renombrado fuera de la aplicación pierde
su identidad y se trata como fichero ajeno. Es el comportamiento correcto:
adivinar por similitud de texto se equivocaría con las reediciones, y una
asociación errónea es peor que ninguna.

---

## ADR-022 — La cola no es un actor, y el compilador lo vigila

**Contexto.** ADR-008 establece que el estado mutable con invariantes temporales
va en actores. `QueueService` tiene ese estado, pero al implementarlo quedó
claro que no encaja del todo: todas sus operaciones son inmediatas —mover
elementos de un `VecDeque`, recolocar un índice— y **ninguna llama a otro
servicio**. El andamiaje de un canal y un bucle no compraba nada.

**Opciones.**
1. Actor completo, por coherencia con los demás.
2. `tokio::sync::Mutex`, que es lo habitual en código asíncrono.
3. `std::sync::Mutex`, con el estado protegido de forma síncrona.

**Decisión.** **Opción 3.**

**Razones.** La opción 1 añade latencia y código para un estado que nadie
retiene más de unos microsegundos. Entre las otras dos, la clave es qué pasa si
alguien, más adelante, añade una consulta a la base de datos dentro de una
sección crítica: con `tokio::sync::Mutex` compila y funciona, y frena a todos
los demás sin que nada lo delate. Con `std::sync::Mutex` **no compila**, porque
su guardia no es `Send` y el `Future` deja de serlo.

Es la diferencia entre una regla escrita en un comentario y una que el
compilador impone. La primera versión de este fichero sí consultaba la base de
datos con el estado bloqueado; el cambio de tipo lo habría impedido desde el
principio.

**Consecuencias.** Ninguna operación de la cola puede volverse asíncrona sin
reestructurarla antes. Es exactamente la fricción que se busca: obliga a
pensarlo en vez de deslizarlo.

---

## ADR-023 — El contrato de tiempo real se verifica, no se promete

**Contexto.** "El callback de audio no asigna memoria" es la invariante que
separa una reproducción limpia de una con chasquidos ocasionales. También es
trivial de romper sin enterarse: un `Vec` que crece, un `format!` en una traza,
un `Box` en un camino de error que casi nunca se toma.

**El problema.** Cuando se rompe, **no falla nada**. El audio sale bien. Solo
se degrada bajo carga, de forma intermitente, y el síntoma no apunta a la
causa.

**Decisión.** Un allocator global instrumentado en un fichero de tests
(`localify-audio/tests/tiempo_real.rs`) que cuenta asignaciones **por hilo** y
se arma justo alrededor de la llamada al mezclador.

**Razones.** Es la única forma de convertir la invariante en algo que la CI
comprueba. Un perfilador la mediría una vez; esto la mide en cada `cargo test`,
y señala qué operación la rompió.

El contador es por hilo porque el resto de la suite corre en paralelo. Y el
primer test del fichero comprueba que el contador **detecta una asignación
deliberada**: sin él, un instrumento roto haría pasar todos los demás.

**Consecuencias.** El fichero necesita `unsafe` para implementar `GlobalAlloc`,
y es el único sitio de `localify-audio` donde aparece —el crate en sí no tiene
ninguno, pese a que la arquitectura se lo permitía—. Los casos cubiertos son el
normal, el fundido, el ecualizador activo, la conversión a mono y el underrun,
que es el más propenso a llevar un camino de error que asigne.

---

## Resumen

| ADR | Decisión | Coste asumido |
|---|---|---|
| 001 | Vanilla TS | Router y store propios |
| 002 | Motor de audio en Rust | La fase más larga y arriesgada |
| 003 | Opus vía libopus en symphonia | Una dependencia C aislada |
| 004 | rusqlite | SQL y mappers a mano |
| 005 | Credenciales de Spotify del usuario | Fricción en el primer arranque |
| 006 | yt-dlp sidecar | Dependencia de binario externo |
| 007 | GrowingFileSource | Complejidad en la capa de I/O |
| 008 | Actores | Andamiaje de mensajes |
| 009 | Claves fraccionarias | Rebalanceo ocasional |
| 010 | Eventos con IDs | Una consulta extra tras el evento |
| 011 | `artist_display` denormalizado | Un camino de escritura único obligatorio |
| 012 | Backend sin traducciones | Test de sincronía de claves |
| 013 | Plataforma en un solo crate | Una indirección irrelevante |
| 014 | Tipos TS generados | Un paso de build |
| 015 | Motor sin conocimiento de cola | Un evento más de coordinación |
| 016 | Sin cancelación | Descargas concurrentes acotadas |
| 017 | Confianza baja no descarga | Menos cobertura automática en música de nicho |
| 018 | Rutas relativas | Un helper obligatorio de resolución |
| 019 | Sin Node ni bundler; oxc en `build.rs` | El chequeo de tipos pasa a ser un job de CI, no un error de build |
| 020 | Remuxear WebM→Ogg tras descargar | Un paso más en la finalización, de milisegundos |
| 021 | Identidad por nombre de fichero además de por etiqueta | Renombrar un fichero fuera de la app pierde una de las dos vías |
| 022 | La cola usa `std::sync::Mutex`, no un actor | Ninguna operación de la cola puede volverse asíncrona sin reestructurarla |
| 023 | Allocator instrumentado para el hilo de audio | Un fichero de tests con `unsafe` |

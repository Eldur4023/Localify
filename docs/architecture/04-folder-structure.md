# 04 — Estructura de carpetas

Workspace de Cargo con 9 crates + un frontend Vite. La estructura refleja la
arquitectura: si un módulo no encaja en una carpeta existente, es señal de que
la arquitectura necesita revisión, no de que haga falta una carpeta nueva.

```
Localify/
├── Cargo.toml                      # workspace: members, deps compartidas, perfiles
├── Cargo.lock
├── rust-toolchain.toml             # fija la versión estable de Rust
├── rustfmt.toml
├── clippy.toml
├── .editorconfig
├── .gitignore
├── LICENSE                         # GPL-3.0
├── README.md
├── CONTRIBUTING.md
├── CHANGELOG.md
│
├── docs/
│   ├── architecture/
│   │   ├── 01-overview.md
│   │   ├── 02-modules.md
│   │   ├── 03-communication.md
│   │   ├── 04-folder-structure.md
│   │   ├── 05-database.md
│   │   ├── 06-api.md
│   │   ├── 07-roadmap.md
│   │   └── 08-decisions.md
│   ├── adr/                        # ADRs posteriores a la Fase 1
│   └── dev/
│       ├── setup.md
│       ├── testing.md
│       └── release.md
│
├── crates/
│   │
│   ├── localify-core/              # ── DOMINIO. Sin dependencias del workspace.
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── domain/
│   │       │   ├── mod.rs
│   │       │   ├── ids.rs          # TrackId, AlbumId, ArtistId, PlaylistId
│   │       │   ├── track.rs
│   │       │   ├── album.rs
│   │       │   ├── artist.rs
│   │       │   ├── playlist.rs
│   │       │   ├── availability.rs
│   │       │   ├── audio.rs        # AudioFormat, Volume, EqProfile, Quality
│   │       │   ├── queue.rs        # PlaybackContext, RepeatMode, QueueEntry
│   │       │   ├── settings.rs
│   │       │   └── lyrics.rs
│   │       ├── ports/              # ── TODOS los traits públicos viven aquí
│   │       │   ├── mod.rs
│   │       │   ├── database.rs
│   │       │   ├── metadata_provider.rs
│   │       │   ├── youtube.rs
│   │       │   ├── audio_engine.rs
│   │       │   ├── platform.rs
│   │       │   ├── clock.rs        # inyectable → tests deterministas
│   │       │   └── services.rs     # los 14 traits de servicio
│   │       ├── events.rs           # DomainEvent + EventBus
│   │       ├── error.rs            # CoreError
│   │       ├── page.rs             # Page<T>, keyset cursors
│   │       └── text.rs             # normalización canónica (usada por metadata Y matcher)
│   │
│   ├── localify-db/                # ── SQLite
│   │   ├── Cargo.toml
│   │   ├── migrations/
│   │   │   ├── V1__initial_schema.sql
│   │   │   ├── V2__fts_index.sql
│   │   │   └── V3__playback_state.sql
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── pool.rs             # pool de lectura + escritor único
│   │       ├── pragmas.rs
│   │       ├── migrations.rs       # refinery
│   │       ├── mappers.rs          # Row → entidad de dominio
│   │       └── repositories/
│   │           ├── mod.rs
│   │           ├── tracks.rs
│   │           ├── albums.rs
│   │           ├── artists.rs
│   │           ├── playlists.rs
│   │           ├── audio_files.rs
│   │           ├── favorites.rs
│   │           ├── history.rs
│   │           ├── youtube_matches.rs
│   │           ├── settings.rs
│   │           ├── cache.rs
│   │           ├── player_state.rs
│   │           └── search_fts.rs
│   │
│   ├── localify-spotify/
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── client.rs           # reqwest + reintentos + rate limit
│   │       ├── auth.rs             # client credentials + refresco de token
│   │       ├── rate_limit.rs
│   │       ├── models.rs           # DTOs crudos de la API
│   │       ├── mapper.rs           # DTO crudo → entidad de dominio
│   │       └── endpoints/
│   │           ├── search.rs
│   │           ├── tracks.rs
│   │           ├── albums.rs
│   │           ├── artists.rs
│   │           └── playlists.rs
│   │
│   ├── localify-ytdlp/
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── sidecar.rs          # localizar/actualizar/ejecutar el binario
│   │       ├── search.rs           # ytsearch + YouTube Music, salida JSON
│   │       ├── scoring/
│   │       │   ├── mod.rs
│   │       │   ├── rules.rs        # tabla de pesos y penalizaciones — datos, no código
│   │       │   ├── duration.rs
│   │       │   ├── source.rs
│   │       │   ├── text.rs
│   │       │   └── breakdown.rs
│   │       ├── download.rs         # spawn, progreso por stdout, .part
│   │       ├── formats.rs          # selección de formato (opus > m4a > resto)
│   │       ├── verify.rs           # integridad post-descarga
│   │       └── ffmpeg.rs           # remux e inspección (nunca recodifica)
│   │
│   ├── localify-audio/
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── engine.rs           # implementación de AudioEngine
│   │       ├── device.rs           # cpal, enumeración, reconexión
│   │       ├── mixer.rs            # callback de tiempo real — sin alloc, sin locks
│   │       ├── voice.rs            # una pista en reproducción
│   │       ├── ring.rs             # ring buffers SPSC
│   │       ├── decode/
│   │       │   ├── mod.rs
│   │       │   ├── symphonia.rs    # FLAC, MP3, AAC, ALAC, Vorbis, WAV
│   │       │   ├── opus.rs         # libopus registrado en el CodecRegistry
│   │       │   └── resample.rs
│   │       ├── source/
│   │       │   ├── mod.rs
│   │       │   ├── file.rs
│   │       │   └── growing.rs      # MediaSource sobre un .part en crecimiento
│   │       ├── dsp/
│   │       │   ├── biquad.rs
│   │       │   ├── equalizer.rs
│   │       │   ├── crossfade.rs    # rampas equal-power
│   │       │   └── limiter.rs
│   │       └── error.rs
│   │
│   ├── localify-platform/          # ── ÚNICO crate con código específico de SO
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── paths.rs            # directorios de datos/config/caché
│   │       ├── secrets.rs          # DPAPI en Windows; keyring en otros
│   │       ├── single_instance.rs
│   │       ├── windows/
│   │       │   ├── mod.rs
│   │       │   ├── smtc.rs         # SystemMediaTransportControls
│   │       │   ├── taskbar.rs      # ITaskbarList3: botones + progreso
│   │       │   └── dpapi.rs
│   │       └── stub/               # no-op para Linux/macOS hasta portar
│   │           └── mod.rs
│   │
│   ├── localify-integrations/
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── discord.rs
│   │       └── lyrics/
│   │           ├── mod.rs
│   │           ├── lrclib.rs
│   │           └── composite.rs
│   │
│   ├── localify-services/          # ── LÓGICA DE NEGOCIO. Solo depende de core.
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── settings.rs
│   │       ├── metadata.rs
│   │       ├── search.rs
│   │       ├── library.rs
│   │       ├── playlist.rs
│   │       ├── recommendation/
│   │       │   ├── mod.rs
│   │       │   ├── vectors.rs      # vector disperso por pista
│   │       │   ├── similarity.rs   # coseno
│   │       │   └── home.rs         # secciones de Inicio
│   │       ├── cache.rs
│   │       ├── notification.rs
│   │       ├── lyrics.rs
│   │       └── actors/
│   │           ├── mod.rs
│   │           ├── playback.rs
│   │           ├── queue.rs
│   │           └── download.rs
│   │
│   └── localify-app/               # ── BINARIO TAURI. El único ensamblador.
│       ├── Cargo.toml
│       ├── build.rs                # tauri-build + transpila frontend/src → frontend/dist (oxc)
│       ├── frontend_build.rs       # borrado de tipos TS y copia de assets
│       ├── tauri.conf.json
│       ├── capabilities/
│       │   └── default.json        # permisos Tauri v2 — mínimos necesarios
│       ├── icons/
│       ├── binaries/               # sidecars empaquetados
│       │   ├── yt-dlp-x86_64-pc-windows-msvc.exe
│       │   └── ffmpeg-x86_64-pc-windows-msvc.exe
│       └── src/
│           ├── main.rs
│           ├── bootstrap.rs        # arranque por etapas
│           ├── context.rs          # AppContext — el ÚNICO sitio con tipos concretos
│           ├── logging.rs
│           ├── bridge.rs           # bus de eventos → Tauri emit
│           ├── error.rs            # CoreError → ApiError
│           ├── dto/
│           │   ├── mod.rs
│           │   ├── track.rs
│           │   ├── album.rs
│           │   ├── artist.rs
│           │   ├── playlist.rs
│           │   ├── player.rs
│           │   ├── search.rs
│           │   ├── settings.rs
│           │   └── page.rs
│           └── commands/
│               ├── mod.rs          # registro del handler
│               ├── player.rs
│               ├── queue.rs
│               ├── library.rs
│               ├── search.rs
│               ├── playlist.rs
│               ├── album.rs
│               ├── artist.rs
│               ├── recommendation.rs
│               ├── lyrics.rs
│               ├── settings.rs
│               ├── integrations.rs
│               └── diagnostics.rs
│
├── frontend/                       # sin Node, sin bundler — ver ADR-019
│   ├── tsconfig.json               # solo para `tsc --noEmit` opcional
│   ├── index.html
│   ├── dist/                       # generado por localify-app/build.rs (git-ignored)
│   ├── public/
│   │   └── fonts/
│   └── src/
│       ├── main.ts                 # arranque + router
│       ├── ipc/
│       │   ├── client.ts           # ÚNICO lugar que llama a invoke()
│       │   ├── events.ts           # listener + resync
│       │   └── types.gen.ts        # generado desde Rust con ts-rs
│       ├── store/
│       │   ├── index.ts
│       │   ├── player.ts
│       │   ├── queue.ts
│       │   ├── library.ts
│       │   └── settings.ts
│       ├── router/
│       │   └── index.ts            # router hash, ~80 líneas
│       ├── views/
│       │   ├── home.ts
│       │   ├── search.ts
│       │   ├── library.ts
│       │   ├── album.ts
│       │   ├── artist.ts
│       │   ├── playlist.ts
│       │   ├── liked.ts
│       │   └── settings.ts
│       ├── components/
│       │   ├── player-bar.ts
│       │   ├── sidebar.ts
│       │   ├── topbar.ts
│       │   ├── track-list/
│       │   │   ├── index.ts
│       │   │   ├── virtual-list.ts # virtualización con reciclado de nodos
│       │   │   └── row.ts
│       │   ├── card.ts
│       │   ├── context-menu.ts
│       │   ├── queue-panel.ts
│       │   ├── lyrics-panel.ts
│       │   ├── now-playing.ts
│       │   ├── slider.ts
│       │   ├── toast.ts
│       │   └── skeleton.ts
│       ├── dnd/
│       │   └── sortable.ts         # drag & drop de playlists
│       ├── i18n/
│       │   ├── index.ts
│       │   ├── es.json
│       │   └── en.json
│       ├── styles/
│       │   ├── reset.css
│       │   ├── tokens.css          # variables de diseño (colores, espaciado, tipografía)
│       │   ├── base.css
│       │   ├── layout.css
│       │   ├── components/
│       │   └── animations.css
│       ├── icons/
│       │   └── index.ts            # sprite SVG inline
│       └── utils/
│           ├── format.ts           # duraciones, fechas
│           ├── dom.ts
│           └── raf.ts              # throttle a rAF
│
├── tests/
│   ├── integration/                # end-to-end sobre el AppContext, sin UI
│   │   ├── search_flow.rs
│   │   ├── download_flow.rs
│   │   ├── playback_flow.rs
│   │   └── playlist_flow.rs
│   └── fixtures/
│       ├── spotify/                # respuestas JSON grabadas
│       ├── ytdlp/                  # salidas JSON de búsqueda para el scorer
│       └── audio/                  # muestras cortas de cada formato
│
├── scripts/
│   ├── fetch-sidecars.ps1          # descarga yt-dlp y ffmpeg firmados
│   ├── gen-types.ps1               # ts-rs: Rust → TypeScript
│   └── dev.ps1
│
└── .github/
    └── workflows/
        ├── ci.yml                  # fmt · clippy -D warnings · test · tsc
        └── release.yml             # build firmado + instalador
```

---

## Convenciones

**Rust**
- Un archivo por concepto; si un archivo pasa de ~400 líneas, se divide.
- `mod.rs` solo reexporta y documenta; no contiene lógica.
- Los traits van en `core/ports`, **nunca** junto a su implementación. Esa
  separación física es lo que impide que alguien importe el concreto por
  descuido.
- Los tests unitarios van en `#[cfg(test)] mod tests` en el mismo archivo. Los
  de integración, en `tests/`.
- `unsafe` solo se permite en `localify-platform` (FFI de Win32) y en
  `localify-audio/ring.rs`, siempre con un comentario `// SAFETY:` que
  justifique la invariante.

**TypeScript**
- Sin `any`. `strict: true`, `noUncheckedIndexedAccess: true`.
- Los tipos del backend se **generan**, no se escriben a mano: `ts-rs` deriva
  `types.gen.ts` desde los DTOs de Rust.
- **Los `import` llevan extensión `.js`** aunque el fichero sea `.ts`. Es un
  requisito de los módulos ES nativos (ADR-019), no una rareza: el navegador
  resuelve la ruta ya transpilada.
- No hay bundler: cada módulo se sirve tal cual. Evita ciclos de importación,
  porque aquí sí se notan.
- Un componente = un archivo = una función `create*` que devuelve
  `{ el: HTMLElement, update(), destroy() }`. Sin clases, sin herencia.

**CSS**
- Tokens de diseño en `tokens.css`; ningún valor de color o espaciado se
  escribe literal en otro sitio.
- Nomenclatura BEM ligera: `.track-row`, `.track-row__title`,
  `.track-row--playing`.
- Sin preprocesador: CSS nativo con `@layer` y variables.

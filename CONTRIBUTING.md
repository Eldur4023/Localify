# Contributing

> The architecture documents under [`docs/architecture/`](docs/architecture/)
> and every comment in the source are written in **Spanish**. This file is in
> English to match the README; everything it points at is not.

---

## The shape of it

Hexagonal architecture over eleven crates. The domain knows nothing about
SQLite, Tauri or YouTube.

```
localify-core          domain types + ports (traits). Depends on nothing.
localify-services      the logic. Depends on core only.
localify-db            SQLite adapters
localify-audio         cpal + symphonia engine
localify-ytdlp         audio acquisition, matching, tagging
localify-ytmusic       InnerTube catalogue
localify-musicbrainz   MusicBrainz catalogue + Cover Art Archive
localify-spotify       Spotify catalogue + public playlist reader
localify-integrations  Discord, Last.fm, lyrics, image fetching
localify-platform      paths, filesystem, secret store, browser
localify-app           Tauri commands, DTOs, wiring
```

**Exactly one file decides which implementation sits behind each port:**
[`crates/localify-app/src/context.rs`](crates/localify-app/src/context.rs). If
you find yourself naming a concrete infrastructure type anywhere else, that's
the bug.

Start with [01 — Overview](docs/architecture/01-overview.md) and
[08 — Decisions](docs/architecture/08-decisions.md); the ADRs carry the
alternatives that were considered and what each one would have cost.

---

## Building

```powershell
cargo build --workspace          # backend + frontend (build.rs transpiles the TS)
cargo test --workspace           # also regenerates frontend/src/ipc/types.gen.ts
cargo clippy --workspace --all-targets
cargo fmt --all
```

Rust stable ≥ 1.85. Node is **optional** and only used for `tsc --noEmit`.

### The frontend has no Node and no bundler

TypeScript is transpiled from Rust with oxc and served as native ES modules
([ADR-019](docs/architecture/08-decisions.md)). Two consequences you need to
hold in your head:

- **Types are erased, not checked.** oxc strips them. `build.rs` runs an
  `oxc_semantic` pass that catches unresolved references — a typo'd import, a
  function that doesn't exist — but it cannot catch a wrong property or a wrong
  shape. `api.wipeDownloads` when the method lives on `library` compiles, ships,
  and fails at runtime. Run `tsc --noEmit` if you have Node.
- `frontend/src/ipc/types.gen.ts` **is committed** even though it's generated
  ([ADR-014](docs/architecture/08-decisions.md)). It's what gives the frontend
  types without running `cargo test` first, and what makes a change to a DTO
  show up as a reviewable diff. `cargo test --workspace` regenerates it; if it
  comes back dirty, commit it.

---

## Invariants

These are not style preferences. Breaking one is a bug even if everything
compiles and every test passes.

**Downloads are invisible.** No download button, no progress bar, no queue the
user manages. Pressing play plays. Anything that makes the user aware there was
a download is wrong — including recommending only songs that happen to be on
disk already.

**Audio paths never cross the IPC bridge.** They're stored relative to the
library folder ([ADR-018](docs/architecture/08-decisions.md)) and resolved in
Rust. Covers are addressed by identifier through the `cover://` scheme.

**Secrets go to the Windows secret store (DPAPI).** Never SQLite, never the
bridge, never the logs. There's a test that checks the last part.

**The backend emits i18n keys; the frontend translates them**
([ADR-012](docs/architecture/08-decisions.md)). `tests/i18n.rs` enforces
`es`/`en` parity and matching `{params}`. No user-facing English or Spanish in
Rust.

**Never write a file in place.** Temp file, verify, atomic rename. A power cut
mid-write must not corrupt a library.

**The audio thread allocates nothing, blocks on nothing and logs nothing.**
`Mezclador::rellenar` runs in a real-time callback.

**Actors own their state.** Playback, downloads and the queue are actors
(`mpsc` + `oneshot`), and their loops never await something slow — that goes to
a child task that reports back through the same channel
([ADR-008](docs/architecture/08-decisions.md)).

---

## Traps this codebase has actually fallen into

Every one of these compiled, passed the suite, and shipped.

**Trait default methods plus a delegating wrapper.** `ProveedorConmutable` wraps
four providers. `MetadataProvider::resolve_recording` has a default body
returning `Ok(None)`. Forget to delegate it and the wrapper silently answers
"nothing" for a catalogue it never asked. No error, no log line, no failing
test. If you add a method with a default body, delegate it in every wrapper and
write a test with a fake that returns its own name.

**`.ok()` on a parse.** A `#[serde(alias = "channel")]` on a field named
`channel` made *every* yt-dlp result unparseable. The `.ok()` swallowing it
turned that into "no candidates found", which is indistinguishable from
"YouTube doesn't have this song". Log why you're dropping something.

**A test that passes with the bug live.** The regression test for that alias
never put both fields in the same JSON. Before you trust a test, make the fix
and check the test fails without it.

**`serde_json::json!` does not omit null keys.** `json!({"assets": maybe})` with
`None` writes `"assets": null`. Discord answers `4000 — "assets" must be an
object` and discards the whole activity. And the test asserting `is_null()`
passed, because `null` and absent read the same if you ask for the value instead
of the key.

**Reading a response and not looking at it.** That same Discord bug went
unnoticed because a rejection arrives as a normal frame, not as a write error.
`Ok(())` was returned either way.

**Zero-length crossfade isn't a crossfade.** `fundir_a(voz, 0)` replaces the
current voice immediately. The engine warns 15 seconds before a track ends so
the longest crossfade fits — so with crossfade off, "prepare the crossfade" cut
every song 15 seconds early.

**All-or-nothing batches.** One duplicate key in a 40-track insert aborted the
whole transaction and nothing persisted, which looked exactly like "the
catalogue is empty".

---

## Verifying against the real service

Undocumented APIs change and memory lies. Before assuming how one behaves, ask
it. These exist for that:

```powershell
cargo run -p localify-ytmusic     --example explorar        # raw InnerTube search
cargo run -p localify-ytdlp       --example emparejar       # what matching picks, and why
cargo run -p localify-musicbrainz --example buscar
cargo run -p localify-spotify     --example lista_publica
cargo run -p localify-db          --example emparejamientos  # tracks stuck rejecting videos
cargo run -p localify-db          --example ajustes          # what's really stored
cargo run -p localify-integrations --example discord -- <app_id>
```

The Discord one exists because from inside the app both failure modes look
identical — the profile doesn't change — and neither leaves a trace. It shows
the replies instead of discarding them.

---

## Conventions

- **Comments explain why, not what.** If a line needs a comment to say what it
  does, rewrite the line. The comments worth keeping are the ones recording a
  decision, a trade-off, or a trap.
- **Names are in Spanish** inside the crates, English at the port boundary where
  the domain vocabulary is English (`TrackRow`, `PlaybackContext`). Follow the
  surrounding file.
- **Tests name the behaviour**, not the function:
  `sin_crossfade_el_aviso_de_final_no_cambia_de_cancion`.
- **`clippy` runs with `--all-targets` and the workspace lints are strict.**
  `print_stdout`, `expect_used` and friends are denied outside examples and
  tests; examples opt out with a documented `#![allow(..., reason = "...")]`.
- **Don't use PowerShell `Get-Content`/`Set-Content` on source files.** They read
  UTF-8 as ANSI and write it back double-encoded with a BOM. It has happened.

---

## Sending a change

**There is no CI.** Nothing runs these for you, so run them yourself:

1. `cargo fmt --all && cargo clippy --workspace --all-targets && cargo test --workspace`
2. If you touched a DTO, commit the regenerated `frontend/src/ipc/types.gen.ts`.
3. If you touched user-facing text, update **both** `es.json` and `en.json` —
   the i18n test will tell you if you didn't.
4. If you fixed a bug, add the test that fails without the fix, and say in the
   test *why* it exists.

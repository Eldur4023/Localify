#pragma once
// Discord Rich Presence over Discord's own local IPC protocol (a Unix
// socket at $XDG_RUNTIME_DIR/discord-ipc-0, framed JSON-RPC-ish messages --
// no SDK, no network call, nothing Discord ships as a library on Linux).
// App-specific, like mpris.cpp: wired into the `window` module's
// WindowControl hook by runtime.cpp/dev.cpp, vendor/lux itself untouched.
//
// Every user configures their OWN Discord Application ID (Settings ->
// Discord): one is deliberately not bundled, so nobody's presence shows up
// under a stranger's app name. See app/api_ajustes.lux's
// `integrations.discordClientId`.
#include <lux_script/value.hpp>

namespace luxdesktop {

// `state` (a Dict from window.discord_update({...}) in LuxScript) carries:
//   clientId (string, "" = disabled), details (string, e.g. track title),
//   state (string, e.g. artist), playing (bool), startEpochS (int, unix
//   seconds the current track started -- powers Discord's "elapsed" timer).
// (Re)connects to Discord's IPC socket if the clientId changed or the
// previous connection dropped, sends SET_ACTIVITY, and is a total no-op
// (closes any open connection, no reconnect attempt) when clientId is "".
// Safe to call every poll tick: a closed/never-opened socket makes the
// (re)connect attempt itself the only cost, and connect() on a Unix socket
// that has no listener fails immediately, never blocks.
void discord_update(const lux_script::Value& state);

// Clears the activity and closes the IPC connection. Called on shutdown so
// Discord does not keep showing "Listening to Localify" for a process that
// already exited.
void discord_shutdown();

} // namespace luxdesktop

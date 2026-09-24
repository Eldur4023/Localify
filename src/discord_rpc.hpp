#pragma once
// Discord Rich Presence over Discord's own local IPC protocol (a Unix
// socket at $XDG_RUNTIME_DIR/discord-ipc-N, framed JSON messages -- no SDK,
// no network call). App-specific, like mpris.cpp: wired into the `window`
// module's WindowControl hook by runtime.cpp/dev.cpp.
//
// Every user configures their OWN Discord Application ID (Settings ->
// Discord): one is deliberately not bundled, so nobody's presence shows up
// under a stranger's app name.
#include <lux_script/value.hpp>

namespace luxdesktop {

// `state` (window.discord_update({...}) in LuxScript) is
//   { clientId: string ("" = integration off),
//     activity: null | { title, artist, album, coverUrl, startS, endS } }
// `activity: null` means "nothing to announce" (paused, stopped): the
// presence is cleared, same as the original Tauri integration -- a profile
// saying "listening" to something paused half an hour ago is worse than an
// empty one.
//
// This only records what SHOULD be shown; whether it goes out right now
// depends on Discord's rate limit (see discord_rpc.cpp). Call it on every
// state change AND periodically (the events poll does): a change that
// arrived inside the rate-limit window is sent by a later call, always the
// latest one, never an intermediate one.
void discord_update(const lux_script::Value& state);

// Clears the activity and closes the IPC connection. Called on shutdown so
// Discord does not keep showing a process that already exited.
void discord_shutdown();

} // namespace luxdesktop

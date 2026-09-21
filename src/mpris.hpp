#pragma once
// MPRIS (org.mpris.MediaPlayer2) media-player D-Bus service, so the
// desktop's own media widgets (GNOME Shell's media controls, KDE Plasma's
// "now playing" applet, hardware/keyboard media keys routed through the
// session's media-keys daemon) can see and control what Localify is
// playing -- the one thing a browser tab could never give this app.
//
// App-specific, NOT part of vendor/lux: it is wired into the generic
// `window` module's WindowControl::mpris_update hook the same way
// desktop_window.cpp wires up notify()/set_menu(), so vendor/lux itself
// stays untouched aside from that one hook declaration.
#include <cstdint>
#include <string>

#include <lux_script/value.hpp>

namespace luxdesktop {

// Registers org.mpris.MediaPlayer2.<app_id> on the session bus. Call once,
// after the window exists and the local HTTP server is listening (control
// actions -- Play, Pause, Next... -- are relayed to it as plain loopback
// POSTs, exactly like a native menu action or a keyboard accelerator
// already do). `display_name` is what MPRIS clients show as "Identity".
void mpris_init(const std::string& app_id, const std::string& display_name, std::uint16_t server_port);

// The WindowControl::mpris_update hook body: `state` is the Dict LuxScript
// passed to window.mpris_update({...}) -- see events.lux. Updates the
// cached track/playback properties and emits PropertiesChanged for
// whatever actually changed. Safe to call every poll tick (~1/s): a no-op
// diff costs one GVariant comparison, not a D-Bus round trip.
void mpris_update(const lux_script::Value& state);

// Unregisters the bus name. Called on shutdown so a slow-to-notice media
// widget does not keep showing a dead session as "still playing".
void mpris_shutdown();

} // namespace luxdesktop

#pragma once
#include <functional>
#include <string>

#include "value.hpp"

namespace lux_script {

// Runtime control surface for the `window` module's callable functions
// (window.minimize(), window.set_title(...), the file dialogs...).
// Registered by the desktop shell (src/runtime.cpp / src/dev.cpp, outside
// this vendored copy) once the actual native window exists -- Lux itself
// has no concept of a window. Every hook defaults to an empty
// std::function, checked before use, so `import window` compiles and a
// plain `lux --check` still works even with no hook installed.
//
// pick_file is the one BLOCKING hook: it runs on the calling thread (the
// window.open_file()/window.save_file() module functions are declared
// `is_async`, so that thread is a lux::blocking_pool() worker, never the
// window's own thread or an event-loop thread) and returns only once the
// native dialog closes -- empty string if the user cancels.
struct WindowControl {
    std::function<void(const std::string&)> set_title;
    std::function<void()>                   minimize;
    std::function<void()>                   maximize;
    std::function<void()>                   restore;
    std::function<void()>                   close;
    std::function<void()>                   fullscreen;
    std::function<void()>                   unfullscreen;
    std::function<void(bool on_top)>        set_always_on_top;
    std::function<std::string(const std::string& suggested_name, bool save_mode)> pick_file;

    // A desktop notification (freedesktop D-Bus spec, not a webview alert).
    // Synchronous, unlike pick_file: it is a local D-Bus round-trip with no
    // unbounded wait on the user, so it does not need `await` on the
    // LuxScript side or a blocking_pool worker underneath.
    std::function<void(const std::string& title, const std::string& body)> notify;

    // Native menu bar: List<Dict{label, items: List<Json>}>, each item
    // either the string "-" (separator) or Dict{label, action}. Clicking
    // an item runs `fetch(action, {method:'POST'})` inside the page --
    // menus are just another way to trigger one of the app's own routes,
    // the same "loopback HTTP is the whole IPC" idea as everything else
    // here, not a second callback mechanism into LuxScript.
    std::function<void(const Value& spec)> set_menu;

    // System tray icon. Left-click toggles the window's visibility --
    // built in, not configurable yet (see window.cpp's comment on why).
    std::function<void(const std::string& icon_path, const std::string& tooltip)> set_tray;

    // The system clipboard (X11 CLIPBOARD selection, GtkClipboard) -- not
    // the webview's own JS clipboard, which needs a user gesture and a
    // permission prompt WebKit does not give a way to pre-approve.
    // clipboard_read is like pick_file, not like notify(): it waits on
    // whatever OTHER process currently owns the selection to answer over
    // X11, an unbounded wait in the same sense a human answering a file
    // dialog is -- `window.clipboard_read()` is `is_async` in window.cpp
    // for exactly that reason, found by a real timeout, not assumed
    // upfront. clipboard_write has no such wait: claiming ownership is a
    // local, unilateral action.
    std::function<std::string()>                  clipboard_read;
    std::function<void(const std::string& text)>  clipboard_write;

    // MPRIS (org.mpris.MediaPlayer2): pushes the current track/playback
    // state out to the session D-Bus so the desktop's own media widgets
    // (GNOME Shell, KDE Plasma, keyboard media keys) can show and control
    // it. `state` is a Dict the app-specific hook interprets itself (see
    // mpris.cpp, not part of this vendored tree) -- window.cpp only knows
    // it is "some JSON to hand off", the same shape set_menu already uses
    // for its own spec. Synchronous like notify(): pushing a property
    // update over an already-open D-Bus connection has no unbounded wait.
    std::function<void(const Value& state)> mpris_update;

    // Discord Rich Presence over Discord's local IPC socket -- see
    // discord_rpc.cpp (app-specific, not part of this vendored tree,
    // same relationship to this hook as mpris.cpp has to mpris_update).
    std::function<void(const Value& state)> discord_update;

    // Runs a snippet of JavaScript inside the window's page, from any
    // thread -- the push half of "loopback HTTP is the whole IPC": the page
    // can always call the backend, but without this the backend could only
    // wait for the page to ask (a poll), and WebKitGTK throttles a hidden
    // window's timers to a few seconds. Fire-and-forget: no return value.
    std::function<void(const std::string& js)> eval_js;
};

WindowControl& window_control();

} // namespace lux_script

#pragma once
#include <atomic>
#include <string>

#include <lux_script/value.hpp>

// Thin wrapper over the vendored webview.h C API (third_party/webview/webview.h)
// -- a single header wrapping WebKitGTK, no Chromium/Node bundled, nothing to
// download. Lux itself is untouched: this project only depends on it as a
// library (vendor/lux), and everything GUI-related lives here instead.
//
// set_menu() takes a lux_script::Value directly (the same List<Dict>/Json
// LuxScript itself works with) rather than a parallel C++ menu-description
// type: window.cpp (vendor/lux) hands it over as-is, and building the
// actual GtkMenu from it is entirely this class's job.
class DesktopWindow {
public:
    struct Options {
        std::string title     = "Lux Desktop";
        int         width     = 1024;
        int         height    = 768;
        bool        resizable = true;
        bool        devtools  = false;
        std::string icon;             // path to an image file, or empty for none
    };

    explicit DesktopWindow(Options opts);
    ~DesktopWindow();

    DesktopWindow(const DesktopWindow&)            = delete;
    DesktopWindow& operator=(const DesktopWindow&) = delete;

    // Points the window at the given URL and blocks the calling thread,
    // pumping the native GTK loop, until the window is closed. Must be
    // called from the same thread that constructed this object.
    void run(const std::string& url);

    // Thread-safe: makes run() return, as if the user had closed the window.
    void terminate();

    // Thread-safe: reloads the current page. Used by dev mode's file
    // watcher (src/dev.cpp) so a hot LuxScript recompile refreshes the
    // window the same way it would a browser tab, no manual F5.
    void reload();

    // The rest back the `window` LuxScript module (vendor/lux/src/lux_script/
    // modules/window.cpp) via WindowControl -- see runtime.cpp/dev.cpp for
    // how the hooks get wired up. All thread-safe: a route handler calling
    // these runs on an event-loop or blocking_pool thread, never the
    // window's own.
    void set_title(const std::string& title);
    void minimize();
    void maximize();
    void restore();
    void fullscreen();
    void unfullscreen();
    void set_always_on_top(bool on_top);

    // Blocking: shows a native file chooser and waits for it to close.
    // Returns the chosen path, or "" if the user canceled. Must be called
    // from a thread that can afford to block (window.open_file()/
    // save_file() are `is_async`, so that is always true here).
    std::string pick_file(const std::string& suggested_name, bool save_mode);

    // A freedesktop desktop notification, sent over the session D-Bus
    // directly (org.freedesktop.Notifications) -- not a libnotify
    // dependency, GLib/GIO already ships with GTK3. Thread-safe: GDBus is,
    // by its own documentation, safe to call from any thread.
    void notify(const std::string& title, const std::string& body);

    // Native menu bar. Rebuilds the window's whole content (menu bar +
    // webview stacked in a box) the first time this is called -- see
    // desktop_window.cpp's comment on webview.h's own widget layout. An
    // item's optional "accel" (GTK accelerator syntax, e.g. "<Control>q")
    // binds a window-wide keyboard shortcut to it.
    void set_menu(const lux_script::Value& spec);

    // System tray icon (GtkStatusIcon -- deprecated since GTK 3.14 but
    // still functional, and the only tray API GTK3 ships without adding a
    // separate libappindicator dependency; see desktop_window.cpp).
    // Left-click toggles the window's visibility.
    void set_tray(const std::string& icon_path, const std::string& tooltip);

    // The system clipboard (X11 CLIPBOARD selection via GtkClipboard), not
    // the webview's own `navigator.clipboard`. Both are quick, bounded
    // local X11 round-trips, same threading posture as notify().
    std::string clipboard_read();
    void        clipboard_write(const std::string& text);

    // The window's own size as of its last resize -- kept up to date by a
    // "configure-event" handler connected at construction time rather than
    // queried on demand, because by the time run() returns the GTK window
    // may already be torn down (its own "destroy" signal is what stops the
    // loop run() is blocking on). The caller (runtime.cpp/dev.cpp) reads
    // these right after run() returns to persist the size for next launch.
    int last_width()  const { return last_width_.load();  }
    int last_height() const { return last_height_.load(); }

private:
    void*             handle_      = nullptr; // webview_t
    void*             menubar_     = nullptr; // GtkWidget* -- null until set_menu()'s first call
    void*             accel_group_ = nullptr; // GtkAccelGroup* -- created alongside menubar_
    void*             tray_        = nullptr; // GtkStatusIcon* -- null until set_tray()'s first call
    std::atomic<int>  last_width_  = 0;
    std::atomic<int>  last_height_ = 0;
    std::atomic<bool> running_     = false;   // guards terminate() against a GTK
                                               // "no main loop running" warning
                                               // once run() has already returned
                                               // on its own.
};

#pragma once
#include <atomic>
#include <string>

// Thin wrapper over the vendored webview.h C API (third_party/webview/webview.h)
// -- a single header wrapping WebKitGTK, no Chromium/Node bundled, nothing to
// download. Lux itself is untouched: this project only depends on it as a
// library (vendor/lux), and everything GUI-related lives here instead.
class DesktopWindow {
public:
    struct Options {
        std::string title     = "Lux Desktop";
        int         width     = 1024;
        int         height    = 768;
        bool        resizable = true;
        bool        devtools  = false;
        std::string icon;             // path to an image file, or empty for none
        // Starts iconified instead of shown -- for a session launcher that
        // wants the app running but out of the way from the first frame
        // (there is no --headless mode, see runtime.cpp's --minimized flag)
        // instead of showing the window and then minimizing it a moment
        // later over HTTP (POST /api/window/minimize, window.lux), which is
        // what this replaces for THAT one case; the endpoint still exists
        // for minimizing/restoring an already-running window on demand.
        bool        start_minimized = false;
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

    // Thread-safe: runs `js` inside the page (window.eval_js()). Runs even
    // while the window is minimized/hidden -- unlike the page's own timers.
    void eval_js(const std::string& js);

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
    std::atomic<int>  last_width_  = 0;
    std::atomic<int>  last_height_ = 0;
    std::atomic<bool> running_     = false;   // guards terminate() against a GTK
                                               // "no main loop running" warning
                                               // once run() has already returned
                                               // on its own.
};

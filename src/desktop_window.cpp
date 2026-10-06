#include "desktop_window.hpp"
#include <webview/webview.h>
#include <gtk/gtk.h>

#include <future>

DesktopWindow::DesktopWindow(Options opts) {
    auto w = webview_create(opts.devtools ? 1 : 0, nullptr);
    webview_set_title(w, opts.title.c_str());
    webview_set_size(w, opts.width, opts.height,
                      opts.resizable ? WEBVIEW_HINT_NONE : WEBVIEW_HINT_FIXED);
    if (!opts.icon.empty()) {
        // Safe to call directly (not through webview_dispatch): the GTK
        // main loop has not started yet at this point in construction, and
        // this only ever runs once, from the same thread that will call
        // run() right after.
        GError* error = nullptr;
        if (!gtk_window_set_icon_from_file(
                GTK_WINDOW(webview_get_window(w)), opts.icon.c_str(), &error)) {
            g_clear_error(&error); // a missing/bad icon is cosmetic, never fatal
        }
    }
    if (opts.start_minimized) {
        // Direct call, not webview_dispatch: same reasoning as the icon
        // load above -- the GTK main loop has not started yet at this
        // point in construction, and this only ever runs once, from the
        // same thread that will call run() right after. Iconifying before
        // the window is ever mapped means it never shows a visible frame
        // first, unlike minimize() (which needs webview_dispatch because it
        // is called later, from an HTTP handler thread, after run() has the
        // GTK loop going).
        gtk_window_iconify(GTK_WINDOW(webview_get_window(w)));
    }
    last_width_  = opts.width;
    last_height_ = opts.height;
    // Tracks the window's live size so it is available AFTER run() returns
    // -- by then the GTK window itself may already be gone (its "destroy"
    // signal is what makes run() return in the first place), so querying
    // it on demand at that point is not reliable.
    g_signal_connect(webview_get_window(w), "configure-event",
        G_CALLBACK(+[](GtkWidget*, GdkEventConfigure* event, gpointer user_data) -> gboolean {
            auto* self = static_cast<DesktopWindow*>(user_data);
            self->last_width_.store(event->width);
            self->last_height_.store(event->height);
            return FALSE; // let the event propagate to GTK's own handling
        }), this);
    handle_ = w;
}

DesktopWindow::~DesktopWindow() {
    if (handle_) webview_destroy(static_cast<webview_t>(handle_));
}

void DesktopWindow::run(const std::string& url) {
    auto w = static_cast<webview_t>(handle_);
    webview_navigate(w, url.c_str());
    running_ = true;
    webview_run(w); // returns once the window is closed (the GTK "destroy"
                     // signal calls terminate() for us -- see webview.h)
    running_ = false;
}

void DesktopWindow::terminate() {
    if (running_.exchange(false)) webview_terminate(static_cast<webview_t>(handle_));
}

void DesktopWindow::reload() {
    // webview_dispatch marshals the call onto the window's own thread --
    // reload() is meant to be called from the file-watcher thread, which is
    // never the thread run() is blocking on.
    webview_dispatch(static_cast<webview_t>(handle_),
        [](webview_t w, void*) { webview_eval(w, "location.reload()"); },
        nullptr);
}

void DesktopWindow::eval_js(const std::string& js) {
    auto* copy = new std::string(js);
    webview_dispatch(static_cast<webview_t>(handle_),
        [](webview_t w, void* arg) {
            auto* s = static_cast<std::string*>(arg);
            webview_eval(w, s->c_str());
            delete s;
        },
        copy);
}

void DesktopWindow::minimize() {
    webview_dispatch(static_cast<webview_t>(handle_),
        [](webview_t w, void*) {
            gtk_window_iconify(GTK_WINDOW(webview_get_window(w)));
        }, nullptr);
}

void DesktopWindow::restore() {
    webview_dispatch(static_cast<webview_t>(handle_),
        [](webview_t w, void*) {
            auto* win = GTK_WINDOW(webview_get_window(w));
            gtk_window_unmaximize(win);
            gtk_window_deiconify(win);
        }, nullptr);
}

namespace {
struct FilePickCtx {
    std::promise<std::string> result;
    std::string               suggested_name;
    bool                      save_mode;
};
} // namespace

std::string DesktopWindow::pick_file(const std::string& suggested_name, bool save_mode) {
    auto* ctx = new FilePickCtx{{}, suggested_name, save_mode};
    auto future = ctx->result.get_future();

    // gtk_dialog_run() nests its own main loop until the dialog closes, so
    // this callback does not return to webview_dispatch's caller (the GTK
    // thread) until the user is done -- exactly why open_file()/save_file()
    // are `is_async`: pick_file() itself blocks the CALLING thread (a
    // lux::blocking_pool() worker) on this future the whole time, never the
    // window's own thread or an event-loop thread.
    webview_dispatch(static_cast<webview_t>(handle_),
        [](webview_t w, void* arg) {
            auto* ctx = static_cast<FilePickCtx*>(arg);
            GtkWindow* parent = GTK_WINDOW(webview_get_window(w));
            GtkFileChooserAction action = ctx->save_mode ? GTK_FILE_CHOOSER_ACTION_SAVE
                                                          : GTK_FILE_CHOOSER_ACTION_OPEN;
            GtkWidget* dialog = gtk_file_chooser_dialog_new(
                ctx->save_mode ? "Save File" : "Open File", parent, action,
                "_Cancel", GTK_RESPONSE_CANCEL,
                ctx->save_mode ? "_Save" : "_Open", GTK_RESPONSE_ACCEPT,
                nullptr);
            if (ctx->save_mode && !ctx->suggested_name.empty())
                gtk_file_chooser_set_current_name(GTK_FILE_CHOOSER(dialog),
                                                   ctx->suggested_name.c_str());

            std::string path;
            if (gtk_dialog_run(GTK_DIALOG(dialog)) == GTK_RESPONSE_ACCEPT) {
                char* filename = gtk_file_chooser_get_filename(GTK_FILE_CHOOSER(dialog));
                if (filename) { path = filename; g_free(filename); }
            }
            gtk_widget_destroy(dialog);
            ctx->result.set_value(path);
            delete ctx;
        }, ctx);

    return future.get();
}

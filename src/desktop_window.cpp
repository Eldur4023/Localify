#include "desktop_window.hpp"
#include <webview/webview.h>
#include <gtk/gtk.h>
#include <gio/gio.h>

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

void DesktopWindow::set_title(const std::string& title) {
    // webview_set_title's thread-safety isn't documented (unlike
    // webview_terminate/webview_dispatch, which explicitly are), so this
    // goes through webview_dispatch just like everything else here that
    // touches the GTK window.
    auto* copy = new std::string(title);
    webview_dispatch(static_cast<webview_t>(handle_),
        [](webview_t w, void* arg) {
            auto* t = static_cast<std::string*>(arg);
            webview_set_title(w, t->c_str());
            delete t;
        }, copy);
}

void DesktopWindow::minimize() {
    webview_dispatch(static_cast<webview_t>(handle_),
        [](webview_t w, void*) {
            gtk_window_iconify(GTK_WINDOW(webview_get_window(w)));
        }, nullptr);
}

void DesktopWindow::maximize() {
    webview_dispatch(static_cast<webview_t>(handle_),
        [](webview_t w, void*) {
            gtk_window_maximize(GTK_WINDOW(webview_get_window(w)));
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

void DesktopWindow::fullscreen() {
    webview_dispatch(static_cast<webview_t>(handle_),
        [](webview_t w, void*) {
            gtk_window_fullscreen(GTK_WINDOW(webview_get_window(w)));
        }, nullptr);
}

void DesktopWindow::unfullscreen() {
    webview_dispatch(static_cast<webview_t>(handle_),
        [](webview_t w, void*) {
            gtk_window_unfullscreen(GTK_WINDOW(webview_get_window(w)));
        }, nullptr);
}

void DesktopWindow::set_always_on_top(bool on_top) {
    webview_dispatch(static_cast<webview_t>(handle_),
        [](webview_t w, void* arg) {
            gtk_window_set_keep_above(GTK_WINDOW(webview_get_window(w)),
                                       arg != nullptr);
        }, reinterpret_cast<void*>(static_cast<intptr_t>(on_top)));
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

namespace {

// A menu item's action is a route path the app itself declared -- not user
// input -- but it still gets escaped before landing inside a JS string
// literal, on principle.
std::string js_escape(const std::string& s) {
    std::string out;
    out.reserve(s.size());
    for (char c : s) {
        if (c == '\\' || c == '"') out += '\\';
        out += c;
    }
    return out;
}

// action carries the base path; checkable is only set for a checkbox item,
// so the handler knows to read back the NEW toggled state and append it
// to the fetch() as `?checked=true|false` -- a plain menu item has no
// state of its own to report, a checkbox does.
struct MenuItemCtx { webview_t w; std::string action; bool checkable; };

// Recursive: an item with its own "items" list becomes a nested submenu
// instead of a leaf -- File > Export > CSV/JSON, as deep as the spec goes.
// A leaf item's "action" is what fires; a submenu item's "action", if any,
// is simply ignored (GTK does not activate a menu item that owns a
// submenu, only the leaves under it). A leaf's optional "accel" (GTK
// accelerator syntax -- "<Control>q", "<Control><Shift>s") binds a
// window-wide keyboard shortcut to it via accel_group, shared across the
// whole menu bar and every rebuild. A leaf with a boolean "checked" key
// becomes a checkbox instead of a plain item.
void populate_submenu(GtkWidget* submenu, const lux_script::Value& items,
                       webview_t w, GtkAccelGroup* accel_group) {
    if (!items.is_list()) return;
    for (const auto& item : items.as_list()) {
        if (item.is_str() && item.as_str() == "-") {
            gtk_menu_shell_append(GTK_MENU_SHELL(submenu), gtk_separator_menu_item_new());
            continue;
        }
        if (!item.is_dict()) continue;
        auto& d = item.as_dict();
        auto label_it   = d.find("label");
        auto items_it   = d.find("items");
        auto checked_it = d.find("checked");
        std::string label = (label_it != d.end() && label_it->second.is_str()) ? label_it->second.as_str() : "";
        bool has_items   = items_it   != d.end() && items_it->second.is_list();
        bool has_checked = !has_items && checked_it != d.end() && checked_it->second.is_bool();

        GtkWidget* menu_item = has_checked
            ? gtk_check_menu_item_new_with_label(label.c_str())
            : gtk_menu_item_new_with_label(label.c_str());
        if (has_checked)
            gtk_check_menu_item_set_active(GTK_CHECK_MENU_ITEM(menu_item), checked_it->second.as_bool());

        if (has_items) {
            GtkWidget* nested = gtk_menu_new();
            populate_submenu(nested, items_it->second, w, accel_group);
            gtk_menu_item_set_submenu(GTK_MENU_ITEM(menu_item), nested);
        } else {
            auto action_it = d.find("action");
            std::string action = (action_it != d.end() && action_it->second.is_str())
                                ? action_it->second.as_str() : "";
            // Clicking a leaf just fires a fetch() into one of the app's
            // own routes -- the same "loopback HTTP is the whole IPC" idea
            // as everything else in this project, not a second
            // native->LuxScript callback path. g_signal_connect_data's
            // destroy notifier frees the context when the item itself is
            // destroyed (a menu rebuild, or the window closing), not after
            // one click.
            g_signal_connect_data(menu_item, "activate",
                G_CALLBACK(+[](GtkMenuItem* self, gpointer user_data) {
                    auto* ctx = static_cast<MenuItemCtx*>(user_data);
                    if (ctx->action.empty()) return;
                    std::string url = ctx->action;
                    if (ctx->checkable) {
                        bool active = gtk_check_menu_item_get_active(GTK_CHECK_MENU_ITEM(self));
                        url += (url.find('?') == std::string::npos ? "?" : "&");
                        url += std::string("checked=") + (active ? "true" : "false");
                    }
                    std::string js = "fetch(\"" + js_escape(url) + "\", {method:'POST'})";
                    webview_eval(ctx->w, js.c_str());
                }),
                new MenuItemCtx{w, action, has_checked},
                [](gpointer data, GClosure*) { delete static_cast<MenuItemCtx*>(data); },
                static_cast<GConnectFlags>(0));

            auto accel_it = d.find("accel");
            if (accel_it != d.end() && accel_it->second.is_str() && !accel_it->second.as_str().empty()) {
                guint key = 0;
                GdkModifierType mods{};
                gtk_accelerator_parse(accel_it->second.as_str().c_str(), &key, &mods);
                if (key != 0)
                    gtk_widget_add_accelerator(menu_item, "activate", accel_group,
                                                key, mods, GTK_ACCEL_VISIBLE);
            }
        }
        gtk_menu_shell_append(GTK_MENU_SHELL(submenu), menu_item);
    }
}

struct MenuCtx { DesktopWindow* self; lux_script::Value spec; };

} // namespace

void DesktopWindow::set_menu(const lux_script::Value& spec) {
    webview_dispatch(static_cast<webview_t>(handle_),
        [](webview_t w, void* arg) {
            auto* ctx = static_cast<MenuCtx*>(arg);
            DesktopWindow* self   = ctx->self;
            GtkWindow*     window = GTK_WINDOW(webview_get_window(w));

            auto* menubar = static_cast<GtkWidget*>(self->menubar_);
            if (!menubar) {
                // webview.h attaches the webview widget directly to the
                // window (gtk_container_add, see the top of this file) --
                // lift it out, wrap it together with a fresh menu bar in a
                // vertical box, and put that box back as the window's one
                // child. Only needed once; a later set_menu() call finds
                // menubar_ already in place and just repopulates it.
                GtkWidget* webview_widget = gtk_bin_get_child(GTK_BIN(window));
                g_object_ref(webview_widget);
                gtk_container_remove(GTK_CONTAINER(window), webview_widget);

                GtkWidget* box = gtk_box_new(GTK_ORIENTATION_VERTICAL, 0);
                menubar = gtk_menu_bar_new();
                gtk_box_pack_start(GTK_BOX(box), menubar, FALSE, FALSE, 0);
                gtk_box_pack_start(GTK_BOX(box), webview_widget, TRUE, TRUE, 0);
                g_object_unref(webview_widget);

                gtk_container_add(GTK_CONTAINER(window), box);
                // box is a brand-new widget: GTK3 widgets start hidden, and
                // simply being inside a visible window does not change
                // that. Without this, both the menu bar and the reparented
                // webview stay invisible -- an empty content area, found
                // live via screenshot (the webview kept running, it just
                // never got mapped/drawn again).
                gtk_widget_show_all(box);
                self->menubar_ = menubar;

                // One accel group for the window's whole lifetime -- shared
                // across every set_menu() rebuild, since accelerators are
                // meant to keep working even after the menu they're
                // documented on gets torn down and rebuilt.
                auto* accels = gtk_accel_group_new();
                gtk_window_add_accel_group(window, accels);
                self->accel_group_ = accels;
            } else {
                for (GList* c = gtk_container_get_children(GTK_CONTAINER(menubar)); c; c = c->next)
                    gtk_widget_destroy(GTK_WIDGET(c->data));
            }

            auto* accel_group = static_cast<GtkAccelGroup*>(self->accel_group_);
            if (ctx->spec.is_list()) {
                for (const auto& top : ctx->spec.as_list()) {
                    if (!top.is_dict()) continue;
                    auto& d = top.as_dict();
                    auto label_it = d.find("label");
                    auto items_it = d.find("items");
                    std::string label = (label_it != d.end() && label_it->second.is_str())
                                       ? label_it->second.as_str() : "";

                    GtkWidget* top_item = gtk_menu_item_new_with_label(label.c_str());
                    GtkWidget* submenu  = gtk_menu_new();
                    if (items_it != d.end()) populate_submenu(submenu, items_it->second, w, accel_group);
                    gtk_menu_item_set_submenu(GTK_MENU_ITEM(top_item), submenu);
                    gtk_menu_shell_append(GTK_MENU_SHELL(menubar), top_item);
                }
            }
            gtk_widget_show_all(menubar);
            delete ctx;
        }, new MenuCtx{this, spec});
}

void DesktopWindow::set_tray(const std::string& icon_path, const std::string& tooltip) {
    struct TrayCtx { DesktopWindow* self; std::string icon_path; std::string tooltip; };
    webview_dispatch(static_cast<webview_t>(handle_),
        [](webview_t w, void* arg) {
            auto* ctx = static_cast<TrayCtx*>(arg);
            // GtkStatusIcon has been deprecated since GTK 3.14, but it is
            // still the only tray icon API GTK3 ships on its own -- the
            // replacement (libappindicator/ayatana-appindicator) is a
            // separate package with its own per-desktop-environment quirks,
            // not something to pull in for this. It still works fine on
            // anything supporting the older XEmbed systray protocol.
            auto* icon = static_cast<GtkStatusIcon*>(ctx->self->tray_);
            if (!icon) {
                icon = gtk_status_icon_new();
                ctx->self->tray_ = icon;
                // Left-click toggles the window instead of taking an
                // `action` route like the menu does: there is no page to
                // fetch() from while the window itself might be hidden.
                g_signal_connect(icon, "activate",
                    G_CALLBACK(+[](GtkStatusIcon*, gpointer user_data) {
                        auto* win = GTK_WINDOW(user_data);
                        if (gtk_widget_get_visible(GTK_WIDGET(win))) {
                            gtk_widget_hide(GTK_WIDGET(win));
                        } else {
                            gtk_widget_show(GTK_WIDGET(win));
                            gtk_window_present(win);
                        }
                    }),
                    webview_get_window(w));
            }
            gtk_status_icon_set_from_file(icon, ctx->icon_path.c_str());
            gtk_status_icon_set_tooltip_text(icon, ctx->tooltip.c_str());
            gtk_status_icon_set_visible(icon, TRUE);
            delete ctx;
        }, new TrayCtx{this, icon_path, tooltip});
}

void DesktopWindow::notify(const std::string& title, const std::string& body) {
    // org.freedesktop.Notifications.Notify -- the same D-Bus call
    // notify-send/libnotify make, done directly over GDBus (already part
    // of GTK3's own dependency chain) instead of adding libnotify as a
    // separate one. GDBus is documented thread-safe; called directly, no
    // webview_dispatch needed, unlike everything above that touches GTK.
    GError* error = nullptr;
    GDBusProxy* proxy = g_dbus_proxy_new_for_bus_sync(
        G_BUS_TYPE_SESSION, G_DBUS_PROXY_FLAGS_NONE, nullptr,
        "org.freedesktop.Notifications", "/org/freedesktop/Notifications",
        "org.freedesktop.Notifications", nullptr, &error);
    if (!proxy) { g_clear_error(&error); return; }

    GVariantBuilder actions, hints;
    g_variant_builder_init(&actions, G_VARIANT_TYPE("as"));
    g_variant_builder_init(&hints, G_VARIANT_TYPE("a{sv}"));

    GVariant* result = g_dbus_proxy_call_sync(proxy, "Notify",
        g_variant_new("(susssasa{sv}i)",
            "Lux Desktop", 0u, "", title.c_str(), body.c_str(),
            &actions, &hints, 5000),
        G_DBUS_CALL_FLAGS_NONE, -1, nullptr, &error);
    if (result) g_variant_unref(result);
    else        g_clear_error(&error); // no notification daemon running: not fatal
    g_object_unref(proxy);
}

namespace {
struct ClipboardReadCtx { std::promise<std::string> result; };
} // namespace

std::string DesktopWindow::clipboard_read() {
    auto* ctx = new ClipboardReadCtx();
    auto future = ctx->result.get_future();
    // GtkClipboard is tied to the default GdkDisplay, which is only safe
    // to touch from the thread running the GTK main loop -- same
    // webview_dispatch + future round-trip as pick_file(), just a much
    // shorter wait (a local X11 round-trip, not a human).
    webview_dispatch(static_cast<webview_t>(handle_),
        [](webview_t, void* arg) {
            auto* ctx = static_cast<ClipboardReadCtx*>(arg);
            GtkClipboard* clipboard = gtk_clipboard_get(GDK_SELECTION_CLIPBOARD);
            gchar* text = gtk_clipboard_wait_for_text(clipboard);
            ctx->result.set_value(text ? text : "");
            if (text) g_free(text);
            delete ctx;
        }, ctx);
    return future.get();
}

void DesktopWindow::clipboard_write(const std::string& text) {
    auto* copy = new std::string(text);
    webview_dispatch(static_cast<webview_t>(handle_),
        [](webview_t, void* arg) {
            auto* t = static_cast<std::string*>(arg);
            GtkClipboard* clipboard = gtk_clipboard_get(GDK_SELECTION_CLIPBOARD);
            gtk_clipboard_set_text(clipboard, t->c_str(), -1);
            delete t;
        }, copy);
}

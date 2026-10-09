// window module (NATIVE-MODULES.md's drop-in pattern): a config namespace
// (`window: { title, width, height, ... }` in `app:`, see WindowConfig)
// AND a small set of callable functions to control the native window from
// a route handler -- both backed by hooks the desktop shell (the separate
// project vendoring this copy of Lux -- see
// ../../../../../src/runtime.cpp and src/dev.cpp) installs once the actual
// window exists, since Lux itself has no concept of a window.
//
// window.open_file()/save_file() are `is_async`: showing a native dialog
// and waiting for the user is an unbounded wait, the same reasoning
// http.get()/os.run() already use it for -- see WindowControl::pick_file's
// comment in window_control.hpp for the threading this relies on.
#include <lux_script/builtin_module.hpp>
#include <lux_script/window_config.hpp>
#include <lux_script/window_control.hpp>

#include <cstdlib>

namespace lux_script {

WindowConfig& window_config() {
    static WindowConfig cfg;
    return cfg;
}

WindowControl& window_control() {
    static WindowControl ctl;
    return ctl;
}

namespace {

Value fn_window_set_title(NativeCtx&, std::vector<Value>& args, std::string& error) {
    if (!args[0].is_str()) { error = "window.set_title() expects a string"; return Value::null(); }
    if (window_control().set_title) window_control().set_title(args[0].as_str());
    return Value::null();
}

Value fn_window_minimize(NativeCtx&, std::vector<Value>&, std::string&) {
    if (window_control().minimize) window_control().minimize();
    return Value::null();
}

Value fn_window_maximize(NativeCtx&, std::vector<Value>&, std::string&) {
    if (window_control().maximize) window_control().maximize();
    return Value::null();
}

Value fn_window_restore(NativeCtx&, std::vector<Value>&, std::string&) {
    if (window_control().restore) window_control().restore();
    return Value::null();
}

Value fn_window_close(NativeCtx&, std::vector<Value>&, std::string&) {
    if (window_control().close) window_control().close();
    return Value::null();
}

Value fn_window_fullscreen(NativeCtx&, std::vector<Value>&, std::string&) {
    if (window_control().fullscreen) window_control().fullscreen();
    return Value::null();
}

Value fn_window_unfullscreen(NativeCtx&, std::vector<Value>&, std::string&) {
    if (window_control().unfullscreen) window_control().unfullscreen();
    return Value::null();
}

Value fn_window_set_always_on_top(NativeCtx&, std::vector<Value>& args, std::string& error) {
    if (!args[0].is_bool()) { error = "window.set_always_on_top() expects a bool"; return Value::null(); }
    if (window_control().set_always_on_top) window_control().set_always_on_top(args[0].as_bool());
    return Value::null();
}

// Empty string means "the user canceled", not an error: a canceled dialog
// is an ordinary, expected outcome for the caller to check with `== ""`,
// not a 500.
Value fn_window_open_file(NativeCtx&, std::vector<Value>&, std::string& error) {
    if (!window_control().pick_file) { error = "window: no native window running"; return Value::null(); }
    return Value::str(window_control().pick_file("", /*save_mode=*/false));
}

Value fn_window_save_file(NativeCtx&, std::vector<Value>& args, std::string& error) {
    if (!args[0].is_str()) { error = "window.save_file() expects a string (suggested file name)"; return Value::null(); }
    if (!window_control().pick_file) { error = "window: no native window running"; return Value::null(); }
    return Value::str(window_control().pick_file(args[0].as_str(), /*save_mode=*/true));
}

Value fn_window_notify(NativeCtx&, std::vector<Value>& args, std::string& error) {
    if (!args[0].is_str() || !args[1].is_str()) {
        error = "window.notify() expects two strings: title, body";
        return Value::null();
    }
    if (window_control().notify) window_control().notify(args[0].as_str(), args[1].as_str());
    return Value::null();
}

Value fn_window_set_menu(NativeCtx&, std::vector<Value>& args, std::string& error) {
    if (!args[0].is_list()) {
        error = "window.set_menu() expects a list: "
                "[{\"label\": \"File\", \"items\": [{\"label\": \"New\", \"action\": \"/new\"}, \"-\"]}]";
        return Value::null();
    }
    if (window_control().set_menu) window_control().set_menu(args[0]);
    return Value::null();
}

Value fn_window_mpris_update(NativeCtx&, std::vector<Value>& args, std::string& error) {
    if (!args[0].is_dict()) {
        error = "window.mpris_update() expects a dict: "
                "{trackId, title, artist, album, artUrl, durationMs, positionMs, status, volume, shuffle, repeat}";
        return Value::null();
    }
    if (window_control().mpris_update) window_control().mpris_update(args[0]);
    return Value::null();
}

Value fn_window_eval_js(NativeCtx&, std::vector<Value>& args, std::string& error) {
    if (!args[0].is_str()) { error = "window.eval_js() expects a string"; return Value::null(); }
    if (window_control().eval_js) window_control().eval_js(args[0].as_str());
    return Value::null();
}

Value fn_window_discord_update(NativeCtx&, std::vector<Value>& args, std::string& error) {
    if (!args[0].is_dict()) {
        error = "window.discord_update() expects a dict: {clientId, activity}";
        return Value::null();
    }
    if (window_control().discord_update) window_control().discord_update(args[0]);
    return Value::null();
}

// Not `is_async`: everything here (finding the icon theme, creating the
// status icon widget) is local and effectively instant -- same reasoning
// as notify().
Value fn_window_set_tray(NativeCtx&, std::vector<Value>& args, std::string& error) {
    if (!args[0].is_str() || !args[1].is_str()) {
        error = "window.set_tray() expects two strings: icon path, tooltip";
        return Value::null();
    }
    if (window_control().set_tray) window_control().set_tray(args[0].as_str(), args[1].as_str());
    return Value::null();
}

// `is_async`, unlike notify()/set_tray(): reading the clipboard waits on
// whatever OTHER process currently owns it to answer an X11 selection
// request, not just on the local display server -- found live, timing out
// past Lux's own request timeout against a slow (but not even hung, just a
// separate process) owner. clipboard_write() has no such wait: claiming
// ownership is a local, unilateral action.
Value fn_window_clipboard_read(NativeCtx&, std::vector<Value>&, std::string& error) {
    if (!window_control().clipboard_read) { error = "window: no native window running"; return Value::null(); }
    return Value::str(window_control().clipboard_read());
}

Value fn_window_clipboard_write(NativeCtx&, std::vector<Value>& args, std::string& error) {
    if (!args[0].is_str()) { error = "window.clipboard_write() expects a string"; return Value::null(); }
    if (window_control().clipboard_write) window_control().clipboard_write(args[0].as_str());
    return Value::null();
}

class WindowModule : public BuiltinModule {
public:
    const char* name() const override { return "window"; }

    const std::vector<BuiltinModuleFn>& functions() const override {
        static const std::vector<BuiltinModuleFn> fns = {
            {"set_title", 1, 1, fn_window_set_title},
            {"minimize",  0, 0, fn_window_minimize},
            {"maximize",  0, 0, fn_window_maximize},
            {"restore",   0, 0, fn_window_restore},
            {"close",     0, 0, fn_window_close},
            {"fullscreen",       0, 0, fn_window_fullscreen},
            {"unfullscreen",     0, 0, fn_window_unfullscreen},
            {"set_always_on_top", 1, 1, fn_window_set_always_on_top},
            {"open_file", 0, 0, fn_window_open_file, /*is_async=*/true},
            {"save_file", 1, 1, fn_window_save_file, /*is_async=*/true},
            {"notify",    2, 2, fn_window_notify},
            {"set_menu",  1, 1, fn_window_set_menu},
            {"mpris_update", 1, 1, fn_window_mpris_update},
            {"discord_update", 1, 1, fn_window_discord_update},
            {"eval_js",   1, 1, fn_window_eval_js},
            {"set_tray",  2, 2, fn_window_set_tray},
            {"clipboard_read",  0, 0, fn_window_clipboard_read, /*is_async=*/true},
            {"clipboard_write", 1, 1, fn_window_clipboard_write},
        };
        return fns;
    }

    bool configure(const std::map<std::string, std::string>& options,
                   std::string& error) override {
        WindowConfig cfg;
        cfg.present = true;
        for (const auto& [key, value] : options) {
            if      (key == "title")     cfg.title     = value;
            else if (key == "width")     cfg.width     = std::atoi(value.c_str());
            else if (key == "height")    cfg.height    = std::atoi(value.c_str());
            else if (key == "resizable") cfg.resizable = (value == "true");
            else if (key == "devtools")  cfg.devtools  = (value == "true");
            else if (key == "icon")      cfg.icon      = value;
            else { error = "window: unknown option '" + key + "'"; return false; }
        }
        window_config() = cfg;
        return true;
    }
};

} // namespace

LUX_REGISTER_MODULE(WindowModule)

} // namespace lux_script

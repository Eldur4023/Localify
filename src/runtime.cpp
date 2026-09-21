// The runtime every app built with add_lux_desktop_app() actually ships:
// extracts its own embedded resources (see respack), boots them through
// Lux's own compiler/HTTP server -- used here purely as a library, Lux's
// own behavior untouched -- and opens a native window pointed at the
// result. One binary, no arguments, no separate runtime to install.
//
// Window title/size come from app/app.lux's own `window:` block (see
// vendor/lux/src/lux_script/modules/window.cpp), never from here or from
// CMakeLists.txt: this file has nothing app-specific in it.
#include "resources.hpp"
#include "desktop_window.hpp"
#include "mpris.hpp"
#include "discord_rpc.hpp"

#include <lux_script/project.hpp>
#include <lux_script/vm.hpp>
#include <lux_script/window_config.hpp>
#include <lux_script/window_control.hpp>
#include <lux/app.hpp>
#include <lux/logger.hpp>

#include <arpa/inet.h>
#include <cctype>
#include <cstdint>
#include <cstdlib>
#include <cstring>
#include <csignal>
#include <fcntl.h>
#include <fstream>
#include <iostream>
#include <mutex>
#include <netinet/in.h>
#include <sys/file.h>
#include <sys/socket.h>
#include <thread>
#include <unistd.h>

namespace fs = std::filesystem;

namespace {

// Set right before the window is force-closed by an external signal (see
// DesktopWindow::terminate() below) -- tells apart "the user closed the
// window" (nothing else knows yet, we raise SIGTERM ourselves) from "a
// signal already started the shutdown" (raising a second one would hit
// app.cpp's own two-signal "Forced exit" path on a shutdown already in
// progress).
std::atomic<bool> g_shutdown_from_signal{false};
DesktopWindow*     g_window = nullptr;
std::mutex         g_window_mutex;

// Backs the `window` LuxScript module's callable functions
// (window.minimize(), window.open_file()...) -- see
// vendor/lux/src/lux_script/modules/window.cpp and window_control.hpp.
// Every hook grabs g_window under the lock and releases it before doing
// anything that might block (pick_file can wait on the user for a while),
// so a slow file dialog never holds up set_title()/minimize()/etc.
void install_window_control_hooks() {
    auto& ctl = lux_script::window_control();
    ctl.set_title = [](const std::string& title) {
        std::lock_guard<std::mutex> lk(g_window_mutex);
        if (g_window) g_window->set_title(title);
    };
    ctl.minimize = [] {
        std::lock_guard<std::mutex> lk(g_window_mutex);
        if (g_window) g_window->minimize();
    };
    ctl.maximize = [] {
        std::lock_guard<std::mutex> lk(g_window_mutex);
        if (g_window) g_window->maximize();
    };
    ctl.restore = [] {
        std::lock_guard<std::mutex> lk(g_window_mutex);
        if (g_window) g_window->restore();
    };
    ctl.close = [] {
        std::lock_guard<std::mutex> lk(g_window_mutex);
        if (g_window) g_window->terminate();
    };
    ctl.fullscreen = [] {
        std::lock_guard<std::mutex> lk(g_window_mutex);
        if (g_window) g_window->fullscreen();
    };
    ctl.unfullscreen = [] {
        std::lock_guard<std::mutex> lk(g_window_mutex);
        if (g_window) g_window->unfullscreen();
    };
    ctl.set_always_on_top = [](bool on_top) {
        std::lock_guard<std::mutex> lk(g_window_mutex);
        if (g_window) g_window->set_always_on_top(on_top);
    };
    ctl.pick_file = [](const std::string& suggested_name, bool save_mode) -> std::string {
        DesktopWindow* w;
        {
            std::lock_guard<std::mutex> lk(g_window_mutex);
            w = g_window;
        }
        return w ? w->pick_file(suggested_name, save_mode) : std::string();
    };
    ctl.notify = [](const std::string& title, const std::string& body) {
        std::lock_guard<std::mutex> lk(g_window_mutex);
        if (g_window) g_window->notify(title, body);
    };
    ctl.set_menu = [](const lux_script::Value& spec) {
        std::lock_guard<std::mutex> lk(g_window_mutex);
        if (g_window) g_window->set_menu(spec);
    };
    ctl.set_tray = [](const std::string& icon_path, const std::string& tooltip) {
        std::lock_guard<std::mutex> lk(g_window_mutex);
        if (g_window) g_window->set_tray(icon_path, tooltip);
    };
    ctl.clipboard_read = []() -> std::string {
        DesktopWindow* w;
        {
            std::lock_guard<std::mutex> lk(g_window_mutex);
            w = g_window;
        }
        return w ? w->clipboard_read() : std::string();
    };
    ctl.clipboard_write = [](const std::string& text) {
        std::lock_guard<std::mutex> lk(g_window_mutex);
        if (g_window) g_window->clipboard_write(text);
    };
    ctl.mpris_update = [](const lux_script::Value& state) {
        luxdesktop::mpris_update(state);
    };
    ctl.discord_update = [](const lux_script::Value& state) {
        luxdesktop::discord_update(state);
    };
}

#ifndef LUXDESKTOP_APP_ID
#define LUXDESKTOP_APP_ID "app"
#endif

// FNV-1a over every embedded file's path, size and bytes -- cheap (it never
// touches disk, just the bytes already sitting in the binary's own rodata)
// and enough to tell "this is the same build I extracted last launch" from
// "the binary changed" or "this is the first launch".
std::uint64_t fnv1a(std::uint64_t h, const void* data, std::size_t len) {
    const auto* p = static_cast<const unsigned char*>(data);
    for (std::size_t i = 0; i < len; ++i) {
        h ^= p[i];
        h *= 1099511628211ULL;
    }
    return h;
}

std::uint64_t resources_fingerprint() {
    std::uint64_t h = 1469598103934665603ULL; // FNV-1a 64-bit offset basis
    for (const auto& f : kEmbeddedFiles) {
        h = fnv1a(h, f.path, std::strlen(f.path));
        h = fnv1a(h, &f.size, sizeof(f.size));
        h = fnv1a(h, f.data, f.size);
    }
    return h;
}

// work_dir is persistent now (see main()'s comment), so re-extracting the
// same ~2.4MB of templates/statics/sources on every ordinary launch is pure
// waste once the first launch already wrote them. Skip the whole pass when
// a marker left by a previous run says this exact build was already
// extracted here.
void extract_resources(const fs::path& dir) {
    fs::path marker = dir / ".resources-fingerprint";
    std::uint64_t current = resources_fingerprint();
    if (std::ifstream in(marker); in) {
        std::uint64_t previous = 0;
        if (in >> previous && previous == current) return;
    }
    for (const auto& f : kEmbeddedFiles) {
        fs::path dest = dir / f.path;
        fs::create_directories(dest.parent_path());
        std::ofstream out(dest, std::ios::binary);
        out.write(reinterpret_cast<const char*>(f.data), static_cast<std::streamsize>(f.size));
    }
    std::ofstream(marker) << current;
}

// Exclusive, non-blocking flock() on a file under work_dir, held for the
// life of the process (the OS drops it when this process exits, normally
// or not). work_dir went from a fresh mkdtemp per launch to one persistent,
// shared path (see main()) specifically so ./data/ (the SQLite library)
// survives restarts -- but that means two instances launched at once would
// now extract_resources() into and serve static files from the very same
// directory, racing truncate-and-rewrite against whatever the other one is
// reading mid-request. Refusing the second launch outright is simpler and
// safer than trying to make concurrent instances share one work_dir.
bool acquire_single_instance_lock(const fs::path& dir) {
    fs::path lock_path = dir / ".lock";
    int fd = ::open(lock_path.c_str(), O_CREAT | O_RDWR, 0644);
    if (fd < 0) return true; // can't even open it: don't block startup over this
    if (::flock(fd, LOCK_EX | LOCK_NB) != 0) {
        ::close(fd);
        return false;
    }
    // Leaked on purpose: the lock must outlive this function, and the OS
    // reclaims the fd (and the flock with it) when the process exits.
    return true;
}

// Binds to loopback with port 0 (the OS picks a free ephemeral port), reads
// it back with getsockname(), then releases it immediately. A small race
// (something else could grab the same port before Lux's own bind) is the
// same trade-off every "find a free port" helper makes; fine for a desktop
// app that only ever talks to itself.
uint16_t find_free_port() {
    int fd = ::socket(AF_INET, SOCK_STREAM, 0);
    if (fd < 0) throw std::runtime_error("socket: " + std::string(std::strerror(errno)));
    sockaddr_in addr{};
    addr.sin_family      = AF_INET;
    addr.sin_port        = 0;
    addr.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
    if (::bind(fd, reinterpret_cast<sockaddr*>(&addr), sizeof(addr)) < 0) {
        ::close(fd);
        throw std::runtime_error("bind: " + std::string(std::strerror(errno)));
    }
    socklen_t len = sizeof(addr);
    ::getsockname(fd, reinterpret_cast<sockaddr*>(&addr), &len);
    uint16_t port = ntohs(addr.sin_port);
    ::close(fd);
    return port;
}

fs::path xdg_data_home() {
    if (const char* xdg = std::getenv("XDG_DATA_HOME"); xdg && *xdg) return fs::path(xdg);
    const char* home = std::getenv("HOME");
    return fs::path(home ? home : ".") / ".local" / "share";
}

// A display name turned into a safe filesystem/.desktop-file identifier:
// lowercase, alphanumeric runs joined by single dashes.
std::string sanitize_id(const std::string& name) {
    std::string out;
    for (char c : name) {
        if (std::isalnum(static_cast<unsigned char>(c)))
            out += static_cast<char>(std::tolower(static_cast<unsigned char>(c)));
        else if (!out.empty() && out.back() != '-')
            out += '-';
    }
    while (!out.empty() && out.back() == '-') out.pop_back();
    return out.empty() ? "lux-desktop-app" : out;
}

std::string resolve_display_name(const std::shared_ptr<lux_script::Module>& mod) {
    const auto& wcfg = lux_script::window_config();
    if (!wcfg.title.empty())            return wcfg.title;
    if (!mod->program.app.name.empty()) return mod->program.app.name;
    return "Lux Desktop App";
}

fs::path xdg_cache_home() {
    if (const char* xdg = std::getenv("XDG_CACHE_HOME"); xdg && *xdg) return fs::path(xdg);
    const char* home = std::getenv("HOME");
    return fs::path(home ? home : ".") / ".cache";
}

fs::path geometry_file(const std::string& id) {
    return xdg_cache_home() / "lux-desktop" / (id + ".geometry");
}

// Sanity bounds, not just "did the file parse": a corrupted or
// hand-edited geometry file should fall back to the window: block's own
// defaults, never hand a 0x0 or a many-million-pixel size to the window.
bool load_saved_geometry(const std::string& id, int& width, int& height) {
    std::ifstream in(geometry_file(id));
    int w = 0, h = 0;
    if (!(in >> w >> h)) return false;
    if (w < 100 || h < 100 || w > 10000 || h > 10000) return false;
    width = w;
    height = h;
    return true;
}

void save_geometry(const std::string& id, int width, int height) {
    if (width < 100 || height < 100 || width > 10000 || height > 10000) return;
    std::error_code ec;
    fs::path f = geometry_file(id);
    fs::create_directories(f.parent_path(), ec);
    std::ofstream out(f);
    if (out) out << width << " " << height << "\n";
}

// Installs a .desktop launcher for THIS exact binary into the current
// user's application menu. Opt-in only (`--install-desktop` on the command
// line) and never run on a plain launch: unlike everything else this
// binary does, it writes outside its own temp directory, into the user's
// actual desktop environment (~/.local/share/applications), so it needs to
// be asked for explicitly.
int install_desktop(const std::shared_ptr<lux_script::Module>& mod, const fs::path& work_dir) {
    char exe_path[4096];
    ssize_t n = ::readlink("/proc/self/exe", exe_path, sizeof(exe_path) - 1);
    if (n <= 0) {
        std::cerr << "luxdesktop: could not resolve this binary's own path\n";
        return 1;
    }
    exe_path[n] = '\0';

    const auto& wcfg = lux_script::window_config();
    std::string display_name = resolve_display_name(mod);
    std::string id = sanitize_id(display_name);

    fs::path data_home = xdg_data_home();
    fs::path apps_dir   = data_home / "applications";
    fs::path icons_dir  = data_home / "icons" / "hicolor" / "256x256" / "apps";
    std::error_code ec;
    fs::create_directories(apps_dir, ec);

    // The icon only exists inside work_dir (respack's extracted copy),
    // which gets deleted when this process exits -- copy it somewhere
    // permanent, in the standard icon theme location, before that happens.
    std::string icon_line;
    if (!wcfg.icon.empty()) {
        fs::path src_icon = work_dir / wcfg.icon;
        if (fs::exists(src_icon, ec)) {
            fs::create_directories(icons_dir, ec);
            fs::path dst_icon = icons_dir / (id + src_icon.extension().string());
            fs::copy_file(src_icon, dst_icon, fs::copy_options::overwrite_existing, ec);
            if (!ec) icon_line = "Icon=" + dst_icon.string() + "\n";
        }
    }

    fs::path desktop_file = apps_dir / (id + ".desktop");
    std::ofstream out(desktop_file);
    out << "[Desktop Entry]\n"
        << "Type=Application\n"
        << "Name=" << display_name << "\n"
        << "Exec=\"" << exe_path << "\"\n"
        << "Terminal=false\n"
        << icon_line
        << "Categories=Utility;\n";
    out.close();

    std::cout << "luxdesktop: installed " << desktop_file.string() << "\n"
              << "            find \"" << display_name << "\" in your application menu.\n";
    return 0;
}

bool wait_for_server(uint16_t port, std::chrono::milliseconds timeout) {
    auto deadline = std::chrono::steady_clock::now() + timeout;
    while (std::chrono::steady_clock::now() < deadline) {
        int fd = ::socket(AF_INET, SOCK_STREAM, 0);
        if (fd >= 0) {
            sockaddr_in addr{};
            addr.sin_family      = AF_INET;
            addr.sin_port        = htons(port);
            addr.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
            bool ok = ::connect(fd, reinterpret_cast<sockaddr*>(&addr), sizeof(addr)) == 0;
            ::close(fd);
            if (ok) return true;
        }
        std::this_thread::sleep_for(std::chrono::milliseconds(20));
    }
    return false;
}

} // namespace

int main(int argc, char** argv) {
    bool install = argc > 1 && std::string(argv[1]) == "--install-desktop";

    // Resources are extracted to a PERSISTENT directory (XDG data home), not
    // a temp one: anything the app writes next to its own sources at runtime
    // -- today that is the SQLite database under ./data/ -- has to survive
    // restarts. Re-extracting on every boot only overwrites the read-only
    // resources themselves (app.lux, templates/, public/); ./data/ is never
    // part of the embedded set (CMakeLists.txt excludes it from respack).
    fs::path work_dir = xdg_data_home() / "lux-desktop" / LUXDESKTOP_APP_ID;
    fs::create_directories(work_dir);
    if (!acquire_single_instance_lock(work_dir)) {
        std::cerr << "luxdesktop: already running (" << work_dir.string() << " is locked)\n";
        return 1;
    }
    extract_resources(work_dir);
    fs::current_path(work_dir); // the app's own paths (templates "./templates",
                                 // static "/x" -> "./public") are written
                                 // relative to itself, so this is the CWD
                                 // they expect.

    std::vector<fs::path> inputs;
    std::string error;
    if (!lux_script::resolve_inputs({"."}, inputs, error)) {
        std::cerr << "luxdesktop: " << error << "\n";
        return 1;
    }

    lux_script::DiagnosticBag diags;
    auto mod = lux_script::compile(inputs, diags);
    if (!diags.empty()) {
        std::cerr << lux_script::format_errors(diags, mod->files);
        return 1;
    }

    if (install) {
        // No remove_all(work_dir) here: unlike the old temp-dir scheme, the
        // work dir is persistent and holds the app's own data now.
        return install_desktop(mod, work_dir);
    }

    lux::App app;
    app.set_templates(mod->program.app.templates_dir);
    for (const auto& m : mod->program.app.statics)
        app.serve_static(m.url_prefix, m.fs_root, m.spa);

    auto dispatch = [mod](lux::Request& req, lux::Response& res) -> lux::Task<void> {
        auto match = mod->router.match(req.method, req.path);
        if (!match.found) {
            res.status(404).json_text(R"({"error":"Not Found"})");
            co_return;
        }
        req.params = std::move(match.params);
        co_await match.handler(req, res);
    };
    app.any("/",  dispatch);
    app.any("/*", dispatch);

    app.on_error([mod](int code, lux::Request& req, lux::Response& res) {
        auto it = mod->error_handlers.find(code);
        if (it == mod->error_handlers.end()) it = mod->error_handlers.find(0);
        if (it == mod->error_handlers.end()) return;
        lux_script::NativeCtx ctx{req, res};
        ctx.error_code    = code;
        ctx.error_message = res.status_code() >= 500 ? "internal error" : "invalid request";
        lux_script::VM vm;
        auto result = vm.start(*it->second, {}, ctx, &mod->functions, nullptr);
        if (result.status == lux_script::VM::Status::Done &&
            !ctx.response_written && !result.value.is_null())
            res.header("Content-Type", "application/json; charset=utf-8")
               .send(result.value.to_json_text());
        res.status(code);
    });

    uint16_t port = find_free_port();

    // Force-closes the window if a signal (CTRL+C from a terminal, a
    // desktop "quit" action, systemd stop...) starts the shutdown before
    // the user closes the window by hand -- otherwise the server drains but
    // the window stays open with nothing behind it.
    app.on_before_stop([] {
        std::lock_guard<std::mutex> lk(g_window_mutex);
        if (g_window) {
            g_shutdown_from_signal.store(true);
            g_window->terminate();
        }
    });

    std::thread server_thread([&app, port] { app.run("127.0.0.1", port); });
    if (!wait_for_server(port, std::chrono::milliseconds(5000)))
        lux::log().warn("server did not come up in time, opening the window anyway");

    const lux_script::WindowConfig& wcfg = lux_script::window_config();
    std::string app_id = sanitize_id(resolve_display_name(mod));

    DesktopWindow::Options opts;
    opts.title     = resolve_display_name(mod);
    opts.width     = wcfg.width;
    opts.height    = wcfg.height;
    opts.resizable = wcfg.resizable;
    opts.devtools  = wcfg.devtools;
    opts.icon      = wcfg.icon;
    // A size the user already resized to on a previous run wins over the
    // window: block's own defaults -- those are a first-launch default,
    // not something to snap back to every time.
    load_saved_geometry(app_id, opts.width, opts.height);

    DesktopWindow window(opts);
    {
        std::lock_guard<std::mutex> lk(g_window_mutex);
        g_window = &window;
    }
    install_window_control_hooks();
    luxdesktop::mpris_init(LUXDESKTOP_APP_ID, opts.title, port);
    window.run("http://127.0.0.1:" + std::to_string(port) + "/");
    luxdesktop::mpris_shutdown();
    luxdesktop::discord_shutdown();
    save_geometry(app_id, window.last_width(), window.last_height());

    // See the comment on g_shutdown_from_signal: only raise SIGTERM
    // ourselves if nothing already started the shutdown.
    if (!g_shutdown_from_signal.load()) std::raise(SIGTERM);
    server_thread.join();

    {
        std::lock_guard<std::mutex> lk(g_window_mutex);
        g_window = nullptr;
    }

    // work_dir es PERSISTENTE (XDG data home): contiene ./data/ con la base
    // de datos de la biblioteca. Borrarlo al apagar era del esquema antiguo
    // de directorio temporal — con datos persistentes sería perder la
    // biblioteca entera en cada cierre de ventana.
    return 0;
}

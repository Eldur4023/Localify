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
#include "desktop_common.hpp"

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
using namespace luxdesktop;

namespace {

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

// Called when the lock above says another instance already owns work_dir:
// asks THAT instance to raise its window instead of just exiting silently.
// Without this, launching the app again while it was already running
// (minimized at login, or just still open) had no visible effect at all --
// which looks exactly like "nothing happened, try again" to whoever clicked
// the icon, even though a second click was never going to do anything
// different. Best-effort: a missing/stale port file just means we exit
// quietly, same as before this existed.
void ask_running_instance_to_show(const fs::path& work_dir) {
    std::ifstream in(work_dir / "port");
    uint16_t port = 0;
    if (!(in >> port) || port == 0) return;
    int fd = ::socket(AF_INET, SOCK_STREAM, 0);
    if (fd < 0) return;
    timeval tv{1, 0};
    ::setsockopt(fd, SOL_SOCKET, SO_SNDTIMEO, &tv, sizeof(tv));
    ::setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &tv, sizeof(tv));
    sockaddr_in addr{};
    addr.sin_family      = AF_INET;
    addr.sin_port        = htons(port);
    addr.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
    if (::connect(fd, reinterpret_cast<sockaddr*>(&addr), sizeof(addr)) == 0) {
        static const char req[] =
            "POST /api/window/restore HTTP/1.1\r\nHost: 127.0.0.1\r\n"
            "Content-Length: 0\r\nConnection: close\r\n\r\n";
        ::send(fd, req, sizeof(req) - 1, 0);
        char buf[64];
        while (::recv(fd, buf, sizeof(buf), 0) > 0) {}
    }
    ::close(fd);
}

fs::path xdg_data_home() {
    if (const char* xdg = std::getenv("XDG_DATA_HOME"); xdg && *xdg) return fs::path(xdg);
    const char* home = std::getenv("HOME");
    return fs::path(home ? home : ".") / ".local" / "share";
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

} // namespace

int main(int argc, char** argv) {
    bool install = argc > 1 && std::string(argv[1]) == "--install-desktop";
    // No --headless mode (unlike the original Tauri app): a session
    // launcher that wants the window running but out of the way from the
    // start passes this instead of the old workaround (launch normal, then
    // POST /api/window/minimize the moment the server answers -- see
    // window.lux, still there for minimizing/restoring an ALREADY-running
    // window on demand, a different case from "start minimized").
    bool start_minimized = false;
    for (int i = 1; i < argc; ++i)
        if (std::string(argv[i]) == "--minimized") start_minimized = true;

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
        ask_running_instance_to_show(work_dir);
        return 1;
    }
    extract_resources(work_dir);
    fs::current_path(work_dir); // the app's own paths (templates "./templates",
                                 // static "/x" -> "./public") are written
                                 // relative to itself, so this is the CWD
                                 // they expect.

    // The sources are the .lux files EMBEDDED in this binary, not whatever
    // sits in work_dir: extraction overwrites but never deletes, so a file
    // removed from app/ in a newer build stayed there and kept being
    // compiled (and serving its routes) forever.
    std::vector<std::string> sources;
    for (const auto& f : kEmbeddedFiles) {
        fs::path p(f.path);
        if (p.extension() == ".lux") sources.push_back(p.string());
    }
    std::vector<fs::path> inputs;
    std::string error;
    if (!lux_script::resolve_inputs(sources, inputs, error)) {
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

    // The port is random by design (find_free_port() above), but external
    // tools on the same machine (a login script, a "now playing" widget)
    // still need a way to find it -- relative to work_dir, same as "data"
    // and everything else the app itself reads/writes.
    std::ofstream("./port") << port;

    const lux_script::WindowConfig& wcfg = lux_script::window_config();
    std::string app_id = sanitize_id(resolve_display_name(mod));

    DesktopWindow::Options opts;
    opts.title     = resolve_display_name(mod);
    opts.width     = wcfg.width;
    opts.height    = wcfg.height;
    opts.resizable = wcfg.resizable;
    opts.devtools  = wcfg.devtools;
    opts.icon      = wcfg.icon;
    opts.start_minimized = start_minimized;
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
    std::error_code ec;
    fs::remove("./port", ec);

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

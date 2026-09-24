// Dev mode: points straight at app/ on disk -- no respack, no embedding, no
// rebuilding this binary after editing a .lux file, a template or a static
// asset. A background thread watches app/'s files exactly the way Lux's own
// `lux` CLI does (see vendor/lux/src/lux_script/main.cpp's watch_loop) and
// recompiles through the same lux_script::compile() the release runtime
// uses -- LuxScript compiles to bytecode in milliseconds, no g++ involved,
// so "no tener que compilar" holds for every edit inside app/. On a
// successful recompile the live Module is swapped and the window's page is
// told to reload, the same way a browser tab would refresh on its own.
//
// This is the ONLY difference from src/runtime.cpp: that one embeds a
// frozen snapshot of app/ into the binary and never watches anything again.
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
#include <atomic>
#include <cctype>
#include <csignal>
#include <cstring>
#include <fstream>
#include <iostream>
#include <mutex>
#include <netinet/in.h>
#include <optional>
#include <sys/socket.h>
#include <thread>
#include <unistd.h>

#ifndef LUXDESKTOP_APP_DIR
#error "LUXDESKTOP_APP_DIR must be set at compile time (see CMakeLists.txt)"
#endif

namespace fs = std::filesystem;
using namespace luxdesktop;

namespace {

std::shared_ptr<lux_script::Module> g_module;
std::mutex                          g_module_mutex;
std::atomic<bool>                   g_stop{false};

std::shared_ptr<lux_script::Module> current_module() {
    std::lock_guard<std::mutex> lk(g_module_mutex);
    return g_module;
}

void publish_module(std::shared_ptr<lux_script::Module> m) {
    std::lock_guard<std::mutex> lk(g_module_mutex);
    g_module = std::move(m);
}

void reload_window() {
    std::lock_guard<std::mutex> lk(g_window_mutex);
    if (g_window) g_window->reload();
}

// Lux's own Module::stamps (vendor/lux) only tracks the .lux files
// themselves -- a template or a static asset changing does not touch it,
// since compile() re-reads those fresh on every call regardless. Dev mode's
// whole point is fast iteration on exactly those files, so the mtime watch
// here additionally walks templates_dir and every static mount's fs_root,
// taking the single latest mtime across all of it as "the" signal.
//
// std::optional, not a default-constructed file_time_type{} as the "nothing
// seen yet" sentinel: libstdc++'s file_clock epoch is not 1970, so a real
// file's time_since_epoch() can legitimately compare as LESS than a
// default-constructed time_point -- found live (every poll read `now == 0`
// forever, never detecting the edit already on disk) rather than guessed.
using MTime = std::optional<fs::file_time_type>;

void consider(MTime& latest, MTime candidate) {
    if (candidate && (!latest || *candidate > *latest)) latest = candidate;
}

MTime latest_mtime_under(const fs::path& root) {
    std::error_code ec;
    MTime latest;
    if (fs::is_regular_file(root, ec)) {
        auto t = fs::last_write_time(root, ec);
        if (!ec) latest = t;
        return latest;
    }
    if (!fs::is_directory(root, ec)) return latest;
    for (auto it = fs::recursive_directory_iterator(
             root, fs::directory_options::skip_permission_denied, ec);
         !ec && it != fs::recursive_directory_iterator(); it.increment(ec)) {
        if (!it->is_regular_file(ec)) continue;
        auto t = fs::last_write_time(it->path(), ec);
        if (!ec) consider(latest, t);
    }
    return latest;
}

MTime aggregate_mtime(const lux_script::Module& mod) {
    MTime latest;
    std::error_code ec;
    for (const auto& [path, stamp] : mod.stamps) {
        auto t = fs::last_write_time(path, ec);
        if (!ec) consider(latest, t);
    }
    if (!mod.program.app.templates_dir.empty())
        consider(latest, latest_mtime_under(mod.program.app.templates_dir));
    for (const auto& m : mod.program.app.statics)
        consider(latest, latest_mtime_under(m.fs_root));
    return latest;
}

// Polls that aggregate mtime and recompiles through the exact same
// lux_script::compile() the packaged runtime uses -- LuxScript compiles to
// bytecode in milliseconds, no g++ involved, so this never rebuilds
// app-dev itself. Keeps the previous (working) module if the new one fails
// to compile, same as Lux's own watch_loop -- a typo never takes the
// window down.
void watch_loop(std::vector<fs::path> inputs) {
    MTime last_seen;
    bool first = true;
    while (!g_stop.load()) {
        std::this_thread::sleep_for(std::chrono::milliseconds(300));
        if (g_stop.load()) return;

        auto mod = current_module();
        if (!mod) continue;

        auto now = aggregate_mtime(*mod);
        if (first) { last_seen = now; first = false; continue; }
        if (now == last_seen) continue;
        last_seen = now;

        lux::log().info("changes detected: recompiling");
        std::this_thread::sleep_for(std::chrono::milliseconds(120)); // let the editor finish writing

        lux_script::DiagnosticBag diags;
        auto next = lux_script::compile(inputs, diags);
        if (!diags.empty()) {
            std::cerr << "\n" << lux_script::format_errors(diags, next->files)
                      << "reload cancelled: still serving the previous version\n\n";
            continue;
        }

        publish_module(next);
        lux::log().info("reloaded: " + std::to_string(next->program.routes.size()) + " route(s)");
        reload_window();
    }
}

} // namespace

int main() {
    fs::current_path(LUXDESKTOP_APP_DIR); // app.lux's own paths ("./templates",
                                           // "./public") are relative to itself

    std::vector<fs::path> inputs;
    std::string error;
    if (!lux_script::resolve_inputs({"."}, inputs, error)) {
        std::cerr << "luxdesktop-dev: " << error << "\n";
        return 1;
    }

    lux_script::DiagnosticBag diags;
    auto mod = lux_script::compile(inputs, diags);
    if (!diags.empty()) {
        std::cerr << lux_script::format_errors(diags, mod->files);
        return 1;
    }
    publish_module(mod);

    lux::App app;
    app.set_templates(mod->program.app.templates_dir);
    for (const auto& m : mod->program.app.statics)
        app.serve_static(m.url_prefix, m.fs_root, m.spa);

    // Reads current_module() on every request (not a captured `mod`), same
    // as vendor/lux's own main.cpp: a hot reload has to reach in-flight
    // dispatch immediately, not just the next process restart.
    auto dispatch = [](lux::Request& req, lux::Response& res) -> lux::Task<void> {
        auto live = current_module();
        if (!live) { res.status(503).json_text(R"({"error":"no module loaded"})"); co_return; }
        auto match = live->router.match(req.method, req.path);
        if (!match.found) {
            res.status(404).json_text(R"({"error":"Not Found"})");
            co_return;
        }
        req.params = std::move(match.params);
        co_await match.handler(req, res);
    };
    app.any("/",  dispatch);
    app.any("/*", dispatch);

    app.on_error([](int code, lux::Request& req, lux::Response& res) {
        auto live = current_module();
        if (!live) return;
        auto it = live->error_handlers.find(code);
        if (it == live->error_handlers.end()) it = live->error_handlers.find(0);
        if (it == live->error_handlers.end()) return;
        lux_script::NativeCtx ctx{req, res};
        ctx.error_code    = code;
        ctx.error_message = res.status_code() >= 500 ? "internal error" : "invalid request";
        lux_script::VM vm;
        auto result = vm.start(*it->second, {}, ctx, &live->functions, nullptr);
        if (result.status == lux_script::VM::Status::Done &&
            !ctx.response_written && !result.value.is_null())
            res.header("Content-Type", "application/json; charset=utf-8")
               .send(result.value.to_json_text());
        res.status(code);
    });

    uint16_t port = find_free_port();

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

    std::thread watcher(watch_loop, inputs);

    const lux_script::WindowConfig& wcfg = lux_script::window_config();
    std::string app_id = sanitize_id(resolve_display_name(mod));

    DesktopWindow::Options opts;
    opts.title     = resolve_display_name(mod);
    opts.width     = wcfg.width;
    opts.height    = wcfg.height;
    opts.resizable = wcfg.resizable;
    opts.devtools  = true; // dev mode: always on, no reason to hide it here
    opts.icon      = wcfg.icon;
    load_saved_geometry(app_id, opts.width, opts.height);

    DesktopWindow window(opts);
    {
        std::lock_guard<std::mutex> lk(g_window_mutex);
        g_window = &window;
    }
    install_window_control_hooks();
    luxdesktop::mpris_init("localify-dev", opts.title, port);
    lux::log().info("lux desktop (dev): watching " + fs::path(LUXDESKTOP_APP_DIR).string());
    window.run("http://127.0.0.1:" + std::to_string(port) + "/");
    luxdesktop::mpris_shutdown();
    luxdesktop::discord_shutdown();
    save_geometry(app_id, window.last_width(), window.last_height());

    if (!g_shutdown_from_signal.load()) std::raise(SIGTERM);
    server_thread.join();

    g_stop.store(true);
    watcher.join();

    {
        std::lock_guard<std::mutex> lk(g_window_mutex);
        g_window = nullptr;
    }
    return 0;
}

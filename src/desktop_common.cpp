// What src/runtime.cpp (the shipped binary) and src/dev.cpp (hot-reload dev
// mode) share: the window pointer, the `window` module hooks, and the small
// port/geometry helpers. Everything else in those two files is what makes
// them different.
#include "desktop_common.hpp"
#include "mpris.hpp"
#include "discord_rpc.hpp"

#include <lux_script/window_config.hpp>
#include <lux_script/window_control.hpp>

#include <arpa/inet.h>
#include <cctype>
#include <cstdlib>
#include <cstring>
#include <fstream>
#include <netinet/in.h>
#include <stdexcept>
#include <sys/socket.h>
#include <thread>
#include <unistd.h>

namespace fs = std::filesystem;

namespace luxdesktop {

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
    ctl.eval_js = [](const std::string& js) {
        std::lock_guard<std::mutex> lk(g_window_mutex);
        if (g_window) g_window->eval_js(js);
    };
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

} // namespace luxdesktop

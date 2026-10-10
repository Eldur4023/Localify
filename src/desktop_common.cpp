// What src/runtime.cpp (the shipped binary) and src/dev.cpp (hot-reload dev
// mode) share: the window pointer, the `window` module hooks, and the small
// port/geometry helpers. Everything else in those two files is what makes
// them different.
#include "desktop_common.hpp"
#include "mpris.hpp"
#include "discord_rpc.hpp"

#include <lux/logger.hpp>
#include <lux_script/audio_control.hpp>
#include <lux_script/db.hpp>
#include <lux_script/schedule.hpp>
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
// so a slow file dialog never holds up minimize()/restore().
void install_window_control_hooks() {
    auto& ctl = lux_script::window_control();
    ctl.minimize = [] {
        std::lock_guard<std::mutex> lk(g_window_mutex);
        if (g_window) g_window->minimize();
    };
    ctl.restore = [] {
        std::lock_guard<std::mutex> lk(g_window_mutex);
        if (g_window) g_window->restore();
    };
    ctl.pick_file = [](const std::string& suggested_name, bool save_mode) -> std::string {
        DesktopWindow* w;
        {
            std::lock_guard<std::mutex> lk(g_window_mutex);
            w = g_window;
        }
        return w ? w->pick_file(suggested_name, save_mode) : std::string();
    };
    ctl.mpris_update = [](const lux_script::Value& state) {
        luxdesktop::mpris_update(state);
    };
    ctl.discord_update = [](const lux_script::Value& state) {
        luxdesktop::discord_update(state);
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

// Waits until the server ANSWERS, not merely accepts: while `on start:` blocks
// run (migrations, applying the saved audio settings) Lux replies 503 to every
// request, and opening the window then would show an error page.
bool wait_for_server(uint16_t port, std::chrono::milliseconds timeout) {
    auto deadline = std::chrono::steady_clock::now() + timeout;
    while (std::chrono::steady_clock::now() < deadline) {
        int fd = ::socket(AF_INET, SOCK_STREAM, 0);
        if (fd >= 0) {
            timeval tv{1, 0};
            ::setsockopt(fd, SOL_SOCKET, SO_SNDTIMEO, &tv, sizeof(tv));
            ::setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &tv, sizeof(tv));
            sockaddr_in addr{};
            addr.sin_family      = AF_INET;
            addr.sin_port        = htons(port);
            addr.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
            bool ready = false;
            if (::connect(fd, reinterpret_cast<sockaddr*>(&addr), sizeof(addr)) == 0) {
                static const char req[] = "GET /api/ready HTTP/1.0\r\nHost: 127.0.0.1\r\n\r\n";
                char buf[16] = {};
                if (::send(fd, req, sizeof(req) - 1, 0) > 0 && ::recv(fd, buf, sizeof(buf) - 1, 0) > 0)
                    ready = std::strncmp(buf + 9, "503", 3) != 0;   // "HTTP/1.1 503 ..."
            }
            ::close(fd);
            if (ready) return true;
        }
        std::this_thread::sleep_for(std::chrono::milliseconds(20));
    }
    return false;
}

// `on start:` blocks still running: requests wait (503) until it is zero.
static std::atomic<int> g_starting{0};

// One `every` block: armed on the main loop, never run twice at once.
// (Same as ScheduledTask in vendor/lux/src/lux_script/main.cpp, which is
// private to the `lux` executable.)
struct ScheduledTask : std::enable_shared_from_this<ScheduledTask> {
    std::string           path, label;
    lux_script::EverySpec spec;
    lux::DispatchFn       dispatch;
    bool                  once = false;      // `on start:`
    bool                  running = false;   // touched only on the loop's thread

    static long long now() {
        return std::chrono::duration_cast<std::chrono::milliseconds>(
            std::chrono::system_clock::now().time_since_epoch()).count();
    }

    // In steps of at most an hour: schedule_timer takes an int of ms.
    void arm(lux::core::EventLoop& loop, long long due) {
        const int wait = static_cast<int>(std::clamp(due - now(), 0LL, 3'600'000LL));
        loop.schedule_timer(wait, [self = shared_from_this(), &loop, due] {
            if (now() < due) { self->arm(loop, due); return; }
            self->run(loop);
            self->arm(loop, self->spec.after(due, now()));
        });
    }

    void run(lux::core::EventLoop& loop) {
        if (running) {
            lux::log().warn("every \"", label, "\": the previous run has not finished, skipping this one");
            return;
        }
        running = true;
        auto req = std::make_shared<lux::Request>();
        auto res = std::make_shared<lux::Response>();
        req->method    = "EVERY";
        req->path      = path;
        req->remote_ip = "127.0.0.1";
        req->loop      = &loop;
        auto task = [](std::shared_ptr<ScheduledTask> self, std::shared_ptr<lux::Request> req,
                       std::shared_ptr<lux::Response> res) -> lux::Task<void> {
            try {
                co_await self->dispatch(*req, *res);
                if (res->status_code() >= 400)
                    lux::log().error("every \"", self->label, "\" failed: ", res->body());
            } catch (const std::exception& e) {
                lux::log().error("every \"", self->label, "\" failed: ", e.what());
            }
            self->running = false;
            if (self->once) g_starting.fetch_sub(1);
        }(shared_from_this(), req, res);
        auto h = task.detach();
        h.promise().loop = &loop;
        h.resume();
    }
};

void serve_module(lux::App& app,
                  std::function<std::shared_ptr<lux_script::Module>()> current,
                  std::function<void()> before_stop) {
    auto first = current();
    for (const auto& m : first->program.app.statics)
        app.serve_static(m.url_prefix, m.fs_root, m.spa);

    // The database pools have their own threads that resume handlers by
    // posting to the event loops: stop them while the loops are still alive.
    app.on_before_stop([before_stop] {
        lux_script::DbRegistry::instance().shutdown();
        if (before_stop) before_stop();
    });

    // Not a coroutine: it returns the route's own Task, and the request holds
    // the module alive for as long as that runs (a reload may publish another).
    lux::DispatchFn dispatch = [current](lux::Request& req, lux::Response& res) -> lux::Task<void> {
        static constexpr auto done = []() -> lux::Task<void> { co_return; };
        auto mod = current();
        if (g_starting.load() > 0 && req.method != "EVERY") {
            res.status(503).header("Retry-After", "1").json_text(R"({"error":"starting"})");
            return done();
        }
        if (!mod) { res.status(503).json_text(R"({"error":"no module loaded"})"); return done(); }
        auto match = mod->router.match(req.method, req.path);
        if (!match.found) {
            res.status(404).json_text(R"({"error":"Not Found"})");
            return done();
        }
        req.params = std::move(match.params);
        req._hold  = mod;
        return (*match.handler)(req, res);
    };
    app.any("/",  dispatch);
    app.any("/*", dispatch);

    // router_ only holds the two catch-alls above: ask the module's REAL
    // router whether a route lives here, so a static mount and a live route
    // never disagree about which one wins.
    app.set_route_probe([current](const std::string& method, const std::string& path) {
        auto mod = current();
        return mod && mod->router.match(method, path).found;
    });

    app.on_async_error([current](int code, lux::Request& req, lux::Response& res) -> lux::Task<void> {
        auto mod = current();   // a copy: it stays alive across the handler's awaits
        if (!mod) co_return;
        co_await lux_script::run_error_handler(*mod, code, req, res);
    });

    // `every` / `on start:`: one timer each on the main loop, dispatched like
    // a request to the live module. A new task or schedule needs a restart.
    std::vector<std::shared_ptr<ScheduledTask>> tasks;
    for (const auto& r : first->program.routes) {
        if (r.method != "EVERY" || r.every.rfind("cmd:", 0) == 0 || r.every.rfind("test:", 0) == 0) continue;
        auto t = std::make_shared<ScheduledTask>();
        t->path     = r.pattern;
        t->label    = r.every;
        t->dispatch = dispatch;
        if (r.every == "start") { t->once = true; g_starting.fetch_add(1); }
        else lux_script::parse_every_spec(r.every, t->spec);
        tasks.push_back(std::move(t));
    }
    if (!tasks.empty())
        app.on_start([tasks](lux::core::EventLoop& loop) {
            for (const auto& t : tasks) {
                if (t->once) t->run(loop);
                else t->arm(loop, t->spec.next(ScheduledTask::now()));
            }
        });
}


AudioEngine& audio_engine() {
    static AudioEngine engine;
    return engine;
}

void install_audio_hooks() {
    using lux_script::Value;
    auto& ctl = lux_script::audio_control();
    auto& e = audio_engine();
    e.start_device();   // a failure leaves the engine silent but usable: the backend sees it in status()

    ctl.load = [&e](const std::string& path, const std::string& id, int start_ms, int dur_ms, bool play) {
        return e.load(path, id, start_ms, dur_ms, play);
    };
    ctl.preload       = [&e](const std::string& path, const std::string& id, int dur_ms) { e.preload(path, id, dur_ms); };
    ctl.play          = [&e] { e.play(); };
    ctl.pause         = [&e] { e.pause(); };
    ctl.stop          = [&e] { e.stop(); };
    ctl.seek          = [&e](int ms) { e.seek(ms); };
    ctl.set_volume    = [&e](double v) { e.set_volume(v); };
    ctl.set_eq        = [&e](const std::vector<double>& g) { e.set_eq(g.data()); };
    ctl.set_crossfade = [&e](int ms) { e.set_crossfade(ms); };
    ctl.status = [&e]() {
        const AudioStatus st = e.status();
        Value::Dict d;
        d["trackId"]    = Value::str(st.track_id);
        d["positionMs"] = Value::integer(st.position_ms);
        d["durationMs"] = Value::integer(st.duration_ms);
        d["bufferedMs"] = Value::integer(st.buffered_ms);
        d["listenedMs"] = Value::integer(st.listened_ms);
        d["playing"]    = Value::boolean(st.playing);
        d["underruns"]  = Value::integer(st.underruns);
        return Value::dict(std::move(d));
    };
    ctl.take_events = [&e]() {
        Value::List out;
        for (const auto& ev : e.take_events()) {
            Value::Dict d;
            d["type"] = Value::str(ev.type == AudioEvent::Handoff ? "handoff" : ev.type == AudioEvent::Ended ? "ended" : "error");
            d["trackId"] = Value::str(ev.track_id);
            d["listenedMs"] = Value::integer(ev.listened_ms);
            out.push_back(Value::dict(std::move(d)));
        }
        return Value::list(std::move(out));
    };
    ctl.devices = [&e]() {
        Value::List out;
        for (const auto& dev : e.list_devices()) {
            Value::Dict d;
            d["id"]        = Value::str(dev.id);
            d["name"]      = Value::str(dev.name);
            d["isDefault"] = Value::boolean(dev.is_default);
            out.push_back(Value::dict(std::move(d)));
        }
        return Value::list(std::move(out));
    };
    ctl.set_device = [&e](const std::string& id) { return e.start_device(id); };
}

void shutdown_audio() { audio_engine().stop_device(); }

} // namespace luxdesktop

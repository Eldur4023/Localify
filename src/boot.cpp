#include "boot.hpp"
#include "resources.hpp"

#include <lux/logger.hpp>
#include <lux_script/schedule.hpp>
#include <lux_script/vm.hpp>

#ifdef __ANDROID__
#include <android/log.h>
#define BOOTLOG(...) __android_log_print(ANDROID_LOG_INFO, "luxdesktop", __VA_ARGS__)
#else
#define BOOTLOG(...) do {} while (0)   // diagnóstico solo en el móvil
#endif

#include <arpa/inet.h>
#include <cerrno>
#include <atomic>
#include <csignal>
#include <cstring>
#include <fstream>
#include <netinet/in.h>
#include <sys/socket.h>
#include <unistd.h>

namespace fs = std::filesystem;

namespace {

// Mientras haya bloques `on start:` en vuelo (migraciones), las peticiones
// HTTP reciben 503 "starting" — igual que hace el main de Lux. Sin la puerta,
// las primeras peticiones del WebView compiten con las migraciones por el
// turno de escritura del pool y éstas se cuelgan.
std::atomic<int> g_arrancando{0};

void extract_resources(const fs::path& dir) {
    for (const auto& f : kEmbeddedFiles) {
        fs::path dest = dir / f.path;
        fs::create_directories(dest.parent_path());
        std::ofstream out(dest, std::ios::binary);
        out.write(reinterpret_cast<const char*>(f.data), static_cast<std::streamsize>(f.size));
    }
}

// Binds to loopback with port 0 (the OS picks a free ephemeral port), reads
// it back with getsockname(), then releases it immediately. A small race
// (something else could grab the same port before Lux's own bind) is the
// same trade-off every "find a free port" helper makes; fine for an app
// that only ever talks to itself.
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

LuxServer::LuxServer(const fs::path& work_dir) {
    // Antes que el bundle de certificados: en una instalación limpia la carpeta aún no
    // existe, el PEM no se podía escribir y TODO https fallaba hasta el segundo arranque.
    fs::create_directories(work_dir);
#ifdef __ANDROID__
    // Android no trae un cacert.pem y su almacén (un fichero por CA, con el
    // hash antiguo de OpenSSL 1.0) no sirve de CAPATH a OpenSSL 3: se junta en
    // un solo PEM, que el módulo http de Lux usa como CAINFO.
    {
        fs::path bundle = work_dir / "cacert.pem";
        std::ofstream out(bundle, std::ios::binary);
        std::error_code ec;
        for (const auto& e : fs::directory_iterator("/apex/com.android.conscrypt/cacerts", ec)) {
            std::ifstream in(e.path(), std::ios::binary);
            out << in.rdbuf() << '\n';
        }
        out.close();
        ::setenv("LUX_CA_BUNDLE", bundle.c_str(), 1);
    }
#endif
    fs::create_directories(work_dir);
    extract_resources(work_dir);
    fs::current_path(work_dir); // the app's own paths (templates "./templates",
                                 // static "/x" -> "./public") are written
                                 // relative to itself, so this is the CWD
                                 // they expect.

    std::vector<fs::path> inputs;
    std::string error;
    if (!lux_script::resolve_inputs({"."}, inputs, error)) throw std::runtime_error(error);

    lux_script::DiagnosticBag diags;
    mod_ = lux_script::compile(inputs, diags);
    if (!diags.empty()) throw std::runtime_error(lux_script::format_errors(diags, mod_->files));

    auto mod = mod_;
    // (sin set_templates: esta versión vendida de Lux no la tiene y la app no
    // usa templates de servidor — la UI vive en el montaje estático de public/)
    for (const auto& m : mod->program.app.statics)
        app_.serve_static(m.url_prefix, m.fs_root, m.spa);

    auto dispatch = [mod](lux::Request& req, lux::Response& res) -> lux::Task<void> {
        if (g_arrancando.load() > 0 && req.method != "EVERY") {
            res.status(503).header("Retry-After", "1").json_text(R"({"error":"starting"})");
            co_return;
        }
        auto match = mod->router.match(req.method, req.path);
        if (!match.found) {
            res.status(404).json_text(R"({"error":"Not Found"})");
            co_return;
        }
        req.params = std::move(match.params);
        req._hold  = mod;   // el módulo vive mientras haya una petición en vuelo
        co_return co_await (*match.handler)(req, res);
    };
    app_.any("/",  dispatch);
    app_.any("/*", dispatch);

    // Los bloques `on start:` (migraciones de db.lux —sin esto la base de
    // datos nunca llega a crearse—) y `every ...:` (colas, descargas, relay).
    // Espejo de lo que el main de Lux hace con LUX_EMBEDDED, pero con los
    // primitivos del EventLoop: ScheduledTask vive en main.cpp y no se exporta.
    struct Tarea { std::string path, label; long long period_ms; };
    std::vector<Tarea> tareas;
    for (const auto& r : mod->program.routes) {
        if (r.method != "EVERY" || r.every.rfind("cmd:", 0) == 0 || r.every.rfind("test:", 0) == 0)
            continue;   // commands and tests only run from `lux run` / `lux test`
        if (r.every == "start") tareas.push_back({r.pattern, "start", 0});
        else {
            lux_script::EverySpec spec;
            if (lux_script::parse_every_spec(r.every, spec))
                tareas.push_back({r.pattern, r.every, spec.period_ms});
            else
                lux::log().warn("every \"", r.every, "\" no se pudo interpretar; bloque ignorado");
        }
    }
    BOOTLOG("on start programado: %zu tarea(s)", tareas.size());
    if (!tareas.empty())
        app_.on_start([mod, dispatch, tareas](lux::core::EventLoop& loop) {
            BOOTLOG("on start: ejecutando %zu tarea(s)", tareas.size());
            auto lanzar = [dispatch](lux::core::EventLoop& loop, const Tarea& t,
                                     const std::shared_ptr<std::atomic<bool>>& running) {
                if (running->exchange(true)) return;   // la anterior no ha terminado
                if (t.period_ms == 0) g_arrancando++;
                auto req  = std::make_shared<lux::Request>();
                auto res  = std::make_shared<lux::Response>();
                req->method    = "EVERY";
                req->path      = t.path;
                req->remote_ip = "127.0.0.1";
                req->loop      = &loop;
                // Parámetros, no capturas: la lambda temporal muere en el primer
                // co_await y sus capturas quedarían colgando (use-after-free).
                auto task = [](lux::DispatchFn dispatch, Tarea t, std::shared_ptr<lux::Request> req,
                               std::shared_ptr<lux::Response> res, std::shared_ptr<std::atomic<bool>> running) -> lux::Task<void> {
                    try {
                        co_await dispatch(*req, *res);
                        if (res->status_code() >= 400)
                            lux::log().error("every \"", t.label, "\" failed: ", res->body());
                    } catch (const std::exception& e) {
                        lux::log().error("every \"", t.label, "\" failed: ", e.what());
                    }
                    running->store(false);
                    if (t.period_ms == 0) g_arrancando--;
                }(dispatch, t, req, res, running);
                auto h = task.detach();
                h.promise().loop = &loop;
                h.resume();
            };
            // Las periódicas NO arrancan hasta que los `on start` (migraciones)
            // terminan: sin esta espera, el primer tick escribe player_state
            // antes de que la tabla exista, y el error + el intento de
            // respuesta matan el proceso antes de abrir la puerta.
            for (const auto& t : tareas)
                if (t.period_ms == 0) lanzar(loop, t, std::make_shared<std::atomic<bool>>(false));   // on start
            if (std::none_of(tareas.begin(), tareas.end(),
                             [](const Tarea& t) { return t.period_ms > 0; }))
                return;
            auto rearmar = std::make_shared<std::function<void(lux::core::EventLoop&)>>();
            *rearmar = [rearmar, lanzar, tareas](lux::core::EventLoop& loop) {
                if (g_arrancando.load() > 0) {
                    loop.schedule_timer(200, [rearmar, &loop] { (*rearmar)(loop); });
                    return;
                }
                for (const auto& t : tareas) {
                    if (t.period_ms == 0) continue;
                    auto paso = std::make_shared<std::function<void()>>();
                    auto running = std::make_shared<std::atomic<bool>>(false);
                    *paso = [paso, &loop, lanzar, t, running]() {
                        lanzar(loop, t, running);
                        loop.schedule_timer(static_cast<int>(t.period_ms), [paso] { (*paso)(); });
                    };
                    loop.schedule_timer(static_cast<int>(t.period_ms), [paso] { (*paso)(); });
                }
            };
            (*rearmar)(loop);
        });

    app_.on_error([mod](int code, lux::Request& req, lux::Response& res) {
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
}

uint16_t LuxServer::start() {
    uint16_t port = find_free_port();
    thread_ = std::thread([this, port] { app_.run("127.0.0.1", port); });
    if (!wait_for_server(port, std::chrono::milliseconds(5000)))
        lux::log().warn("server did not come up in time, continuing anyway");
    return port;
}

void LuxServer::stop() {
    if (!thread_.joinable()) return;
    std::raise(SIGTERM);
    thread_.join();
}

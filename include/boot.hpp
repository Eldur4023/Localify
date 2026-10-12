#pragma once
// The platform-independent half of every Lux Desktop app: unpack the
// embedded resources, compile them with Lux's own compiler, run the HTTP
// server on loopback. No window, no GTK -- the desktop shell
// (src/runtime.cpp) and the Android one (src/android.cpp) both sit on top
// of this and only differ in what they point at the returned port.
#include <cstdint>
#include <filesystem>
#include <memory>
#include <thread>

#include <lux/app.hpp>
#include <lux_script/project.hpp>

class LuxServer {
public:
    // Extracts the embedded resources into work_dir (persistent: ./data/
    // lives there), makes it the CWD and compiles the app. Throws
    // std::runtime_error with the compiler's own diagnostics on failure.
    explicit LuxServer(const std::filesystem::path& work_dir);

    const std::shared_ptr<lux_script::Module>& module() const { return mod_; }
    lux::App& app() { return app_; }

    // Starts serving on 127.0.0.1:<free port> in a background thread and
    // returns that port once it accepts connections.
    uint16_t start();

    // stop() asks the server to drain (SIGTERM, the path lux::App already
    // handles) and joins; join() only joins, for when a signal already
    // started the shutdown.
    void stop();
    void join() { if (thread_.joinable()) thread_.join(); }

private:
    std::shared_ptr<lux_script::Module> mod_;
    lux::App                            app_;
    std::thread                         thread_;
};

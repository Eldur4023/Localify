// A server with no window: compiles an app directory, starts the audio engine
// and serves it on a port. For end-to-end tests (tests/e2e_audio.py) and for
// poking at the backend from a terminal. Set LUX_AUDIO_NULL=1 and nothing is
// played: the engine still runs, it just has no sound card behind it.
//
//   app-headless <app-dir> [port]       (port 0 = pick one; prints it)
#include "desktop_common.hpp"

#include <lux/app.hpp>
#include <lux/logger.hpp>
#include <lux_script/project.hpp>

#include <csignal>
#include <iostream>
#include <thread>

namespace fs = std::filesystem;
using namespace luxdesktop;

int main(int argc, char** argv) {
    if (argc < 2) { std::cerr << "usage: app-headless <app-dir> [port]\n"; return 2; }
    fs::current_path(argv[1]);
    std::vector<fs::path> inputs;
    std::string error;
    if (!lux_script::resolve_inputs({"."}, inputs, error)) { std::cerr << error << "\n"; return 1; }
    lux_script::DiagnosticBag diags;
    auto mod = lux_script::compile(inputs, diags);
    if (!diags.empty()) { std::cerr << lux_script::format_errors(diags, mod->files); return 1; }

    lux::App app;
    serve_module(app, [mod] { return mod; });
    install_audio_hooks();
    const uint16_t port = argc > 2 && std::atoi(argv[2]) > 0 ? static_cast<uint16_t>(std::atoi(argv[2])) : find_free_port();
    std::cout << "port " << port << std::endl;
    app.run("127.0.0.1", port);   // returns on SIGTERM/SIGINT
    shutdown_audio();
    return 0;
}

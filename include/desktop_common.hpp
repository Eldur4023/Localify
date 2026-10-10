#pragma once
// Shared by src/runtime.cpp and src/dev.cpp -- see src/desktop_common.cpp.
#include "audio_engine.hpp"
#include "desktop_window.hpp"

#include <lux_script/project.hpp>

#include <lux/app.hpp>

#include <atomic>
#include <chrono>
#include <functional>
#include <cstdint>
#include <filesystem>
#include <memory>
#include <mutex>
#include <string>

namespace luxdesktop {

extern std::atomic<bool> g_shutdown_from_signal;
extern DesktopWindow*    g_window;
extern std::mutex        g_window_mutex;

void        install_window_control_hooks();

// The one audio engine of the process, and the `audio` module's hooks onto it.
// install_audio_hooks() also opens the output device (the system default; the
// backend picks another one later with audio.set_device()).
AudioEngine& audio_engine();
void         install_audio_hooks();
void         shutdown_audio();
uint16_t    find_free_port();
std::string sanitize_id(const std::string& name);
std::string resolve_display_name(const std::shared_ptr<lux_script::Module>& mod);
bool        load_saved_geometry(const std::string& id, int& width, int& height);
void        save_geometry(const std::string& id, int width, int height);
bool        wait_for_server(uint16_t port, std::chrono::milliseconds timeout);

// Mounts the module `current()` returns onto `app` exactly the way the `lux`
// binary does (vendor/lux/src/lux_script/main.cpp): static mounts, one
// catch-all dispatcher (so `get`/`post`/`sse`/`ws` routes all work), `on error`
// handlers, and the `every "..."` / `on start:` blocks as timers on the main
// loop. `current` is asked again on every request, so dev mode can swap the
// module on a hot reload. `before_stop` runs (after the database pools are
// shut down) once the server has drained.
void serve_module(lux::App& app,
                  std::function<std::shared_ptr<lux_script::Module>()> current,
                  std::function<void()> before_stop = {});

} // namespace luxdesktop

#pragma once
// Shared by src/runtime.cpp and src/dev.cpp -- see src/desktop_common.cpp.
#include "desktop_window.hpp"

#include <lux_script/project.hpp>

#include <atomic>
#include <chrono>
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
uint16_t    find_free_port();
std::string sanitize_id(const std::string& name);
std::string resolve_display_name(const std::shared_ptr<lux_script::Module>& mod);
bool        load_saved_geometry(const std::string& id, int& width, int& height);
void        save_geometry(const std::string& id, int width, int height);
bool        wait_for_server(uint16_t port, std::chrono::milliseconds timeout);

} // namespace luxdesktop

#pragma once
#include <string>

namespace lux_script {

// Populated by the `window` native module's configure()
// (src/lux_script/modules/window.cpp) from the app: block's
// `window: { ... }` sub-block, at compile time -- read back by the desktop
// shell (see ../../../../src/runtime.cpp, outside this vendored copy) right
// after lux_script::compile() returns, to size/title the native window.
// `window` also exposes a handful of callable functions (set_title,
// minimize, the file dialogs...) -- see window_control.hpp for those; this
// struct is purely the config namespace, the exact same mechanism
// `sqlite: { ... }` already uses for `file`/`pool`.
struct WindowConfig {
    bool        present   = false; // true once `import window` + configure() ran
    std::string title;
    int         width     = 1024;
    int         height    = 768;
    bool        resizable = true;
    bool        devtools  = false;
    std::string icon;             // path, relative to the app's own directory
};

WindowConfig& window_config();

} // namespace lux_script

#pragma once
#include <cstddef>
#include <vector>

// One entry per file embedded into the binary by respack (tools/respack) at
// build time. `path` is relative to the app's own source directory (e.g.
// "app.lux", "templates/index.html") -- runtime.cpp recreates that same
// relative layout under a temp directory before handing it to Lux.
struct EmbeddedFile {
    const char*          path;
    const unsigned char* data;
    std::size_t          size;
};

// Defined by the generated <app>_resources.cpp (one per app, see
// add_lux_desktop_app() in CMakeLists.txt) -- never hand-written.
extern const std::vector<EmbeddedFile> kEmbeddedFiles;

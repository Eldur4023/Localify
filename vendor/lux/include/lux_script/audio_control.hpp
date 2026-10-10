#pragma once
#include <functional>
#include <string>
#include <vector>

#include "value.hpp"

namespace lux_script {

// The `audio` module's callable functions (audio.load(), audio.pause()...)
// are backed by these hooks, installed by the desktop shell (src/runtime.cpp,
// src/dev.cpp, outside this vendored copy) once its audio engine exists --
// Lux itself has no concept of one. Same arrangement as window_control.hpp.
// Every hook defaults to an empty std::function and is checked before use,
// so `import audio` compiles and a plain `lux --check` works with none set.
//
// Everything is synchronous: the engine mixes on its own thread, so these
// only set a flag, hand over a path or read an atomic.
struct AudioControl {
    // Replaces what is playing. start_ms = where to begin, duration_ms = the
    // track's length from its metadata (the crossfade needs it). Returns
    // false if the engine did not accept it.
    std::function<bool(const std::string& path, const std::string& track_id,
                       int start_ms, int duration_ms, bool play)> load;
    // What plays after the current track, decoded ahead for a gapless or
    // crossfaded handover. An empty path clears it.
    std::function<void(const std::string& path, const std::string& track_id, int duration_ms)> preload;
    std::function<void()>                    play;
    std::function<void()>                    pause;
    std::function<void()>                    stop;
    std::function<void(int ms)>              seek;
    std::function<void(double v)>            set_volume;      // 0..1, perceptual
    std::function<void(const std::vector<double>& gains_db)> set_eq;   // 10 bands
    std::function<void(int ms)>              set_crossfade;

    // {trackId, positionMs, durationMs, bufferedMs, listenedMs, playing, underruns}
    std::function<Value()>                   status;
    // [{type: "handoff" | "ended" | "error", trackId, listenedMs}], oldest first, then cleared
    std::function<Value()>                   take_events;
    // [{id, name, isDefault}]
    std::function<Value()>                   devices;
    // "" = the system default
    std::function<bool(const std::string& id)> set_device;
};

AudioControl& audio_control();

} // namespace lux_script

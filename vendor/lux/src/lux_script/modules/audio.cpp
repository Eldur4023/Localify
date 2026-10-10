// audio module (NATIVE-MODULES.md's drop-in pattern): control of the desktop
// shell's native audio engine from a route handler or an `every` task. Backed
// by hooks the shell installs -- see audio_control.hpp. With no engine
// running (plain `lux`, `--check`) every call is a harmless no-op.
#include <lux_script/audio_control.hpp>
#include <lux_script/builtin_module.hpp>

namespace lux_script {

AudioControl& audio_control() {
    static AudioControl ctl;
    return ctl;
}

namespace {

Value fn_audio_load(NativeCtx&, std::vector<Value>& a, std::string&) {
    auto& c = audio_control();
    return Value::boolean(c.load && c.load(a[0].as_str(), a[1].as_str(), static_cast<int>(a[2].as_int()),
                                           static_cast<int>(a[3].as_int()), a[4].as_bool()));
}

Value fn_audio_preload(NativeCtx&, std::vector<Value>& a, std::string&) {
    auto& c = audio_control();
    if (c.preload) c.preload(a[0].as_str(), a[1].as_str(), static_cast<int>(a[2].as_int()));
    return Value::null();
}

Value fn_audio_play(NativeCtx&, std::vector<Value>&, std::string&)  { if (auto& c = audio_control(); c.play)  c.play();  return Value::null(); }
Value fn_audio_pause(NativeCtx&, std::vector<Value>&, std::string&) { if (auto& c = audio_control(); c.pause) c.pause(); return Value::null(); }
Value fn_audio_stop(NativeCtx&, std::vector<Value>&, std::string&)  { if (auto& c = audio_control(); c.stop)  c.stop();  return Value::null(); }

Value fn_audio_seek(NativeCtx&, std::vector<Value>& a, std::string&) {
    if (auto& c = audio_control(); c.seek) c.seek(static_cast<int>(a[0].as_int()));
    return Value::null();
}

Value fn_audio_set_volume(NativeCtx&, std::vector<Value>& a, std::string&) {
    if (auto& c = audio_control(); c.set_volume) c.set_volume(a[0].as_float());
    return Value::null();
}

Value fn_audio_set_eq(NativeCtx&, std::vector<Value>& a, std::string& error) {
    auto& l = a[0].as_list();
    if (l.size() != 10) { error = "audio.set_eq() expects 10 gains in dB (31 Hz ... 16 kHz)"; return Value::null(); }
    std::vector<double> g;
    for (const auto& v : l) {
        if (!v.is_num()) { error = "audio.set_eq(): every gain must be a number"; return Value::null(); }
        g.push_back(v.as_float());
    }
    if (auto& c = audio_control(); c.set_eq) c.set_eq(g);
    return Value::null();
}

Value fn_audio_set_crossfade(NativeCtx&, std::vector<Value>& a, std::string&) {
    if (auto& c = audio_control(); c.set_crossfade) c.set_crossfade(static_cast<int>(a[0].as_int()));
    return Value::null();
}

Value fn_audio_status(NativeCtx&, std::vector<Value>&, std::string&) {
    auto& c = audio_control();
    if (c.status) return c.status();
    Value::Dict d;
    d["trackId"]     = Value::str("");
    d["positionMs"]  = Value::integer(0);
    d["durationMs"]  = Value::integer(0);
    d["bufferedMs"]  = Value::integer(0);
    d["listenedMs"]  = Value::integer(0);
    d["playing"]     = Value::boolean(false);
    d["underruns"]   = Value::integer(0);
    return Value::dict(std::move(d));
}

Value fn_audio_take_events(NativeCtx&, std::vector<Value>&, std::string&) {
    auto& c = audio_control();
    return c.take_events ? c.take_events() : Value::list();
}

Value fn_audio_devices(NativeCtx&, std::vector<Value>&, std::string&) {
    auto& c = audio_control();
    return c.devices ? c.devices() : Value::list();
}

Value fn_audio_set_device(NativeCtx&, std::vector<Value>& a, std::string&) {
    auto& c = audio_control();
    return Value::boolean(c.set_device && c.set_device(a[0].as_str()));
}

class AudioModule : public BuiltinModule {
public:
    const char* name() const override { return "audio"; }

    const std::vector<BuiltinModuleFn>& functions() const override {
        static const std::vector<BuiltinModuleFn> fns = {
            {"load",          "ssiib>b", fn_audio_load},
            {"preload",       "ssi",     fn_audio_preload},
            {"play",          "",        fn_audio_play},
            {"pause",         "",        fn_audio_pause},
            {"stop",          "",        fn_audio_stop},
            {"seek",          "i",       fn_audio_seek},
            {"set_volume",    "n",       fn_audio_set_volume},
            {"set_eq",        "l",       fn_audio_set_eq},
            {"set_crossfade", "i",       fn_audio_set_crossfade},
            {"status",        "",        fn_audio_status},
            {"take_events",   ">l",      fn_audio_take_events},
            {"devices",       ">l",      fn_audio_devices},
            {"set_device",    "s>b",     fn_audio_set_device},
        };
        return fns;
    }
};

} // namespace

LUX_REGISTER_MODULE(AudioModule)

} // namespace lux_script

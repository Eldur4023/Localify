// Tests del motor de audio SIN dispositivo: llaman a render() a mano y
// miran las muestras, la posicion y los avisos. Necesita ffmpeg en el PATH
// (es lo que decodifica, igual que en la app).
#include "audio_engine.hpp"

#include <chrono>
#include <clocale>
#include <cmath>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <fstream>
#include <string>
#include <thread>
#include <vector>

using luxdesktop::AudioEngine;
using luxdesktop::AudioEvent;

static int g_fail = 0;
#define CHECK(cond, ...) do { if (!(cond)) { ++g_fail; std::printf("  FALLO l.%d: %s -- ", __LINE__, #cond); std::printf(__VA_ARGS__); std::printf("\n"); } } while (0)

static std::string tmp(const char* name) {
    const char* d = std::getenv("TMPDIR");
    return std::string(d && *d ? d : "/tmp") + "/lux-audio-test-" + name;
}

// WAV PCM16 estereo a 48 kHz con un seno.
static std::string wav(const char* name, double seconds, double hz, double amp) {
    const std::string path = tmp(name) + ".wav";
    const uint32_t frames = static_cast<uint32_t>(seconds * 48000), bytes = frames * 4;
    std::ofstream f(path, std::ios::binary);
    auto u32 = [&](uint32_t v) { f.write(reinterpret_cast<char*>(&v), 4); };
    auto u16 = [&](uint16_t v) { f.write(reinterpret_cast<char*>(&v), 2); };
    f.write("RIFF", 4); u32(36 + bytes); f.write("WAVEfmt ", 8); u32(16); u16(1); u16(2);
    u32(48000); u32(48000 * 4); u16(4); u16(16); f.write("data", 4); u32(bytes);
    for (uint32_t i = 0; i < frames; ++i) {
        const int16_t s = static_cast<int16_t>(std::sin(2 * M_PI * hz * i / 48000.0) * amp * 32767);
        u16(static_cast<uint16_t>(s)); u16(static_cast<uint16_t>(s));
    }
    return path;
}

static void sleep_ms(int ms) { std::this_thread::sleep_for(std::chrono::milliseconds(ms)); }

// Renderiza `ms` en trozos de 10 ms, dejando respirar a los decodificadores.
static std::vector<float> render_ms(AudioEngine& e, int ms) {
    std::vector<float> all;
    for (int t = 0; t < ms; t += 10) {
        float buf[480 * 2];
        e.render(buf, 480);
        all.insert(all.end(), buf, buf + 960);
        sleep_ms(1);
    }
    return all;
}

static float peak(const std::vector<float>& v, int from_ms, int to_ms) {
    float p = 0;
    for (size_t i = static_cast<size_t>(from_ms) * 96; i < std::min(v.size(), static_cast<size_t>(to_ms) * 96); ++i)
        p = std::max(p, std::fabs(v[i]));
    return p;
}

// Racha mas larga de muestras EXACTAMENTE a cero (un seno solo toca el cero
// una muestra suelta): una racha larga es un hueco.
static int longest_zero_run(const std::vector<float>& v, int from_ms, int to_ms) {
    int best = 0, run = 0;
    for (size_t i = static_cast<size_t>(from_ms) * 96; i + 1 < std::min(v.size(), static_cast<size_t>(to_ms) * 96); i += 2) {
        if (v[i] == 0.0f && v[i + 1] == 0.0f) best = std::max(best, ++run); else run = 0;
    }
    return best;
}

static void settle(AudioEngine& e) {           // deja que ffmpeg llene el anillo
    for (int i = 0; i < 60; ++i) { sleep_ms(10); float b[96]; (void)b; }
    (void)e;
}

// Hasta que la pista pedida este adoptada por el mezclador y tenga datos.
static void wait_adopted(AudioEngine& e) {
    for (int i = 0; i < 300; ++i) {
        float b[480 * 2];
        e.render(b, 480);
        if (e.status().buffered_ms > 0) return;
        sleep_ms(5);
    }
}

static const AudioEvent* find_event(const std::vector<AudioEvent>& evs, AudioEvent::Type t) {
    for (const auto& e : evs) if (e.type == t) return &e;
    return nullptr;
}

#define TEST(name) std::printf("%s\n", name)

int main() {
    // Como en la app (GTK hace setlocale): con coma decimal, el salto se rompía.
    if (!std::setlocale(LC_ALL, "es_ES.UTF-8")) std::setlocale(LC_ALL, "de_DE.UTF-8");
    const std::string a = wav("a", 2.0, 440, 0.5), b = wav("b", 2.0, 880, 0.5), shortw = wav("s", 0.3, 440, 0.5);
    const std::string tone1k = wav("k", 3.0, 1000, 0.1);

    TEST("reproduce y avanza la posicion");
    {
        AudioEngine e;
        e.load(a, "A", 0, 2000, true);
        auto st = e.status();
        CHECK(st.track_id == "A" && st.position_ms == 0, "id=%s pos=%d", st.track_id.c_str(), st.position_ms);
        settle(e); wait_adopted(e);
        auto s = render_ms(e, 1000);
        st = e.status();
        CHECK(peak(s, 100, 1000) > 0.45f && peak(s, 100, 1000) <= 0.5f, "pico=%f", peak(s, 100, 1000));
        CHECK(std::abs(st.position_ms - 1000) < 80, "pos=%d", st.position_ms);
        CHECK(std::abs(st.listened_ms - st.position_ms) < 80, "escuchado=%d pos=%d", st.listened_ms, st.position_ms);
        CHECK(st.playing, "playing");
    }

    TEST("pausa: silencio, la posicion no se mueve; reanudar continua");
    {
        AudioEngine e;
        e.load(a, "A", 0, 2000, true); settle(e); wait_adopted(e);
        render_ms(e, 300);
        e.pause();
        render_ms(e, 50);                           // la rampa de bajada
        const int pos = e.status().position_ms;
        auto s = render_ms(e, 200);
        CHECK(peak(s, 0, 200) == 0.0f, "pico en pausa=%f", peak(s, 0, 200));
        CHECK(e.status().position_ms == pos, "pos %d -> %d", pos, e.status().position_ms);
        e.play();
        s = render_ms(e, 200);
        CHECK(peak(s, 20, 200) > 0.4f, "pico al reanudar=%f", peak(s, 20, 200));
        CHECK(e.status().position_ms > pos + 150, "pos=%d", e.status().position_ms);
    }

    TEST("seek: la posicion salta de inmediato y el audio continua desde ahi");
    {
        AudioEngine e;
        e.load(a, "A", 0, 2000, true); settle(e); wait_adopted(e);
        render_ms(e, 200);
        e.seek(1500);
        CHECK(e.status().position_ms == 1500, "pos tras seek=%d", e.status().position_ms);
        settle(e);
        auto s = render_ms(e, 200);
        const int pos = e.status().position_ms;
        CHECK(pos >= 1500 && pos < 1750, "pos=%d", pos);
        CHECK(peak(s, 50, 200) > 0.4f, "pico=%f", peak(s, 50, 200));
        CHECK(e.status().listened_ms < 700, "escuchado=%d (el salto no debe contar)", e.status().listened_ms);
    }

    TEST("gapless: relevo sin hueco");
    {
        AudioEngine e;
        e.load(shortw, "A", 0, 300, true);   // 0,3 s
        const std::string b2 = wav("b2", 1.0, 880, 0.5);
        e.preload(b2, "B", 1000);
        settle(e); wait_adopted(e);
        auto s = render_ms(e, 800);
        auto evs = e.take_events();
        const auto* h = find_event(evs, AudioEvent::Handoff);
        CHECK(h && h->track_id == "B", "handoff a %s", h ? h->track_id.c_str() : "(ninguno)");
        CHECK(h && std::abs(h->listened_ms - 300) < 60, "escuchado de A=%d", h ? h->listened_ms : -1);
        CHECK(longest_zero_run(s, 30, 790) < 8, "hueco de %d frames", longest_zero_run(s, 30, 790));
        CHECK(e.status().track_id == "B", "suena %s", e.status().track_id.c_str());
        CHECK(e.status().position_ms > 350 && e.status().position_ms < 600, "pos de B=%d", e.status().position_ms);
    }

    TEST("crossfade: B entra mientras A sale y B ya va por delante al relevo");
    {
        AudioEngine e;
        e.set_crossfade(400);
        e.load(a, "A", 0, 2000, true);
        e.preload(b, "B", 2000);
        settle(e); wait_adopted(e);
        auto s = render_ms(e, 2100);
        auto evs = e.take_events();
        const auto* h = find_event(evs, AudioEvent::Handoff);
        CHECK(h && h->track_id == "B", "handoff");
        CHECK(longest_zero_run(s, 30, 2090) < 8, "hueco de %d frames", longest_zero_run(s, 30, 2090));
        const int pos = e.status().position_ms;     // 400 de solape + ~100 ya de B en solitario
        CHECK(pos > 420 && pos < 650, "pos de B=%d", pos);
        CHECK(peak(s, 1700, 1990) < 0.75f, "pico en el solape=%f", peak(s, 1700, 1990));
    }

    TEST("fin sin nada detras: Ended y el motor queda vacio");
    {
        AudioEngine e;
        e.load(shortw, "A", 0, 300, true); settle(e); wait_adopted(e);
        render_ms(e, 600);
        auto evs = e.take_events();
        const auto* en = find_event(evs, AudioEvent::Ended);
        CHECK(en && en->track_id == "A", "ended");
        CHECK(e.status().track_id.empty(), "sigue con %s", e.status().track_id.c_str());
    }

    TEST("un fichero que no existe da Error, no silencio mudo");
    {
        AudioEngine e;
        e.load("/no/existe.wav", "X", 0, 1000, true);
        settle(e);
        render_ms(e, 200);
        auto evs = e.take_events();
        CHECK(find_event(evs, AudioEvent::Error), "sin evento de error");
    }

    TEST("volumen: v^3");
    {
        AudioEngine e;
        e.set_volume(0.5);
        e.load(a, "A", 0, 2000, true); settle(e); wait_adopted(e);
        auto s = render_ms(e, 800);
        const float p = peak(s, 500, 800);
        CHECK(std::abs(p - 0.5f * 0.125f) < 0.01f, "pico=%f esperado %f", p, 0.5f * 0.125f);
    }

    TEST("ecualizador: +12 dB en 1 kHz multiplica por ~4");
    {
        AudioEngine e;
        double g[10] = {0, 0, 0, 0, 0, 12, 0, 0, 0, 0};
        e.set_eq(g);
        e.load(tone1k, "K", 0, 3000, true); settle(e); wait_adopted(e);
        auto s = render_ms(e, 1000);
        const float p = peak(s, 600, 1000);
        CHECK(p > 0.35f && p < 0.45f, "pico=%f esperado ~0.40", p);
    }

    std::printf(g_fail ? "\n%d FALLO(S)\n" : "\ntodo bien\n", g_fail);
    return g_fail ? 1 : 0;
}

// Ver include/audio_engine.hpp para el reparto de papeles.
//
// Piezas:
//   Voice   una pista en curso: un ffmpeg hijo que escupe PCM f32 estereo a
//           48 kHz por una tuberia, un hilo que la vuelca a un anillo SPSC
//           de 4 s, y los contadores que dicen donde va.
//   Impl    el estado del mezclador. Lo que toca el hilo de audio son
//           atomicos y campos que solo el toca; lo que crea/destruye voces
//           (abrir ffmpeg, matar, liberar) lo hace el hilo que llama a la
//           API, bajo `mu`.
//
// Ciclo de vida de una voz: la crea load()/preload() y la deja en `pending`
// o `next`; el hilo de audio la adopta como `cur`; al terminar o ser
// sustituida la marca `dead`; y reap() (siempre desde el lado de la API) la
// libera pasados 250 ms de verla muerta, que es mucho mas que un callback.
// Asi el hilo de audio nunca libera memoria ni toca un mutex.
#include "audio_engine.hpp"

#include <algorithm>
#include <chrono>
#include <cerrno>
#include <cmath>
#include <csignal>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <fcntl.h>
#include <spawn.h>
#include <sys/wait.h>
#include <thread>
#include <unistd.h>
#include <unordered_map>

#define MA_NO_DECODING
#define MA_NO_ENCODING
#define MA_NO_GENERATION
#define MA_NO_RESOURCE_MANAGER
#define MA_NO_NODE_GRAPH
#define MA_NO_ENGINE
#define MA_ENABLE_ONLY_SPECIFIC_BACKENDS
#define MA_ENABLE_PULSEAUDIO
#define MA_ENABLE_ALSA
#define MA_ENABLE_NULL
#define MINIAUDIO_IMPLEMENTATION
#pragma GCC diagnostic push
#pragma GCC diagnostic ignored "-Wunused-parameter"
#pragma GCC diagnostic ignored "-Wmissing-field-initializers"
#pragma GCC diagnostic ignored "-Wcast-function-type"
#pragma GCC diagnostic ignored "-Wunused-function"
#pragma GCC diagnostic ignored "-Wtype-limits"
#pragma GCC diagnostic ignored "-Wimplicit-fallthrough"
#include "miniaudio/miniaudio.h"
#pragma GCC diagnostic pop

extern char** environ;

namespace luxdesktop {

namespace {

constexpr uint64_t kCap       = 4ull * AudioEngine::kRate;   // anillo: 4 s por voz
constexpr int      kBlock     = 1024;                        // frames por trozo de render
constexpr int      kPrebuf    = 2400;                        // 50 ms: lo minimo para adoptar una voz
constexpr float    kGateStep  = 1.0f / 256.0f;               // rampa de pausa/salto: ~5 ms
constexpr int      kEvCap     = 32;

const double kBandHz[10] = {31, 62, 125, 250, 500, 1000, 2000, 4000, 8000, 16000};
constexpr double kBandQ  = 1.41;

struct Biquad {
    double b0 = 1, b1 = 0, b2 = 0, a1 = 0, a2 = 0;
    double z1 = 0, z2 = 0;
    float process(float x) {
        double y = b0 * x + z1;
        z1 = b1 * x - a1 * y + z2;
        z2 = b2 * x - a2 * y;
        return static_cast<float>(y);
    }
};

// Filtro "peaking" del cookbook de RBJ: lo mismo que BiquadFilterNode.
Biquad peaking(double hz, double q, double gain_db) {
    const double A  = std::pow(10.0, gain_db / 40.0);
    const double w0 = 2.0 * M_PI * hz / AudioEngine::kRate;
    const double al = std::sin(w0) / (2.0 * q);
    const double a0 = 1.0 + al / A;
    Biquad f;
    f.b0 = (1.0 + al * A) / a0;
    f.b1 = (-2.0 * std::cos(w0)) / a0;
    f.b2 = (1.0 - al * A) / a0;
    f.a1 = (-2.0 * std::cos(w0)) / a0;
    f.a2 = (1.0 - al / A) / a0;
    return f;
}

} // namespace

// ── Voice ───────────────────────────────────────────────────────────────────

struct AudioEngine::Voice {
    std::string path, track_id;
    uint64_t    start_frame = 0;       // donde cae el primer frame del anillo dentro de la pista
    uint64_t    duration_frames = 0;   // metadatos (0 = no se sabe)
    uint64_t    token = 0;
    bool        keep_listened = false; // un seek no reinicia lo escuchado

    std::vector<float>    buf;         // kCap * 2 floats
    std::atomic<uint64_t> head{0};     // frames producidos (monotono)
    std::atomic<uint64_t> tail{0};     // frames consumidos (monotono)
    std::atomic<bool>     eof{false};
    std::atomic<bool>     stop{false};
    std::atomic<bool>     dead{false};  // el hilo de audio ya no la usa

    // Solo del lado de la API:
    pid_t       pid = -1;
    int         fd  = -1;
    std::thread th;
    bool        seen_dead = false;
    std::chrono::steady_clock::time_point seen_at;

    Voice() : buf(kCap * 2) {}
    ~Voice() {
        shutdown();
        if (th.joinable()) th.join();
        if (fd >= 0) ::close(fd);
        if (pid > 0) ::waitpid(pid, nullptr, 0);
    }

    void shutdown() {
        stop.store(true, std::memory_order_release);
        if (pid > 0) ::kill(pid, SIGKILL);
    }

    // Lanza ffmpeg y el hilo que vuelca su salida al anillo.
    void start() {
        const char* bin = std::getenv("LUX_FFMPEG");
        // A mano, en milisegundos enteros: std::to_string(double) usa el locale
        // de C, y dentro de la app GTK ya hizo setlocale(): con es_ES salía
        // "126,326000" y ffmpeg no lo entendía (el salto no sonaba).
        const unsigned long long ms = start_frame * 1000 / AudioEngine::kRate;
        char frac[8];
        std::snprintf(frac, sizeof(frac), "%03llu", ms % 1000);
        std::string ss = std::to_string(ms / 1000) + "." + frac;
        std::vector<const char*> argv = {bin && *bin ? bin : "ffmpeg", "-nostdin", "-v", "quiet", "-threads", "1"};
        if (start_frame > 0) { argv.push_back("-ss"); argv.push_back(ss.c_str()); }
        for (const char* a : {"-i", path.c_str(), "-vn", "-sn", "-dn", "-ac", "2", "-ar", "48000",
                              "-f", "f32le", "pipe:1"})
            argv.push_back(a);
        argv.push_back(nullptr);

        int p[2];
        if (::pipe2(p, O_CLOEXEC) != 0) { eof.store(true); return; }
        posix_spawn_file_actions_t fa;
        posix_spawn_file_actions_init(&fa);
        posix_spawn_file_actions_adddup2(&fa, p[1], 1);
        posix_spawn_file_actions_addopen(&fa, 0, "/dev/null", O_RDONLY, 0);
        posix_spawn_file_actions_addopen(&fa, 2, "/dev/null", O_WRONLY, 0);
        int rc = posix_spawnp(&pid, argv[0], &fa, nullptr, const_cast<char* const*>(argv.data()), environ);
        posix_spawn_file_actions_destroy(&fa);
        ::close(p[1]);
        if (rc != 0) { pid = -1; ::close(p[0]); eof.store(true); return; }
        fd = p[0];
        th = std::thread([this] { decode_loop(); });
    }

    void decode_loop() {
        constexpr size_t kChunkFrames = 8192;
        std::vector<unsigned char> tmp(kChunkFrames * 8);
        size_t carry = 0;   // bytes de un frame a medias
        while (!stop.load(std::memory_order_acquire)) {
            const uint64_t h = head.load(std::memory_order_relaxed);
            const uint64_t space = kCap - (h - tail.load(std::memory_order_acquire));
            if (space < 2048) { std::this_thread::sleep_for(std::chrono::milliseconds(5)); continue; }
            const size_t want = std::min<uint64_t>(space, kChunkFrames) * 8 - carry;
            const ssize_t n = ::read(fd, tmp.data() + carry, want);
            if (n < 0) { if (errno == EINTR) continue; break; }
            if (n == 0) break;
            const size_t bytes = carry + static_cast<size_t>(n);
            const size_t frames = bytes / 8;
            carry = bytes % 8;
            const uint64_t idx = h % kCap;
            const size_t first = std::min<uint64_t>(frames, kCap - idx);
            std::memcpy(&buf[idx * 2], tmp.data(), first * 8);
            if (first < frames) std::memcpy(&buf[0], tmp.data() + first * 8, (frames - first) * 8);
            if (carry) std::memmove(tmp.data(), tmp.data() + frames * 8, carry);
            head.store(h + frames, std::memory_order_release);
        }
        eof.store(true, std::memory_order_release);
    }
};

// ── Impl ────────────────────────────────────────────────────────────────────

struct AudioEngine::Impl {
    struct Ev { int type; uint64_t token; uint64_t listened_frames; };

    // Compartidos con el hilo de audio.
    std::atomic<Voice*> cur{nullptr};      // solo el hilo de audio la cambia
    std::atomic<Voice*> pending{nullptr};  // cargada, a la espera de relevar a `cur`
    std::atomic<Voice*> next{nullptr};     // precargada para el relevo
    std::atomic<bool>   want_play{false};
    std::atomic<bool>   unload_req{false};
    std::atomic<int>    xfade_frames{0};
    std::atomic<float>  vol_target{1.0f};
    std::atomic<float>  eq_gain[10];
    std::atomic<uint32_t> eq_version{1};
    std::atomic<uint64_t> listened{0};
    std::atomic<int>    underruns{0};
    Ev                  evbuf[kEvCap];
    std::atomic<uint32_t> ev_head{0}, ev_tail{0};

    // Solo del hilo de audio.
    float   gate = 0.0f;
    float   vol  = 1.0f;
    Biquad  eq[10][2];
    bool    eq_on[10] = {};
    bool    eq_active = false;
    uint32_t eq_applied = 0;

    // Solo del lado de la API (bajo mu).
    std::mutex mu;
    std::vector<std::unique_ptr<Voice>> all;
    std::unordered_map<uint64_t, std::string> names;   // token -> track id
    uint64_t next_token = 1;

    // Dispositivo.
    ma_context ctx{};
    ma_device  dev{};
    bool       ctx_ok = false;
    bool       dev_ok = false;
    AudioEngine* owner = nullptr;

    Impl() { for (auto& g : eq_gain) g.store(0.0f); }

    // El id de un dispositivo como texto: en PulseAudio/PipeWire es el nombre
    // del sink (el mismo que `pactl list sinks`), que es lo que guardaban los
    // ajustes de antes del motor nativo.
    std::string id_text(const ma_device_id& id) const {
        switch (ctx.backend) {
            case ma_backend_pulseaudio: return id.pulse;
            case ma_backend_alsa:       return id.alsa;
            default:                    return std::to_string(id.nullbackend);
        }
    }

    bool open_context(const std::string& app) {
        if (ctx_ok) return true;
        ma_context_config cc = ma_context_config_init();
        cc.pulse.pApplicationName = app.c_str();
        std::vector<ma_backend> backends;
        if (const char* nul = std::getenv("LUX_AUDIO_NULL"); nul && *nul == '1') backends = {ma_backend_null};
        else backends = {ma_backend_pulseaudio, ma_backend_alsa};
        if (ma_context_init(backends.data(), static_cast<ma_uint32>(backends.size()), &cc, &ctx) != MA_SUCCESS)
            return false;
        ctx_ok = true;
        return true;
    }

    // ─ hilo de audio ────────────────────────────────────────────────────────

    void emit(int type, uint64_t token, uint64_t listened_frames) {
        const uint32_t h = ev_head.load(std::memory_order_relaxed);
        if (h - ev_tail.load(std::memory_order_acquire) >= kEvCap) return;   // lleno: se pierde
        evbuf[h % kEvCap] = {type, token, listened_frames};
        ev_head.store(h + 1, std::memory_order_release);
    }

    static int ring_read(Voice* v, float* dst, int maxf) {
        const uint64_t t = v->tail.load(std::memory_order_relaxed);
        const uint64_t h = v->head.load(std::memory_order_acquire);
        const int avail = static_cast<int>(std::min<uint64_t>(h - t, static_cast<uint64_t>(maxf)));
        if (avail <= 0) return 0;
        const uint64_t idx = t % kCap;
        const int first = static_cast<int>(std::min<uint64_t>(avail, kCap - idx));
        std::memcpy(dst, &v->buf[idx * 2], static_cast<size_t>(first) * 8);
        if (first < avail) std::memcpy(dst + first * 2, &v->buf[0], static_cast<size_t>(avail - first) * 8);
        v->tail.store(t + avail, std::memory_order_release);
        return avail;
    }

    static bool ready(Voice* v) {
        return v && (v->head.load(std::memory_order_acquire) >= kPrebuf || v->eof.load(std::memory_order_acquire));
    }

    void refresh_eq() {
        const uint32_t ver = eq_version.load(std::memory_order_acquire);
        if (ver == eq_applied) return;
        eq_applied = ver;
        eq_active = false;
        for (int b = 0; b < 10; ++b) {
            const double g = eq_gain[b].load(std::memory_order_relaxed);
            eq_on[b] = std::fabs(g) > 0.05;
            if (!eq_on[b]) continue;
            eq_active = true;
            Biquad f = peaking(kBandHz[b], kBandQ, g);
            for (int ch = 0; ch < 2; ++ch) {   // conserva el estado: sin clic al mover un deslizador
                eq[b][ch].b0 = f.b0; eq[b][ch].b1 = f.b1; eq[b][ch].b2 = f.b2;
                eq[b][ch].a1 = f.a1; eq[b][ch].a2 = f.a2;
            }
        }
    }

    // Una voz que el hilo de audio deja de usar para siempre.
    static void retire(Voice* v) { if (v) v->dead.store(true, std::memory_order_release); }

    // Pasa `nx` a ser la voz actual. Devuelve false si la API la cambio en medio.
    bool promote(Voice*& c, Voice* nx) {
        Voice* expected = nx;
        if (!next.compare_exchange_strong(expected, nullptr, std::memory_order_acq_rel)) return false;
        retire(c);
        cur.store(nx, std::memory_order_release);
        c = nx;
        return true;
    }

    void render_block(float* out, int n) {
        std::memset(out, 0, static_cast<size_t>(n) * 8);
        refresh_eq();

        Voice* c = cur.load(std::memory_order_relaxed);
        Voice* p = pending.load(std::memory_order_acquire);
        const bool p_ready = ready(p);
        bool want_swap = p_ready || (unload_req.load(std::memory_order_acquire) && !p);
        const bool play = want_play.load(std::memory_order_relaxed);

        // Relevo pedido por la API (load/seek/stop): solo con el volumen a cero.
        if (want_swap && (!c || gate <= 0.0f)) {
            if (p_ready) {
                Voice* expected = p;
                if (pending.compare_exchange_strong(expected, nullptr, std::memory_order_acq_rel)) {
                    retire(c);
                    cur.store(p, std::memory_order_release);
                    c = p;
                    if (!p->keep_listened) listened.store(0, std::memory_order_relaxed);
                    want_swap = false;   // ya esta hecho: este mismo bloque ya suena
                }
            } else {
                retire(c);
                cur.store(nullptr, std::memory_order_release);
                c = nullptr;
                unload_req.store(false, std::memory_order_release);
                listened.store(0, std::memory_order_relaxed);
                want_swap = false;
            }
        }
        if (!c) { gate = 0.0f; return; }

        // Con otra pista (o un salto) en camino, la actual baja a cero y espera:
        // seguir tocándola los ~100 ms que tarda ffmpeg en arrancar sonaba a
        // "blip" de la canción vieja (y la consumía sin motivo).
        const bool cambiando = want_swap || pending.load(std::memory_order_acquire) != nullptr;
        const float target = (play && !cambiando) ? 1.0f : 0.0f;
        if (gate <= 0.0f && target <= 0.0f) return;   // en pausa: silencio y no se consume nada

        float A[kBlock * 2];
        float B[kBlock * 2];

        const bool eof_before = c->eof.load(std::memory_order_acquire);
        const uint64_t t_before = c->tail.load(std::memory_order_relaxed);
        int got = ring_read(c, A, n);
        const bool drained = eof_before && c->head.load(std::memory_order_acquire) == c->tail.load(std::memory_order_relaxed);

        Voice* nx = next.load(std::memory_order_acquire);
        const bool nx_ready = ready(nx);
        const int xf = xfade_frames.load(std::memory_order_relaxed);
        int audible = got;      // frames de `c` que cuentan como escuchados
        int promote_at = -1;    // indice donde `c` cede el sitio a `nx`

        // Crossfade: en los ultimos `xf` frames segun los metadatos, la voz
        // siguiente entra con ganancia equal-power mientras la actual sale.
        if (xf > 0 && nx_ready && c->duration_frames > 0 && !drained) {
            const int64_t dur  = static_cast<int64_t>(c->duration_frames);
            const int64_t pos0 = static_cast<int64_t>(c->start_frame + t_before);
            const int64_t i0   = std::max<int64_t>(0, dur - xf - pos0);
            const int64_t i1   = dur - pos0;                 // donde la actual "acaba" segun los metadatos
            if (i0 < got) {
                const int end = static_cast<int>(std::min<int64_t>(got, std::max<int64_t>(i1, i0)));
                const int cnt = end - static_cast<int>(i0);
                const int gb  = cnt > 0 ? ring_read(nx, B, cnt) : 0;
                for (int k = 0; k < cnt; ++k) {
                    const int i = static_cast<int>(i0) + k;
                    const double t = std::clamp(1.0 - static_cast<double>(dur - (pos0 + i)) / xf, 0.0, 1.0);
                    const float go = static_cast<float>(std::cos(t * M_PI_2));
                    const float gi = static_cast<float>(std::sin(t * M_PI_2));
                    const float bl = k < gb ? B[k * 2] : 0.0f, br = k < gb ? B[k * 2 + 1] : 0.0f;
                    A[i * 2]     = A[i * 2] * go + bl * gi;
                    A[i * 2 + 1] = A[i * 2 + 1] * go + br * gi;
                }
                if (i1 <= got) promote_at = static_cast<int>(std::max<int64_t>(i1, 0));
            }
        }

        // Sin crossfade (o sin nada con que mezclar): relevo gapless en cuanto
        // la actual se vacia del todo.
        if (promote_at < 0 && drained && got < n) promote_at = got;

        if (promote_at >= 0) {
            audible = std::min(audible, promote_at);
            if (nx_ready && promote(c, nx)) {
                // Lo que falta del bloque sale ya de la nueva voz.
                const int rest = n - promote_at;
                const int gn = ring_read(c, A + promote_at * 2, rest);
                if (gn < rest) std::memset(A + (promote_at + gn) * 2, 0, static_cast<size_t>(rest - gn) * 8);
                if (target > 0.0f) listened.fetch_add(static_cast<uint64_t>(audible), std::memory_order_relaxed);
                emit(0, c->token, listened.load(std::memory_order_relaxed));
                listened.store(c->tail.load(std::memory_order_relaxed), std::memory_order_relaxed);
                got = n;
                audible = 0;
            } else if (drained) {
                // Se acabo y no hay nada detras.
                if (target > 0.0f) listened.fetch_add(static_cast<uint64_t>(audible), std::memory_order_relaxed);
                // Sin una sola muestra desde el principio: no se pudo decodificar.
                // Desde una posición a mitad, en cambio, es que ya se acabó.
                const bool fallo = c->head.load(std::memory_order_relaxed) == 0 && c->start_frame == 0;
                emit(fallo ? 2 : 1, c->token,
                     listened.load(std::memory_order_relaxed));
                retire(c);
                cur.store(nullptr, std::memory_order_release);
                listened.store(0, std::memory_order_relaxed);
                audible = 0;
                c = nullptr;
            }
        } else if (got < n && !drained && target > 0.0f) {
            underruns.fetch_add(1, std::memory_order_relaxed);
        }
        if (c && target > 0.0f && audible > 0) listened.fetch_add(static_cast<uint64_t>(audible), std::memory_order_relaxed);

        // Ganancia (pausa/salto sin clic + volumen suavizado), ecualizador y limitador.
        const float vt = vol_target.load(std::memory_order_relaxed);
        for (int i = 0; i < n; ++i) {
            if (gate < target) gate = std::min(target, gate + kGateStep);
            else if (gate > target) gate = std::max(target, gate - kGateStep);
            vol += (vt - vol) * 0.0015f;
            const float g = gate * vol;
            float l = A[i * 2] * g, r = A[i * 2 + 1] * g;
            if (eq_active) {
                for (int b = 0; b < 10; ++b) {
                    if (!eq_on[b]) continue;
                    l = eq[b][0].process(l);
                    r = eq[b][1].process(r);
                }
            }
            auto limit = [](float x) {
                const float a = std::fabs(x);
                if (a <= 0.9f) return x;
                const float y = 0.9f + 0.1f * std::tanh((a - 0.9f) / 0.1f);
                return x < 0 ? -y : y;
            };
            out[i * 2]     = limit(l);
            out[i * 2 + 1] = limit(r);
        }
    }

    // ─ lado de la API ───────────────────────────────────────────────────────

    // Libera lo que el hilo de audio dejo de usar hace rato. Con `mu` tomado.
    void reap() {
        const auto now = std::chrono::steady_clock::now();
        for (auto it = all.begin(); it != all.end();) {
            Voice* v = it->get();
            if (v->dead.load(std::memory_order_acquire)) {
                if (!v->seen_dead) { v->seen_dead = true; v->seen_at = now; v->shutdown(); }
                if (now - v->seen_at > std::chrono::milliseconds(250)) {
                    names.erase(v->token);
                    it = all.erase(it);
                    continue;
                }
            }
            ++it;
        }
    }

    // Una voz que la API sustituye antes de que llegara a sonar.
    void discard(Voice* v) {
        if (!v) return;
        v->shutdown();
        v->dead.store(true, std::memory_order_release);
    }

    Voice* make_voice(const std::string& path, const std::string& id, uint64_t start_frame,
                      uint64_t duration_frames) {
        auto v = std::make_unique<Voice>();
        v->path = path;
        v->track_id = id;
        v->start_frame = start_frame;
        v->duration_frames = duration_frames;
        v->token = next_token++;
        v->start();
        names[v->token] = id;
        Voice* raw = v.get();
        all.push_back(std::move(v));
        return raw;
    }

    static void data_callback(ma_device* d, void* out, const void*, ma_uint32 frames) {
        static_cast<AudioEngine*>(d->pUserData)->render(static_cast<float*>(out), static_cast<int>(frames));
    }
};

// ── API ─────────────────────────────────────────────────────────────────────

AudioEngine::AudioEngine() : d_(std::make_unique<Impl>()) { d_->owner = this; }

AudioEngine::~AudioEngine() {
    stop_device();
    if (d_->ctx_ok) ma_context_uninit(&d_->ctx);
    // ~Impl destruye las voces (mata ffmpeg y une los hilos).
}

bool AudioEngine::start_device(const std::string& device_id, const std::string& app_name) {
    stop_device();
    Impl& s = *d_;
    if (!s.open_context(app_name)) return false;
    ma_device_config cfg = ma_device_config_init(ma_device_type_playback);
    cfg.playback.format   = ma_format_f32;
    cfg.playback.channels = kChannels;
    cfg.sampleRate        = kRate;
    cfg.periodSizeInMilliseconds = 20;
    cfg.dataCallback      = Impl::data_callback;
    cfg.pUserData         = this;
    cfg.pulse.pStreamNamePlayback = app_name.c_str();

    ma_device_id chosen{};
    if (!device_id.empty()) {
        ma_device_info* infos = nullptr; ma_uint32 count = 0;
        if (ma_context_get_devices(&s.ctx, &infos, &count, nullptr, nullptr) == MA_SUCCESS)
            for (ma_uint32 i = 0; i < count; ++i)
                if (device_id == s.id_text(infos[i].id)) { chosen = infos[i].id; cfg.playback.pDeviceID = &chosen; break; }
    }
    if (ma_device_init(&s.ctx, &cfg, &s.dev) != MA_SUCCESS) return false;
    if (ma_device_start(&s.dev) != MA_SUCCESS) { ma_device_uninit(&s.dev); return false; }
    s.dev_ok = true;
    return true;
}

void AudioEngine::stop_device() {
    if (!d_->dev_ok) return;
    ma_device_uninit(&d_->dev);   // espera a que el callback termine
    d_->dev_ok = false;
}

std::vector<AudioDevice> AudioEngine::list_devices() {
    Impl& s = *d_;
    std::vector<AudioDevice> out;
    if (!s.open_context("Localify")) return out;
    ma_device_info* infos = nullptr; ma_uint32 count = 0;
    if (ma_context_get_devices(&s.ctx, &infos, &count, nullptr, nullptr) != MA_SUCCESS) return out;
    for (ma_uint32 i = 0; i < count; ++i)
        out.push_back({s.id_text(infos[i].id), infos[i].name, infos[i].isDefault != 0});
    return out;
}

bool AudioEngine::load(const std::string& path, const std::string& track_id,
                       int start_ms, int duration_ms, bool play) {
    if (path.empty()) return false;
    Impl& s = *d_;
    std::lock_guard<std::mutex> lk(s.mu);
    s.reap();
    const uint64_t start = static_cast<uint64_t>(std::max(0, start_ms)) * kRate / 1000;
    const uint64_t dur   = static_cast<uint64_t>(std::max(0, duration_ms)) * kRate / 1000;
    Voice* v = s.make_voice(path, track_id, start, dur);
    s.discard(s.pending.exchange(v, std::memory_order_acq_rel));
    s.unload_req.store(false, std::memory_order_release);
    s.want_play.store(play, std::memory_order_release);
    return true;
}

void AudioEngine::seek(int ms) {
    Impl& s = *d_;
    std::lock_guard<std::mutex> lk(s.mu);
    s.reap();
    Voice* base = s.pending.load(std::memory_order_acquire);
    if (!base) base = s.cur.load(std::memory_order_acquire);
    if (!base) return;
    const uint64_t start = static_cast<uint64_t>(std::max(0, ms)) * kRate / 1000;
    Voice* v = s.make_voice(base->path, base->track_id, start, base->duration_frames);
    v->keep_listened = true;
    s.discard(s.pending.exchange(v, std::memory_order_acq_rel));
}

void AudioEngine::preload(const std::string& path, const std::string& track_id, int duration_ms) {
    Impl& s = *d_;
    std::lock_guard<std::mutex> lk(s.mu);
    s.reap();
    Voice* old = s.next.load(std::memory_order_acquire);
    if (path.empty()) { s.discard(s.next.exchange(nullptr, std::memory_order_acq_rel)); return; }
    if (old && old->path == path && old->start_frame == 0) return;   // ya esta
    Voice* v = s.make_voice(path, track_id, 0,
                            static_cast<uint64_t>(std::max(0, duration_ms)) * kRate / 1000);
    s.discard(s.next.exchange(v, std::memory_order_acq_rel));
}

void AudioEngine::play()  { d_->want_play.store(true,  std::memory_order_release); }
void AudioEngine::pause() { d_->want_play.store(false, std::memory_order_release); }

void AudioEngine::stop() {
    Impl& s = *d_;
    std::lock_guard<std::mutex> lk(s.mu);
    s.discard(s.pending.exchange(nullptr, std::memory_order_acq_rel));
    s.discard(s.next.exchange(nullptr, std::memory_order_acq_rel));
    s.want_play.store(false, std::memory_order_release);
    s.unload_req.store(true, std::memory_order_release);
}

void AudioEngine::set_volume(double v) {
    v = std::clamp(v, 0.0, 1.0);
    d_->vol_target.store(static_cast<float>(v * v * v), std::memory_order_relaxed);   // v^3: la curva del original
}

void AudioEngine::set_eq(const double gains_db[10]) {
    for (int i = 0; i < 10; ++i) d_->eq_gain[i].store(static_cast<float>(gains_db[i]), std::memory_order_relaxed);
    d_->eq_version.fetch_add(1, std::memory_order_release);
}

void AudioEngine::set_crossfade(int ms) {
    d_->xfade_frames.store(std::clamp(ms, 0, 12000) * kRate / 1000, std::memory_order_relaxed);
}

AudioStatus AudioEngine::status() const {
    Impl& s = *d_;
    std::lock_guard<std::mutex> lk(s.mu);
    s.reap();
    AudioStatus st;
    Voice* p = s.pending.load(std::memory_order_acquire);
    Voice* c = s.cur.load(std::memory_order_acquire);
    Voice* v = p ? p : c;   // una carga o un salto en curso ya cuentan: no parpadea la posicion
    if (!v) return st;
    const uint64_t played = p ? 0 : v->tail.load(std::memory_order_relaxed);
    st.track_id    = v->track_id;
    st.position_ms = static_cast<int>((v->start_frame + played) * 1000 / kRate);
    st.duration_ms = static_cast<int>(v->duration_frames * 1000 / kRate);
    st.buffered_ms = p ? 0 : static_cast<int>((v->head.load(std::memory_order_relaxed) - played) * 1000 / kRate);
    st.listened_ms = static_cast<int>(s.listened.load(std::memory_order_relaxed) * 1000 / kRate);
    st.playing     = s.want_play.load(std::memory_order_relaxed);
    st.underruns   = s.underruns.load(std::memory_order_relaxed);
    return st;
}

std::vector<AudioEvent> AudioEngine::take_events() {
    Impl& s = *d_;
    std::vector<AudioEvent> out;
    std::lock_guard<std::mutex> lk(s.mu);
    uint32_t t = s.ev_tail.load(std::memory_order_relaxed);
    const uint32_t h = s.ev_head.load(std::memory_order_acquire);
    for (; t != h; ++t) {
        const auto& e = s.evbuf[t % kEvCap];
        AudioEvent ev;
        ev.type = e.type == 0 ? AudioEvent::Handoff : e.type == 1 ? AudioEvent::Ended : AudioEvent::Error;
        auto it = s.names.find(e.token);
        ev.track_id = it == s.names.end() ? "" : it->second;
        ev.listened_ms = static_cast<int>(e.listened_frames * 1000 / kRate);
        out.push_back(std::move(ev));
    }
    s.ev_tail.store(t, std::memory_order_release);
    return out;
}

void AudioEngine::render(float* out, int frames) {
    while (frames > 0) {
        const int n = std::min(frames, kBlock);
        d_->render_block(out, n);
        out += n * 2;
        frames -= n;
    }
}

} // namespace luxdesktop

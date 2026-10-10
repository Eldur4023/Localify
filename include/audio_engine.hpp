#pragma once
// El motor de audio de Localify: el equivalente de crates/localify-audio del
// original (cpal + symphonia), con otras piezas -- miniaudio para la salida y
// un ffmpeg hijo para decodificar -- y el mismo reparto de papeles:
//
//   * El motor solo MEZCLA. No sabe de colas, shuffle ni repeticion (ADR-015):
//     quien decide que suena despues es el backend, que le da la pista actual
//     (load) y la siguiente (preload) y recibe avisos (take_events).
//   * El hilo de audio (render) no asigna memoria, no toma locks y no hace
//     I/O: solo lee de anillos SPSC que rellenan hilos decodificadores y lee
//     parametros atomicos. Lo demas (abrir ffmpeg, matar voces, liberar) pasa
//     en el hilo que llama a la API.
//   * La posicion sale de aqui: es lo que se oye, no una estimacion.
//
// render() es publico y no depende del dispositivo: los tests lo llaman en
// bucle y comprueban muestras, posiciones y avisos sin tarjeta de sonido.
#include <atomic>
#include <cstdint>
#include <memory>
#include <mutex>
#include <string>
#include <utility>
#include <vector>

namespace luxdesktop {

struct AudioEvent {
    enum Type { Handoff, Ended, Error } type;
    // Handoff: la pista que ENTRA (el motor ya la esta tocando, gapless o
    //          crossfade) y lo escuchado de la que sale.
    // Ended:   la que acaba de terminar sin nada detras; el backend decide.
    // Error:   la que no se pudo decodificar (ffmpeg fallo o no existe).
    std::string track_id;
    int         listened_ms = 0;
};

struct AudioStatus {
    std::string track_id;          // "" = nada cargado
    int         position_ms = 0;
    int         duration_ms = 0;   // la de los metadatos que dio load()
    int         buffered_ms = 0;   // decodificado y aun sin sonar
    int         listened_ms = 0;   // sonado de verdad en esta pista (sin pausas ni saltos)
    bool        playing = false;
    int         underruns = 0;
};

struct AudioDevice {
    std::string id;          // estable entre sesiones (en PulseAudio/PipeWire, el nombre del sink)
    std::string name;        // para mostrar
    bool        is_default = false;
};

class AudioEngine {
public:
    static constexpr int kRate     = 48000;
    static constexpr int kChannels = 2;

    AudioEngine();
    ~AudioEngine();
    AudioEngine(const AudioEngine&) = delete;
    AudioEngine& operator=(const AudioEngine&) = delete;

    // ── Dispositivo ────────────────────────────────────────────────────────
    // device_id vacio (o que ya no existe) = el predeterminado del sistema.
    // Devuelve false (y deja el motor sin salida, pero usable con render()) si
    // no hay forma de abrir ninguno.
    bool start_device(const std::string& device_id = "", const std::string& app_name = "Localify");
    void stop_device();
    std::vector<AudioDevice> list_devices();

    // ── Control (cualquier hilo no-RT) ─────────────────────────────────────
    // Sustituye lo que suena. start_ms = donde empezar; duration_ms = la de
    // los metadatos (para el crossfade). Devuelve false si path esta vacio.
    bool load(const std::string& path, const std::string& track_id,
              int start_ms, int duration_ms, bool play);
    // Lo que sonara al terminar la actual (gapless/crossfade). path vacio =
    // nada. Se decodifica ya, para que el relevo no tenga hueco.
    void preload(const std::string& path, const std::string& track_id, int duration_ms);
    void play();
    void pause();
    void stop();                 // descarga la pista actual
    void seek(int ms);           // misma pista, otra posicion
    void set_volume(double v);   // 0..1, percibido (se aplica v^3, como el original)
    void set_eq(const double gains_db[10]);
    void set_crossfade(int ms);

    AudioStatus             status() const;
    std::vector<AudioEvent> take_events();

    // El mezclador. `out` son frames*2 floats intercalados. Lo llama el
    // callback del dispositivo; los tests, a mano.
    void render(float* out, int frames);

private:
    struct Voice;
    struct Impl;
    std::unique_ptr<Impl> d_;
};

} // namespace luxdesktop

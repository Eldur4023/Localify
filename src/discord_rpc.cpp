#include "discord_rpc.hpp"

#include <cstdint>
#include <cstdlib>
#include <cstring>
#include <mutex>
#include <sstream>
#include <sys/socket.h>
#include <sys/types.h>
#include <sys/un.h>
#include <unistd.h>

using lux_script::Value;

namespace luxdesktop {
namespace {

struct State {
    std::mutex mutex;
    int fd = -1;
    std::string connected_client_id; // "" while not connected
    std::string last_details, last_state;
    bool last_playing = false;
    long long last_start = 0;
};

State& state() { static State s; return s; }

void close_fd_locked() {
    State& s = state();
    if (s.fd >= 0) { ::close(s.fd); s.fd = -1; }
    s.connected_client_id.clear();
}

// Every string that ends up inside the hand-built JSON below can contain
// arbitrary user/catalogue data (a track title, an artist name) -- quotes,
// backslashes, control characters, all fair game. This is the same class
// of problem json.lux's own parser exists for on the LuxScript side, just
// the write direction instead of the read one.
std::string json_escape(const std::string& s) {
    std::string out;
    out.reserve(s.size() + 8);
    for (unsigned char c : s) {
        switch (c) {
            case '"':  out += "\\\""; break;
            case '\\': out += "\\\\"; break;
            case '\n': out += "\\n";  break;
            case '\r': out += "\\r";  break;
            case '\t': out += "\\t";  break;
            default:
                if (c < 0x20) {
                    char buf[8];
                    std::snprintf(buf, sizeof(buf), "\\u%04x", c);
                    out += buf;
                } else {
                    out += static_cast<char>(c);
                }
        }
    }
    return out;
}

bool write_all(int fd, const char* data, size_t len) {
    size_t sent = 0;
    while (sent < len) {
        ssize_t n = ::send(fd, data + sent, len - sent, MSG_NOSIGNAL);
        if (n <= 0) return false;
        sent += static_cast<size_t>(n);
    }
    return true;
}

bool send_frame(int fd, std::int32_t opcode, const std::string& json) {
    char header[8];
    std::memcpy(header, &opcode, 4);
    std::int32_t len = static_cast<std::int32_t>(json.size());
    std::memcpy(header + 4, &len, 4);
    return write_all(fd, header, 8) && write_all(fd, json.data(), json.size());
}

// One short blocking read with a real timeout -- just enough to know
// "something answered" after the handshake. The actual READY payload is
// never parsed: the only thing that matters here is that Discord accepted
// the client_id and did not close the socket on us.
bool read_some_reply(int fd) {
    timeval tv{2, 0};
    ::setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &tv, sizeof(tv));
    char buf[512];
    ssize_t n = ::recv(fd, buf, sizeof(buf), 0);
    return n > 0;
}

// Discord (stable/PTB/Canary, and Flatpak/Snap sandboxed builds) each claim
// their own discord-ipc-N slot under the same runtime dir; trying a
// handful in order is the standard client-side convention, not a guess
// specific to this app.
int connect_and_handshake(const std::string& client_id) {
    const char* runtime_dir = std::getenv("XDG_RUNTIME_DIR");
    std::string base = runtime_dir && *runtime_dir ? runtime_dir : "/tmp";
    for (int i = 0; i < 10; ++i) {
        std::string path = base + "/discord-ipc-" + std::to_string(i);
        int fd = ::socket(AF_UNIX, SOCK_STREAM, 0);
        if (fd < 0) continue;
        sockaddr_un addr{};
        addr.sun_family = AF_UNIX;
        std::strncpy(addr.sun_path, path.c_str(), sizeof(addr.sun_path) - 1);
        timeval tv{1, 0};
        ::setsockopt(fd, SOL_SOCKET, SO_SNDTIMEO, &tv, sizeof(tv));
        if (::connect(fd, reinterpret_cast<sockaddr*>(&addr), sizeof(addr)) != 0) {
            ::close(fd);
            continue;
        }
        std::string hs = "{\"v\":1,\"client_id\":\"" + json_escape(client_id) + "\"}";
        if (!send_frame(fd, /*HANDSHAKE=*/0, hs) || !read_some_reply(fd)) {
            ::close(fd);
            continue;
        }
        return fd; // connected and Discord answered
    }
    return -1;
}

} // namespace

void discord_update(const Value& d) {
    if (!d.is_dict()) return;
    auto& dict = const_cast<Value&>(d).as_dict();
    Value client_id_v = dict["clientId"];
    std::string client_id = client_id_v.is_str() ? client_id_v.as_str() : "";

    State& s = state();
    std::lock_guard<std::mutex> lk(s.mutex);

    if (client_id.empty()) {
        // Disabled, or the user cleared the field: drop any open
        // connection instead of leaving a stale presence showing.
        if (s.fd >= 0) close_fd_locked();
        return;
    }
    if (s.fd < 0 || s.connected_client_id != client_id) {
        close_fd_locked();
        int fd = connect_and_handshake(client_id);
        if (fd < 0) return; // Discord not running, or wrong id -- next poll tries again
        s.fd = fd;
        s.connected_client_id = client_id;
        // Force the next SET_ACTIVITY through even if the track/status
        // happen to be identical to what was cached from a previous,
        // now-dead connection.
        s.last_details.clear();
        s.last_state.clear();
        s.last_playing = false;
        s.last_start = -1;
    }

    Value details_v = dict["details"], state_v = dict["state"], playing_v = dict["playing"], start_v = dict["startEpochS"];
    std::string details = details_v.is_str() ? details_v.as_str() : "";
    std::string state_s = state_v.is_str() ? state_v.as_str() : "";
    bool playing = playing_v.as_bool();
    long long start = start_v.is_null() ? 0 : start_v.as_int();

    if (details == s.last_details && state_s == s.last_state &&
        playing == s.last_playing && start == s.last_start) {
        return; // nothing actually changed since the last push
    }

    std::ostringstream args;
    args << "{\"pid\":" << static_cast<long long>(::getpid()) << ",\"activity\":{";
    args << "\"details\":\"" << json_escape(details.empty() ? "Idle" : details) << "\"";
    if (!state_s.empty())
        args << ",\"state\":\"" << json_escape(state_s) << "\"";
    if (playing && start > 0)
        args << ",\"timestamps\":{\"start\":" << start << "}";
    args << "}}";
    std::string payload = "{\"cmd\":\"SET_ACTIVITY\",\"args\":" + args.str() + ",\"nonce\":\"localify\"}";

    if (!send_frame(s.fd, /*FRAME=*/1, payload)) {
        // The pipe died between polls (Discord restarted, quit...): drop
        // it now so the NEXT poll reconnects instead of writing into a
        // dead fd every second until something notices.
        close_fd_locked();
        return;
    }
    s.last_details = details; s.last_state = state_s;
    s.last_playing = playing; s.last_start = start;
}

void discord_shutdown() {
    State& s = state();
    std::lock_guard<std::mutex> lk(s.mutex);
    if (s.fd >= 0) {
        // CLEAR: SET_ACTIVITY with an absent/empty activity removes it.
        send_frame(s.fd, 1, "{\"cmd\":\"SET_ACTIVITY\",\"args\":{\"pid\":" +
            std::to_string(static_cast<long long>(::getpid())) + "},\"nonce\":\"localify-clear\"}");
    }
    close_fd_locked();
}

} // namespace luxdesktop

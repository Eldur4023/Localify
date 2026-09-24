#include "discord_rpc.hpp"

// Port of the original Tauri integration (crates/localify-integrations/src/
// discord), which is the reference for what "working" means here:
//
//  - activity type 2 ("Listening"), title on the first line, "artist ·
//    album" on the second, the cover as a PUBLIC url (Discord's own client
//    downloads it, in its own process: a local path or the app's loopback
//    port mean nothing there), and BOTH timestamps so Discord draws a
//    progress bar instead of an elapsed-time counter;
//  - paused/stopped clears the presence;
//  - Discord drops updates past a few per 20 s, so this keeps the LATEST
//    desired activity and sends it as soon as the window reopens: skipping
//    five tracks fast loses the intermediate ones, never the final one;
//  - Discord answers every command; the answer is read (a rejected activity
//    is logged instead of silently vanishing, and unread replies do not
//    pile up in the socket).

#include <chrono>
#include <cmath>
#include <cstdint>
#include <cstdlib>
#include <cstring>
#include <iostream>
#include <mutex>
#include <optional>
#include <poll.h>
#include <sstream>
#include <string>
#include <sys/socket.h>
#include <sys/types.h>
#include <sys/un.h>
#include <unistd.h>

using lux_script::Value;
using Clock = std::chrono::steady_clock;

namespace luxdesktop {
namespace {

// Minimum spacing between two SET_ACTIVITY. Discord's documented limit is
// 5 updates / 20 s per client; 5 s stays under it with margin.
constexpr auto kMinInterval = std::chrono::seconds(5);
constexpr auto kMaxBackoff  = std::chrono::seconds(60);
constexpr auto kFirstBackoff = std::chrono::seconds(5);

constexpr std::int32_t OP_HANDSHAKE = 0;
constexpr std::int32_t OP_FRAME     = 1;
constexpr std::int32_t OP_CLOSE     = 2;
constexpr std::uint32_t kMaxPayload = 64 * 1024;

struct Activity {
    std::string title, artist, album, cover;
    long long start_s = 0, end_s = 0;

    // A start that moved by a second or two is the same activity: the
    // anchor comes from the audio clock and can jitter a little between
    // reports, and resending for that would burn the rate limit for nothing.
    bool same_as(const Activity& o) const {
        return title == o.title && artist == o.artist && album == o.album &&
               cover == o.cover && std::llabs(start_s - o.start_s) <= 2 &&
               std::llabs(end_s - o.end_s) <= 2;
    }
};

struct State {
    std::mutex mutex;
    int fd = -1;
    std::string connected_client_id;
    // What Discord currently shows (as far as we know) and what it should
    // show. `published_known` is false right after (re)connecting: Discord's
    // view is unknown then, so the next update always goes out.
    std::optional<Activity> desired, published;
    bool published_known = false;
    Clock::time_point last_send{};
    Clock::time_point retry_at{};
    Clock::duration backoff = kFirstBackoff;
    long long nonce = 0;
};

State& state() { static State s; return s; }

void close_fd_locked(State& s) {
    if (s.fd >= 0) { ::close(s.fd); s.fd = -1; }
    s.connected_client_id.clear();
    s.published_known = false;
}

std::string json_escape(const std::string& in) {
    std::string out;
    out.reserve(in.size() + 8);
    for (unsigned char c : in) {
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

// Discord rejects the WHOLE activity if a text field is shorter than 2 or
// longer than 128 characters. Cut on a UTF-8 boundary (never in the middle
// of a multibyte character) and pad a 1-character string with a zero-width
// space.
std::string fit_text(std::string s) {
    size_t chars = 0, cut = s.size();
    for (size_t i = 0; i < s.size(); ++i) {
        if ((static_cast<unsigned char>(s[i]) & 0xC0) != 0x80) {
            if (chars == 128) { cut = i; break; }
            ++chars;
        }
    }
    s.resize(cut);
    if (chars < 2) s += "\xE2\x80\x8B";
    return s;
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

bool read_exact(int fd, char* buf, size_t len, int timeout_ms) {
    size_t got = 0;
    while (got < len) {
        pollfd p{fd, POLLIN, 0};
        if (::poll(&p, 1, timeout_ms) <= 0) return false;
        ssize_t n = ::recv(fd, buf + got, len - got, 0);
        if (n <= 0) return false;
        got += static_cast<size_t>(n);
    }
    return true;
}

// Reads one frame. `false` = timeout, closed socket or garbage.
bool read_frame(int fd, int timeout_ms, std::int32_t& op, std::string& payload) {
    char header[8];
    if (!read_exact(fd, header, 8, timeout_ms)) return false;
    std::uint32_t len;
    std::memcpy(&op, header, 4);
    std::memcpy(&len, header + 4, 4);
    if (len > kMaxPayload) return false;
    payload.assign(len, '\0');
    return len == 0 || read_exact(fd, payload.data(), len, timeout_ms);
}

// Drains whatever Discord sent that nobody read yet (answers to earlier
// commands). `false` if the socket turned out to be closed.
bool drain(int fd) {
    for (;;) {
        pollfd p{fd, POLLIN, 0};
        int r = ::poll(&p, 1, 0);
        if (r <= 0) return true;
        if (p.revents & (POLLHUP | POLLERR)) return false;
        std::int32_t op;
        std::string payload;
        if (!read_frame(fd, 100, op, payload)) return false;
        if (op == OP_CLOSE) return false;
    }
}

// Every place a Discord build (stable, Canary, Flatpak, Snap) may put its
// socket -- the same list the original integration tried.
std::optional<int> connect_and_handshake(const std::string& client_id) {
    const char* bases_env[] = {"XDG_RUNTIME_DIR", "TMPDIR"};
    std::string bases[3];
    int nb = 0;
    for (const char* e : bases_env)
        if (const char* v = std::getenv(e); v && *v) bases[nb++] = v;
    bases[nb++] = "/tmp";
    const char* subdirs[] = {"", "app/com.discordapp.Discord/",
                             "app/com.discordapp.DiscordCanary/", "snap.discord/",
                             "snap.discord-canary/"};
    for (int b = 0; b < nb; ++b) {
        for (const char* sub : subdirs) {
            for (int i = 0; i < 10; ++i) {
                std::string path = bases[b] + "/" + sub + "discord-ipc-" + std::to_string(i);
                int fd = ::socket(AF_UNIX, SOCK_STREAM, 0);
                if (fd < 0) continue;
                sockaddr_un addr{};
                addr.sun_family = AF_UNIX;
                std::strncpy(addr.sun_path, path.c_str(), sizeof(addr.sun_path) - 1);
                if (::connect(fd, reinterpret_cast<sockaddr*>(&addr), sizeof(addr)) != 0) {
                    ::close(fd);
                    continue;
                }
                std::string hs = "{\"v\":1,\"client_id\":\"" + json_escape(client_id) + "\"}";
                std::int32_t op = -1;
                std::string reply;
                if (!send_frame(fd, OP_HANDSHAKE, hs) || !read_frame(fd, 2000, op, reply) ||
                    op != OP_FRAME) {
                    // op == OP_CLOSE here is Discord refusing the handshake,
                    // typically a wrong Application ID -- say so once.
                    if (op == OP_CLOSE)
                        std::cerr << "discord: handshake rejected: " << reply << "\n";
                    ::close(fd);
                    continue;
                }
                return fd;
            }
        }
    }
    return std::nullopt;
}

std::string activity_json(const Activity& a) {
    std::string second = a.album.empty() ? a.artist
                       : a.artist.empty() ? a.album
                       : a.artist + " \xC2\xB7 " + a.album;
    std::ostringstream o;
    o << "{\"type\":2"
      << ",\"details\":\"" << json_escape(fit_text(a.title)) << "\"";
    if (!second.empty())
        o << ",\"state\":\"" << json_escape(fit_text(second)) << "\"";
    if (a.start_s > 0) {
        o << ",\"timestamps\":{\"start\":" << a.start_s;
        if (a.end_s > a.start_s) o << ",\"end\":" << a.end_s;
        o << "}";
    }
    // `assets` only when there is an image: present-but-empty makes Discord
    // reserve the slot and draw a question mark, and `null` makes it reject
    // the whole activity ("assets must be an object").
    if (!a.cover.empty()) {
        o << ",\"assets\":{\"large_image\":\"" << json_escape(a.cover) << "\""
          << ",\"large_text\":\"" << json_escape(fit_text(a.album.empty() ? a.title : a.album))
          << "\"}";
    }
    o << "}";
    return o.str();
}

const Value* field(const Value& d, const char* key) {
    if (!d.is_dict()) return nullptr;
    auto& dict = d.as_dict();
    auto it = dict.find(key);
    return it == dict.end() ? nullptr : &it->second;
}

std::string str_field(const Value& d, const char* key) {
    const Value* v = field(d, key);
    return v && v->is_str() ? v->as_str() : std::string();
}

long long num_field(const Value& d, const char* key) {
    const Value* v = field(d, key);
    return v && v->is_num() ? static_cast<long long>(std::llround(v->as_float())) : 0;
}

// Sends `desired` (or a clear) if the rate limit and the connection allow
// it. Caller holds the lock.
void flush_locked(State& s, const std::string& client_id) {
    auto now = Clock::now();
    bool same = s.published_known &&
        (s.desired.has_value() == s.published.has_value()) &&
        (!s.desired || s.desired->same_as(*s.published));
    if (s.fd >= 0 && s.connected_client_id == client_id) {
        if (!drain(s.fd)) close_fd_locked(s);
    }
    if (same && s.fd >= 0) return;
    if (now < s.last_send + kMinInterval || now < s.retry_at) return;

    if (s.fd < 0 || s.connected_client_id != client_id) {
        close_fd_locked(s);
        auto fd = connect_and_handshake(client_id);
        if (!fd) {
            // Discord closed: back off instead of trying every poll.
            s.retry_at = now + s.backoff;
            s.backoff = std::min<Clock::duration>(s.backoff * 2, kMaxBackoff);
            return;
        }
        s.fd = *fd;
        s.connected_client_id = client_id;
        s.backoff = kFirstBackoff;
        s.published_known = false;
    }
    if (s.published_known && same) return;

    std::string args = "{\"pid\":" + std::to_string(static_cast<long long>(::getpid()));
    if (s.desired) args += ",\"activity\":" + activity_json(*s.desired);
    args += "}";
    std::string nonce = "localify-" + std::to_string(++s.nonce);
    std::string payload =
        "{\"cmd\":\"SET_ACTIVITY\",\"args\":" + args + ",\"nonce\":\"" + nonce + "\"}";

    if (!send_frame(s.fd, OP_FRAME, payload)) {
        // The pipe died (Discord restarted/quit): reconnect on a later call.
        close_fd_locked(s);
        s.retry_at = now + std::chrono::seconds(2);
        return;
    }
    s.last_send = now;
    std::int32_t op = -1;
    std::string reply;
    if (read_frame(s.fd, 1000, op, reply)) {
        if (op == OP_CLOSE) {
            close_fd_locked(s);
            return;
        }
        if (reply.find("\"evt\":\"ERROR\"") != std::string::npos) {
            // The pipe is fine, what was sent is not: resending the same
            // thing would fail the same way forever, so it counts as
            // published (the next track brings a new activity) and the
            // reason goes to the log instead of vanishing.
            std::cerr << "discord: activity rejected: " << reply << "\n";
        }
    }
    s.published = s.desired;
    s.published_known = true;
}

} // namespace

void discord_update(const Value& d) {
    if (!d.is_dict()) return;
    std::string client_id = str_field(d, "clientId");

    State& s = state();
    std::lock_guard<std::mutex> lk(s.mutex);

    if (client_id.empty()) {
        // Off, or no Application ID: dropping the IPC connection is enough
        // for Discord to clear the presence on its own.
        close_fd_locked(s);
        s.desired.reset();
        s.published.reset();
        return;
    }

    const Value* a = field(d, "activity");
    if (a && a->is_dict() && !str_field(*a, "title").empty()) {
        Activity act;
        act.title  = str_field(*a, "title");
        act.artist = str_field(*a, "artist");
        act.album  = str_field(*a, "album");
        act.cover  = str_field(*a, "coverUrl");
        act.start_s = num_field(*a, "startS");
        act.end_s   = num_field(*a, "endS");
        s.desired = act;
    } else {
        s.desired.reset();
    }
    flush_locked(s, client_id);
}

void discord_shutdown() {
    State& s = state();
    std::lock_guard<std::mutex> lk(s.mutex);
    if (s.fd >= 0) {
        send_frame(s.fd, OP_FRAME, "{\"cmd\":\"SET_ACTIVITY\",\"args\":{\"pid\":" +
            std::to_string(static_cast<long long>(::getpid())) + "},\"nonce\":\"localify-bye\"}");
    }
    close_fd_locked(s);
}

} // namespace luxdesktop

#include "mpris.hpp"

#include <gio/gio.h>

#include <arpa/inet.h>
#include <cstring>
#include <mutex>
#include <netinet/in.h>
#include <sys/socket.h>
#include <sys/types.h>
#include <unistd.h>

using lux_script::Value;

namespace luxdesktop {
namespace {

// The two interfaces MPRIS actually requires (Root + Player). TrackList and
// Playlists are both optional and this app has neither a flat track list
// nor named playlists in the MPRIS sense, so `HasTrackList` is false and
// those interfaces are omitted entirely, exactly as the spec allows.
const char* kIntrospectionXml = R"xml(
<node>
  <interface name="org.mpris.MediaPlayer2">
    <method name="Raise"/>
    <method name="Quit"/>
    <property name="CanQuit" type="b" access="read"/>
    <property name="CanRaise" type="b" access="read"/>
    <property name="HasTrackList" type="b" access="read"/>
    <property name="Identity" type="s" access="read"/>
    <property name="SupportedUriSchemes" type="as" access="read"/>
    <property name="SupportedMimeTypes" type="as" access="read"/>
  </interface>
  <interface name="org.mpris.MediaPlayer2.Player">
    <method name="Next"/>
    <method name="Previous"/>
    <method name="Pause"/>
    <method name="PlayPause"/>
    <method name="Stop"/>
    <method name="Play"/>
    <method name="Seek"><arg direction="in" type="x" name="Offset"/></method>
    <method name="SetPosition">
      <arg direction="in" type="o" name="TrackId"/>
      <arg direction="in" type="x" name="Position"/>
    </method>
    <method name="OpenUri"><arg direction="in" type="s" name="Uri"/></method>
    <property name="PlaybackStatus" type="s" access="read"/>
    <property name="LoopStatus" type="s" access="readwrite"/>
    <property name="Rate" type="d" access="readwrite"/>
    <property name="Shuffle" type="b" access="readwrite"/>
    <property name="Metadata" type="a{sv}" access="read"/>
    <property name="Volume" type="d" access="readwrite"/>
    <property name="Position" type="x" access="read"/>
    <property name="MinimumRate" type="d" access="read"/>
    <property name="MaximumRate" type="d" access="read"/>
    <property name="CanGoNext" type="b" access="read"/>
    <property name="CanGoPrevious" type="b" access="read"/>
    <property name="CanPlay" type="b" access="read"/>
    <property name="CanPause" type="b" access="read"/>
    <property name="CanSeek" type="b" access="read"/>
    <property name="CanControl" type="b" access="read"/>
  </interface>
</node>
)xml";

// Cached track/playback state, written by mpris_update() (called from the
// LuxScript side on every poll tick) and read by the property-get and
// PropertiesChanged-emission code. One mutex: MPRIS method calls arrive on
// the GLib main loop thread (the same one the window itself runs on,
// g_bus_own_name uses the default GMainContext with no override), and
// mpris_update() is also always called from that same thread today -- but
// the lock is cheap and removes any need to prove that stays true.
struct State {
    std::mutex mutex;
    std::string track_id  = "/org/mpris/MediaPlayer2/TrackList/NoTrack";
    std::string title;
    std::string artist;
    std::string album;
    std::string art_url;
    long long   duration_us = 0;
    std::string status = "Stopped"; // Playing | Paused | Stopped
    long long   position_us = 0;
    double      volume = 1.0;
    bool        shuffle = false;
    std::string loop_status = "None"; // None | Track | Playlist
    bool        has_track = false;
};

State&            state() { static State s; return s; }
GDBusConnection*  g_conn = nullptr;
guint             g_owner_id = 0;
guint             g_reg_root = 0;
guint             g_reg_player = 0;
std::uint16_t     g_port = 0;

// A trusted, same-machine loopback POST -- no HTTP client library needed
// for this (the very same "raw socket" approach runtime.cpp already uses
// for find_free_port()/wait_for_server()), and no response body is ever
// read: the route's JSON reply is not interesting here, only that the
// request was sent. Best-effort: a media key press that arrives while the
// server is mid-restart is not worth surfacing an error for.
void post_local(const std::string& path, const std::string& body = "{}") {
    int fd = ::socket(AF_INET, SOCK_STREAM, 0);
    if (fd < 0) return;
    timeval tv{2, 0};
    ::setsockopt(fd, SOL_SOCKET, SO_SNDTIMEO, &tv, sizeof(tv));
    ::setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &tv, sizeof(tv));
    sockaddr_in addr{};
    addr.sin_family      = AF_INET;
    addr.sin_port        = htons(g_port);
    addr.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
    if (::connect(fd, reinterpret_cast<sockaddr*>(&addr), sizeof(addr)) != 0) {
        ::close(fd);
        return;
    }
    std::string req = "POST " + path + " HTTP/1.1\r\n"
        "Host: 127.0.0.1\r\n"
        "Content-Type: application/json\r\n"
        "Content-Length: " + std::to_string(body.size()) + "\r\n"
        "Connection: close\r\n\r\n" + body;
    ::send(fd, req.data(), req.size(), 0);
    char buf[256];
    while (::recv(fd, buf, sizeof(buf), 0) > 0) {} // drain so the server sees a clean close
    ::close(fd);
}

GVariant* build_metadata_locked() {
    GVariantBuilder b;
    g_variant_builder_init(&b, G_VARIANT_TYPE("a{sv}"));
    State& s = state();
    std::string track_path = s.has_track
        ? "/org/localify/track/" + s.track_id
        : "/org/mpris/MediaPlayer2/TrackList/NoTrack";
    // mpris:trackid MUST be a D-Bus object path, not the app's own opaque
    // id -- sanitize to [A-Za-z0-9_/] (a YouTube-derived id is already
    // that shape; this is a floor, not a real-world workaround).
    std::string safe_path;
    for (char c : track_path) {
        safe_path += (std::isalnum(static_cast<unsigned char>(c)) || c == '/' || c == '_') ? c : '_';
    }
    g_variant_builder_add(&b, "{sv}", "mpris:trackid", g_variant_new_object_path(safe_path.c_str()));
    if (s.has_track) {
        g_variant_builder_add(&b, "{sv}", "mpris:length", g_variant_new_int64(s.duration_us));
        if (!s.title.empty())
            g_variant_builder_add(&b, "{sv}", "xesam:title", g_variant_new_string(s.title.c_str()));
        if (!s.artist.empty()) {
            const char* artists[2] = { s.artist.c_str(), nullptr };
            g_variant_builder_add(&b, "{sv}", "xesam:artist", g_variant_new_strv(artists, 1));
        }
        if (!s.album.empty())
            g_variant_builder_add(&b, "{sv}", "xesam:album", g_variant_new_string(s.album.c_str()));
        if (!s.art_url.empty())
            g_variant_builder_add(&b, "{sv}", "mpris:artUrl", g_variant_new_string(s.art_url.c_str()));
    }
    return g_variant_builder_end(&b);
}

void method_call(GDBusConnection* conn, const gchar*, const gchar*, const gchar* iface,
                  const gchar* method, GVariant* params, GDBusMethodInvocation* inv, gpointer) {
    if (g_strcmp0(iface, "org.mpris.MediaPlayer2") == 0) {
        // Raise/Quit: this app has no "bring window to front" hook wired
        // through here yet, and quitting the whole app from a media widget
        // is not something anything currently asks for -- both are no-ops
        // that still answer the call so a client never sees a timeout.
        g_dbus_method_invocation_return_value(inv, nullptr);
        return;
    }
    if (g_strcmp0(method, "Next") == 0)      post_local("/api/player/next");
    else if (g_strcmp0(method, "Previous") == 0) post_local("/api/player/previous");
    else if (g_strcmp0(method, "Pause") == 0)    post_local("/api/player/pause");
    else if (g_strcmp0(method, "Play") == 0)     post_local("/api/player/resume");
    else if (g_strcmp0(method, "PlayPause") == 0) post_local("/api/player/toggle");
    else if (g_strcmp0(method, "Stop") == 0)     post_local("/api/player/pause");
    else if (g_strcmp0(method, "Seek") == 0) {
        gint64 offset_us = 0;
        g_variant_get(params, "(x)", &offset_us);
        long long new_pos_ms;
        { std::lock_guard<std::mutex> lk(state().mutex);
          new_pos_ms = (state().position_us + offset_us) / 1000; }
        if (new_pos_ms < 0) new_pos_ms = 0;
        post_local("/api/player/seek", "{\"positionMs\":" + std::to_string(new_pos_ms) + "}");
    } else if (g_strcmp0(method, "SetPosition") == 0) {
        const gchar* track_path = nullptr;
        gint64 pos_us = 0;
        g_variant_get(params, "(&ox)", &track_path, &pos_us);
        long long pos_ms = pos_us / 1000;
        if (pos_ms < 0) pos_ms = 0;
        post_local("/api/player/seek", "{\"positionMs\":" + std::to_string(pos_ms) + "}");
    } else if (g_strcmp0(method, "OpenUri") == 0) {
        // Not supported (this app has no concept of an external URI to
        // hand it) -- answered, not left hanging.
    }
    g_dbus_method_invocation_return_value(inv, nullptr);
    (void)conn;
}

GVariant* get_property(GDBusConnection*, const gchar*, const gchar*, const gchar* iface,
                        const gchar* prop, GError**, gpointer) {
    std::lock_guard<std::mutex> lk(state().mutex);
    State& s = state();
    if (g_strcmp0(iface, "org.mpris.MediaPlayer2") == 0) {
        if (g_strcmp0(prop, "CanQuit") == 0)          return g_variant_new_boolean(false);
        if (g_strcmp0(prop, "CanRaise") == 0)         return g_variant_new_boolean(false);
        if (g_strcmp0(prop, "HasTrackList") == 0)     return g_variant_new_boolean(false);
        if (g_strcmp0(prop, "Identity") == 0)         return g_variant_new_string("Localify");
        if (g_strcmp0(prop, "SupportedUriSchemes") == 0) { const char* v[1] = {nullptr}; return g_variant_new_strv(v, 0); }
        if (g_strcmp0(prop, "SupportedMimeTypes") == 0)  { const char* v[1] = {nullptr}; return g_variant_new_strv(v, 0); }
        return nullptr;
    }
    if (g_strcmp0(prop, "PlaybackStatus") == 0) return g_variant_new_string(s.status.c_str());
    if (g_strcmp0(prop, "LoopStatus") == 0)     return g_variant_new_string(s.loop_status.c_str());
    if (g_strcmp0(prop, "Rate") == 0)           return g_variant_new_double(1.0);
    if (g_strcmp0(prop, "Shuffle") == 0)        return g_variant_new_boolean(s.shuffle);
    if (g_strcmp0(prop, "Metadata") == 0)       return build_metadata_locked();
    if (g_strcmp0(prop, "Volume") == 0)         return g_variant_new_double(s.volume);
    if (g_strcmp0(prop, "Position") == 0)       return g_variant_new_int64(s.position_us);
    if (g_strcmp0(prop, "MinimumRate") == 0)    return g_variant_new_double(1.0);
    if (g_strcmp0(prop, "MaximumRate") == 0)    return g_variant_new_double(1.0);
    if (g_strcmp0(prop, "CanGoNext") == 0)      return g_variant_new_boolean(true);
    if (g_strcmp0(prop, "CanGoPrevious") == 0)  return g_variant_new_boolean(true);
    if (g_strcmp0(prop, "CanPlay") == 0)        return g_variant_new_boolean(true);
    if (g_strcmp0(prop, "CanPause") == 0)       return g_variant_new_boolean(true);
    if (g_strcmp0(prop, "CanSeek") == 0)        return g_variant_new_boolean(true);
    if (g_strcmp0(prop, "CanControl") == 0)     return g_variant_new_boolean(true);
    return nullptr;
}

// LoopStatus/Rate/Shuffle/Volume are declared read-write per spec (some
// clients refuse to show a control at all for a read-only property), but
// this app's own settings surface, not a remote widget, is where those
// actually get changed -- accept the write silently without applying it
// rather than reporting a capability with no effect behind it.
gboolean set_property(GDBusConnection*, const gchar*, const gchar*, const gchar*,
                       const gchar*, GVariant*, GError**, gpointer) {
    return true;
}

const GDBusInterfaceVTable kVTable = { method_call, get_property, set_property, {nullptr} };

void on_bus_acquired(GDBusConnection* conn, const gchar*, gpointer) {
    GError* error = nullptr;
    GDBusNodeInfo* node = g_dbus_node_info_new_for_xml(kIntrospectionXml, &error);
    if (!node) { g_clear_error(&error); return; }
    g_conn = conn;
    GDBusInterfaceInfo* root_iface   = g_dbus_node_info_lookup_interface(node, "org.mpris.MediaPlayer2");
    GDBusInterfaceInfo* player_iface = g_dbus_node_info_lookup_interface(node, "org.mpris.MediaPlayer2.Player");
    g_reg_root = g_dbus_connection_register_object(conn, "/org/mpris/MediaPlayer2",
        root_iface, &kVTable, nullptr, nullptr, &error);
    if (!g_reg_root) g_clear_error(&error);
    g_reg_player = g_dbus_connection_register_object(conn, "/org/mpris/MediaPlayer2",
        player_iface, &kVTable, nullptr, nullptr, &error);
    if (!g_reg_player) g_clear_error(&error);
    g_dbus_node_info_unref(node);
}

void emit_changed(const char* iface, GVariantBuilder* changed) {
    if (!g_conn) return;
    GVariantBuilder invalidated;
    g_variant_builder_init(&invalidated, G_VARIANT_TYPE("as"));
    g_dbus_connection_emit_signal(g_conn, nullptr, "/org/mpris/MediaPlayer2",
        "org.freedesktop.DBus.Properties", "PropertiesChanged",
        g_variant_new("(sa{sv}as)", iface, changed, &invalidated), nullptr);
}

std::string jstr(const Value& d, const char* key) {
    if (!d.is_dict()) return "";
    Value v = const_cast<Value&>(d).as_dict()[key];
    return v.is_str() ? v.as_str() : "";
}

} // namespace

void mpris_init(const std::string& app_id, const std::string&, std::uint16_t server_port) {
    g_port = server_port;
    std::string bus_name = "org.mpris.MediaPlayer2." + app_id;
    // Sanitize like sanitize_id() in runtime.cpp: D-Bus well-known names
    // only allow [A-Za-z0-9_], dashes not included.
    for (char& c : bus_name) if (c == '-') c = '_';
    g_owner_id = g_bus_own_name(G_BUS_TYPE_SESSION, bus_name.c_str(), G_BUS_NAME_OWNER_FLAGS_NONE,
        on_bus_acquired, nullptr, nullptr, nullptr, nullptr);
}

void mpris_update(const Value& d) {
    if (!d.is_dict()) return;
    State& s = state();
    std::string iface_root_unused;
    GVariantBuilder player_changed;
    g_variant_builder_init(&player_changed, G_VARIANT_TYPE("a{sv}"));
    bool metadata_changed = false;
    bool status_changed = false;

    {
        std::lock_guard<std::mutex> lk(s.mutex);
        std::string new_track_id = jstr(d, "trackId");
        std::string new_title    = jstr(d, "title");
        std::string new_artist   = jstr(d, "artist");
        std::string new_album    = jstr(d, "album");
        std::string new_art      = jstr(d, "artUrl");
        std::string new_status_raw = jstr(d, "status"); // "playing"|"paused"|"stopped"
        std::string new_status = new_status_raw == "playing" ? "Playing"
                                : new_status_raw == "paused"  ? "Paused" : "Stopped";
        Value dm = const_cast<Value&>(d).as_dict()["durationMs"];
        Value pm = const_cast<Value&>(d).as_dict()["positionMs"];
        Value vo = const_cast<Value&>(d).as_dict()["volume"];
        Value sh = const_cast<Value&>(d).as_dict()["shuffle"];
        Value rp = const_cast<Value&>(d).as_dict()["repeat"]; // "off"|"track"|"queue"
        long long new_duration_us = dm.is_null() ? 0 : static_cast<long long>(dm.as_float() * 1000.0);
        long long new_position_us = pm.is_null() ? 0 : static_cast<long long>(pm.as_float() * 1000.0);
        double new_volume = vo.is_null() ? s.volume : vo.as_float();
        bool new_shuffle = sh.is_null() ? s.shuffle : sh.as_bool();
        std::string rp_raw = rp.is_str() ? rp.as_str() : "off";
        std::string new_loop = rp_raw == "track" ? "Track" : rp_raw == "queue" ? "Playlist" : "None";
        bool new_has_track = new_track_id != "";

        metadata_changed = new_track_id != s.track_id || new_title != s.title ||
            new_artist != s.artist || new_album != s.album || new_art != s.art_url ||
            new_duration_us != s.duration_us || new_has_track != s.has_track;
        status_changed = new_status != s.status;
        bool volume_changed = new_volume != s.volume;
        bool shuffle_changed = new_shuffle != s.shuffle;
        bool loop_changed = new_loop != s.loop_status;

        s.track_id = new_track_id; s.title = new_title; s.artist = new_artist;
        s.album = new_album; s.art_url = new_art; s.duration_us = new_duration_us;
        s.status = new_status; s.position_us = new_position_us; s.volume = new_volume;
        s.shuffle = new_shuffle; s.loop_status = new_loop; s.has_track = new_has_track;

        if (metadata_changed) g_variant_builder_add(&player_changed, "{sv}", "Metadata", build_metadata_locked());
        if (status_changed)   g_variant_builder_add(&player_changed, "{sv}", "PlaybackStatus", g_variant_new_string(s.status.c_str()));
        if (volume_changed)   g_variant_builder_add(&player_changed, "{sv}", "Volume", g_variant_new_double(s.volume));
        if (shuffle_changed)  g_variant_builder_add(&player_changed, "{sv}", "Shuffle", g_variant_new_boolean(s.shuffle));
        if (loop_changed)     g_variant_builder_add(&player_changed, "{sv}", "LoopStatus", g_variant_new_string(s.loop_status.c_str()));
    }
    // Position deliberately has no PropertiesChanged signal: the MPRIS
    // spec says not to (clients that care poll Position directly, and a
    // ~1/s tick would just be a stream of Seeked-shaped noise otherwise).
    if (metadata_changed || status_changed) emit_changed("org.mpris.MediaPlayer2.Player", &player_changed);
    else g_variant_builder_clear(&player_changed);
}

void mpris_shutdown() {
    if (g_conn) {
        if (g_reg_root)   g_dbus_connection_unregister_object(g_conn, g_reg_root);
        if (g_reg_player) g_dbus_connection_unregister_object(g_conn, g_reg_player);
    }
    if (g_owner_id) g_bus_unown_name(g_owner_id);
}

} // namespace luxdesktop

#pragma once
// Loopback-socket bits shared by desktop_common.cpp and runtime.cpp, over
// BSD sockets (Linux) or Winsock (Windows).
#ifdef _WIN32
#  ifndef WIN32_LEAN_AND_MEAN
#    define WIN32_LEAN_AND_MEAN
#  endif
#  ifndef NOMINMAX
#    define NOMINMAX
#  endif
#  include <winsock2.h>
#  include <ws2tcpip.h>
#else
#  include <arpa/inet.h>
#  include <netinet/in.h>
#  include <sys/socket.h>
#  include <unistd.h>
#endif
#include <cerrno>
#include <string>
#include <cstring>

namespace luxdesktop {

#ifdef _WIN32
using sock_t = SOCKET;
using sock_len_t = int;
constexpr sock_t kBadSock = INVALID_SOCKET;
inline void net_init() { static const int once = [] { WSADATA d; return WSAStartup(MAKEWORD(2, 2), &d); }(); (void)once; }
inline void sock_close(sock_t s) { ::closesocket(s); }
inline std::string sock_error() { return "winsock error " + std::to_string(WSAGetLastError()); }
inline void set_timeouts(sock_t s, int seconds) {
    DWORD ms = static_cast<DWORD>(seconds) * 1000;   // Windows takes milliseconds, not a timeval
    ::setsockopt(s, SOL_SOCKET, SO_SNDTIMEO, reinterpret_cast<const char*>(&ms), sizeof(ms));
    ::setsockopt(s, SOL_SOCKET, SO_RCVTIMEO, reinterpret_cast<const char*>(&ms), sizeof(ms));
}
#else
using sock_t = int;
using sock_len_t = socklen_t;
constexpr sock_t kBadSock = -1;
inline void net_init() {}
inline void sock_close(sock_t s) { ::close(s); }
inline std::string sock_error() { return std::strerror(errno); }
inline void set_timeouts(sock_t s, int seconds) {
    timeval tv{seconds, 0};
    ::setsockopt(s, SOL_SOCKET, SO_SNDTIMEO, &tv, sizeof(tv));
    ::setsockopt(s, SOL_SOCKET, SO_RCVTIMEO, &tv, sizeof(tv));
}
#endif

} // namespace luxdesktop

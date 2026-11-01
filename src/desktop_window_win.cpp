// Windows twin of desktop_window.cpp: the vendored webview.h picks its
// WebView2 (Edge) backend by itself under _WIN32, and webview_get_window()
// returns the HWND, which is all the Win32 calls below need. Build needs
// WebView2.h (see CMakeLists.txt); at runtime the WebView2 Runtime (shipped
// with Windows 11 / Edge) must be installed.
#include "desktop_window.hpp"
#include <webview/webview.h>

#include <commdlg.h>
#include <windows.h>

#include <filesystem>
#include <fstream>
#include <future>
#include <iterator>
#include <vector>

namespace {

HWND hwnd_of(webview_t w) { return static_cast<HWND>(webview_get_window(w)); }

WNDPROC g_old_proc = nullptr;
DesktopWindow* g_size_owner = nullptr;
std::atomic<int>* g_wptr = nullptr;
std::atomic<int>* g_hptr = nullptr;

// Tracks the live client size (in 96-dpi units, what webview_set_size takes)
// so it is still known after the window is destroyed and run() has returned.
LRESULT CALLBACK size_proc(HWND h, UINT msg, WPARAM wp, LPARAM lp) {
    if (msg == WM_SIZE && wp != SIZE_MINIMIZED && wp != SIZE_MAXIMIZED && g_wptr) {
        UINT dpi = GetDpiForWindow(h);
        if (dpi == 0) dpi = 96;
        g_wptr->store(static_cast<int>(LOWORD(lp)) * 96 / static_cast<int>(dpi));
        g_hptr->store(static_cast<int>(HIWORD(lp)) * 96 / static_cast<int>(dpi));
    }
    return CallWindowProcW(g_old_proc, h, msg, wp, lp);
}

std::wstring widen(const std::string& u) {
    if (u.empty()) return {};
    int n = MultiByteToWideChar(CP_UTF8, 0, u.data(), (int)u.size(), nullptr, 0);
    std::wstring w(n, L'\0');
    MultiByteToWideChar(CP_UTF8, 0, u.data(), (int)u.size(), w.data(), n);
    return w;
}

// .ico via LoadImage; anything else (the app ships a PNG) is handed to
// CreateIconFromResourceEx, which decodes PNG data directly.
HICON load_icon(const std::string& path) {
    std::wstring wp = widen(path);
    if (wp.size() > 4 && _wcsicmp(wp.c_str() + wp.size() - 4, L".ico") == 0)
        return static_cast<HICON>(LoadImageW(nullptr, wp.c_str(), IMAGE_ICON, 0, 0, LR_LOADFROMFILE | LR_DEFAULTSIZE));
    std::ifstream in(std::filesystem::path(wp), std::ios::binary);
    std::vector<unsigned char> bytes((std::istreambuf_iterator<char>(in)), std::istreambuf_iterator<char>());
    if (bytes.empty()) return nullptr;
    return CreateIconFromResourceEx(bytes.data(), (DWORD)bytes.size(), TRUE, 0x00030000, 256, 256, LR_DEFAULTCOLOR);
}

} // namespace

DesktopWindow::DesktopWindow(Options opts) {
    auto w = webview_create(opts.devtools ? 1 : 0, nullptr);
    webview_set_title(w, opts.title.c_str());
    webview_set_size(w, opts.width, opts.height,
                      opts.resizable ? WEBVIEW_HINT_NONE : WEBVIEW_HINT_FIXED);
    HWND h = hwnd_of(w);
    if (!opts.icon.empty()) {
        if (HICON ic = load_icon(opts.icon)) {   // a missing/bad icon is cosmetic, never fatal
            SendMessageW(h, WM_SETICON, ICON_BIG, (LPARAM)ic);
            SendMessageW(h, WM_SETICON, ICON_SMALL, (LPARAM)ic);
        }
    }
    // webview.h already showed the window; minimize it right away.
    if (opts.start_minimized) ShowWindow(h, SW_SHOWMINNOACTIVE);
    last_width_  = opts.width;
    last_height_ = opts.height;
    g_size_owner = this; g_wptr = &last_width_; g_hptr = &last_height_;
    g_old_proc = reinterpret_cast<WNDPROC>(
        SetWindowLongPtrW(h, GWLP_WNDPROC, reinterpret_cast<LONG_PTR>(size_proc)));
    handle_ = w;
}

DesktopWindow::~DesktopWindow() {
    g_wptr = g_hptr = nullptr;
    if (handle_) webview_destroy(static_cast<webview_t>(handle_));
}

void DesktopWindow::run(const std::string& url) {
    auto w = static_cast<webview_t>(handle_);
    webview_navigate(w, url.c_str());
    running_ = true;
    webview_run(w);   // returns on WM_DESTROY (webview.h calls terminate())
    running_ = false;
}

void DesktopWindow::terminate() {
    if (running_.exchange(false)) webview_terminate(static_cast<webview_t>(handle_));
}

void DesktopWindow::reload() {
    webview_dispatch(static_cast<webview_t>(handle_),
        [](webview_t w, void*) { webview_eval(w, "location.reload()"); }, nullptr);
}

void DesktopWindow::minimize() {
    webview_dispatch(static_cast<webview_t>(handle_),
        [](webview_t w, void*) { ShowWindow(hwnd_of(w), SW_MINIMIZE); }, nullptr);
}

void DesktopWindow::restore() {
    webview_dispatch(static_cast<webview_t>(handle_),
        [](webview_t w, void*) {
            HWND h = hwnd_of(w);
            // Only un-minimize: SW_RESTORE on a maximized window would un-maximize it.
            if (IsIconic(h)) ShowWindow(h, SW_RESTORE); else ShowWindow(h, SW_SHOW);
            SetForegroundWindow(h);
        }, nullptr);
}

namespace {
struct FilePickCtx {
    std::promise<std::string> result;
    std::string               suggested_name;
    bool                      save_mode;
};
} // namespace

std::string DesktopWindow::pick_file(const std::string& suggested_name, bool save_mode) {
    auto* ctx = new FilePickCtx{{}, suggested_name, save_mode};
    auto future = ctx->result.get_future();
    // GetOpenFileName runs its own modal loop on the window's thread; this
    // (blocking_pool) thread just waits on the future, like the GTK version.
    webview_dispatch(static_cast<webview_t>(handle_),
        [](webview_t w, void* arg) {
            auto* ctx = static_cast<FilePickCtx*>(arg);
            wchar_t buf[32768] = {};
            std::wstring name = widen(ctx->suggested_name);
            wcsncpy(buf, name.c_str(), 260);
            OPENFILENAMEW ofn{};
            ofn.lStructSize = sizeof(ofn);
            ofn.hwndOwner   = hwnd_of(w);
            ofn.lpstrFile   = buf;
            ofn.nMaxFile    = (DWORD)(sizeof(buf) / sizeof(buf[0]));
            ofn.Flags       = OFN_EXPLORER | OFN_NOCHANGEDIR |
                              (ctx->save_mode ? OFN_OVERWRITEPROMPT : OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST);
            BOOL ok = ctx->save_mode ? GetSaveFileNameW(&ofn) : GetOpenFileNameW(&ofn);
            std::string path;
            if (ok) {
                int n = WideCharToMultiByte(CP_UTF8, 0, buf, -1, nullptr, 0, nullptr, nullptr);
                if (n > 1) {
                    path.resize(n - 1);
                    WideCharToMultiByte(CP_UTF8, 0, buf, -1, path.data(), n, nullptr, nullptr);
                }
            }
            ctx->result.set_value(path);
            delete ctx;
        }, ctx);
    return future.get();
}

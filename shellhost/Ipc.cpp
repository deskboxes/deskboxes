// Named-pipe server: the core (deskboxes.exe) asks the host for menus. Protocol in
// docs/ARCHITECTURE.md §11: u32 LE length + UTF-8 JSON { "v":1, "t":type, "id":n, ... }.
//
// Threads: a reader thread blocks on the pipe and posts each message to the XAML UI thread,
// where all shell work runs (handlers and their class factories are bound to that STA).
// The pipe handle is overlapped so a pending read never blocks a write from the UI thread.
// When the core disconnects, the host exits: it never outlives its core.

#include "pch.h"
#include "Ipc.h"
#include "Log.h"
#include "MenuModel.h"
#include "MenuView.h"
#include "Settings.h"
#include <powrprof.h>
#pragma comment(lib, "powrprof.lib")

using namespace winrt;
using namespace winrt::Windows::Data::Json;

namespace dbx
{
    namespace
    {
        HANDLE g_pipe = INVALID_HANDLE_VALUE;
        std::mutex g_writeLock;
        winrt::Microsoft::UI::Dispatching::DispatcherQueue g_ui{ nullptr };
        std::shared_ptr<BuiltMenu> g_prefetched;   // last prefetch result
        std::wstring g_prefetchKey;
        double g_settingsOpenId = 0;
        bool g_menuOpen = false;
        JsonObject g_nextShow{ nullptr };   // a "show" that came while a menu was open
        std::atomic<uint64_t> g_prefetchSeq{ 0 };   // prefetches read from the pipe so far
        std::atomic<int> g_showsWaiting{ 0 };       // shows read, not handled yet
        void Prewarm();
        // The first menu is built ahead once the core has its "ready" (0) and the package scan
        // is done (1), a moment later (what the core asks for first, as Settings, goes first).
        bool g_said[2] = { false, false };
        void PrewarmSoon()
        {
            static bool started = false;
            if (started || !g_said[0] || !g_said[1]) return;
            started = true;
            static winrt::Microsoft::UI::Dispatching::DispatcherQueueTimer t{ nullptr };
            t = g_ui.CreateTimer();
            t.Interval(std::chrono::milliseconds(400));
            t.IsRepeating(false);
            t.Tick([](auto&&, auto&&) { Prewarm(); });
            t.Start();
        }
        void RecycleWhenIdle(const wchar_t* why);

        bool IoWait(BOOL ok, OVERLAPPED& ov, DWORD& n)
        {
            if (!ok && GetLastError() != ERROR_IO_PENDING) return false;
            return GetOverlappedResult(g_pipe, &ov, &n, TRUE) != FALSE;
        }

        bool ReadExact(void* buf, DWORD size)
        {
            BYTE* p = static_cast<BYTE*>(buf);
            OVERLAPPED ov{}; ov.hEvent = CreateEventW(nullptr, TRUE, FALSE, nullptr);
            bool ok = true;
            while (size && ok)
            {
                DWORD n = 0; ResetEvent(ov.hEvent);
                ok = IoWait(ReadFile(g_pipe, p, size, nullptr, &ov), ov, n) && n > 0;
                p += n; size -= n;
            }
            CloseHandle(ov.hEvent);
            return ok;
        }

        bool Send(JsonObject const& o)
        {
            o.SetNamedValue(L"v", JsonValue::CreateNumberValue(1));
            std::string body = WideToUtf8(o.Stringify().c_str());
            uint32_t len = (uint32_t)body.size();
            std::lock_guard lock(g_writeLock);
            if (g_pipe == INVALID_HANDLE_VALUE) return false;
            OVERLAPPED ov{}; ov.hEvent = CreateEventW(nullptr, TRUE, FALSE, nullptr);
            DWORD n = 0;
            bool ok = IoWait(WriteFile(g_pipe, &len, 4, nullptr, &ov), ov, n) && n == 4;
            ResetEvent(ov.hEvent);
            if (ok) ok = IoWait(WriteFile(g_pipe, body.data(), len, nullptr, &ov), ov, n) && n == len;
            CloseHandle(ov.hEvent);
            if (!ok) Log(L"pipe write failed (%lu)", GetLastError());
            return ok;
        }

        JsonObject Msg(const wchar_t* type, double id)
        {
            JsonObject o;
            o.SetNamedValue(L"t", JsonValue::CreateStringValue(type));
            o.SetNamedValue(L"id", JsonValue::CreateNumberValue(id));
            return o;
        }

        std::wstring Str(JsonObject const& o, const wchar_t* k) { return o.HasKey(k) ? std::wstring(o.GetNamedString(k, L"").c_str()) : std::wstring(); }
        bool Bool(JsonObject const& o, const wchar_t* k) { return o.HasKey(k) && o.GetNamedBoolean(k, false); }

        std::vector<MenuEntry> Entries(JsonArray const& a)
        {
            std::vector<MenuEntry> out;
            for (auto const& x : a)
            {
                auto o = x.GetObject();
                MenuEntry e;
                e.source = Source::Deskboxes;
                e.separator = Bool(o, L"separator");
                e.id = Str(o, L"id");
                // "@dll,-id": Windows' own text for an entry Explorer has too (View, Sort by).
                std::wstring text = Str(o, L"text");
                if (!text.empty() && text[0] == L'@')
                {
                    wchar_t buf[512]{};
                    if (SUCCEEDED(SHLoadIndirectString(text.c_str(), buf, ARRAYSIZE(buf), nullptr))) text = buf;
                }
                e.text = StripMnemonic(text, &e.accessKey);
                e.glyph = Str(o, L"glyph");
                e.iconRef = Str(o, L"icon"); // "path,index"
                e.color = Str(o, L"color");
                e.checked = Bool(o, L"checked");
                e.radio = Bool(o, L"radio");
                e.checkable = Bool(o, L"checkable");
                e.disabled = o.HasKey(L"enabled") && !o.GetNamedBoolean(L"enabled", true);
                e.packageFamily = Str(o, L"placement"); // InsertExtras reads the placement here
                if (o.HasKey(L"children")) { e.children = Entries(o.GetNamedArray(L"children")); e.hasSubmenu = true; e.childrenLoaded = true; }
                out.push_back(std::move(e));
            }
            return out;
        }

        MenuRequest Request(JsonObject const& o)
        {
            MenuRequest r;
            r.background = Str(o, L"kind") == L"background";
            r.ownOnly = Str(o, L"kind") == L"own";
            r.folder = Str(o, L"folder");
            if (o.HasKey(L"items")) for (auto const& x : o.GetNamedArray(L"items")) r.items.push_back(x.GetString().c_str());
            r.shift = Bool(o, L"shift");
            r.keyboard = Bool(o, L"keyboard");
            r.coreUndo = Bool(o, L"undo");
            r.coreRedo = Bool(o, L"redo");
            r.coreUndoText = Str(o, L"undo_text");
            r.coreRedoText = Str(o, L"redo_text");
            r.desktop = r.background && (r.folder.empty() || Bool(o, L"desktop"));
            r.pt.x = (LONG)o.GetNamedNumber(L"x", 0);
            r.pt.y = (LONG)o.GetNamedNumber(L"y", 0);
            r.owner = (HWND)(uintptr_t)o.GetNamedNumber(L"owner", 0);
            if (o.HasKey(L"extra")) { r.extra = Entries(o.GetNamedArray(L"extra")); r.extraJson = o.GetNamedArray(L"extra").Stringify().c_str(); }
            if (o.HasKey(L"own"))
                for (auto const& kv : o.GetNamedObject(L"own")) r.ownChildren[kv.Key().c_str()] = Entries(kv.Value().GetArray());
            return r;
        }

        // Cache key of a prefetch: what decides the menu's content (not the position).
        std::wstring Key(const MenuRequest& r)
        {
            std::wstring k = r.background ? L"bg|" + r.folder : L"items";
            for (auto& i : r.items) k += L"|" + i;
            if (r.shift) k += L"|shift";
            k += L"|" + r.extraJson; // extras are built into the menu
            return k;
        }

        void Handle(JsonObject const& o)
        {
            auto t = Str(o, L"t");
            double id = o.GetNamedNumber(L"id", 0);
            if (t == L"hello")
            {
                auto m = Msg(L"ready", id);
                m.SetNamedValue(L"proto", JsonValue::CreateNumberValue(1));
                Send(m);
                // Only now the first menu is built ahead: before, "ready" (and with it every
                // menu and Settings) waited behind it, 0.6-2.8 s after the host started.
                g_said[0] = true;
                PrewarmSoon();
            }
            else if (t == L"prefetch")
            {
                // Build ahead (on hover / selection) so "show" only has to draw.
                MenuRequest r = Request(o);
                g_prefetchKey = Key(r);
                g_prefetched = BuildMenu(r);
                Log(L"prefetch %zu item(s): resolve %.1f, classic %.1f, packaged %.1f, total %.1f ms",
                    r.items.size(), g_prefetched->msResolve, g_prefetched->msClassic, g_prefetched->msPackaged, g_prefetched->msTotal);
            }
            else if (t == L"show")
            {
                // A right-click elsewhere while a menu is open: that menu closes and the new one
                // opens where the click was, as Explorer's do.
                if (g_menuOpen)
                {
                    Log(L"show %.0f: closing the open menu first", id);
                    g_nextShow = o;
                    HideMenuView();
                    return;
                }
                double t0 = NowMs();
                // The UI thread is on it (see "got").
                Send(Msg(L"building", id));
                MenuRequest r = Request(o);
                std::shared_ptr<BuiltMenu> m;
                // A prefetched model is reused when it was built for the same items and extras (the
                // background menu's own entries change per show, so it is always built at show).
                if (g_prefetched && g_prefetchKey == Key(r) && r.ownChildren.empty() && !r.desktop && !r.ownOnly)
                {
                    m = g_prefetched;
                    POINT pt = r.pt; bool kb = r.keyboard;
                    m->req.pt = pt; m->req.keyboard = kb; m->req.owner = r.owner;
                }
                else
                {
                    m = BuildMenu(r);
                    Log(L"show %zu item(s): built at show (%s): resolve %.1f, classic %.1f, packaged %.1f, total %.1f ms",
                        r.items.size(), g_prefetched ? L"prefetch key differed" : L"no prefetch",
                        m->msResolve, m->msClassic, m->msPackaged, m->msTotal);
                }
                g_prefetched.reset(); g_prefetchKey.clear();
                if (m->row.empty() && m->list.empty())
                {
                    auto c = Msg(L"closed", id); c.SetNamedValue(L"invoked", JsonValue::CreateNullValue());
                    c.SetNamedValue(L"error", JsonValue::CreateStringValue(L"empty menu"));
                    Send(c);
                    return;
                }
                Log(L"show %.0f: opening the menu view", id);
                g_menuOpen = true;
                MenuViewEvents ev;
                ev.opened = [id, t0](double) {
                    auto s = Msg(L"shown", id);
                    s.SetNamedValue(L"ms", JsonValue::CreateNumberValue(NowMs() - t0));
                    Send(s);
                };
                ev.command = [id](const std::wstring& cmd) {
                    auto c = Msg(L"command", id);
                    c.SetNamedValue(L"cmd", JsonValue::CreateStringValue(cmd));
                    Send(c);
                };
                // Opened, but nothing of it on a screen (MenuView): the core asks a fresh host to
                // show it again, and this one makes way (RecycleWhenIdle).
                auto nothing = std::make_shared<bool>(false);
                ev.nothingShown = [nothing] { *nothing = true; RecycleWhenIdle(L"a menu showed nothing"); };
                ev.done = [id, nothing](const std::wstring& invoked) {
                    g_menuOpen = false;
                    auto c = Msg(L"closed", id);
                    c.SetNamedValue(L"invoked", invoked.empty() ? JsonValue::CreateNullValue() : JsonValue::CreateStringValue(invoked));
                    if (*nothing) c.SetNamedValue(L"error", JsonValue::CreateStringValue(L"nothing shown"));
                    Send(c);
                    // The menu asked for while this one was open.
                    if (g_nextShow)
                    {
                        auto next = g_nextShow;
                        g_nextShow = nullptr;
                        // After the old view is torn down (queued by its Closed handler).
                        g_ui.TryEnqueue(winrt::Microsoft::UI::Dispatching::DispatcherQueuePriority::Low, [next] { try { Handle(next); } catch (...) {} });
                    }
                };
                try { ShowMenuView(m, std::move(ev)); }
                catch (winrt::hresult_error const& e)
                {
                    // Never left thinking a menu is open (every later "show" would wait for it).
                    Log(L"show %.0f: menu view failed 0x%08X %s", id, (unsigned)e.code().value, e.message().c_str());
                    g_menuOpen = false;
                    auto c = Msg(L"closed", id); c.SetNamedValue(L"invoked", JsonValue::CreateNullValue());
                    c.SetNamedValue(L"error", JsonValue::CreateStringValue(L"menu view failed"));
                    Send(c);
                }
            }
            else if (t == L"settings")
            {
                bool open = Bool(o, L"open");
                if (open) g_settingsOpenId = id;
                ShowSettings(o, open, [](JsonObject m)
                {
                    m.SetNamedValue(L"t", JsonValue::CreateStringValue(L"setting"));
                    m.SetNamedValue(L"id", JsonValue::CreateNumberValue(0));
                    Send(m);
                });
            }
            else if (t == L"colorpicker")
            {
                ShowColorPicker(o, [](JsonObject m)
                {
                    m.SetNamedValue(L"t", JsonValue::CreateStringValue(L"setting"));
                    m.SetNamedValue(L"id", JsonValue::CreateNumberValue(0));
                    Send(m);
                });
            }
            else if (t == L"dialog")
            {
                ShowDialog(o, [](JsonObject m)
                {
                    m.SetNamedValue(L"t", JsonValue::CreateStringValue(L"setting"));
                    m.SetNamedValue(L"id", JsonValue::CreateNumberValue(0));
                    Send(m);
                });
            }
            else if (t == L"quit")
            {
                Log(L"quit requested");
                winrt::Microsoft::UI::Xaml::Application::Current().Exit();
            }
            else Log(L"pipe: unknown message type '%s' ignored", t.c_str());
        }

        void Prewarm()
        {
            // Pay the package scan and the out-of-process activations once, before the first
            // right-click (cold build ~0.6 s vs ~25-75 ms warm; REPORT.md §7).
            double t0 = NowMs();
            LoadPackagedRegistrations();
            MenuRequest r; r.background = true; r.desktop = true;
            auto m = BuildMenu(r);
            Log(L"prewarm: desktop background menu in %.1f ms (%zu entries)", NowMs() - t0, m->list.size());
        }

        // After a sleep, or the screens going off and on, WinUI's menus could stay broken in this
        // process: a right-click showed nothing until the host was ended by hand (WinUI popups have
        // known faults of this kind with no fix: microsoft-ui-xaml#11068, #9832). So an open menu
        // is closed before the PC sleeps, and afterwards this host makes way for a fresh one as
        // soon as no menu or window of it is open (the core starts the next at once, and it builds
        // the first menu ahead again). MenuView also checks every menu it opens (MenuShowsNothing).
        bool g_recycle = false;
        const wchar_t* g_recycleWhy = L"";
        winrt::Microsoft::UI::Dispatching::DispatcherQueueTimer g_recycleTimer{ nullptr };

        void RecycleNow()
        {
            if (!g_recycle) return;
            if (g_menuOpen || HostWindowsOpen()) return;   // (the timer asks again)
            Log(L"recycling: %s; a fresh host takes over", g_recycleWhy);
            if (g_recycleTimer) g_recycleTimer.Stop();
            // The core sees the pipe close and starts the next host (crates/desktop start_host).
            g_ui.TryEnqueue([] { winrt::Microsoft::UI::Xaml::Application::Current().Exit(); });
        }

        void RecycleWhenIdle(const wchar_t* why)
        {
            if (g_recycle) return;
            g_recycle = true;
            g_recycleWhy = why;
            g_recycleTimer = g_ui.CreateTimer();
            g_recycleTimer.Interval(std::chrono::seconds(2));
            g_recycleTimer.Tick([](auto&&, auto&&) { RecycleNow(); });
            g_recycleTimer.Start();
            RecycleNow();
        }

        ULONG CALLBACK OnPower(PVOID, ULONG type, PVOID setting)
        {
            if (type == PBT_APMSUSPEND)
                g_ui.TryEnqueue([] { if (g_menuOpen) { Log(L"the PC sleeps: closing the open menu"); HideMenuView(); } });
            else if (type == PBT_APMRESUMEAUTOMATIC)
                g_ui.TryEnqueue([] { RecycleWhenIdle(L"the PC woke from sleep"); });
            else if (type == PBT_POWERSETTINGCHANGE && setting)
            {
                // The screens (GUID_CONSOLE_DISPLAY_STATE: 0 off, 1 on, 2 dimmed). Off and on again
                // can be monitors that were disconnected and connected again.
                static std::atomic<DWORD> last{ 1 };
                auto* ps = static_cast<POWERBROADCAST_SETTING*>(setting);
                if (ps->PowerSetting == GUID_CONSOLE_DISPLAY_STATE && ps->DataLength >= sizeof(DWORD))
                {
                    DWORD now = *reinterpret_cast<DWORD*>(ps->Data);
                    DWORD before = last.exchange(now);
                    if (now == 1 && before == 0)
                        g_ui.TryEnqueue([] { RecycleWhenIdle(L"the screens came back on"); });
                    else if (now == 0)
                        g_ui.TryEnqueue([] { if (g_menuOpen) HideMenuView(); });
                }
            }
            return 0;
        }

        void WatchPower()
        {
            static DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS p{ OnPower, nullptr };
            HPOWERNOTIFY sleep = nullptr, screens = nullptr;
            DWORD e1 = PowerRegisterSuspendResumeNotification(DEVICE_NOTIFY_CALLBACK, &p, &sleep);
            DWORD e2 = PowerSettingRegisterNotification(&GUID_CONSOLE_DISPLAY_STATE, DEVICE_NOTIFY_CALLBACK, &p, &screens);
            if (e1 || e2) Log(L"power notifications: sleep %lu, screens %lu", e1, e2);
        }

        void ReaderThread()
        {
            OVERLAPPED ov{}; ov.hEvent = CreateEventW(nullptr, TRUE, FALSE, nullptr);
            DWORD n = 0;
            BOOL c = ConnectNamedPipe(g_pipe, &ov);
            bool connected = c || GetLastError() == ERROR_PIPE_CONNECTED || (GetLastError() == ERROR_IO_PENDING && GetOverlappedResult(g_pipe, &ov, &n, TRUE));
            CloseHandle(ov.hEvent);
            if (!connected) Log(L"pipe connect failed (%lu)", GetLastError());
            else
            {
                Log(L"pipe: core connected");
                for (;;)
                {
                    uint32_t len = 0;
                    if (!ReadExact(&len, 4)) break;
                    if (len == 0 || len > (16u << 20)) { Log(L"pipe: bad frame length %u", len); break; }
                    std::string body(len, '\0');
                    if (!ReadExact(body.data(), len)) break;
                    // A prefetch is built on the UI thread: one that a newer prefetch or a "show"
                    // came after while it waited is of no use (a show drops it anyway), and is
                    // not built, so the menu asked for is not kept waiting behind it.
                    // (The core writes compact JSON: `"t":"…"` is the type; inside a string a
                    // quote is escaped, so it cannot match there.)
                    bool isPrefetch = body.find("\"t\":\"prefetch\"") != std::string::npos;
                    bool isShow = !isPrefetch && body.find("\"t\":\"show\"") != std::string::npos;
                    uint64_t seq = isPrefetch ? ++g_prefetchSeq : 0;
                    if (isShow) ++g_showsWaiting;
                    std::wstring text = Utf8ToWide(body);
                    double at = NowMs();
                    // A show is acknowledged at once, from here: the core knows this host is
                    // alive and reading, and gives a slow PC the time to build the menu (it
                    // took one that was only slow for a stuck one before).
                    if (isShow)
                    {
                        try { Send(Msg(L"got", JsonObject::Parse(text).GetNamedNumber(L"id", 0))); }
                        catch (winrt::hresult_error const&) {}
                    }
                    g_ui.TryEnqueue([text, isPrefetch, isShow, seq, at]
                    {
                        if (isShow) --g_showsWaiting;
                        // Time waited behind other work on the UI thread (a menu being built,
                        // Settings): logged when noticeable, apart from the build's own phases.
                        double waited = NowMs() - at;
                        if ((isShow || isPrefetch) && waited >= 16)
                            Log(L"%s waited %.1f ms in the UI queue", isShow ? L"show" : L"prefetch", waited);
                        if (isPrefetch && (seq != g_prefetchSeq.load() || g_showsWaiting.load() > 0))
                        {
                            Log(L"prefetch %llu skipped: %s came after it", seq, seq != g_prefetchSeq.load() ? L"a newer one" : L"a show");
                            return;
                        }
                        try { Handle(JsonObject::Parse(text)); }
                        catch (winrt::hresult_error const& e) { Log(L"pipe: bad message (%s): %.200s", e.message().c_str(), text.c_str()); }
                    });
                }
            }
            Log(L"pipe: core disconnected; host exits");
            g_ui.TryEnqueue([] { winrt::Microsoft::UI::Xaml::Application::Current().Exit(); });
        }

        std::wstring CurrentUserSid()
        {
            HANDLE tok = nullptr; std::wstring s;
            if (!OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &tok)) return s;
            BYTE buf[256]; DWORD sz = 0;
            if (GetTokenInformation(tok, TokenUser, buf, sizeof(buf), &sz))
            {
                LPWSTR str = nullptr;
                if (ConvertSidToStringSidW(reinterpret_cast<TOKEN_USER*>(buf)->User.Sid, &str)) { s = str; LocalFree(str); }
            }
            CloseHandle(tok);
            return s;
        }
    }

    void SettingsClosed()
    {
        Send(Msg(L"settingsClosed", g_settingsOpenId));
    }

    bool StartPipeServer(const std::wstring& pipeName)
    {
        g_ui = winrt::Microsoft::UI::Dispatching::DispatcherQueue::GetForCurrentThread();
        // Only the current user (and nobody else, not even administrators) may connect.
        std::wstring sid = CurrentUserSid();
        std::wstring sddl = L"D:P(A;;GA;;;" + sid + L")";
        PSECURITY_DESCRIPTOR sd = nullptr;
        if (sid.empty() || !ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl.c_str(), SDDL_REVISION_1, &sd, nullptr))
        {
            Log(L"pipe: cannot build security descriptor (%lu)", GetLastError());
            return false;
        }
        SECURITY_ATTRIBUTES sa{ sizeof(sa), sd, FALSE };
        std::wstring path = L"\\\\.\\pipe\\" + pipeName;
        g_pipe = CreateNamedPipeW(path.c_str(), PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED | FILE_FLAG_FIRST_PIPE_INSTANCE,
                                  PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                                  1, 64 * 1024, 64 * 1024, 0, &sa);
        LocalFree(sd);
        if (g_pipe == INVALID_HANDLE_VALUE) { Log(L"pipe: CreateNamedPipe(%s) failed (%lu)", path.c_str(), GetLastError()); return false; }
        Log(L"pipe: listening on %s", path.c_str());
        // The package scan meanwhile, on a thread of its own (it reads every package's manifest).
        StartPackagedScan([] { g_ui.TryEnqueue([] { g_said[1] = true; PrewarmSoon(); }); });
        std::thread(ReaderThread).detach();
        WatchPower();
        return true;
    }
}

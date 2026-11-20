// deskboxes-shellhost entry point.
//
//   --dump-menu [--out FILE] [--json] [--classic] [--shift] (--background [--folder P] | --items P1 P2 ...)
//        Build the menu model exactly as the view would and write it as text (UTF-8). No XAML.
//   --show-menu [--at X Y] [--keyboard] (same selection args)
//        Show the menu at the cursor or at physical pixel X,Y (test harness); --keyboard shows KeyTips.
//   --pipe NAME                          Serve the core over \.\pipe\NAME (normal mode; exits with the core).
//
// Never shows a MessageBox: errors go to the log and the exit code.

#include "pch.h"
#include "App.xaml.h"
#include "Log.h"
#include "MenuModel.h"
#include "MenuView.h"
#include "Ipc.h"

using namespace winrt;

namespace
{
    struct Args
    {
        bool dump = false, show = false, classic = false, json = false, at = false;
        std::wstring out, pipe;
        dbx::MenuRequest req;
    };

    Args Parse()
    {
        Args a;
        int argc = 0;
        LPWSTR* argv = CommandLineToArgvW(GetCommandLineW(), &argc);
        for (int i = 1; i < argc; ++i)
        {
            std::wstring s = argv[i];
            auto next = [&]() -> std::wstring { return i + 1 < argc ? argv[++i] : L""; };
            if (s == L"--dump-menu") a.dump = true;
            else if (s == L"--show-menu") a.show = true;
            else if (s == L"--classic") a.classic = true;
            else if (s == L"--json") a.json = true;
            else if (s == L"--shift") a.req.shift = true;
            else if (s == L"--out") a.out = next();
            else if (s == L"--pipe") a.pipe = next();
            else if (s == L"--at") { a.at = true; a.req.pt.x = _wtoi(next().c_str()); a.req.pt.y = _wtoi(next().c_str()); }
            else if (s == L"--keyboard") a.req.keyboard = true;
            else if (s == L"--background") a.req.background = true;
            else if (s == L"--folder") a.req.folder = next();
            else if (s == L"--items") { while (i + 1 < argc) a.req.items.push_back(argv[++i]); }
        }
        LocalFree(argv);
        a.req.desktop = a.req.background && a.req.folder.empty();        return a;
    }

    void WriteUtf8(const std::wstring& path, const std::wstring& text)
    {
        std::string u = "\xEF\xBB\xBF" + dbx::WideToUtf8(text);
        if (path.empty())
        {
            DWORD w = 0;
            WriteFile(GetStdHandle(STD_OUTPUT_HANDLE), u.data(), (DWORD)u.size(), &w, nullptr);
            return;
        }
        HANDLE f = CreateFileW(path.c_str(), GENERIC_WRITE, 0, nullptr, CREATE_ALWAYS, 0, nullptr);
        if (f == INVALID_HANDLE_VALUE) { dbx::Log(L"cannot write %s (%lu)", path.c_str(), GetLastError()); return; }
        DWORD w = 0; WriteFile(f, u.data(), (DWORD)u.size(), &w, nullptr); CloseHandle(f);
    }

    std::wstring ElevationLine()
    {
        HANDLE tok = nullptr; TOKEN_ELEVATION el{}; DWORD sz = 0;
        if (OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &tok))
        {
            GetTokenInformation(tok, TokenElevation, &el, sizeof(el), &sz);
            CloseHandle(tok);
        }
        return el.TokenIsElevated ? L"elevated" : L"not elevated";
    }

    int DumpMode(Args& a)
    {
        winrt::init_apartment(winrt::apartment_type::single_threaded);
        DWORD ubr = 0; DWORD build = dbx::WindowsBuild(&ubr);
        std::wstring text;
        wchar_t hdr[256];
        swprintf_s(hdr, L"deskboxes-shellhost dump; Windows build %lu.%lu; token %s; policy '%s'\n",
            build, ubr, ElevationLine().c_str(), dbx::GetPolicy().name.c_str());
        text += hdr;
        text += a.req.background ? L"target: background of " + (a.req.folder.empty() ? std::wstring(L"desktop") : a.req.folder) + L"\n"
                                 : L"target: " + std::to_wstring(a.req.items.size()) + L" item(s), first " + (a.req.items.empty() ? L"" : a.req.items.front()) + L"\n";
        // Run twice: the first build pays activation of out-of-process handlers (cold),
        // the second shows the warm cost a user sees on every later right-click.
        auto cold = dbx::BuildMenu(a.req);
        auto warm = dbx::BuildMenu(a.req);
        if (a.json)
        {
            std::wstring j = dbx::DumpMenuJson(*warm, a.classic);
            wchar_t pre[160];
            swprintf_s(pre, L"{\"build\":\"%lu.%lu\",\"elevated\":%s,\"coldMs\":%.1f,", build, ubr, ElevationLine() == L"elevated" ? L"true" : L"false", cold->msTotal);
            WriteUtf8(a.out, pre + j.substr(1));
            return warm->list.empty() && warm->row.empty() ? 1 : 0;
        }
        wchar_t t[128]; swprintf_s(t, L"cold total %.1f ms\n", cold->msTotal);
        text += t;
        text += dbx::DumpMenu(*warm, a.classic);
        WriteUtf8(a.out, text);
        return warm->list.empty() && warm->row.empty() ? 1 : 0;
    }

    int XamlMode(Args& a)
    {
        winrt::init_apartment(winrt::apartment_type::single_threaded);
        dbx::g_onLaunched = [&a]
        {
            if (!a.pipe.empty())
            {
                // Long-lived host: no window until the core asks for a menu.
                Microsoft::UI::Xaml::Application::Current().DispatcherShutdownMode(Microsoft::UI::Xaml::DispatcherShutdownMode::OnExplicitShutdown);
                if (!dbx::StartPipeServer(a.pipe)) Microsoft::UI::Xaml::Application::Current().Exit();
            }
            else if (a.show)
            {
                if (!a.at) GetCursorPos(&a.req.pt);
                std::shared_ptr<dbx::BuiltMenu> m = dbx::BuildMenu(a.req);
                dbx::MenuViewEvents ev;
                ev.command = [](const std::wstring& cmd) { dbx::Log(L"core command (no core in --show-menu): %s", cmd.c_str()); };
                ev.done = [](const std::wstring& id)
                {
                    dbx::Log(L"menu closed: %s", id.empty() ? L"(dismissed)" : id.c_str());
                    Microsoft::UI::Xaml::Application::Current().Exit();
                };
                dbx::ShowMenuView(m, std::move(ev));
            }
        };
        Microsoft::UI::Xaml::Application::Start([](auto&&) { make<winrt::DeskboxesShellHost::implementation::App>(); });
        return 0;
    }
}

// A crash must never put a dialog on the user's screen (no Windows Error Reporting UI, no
// hard-error box): log the exception and its module offset, then end the process. The core
// restarts the host (crates/desktop start_host).
static LONG WINAPI CrashFilter(EXCEPTION_POINTERS* ep)
{
    auto* er = ep ? ep->ExceptionRecord : nullptr;
    HMODULE mod = nullptr; wchar_t name[MAX_PATH] = L"?";
    if (er && GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, (LPCWSTR)er->ExceptionAddress, &mod))
        GetModuleFileNameW(mod, name, MAX_PATH);
    dbx::Log(L"CRASH: exception 0x%08X at %p (%s+0x%llX); exiting without UI",
        er ? er->ExceptionCode : 0, er ? er->ExceptionAddress : nullptr, name,
        er && mod ? (unsigned long long)((const char*)er->ExceptionAddress - (const char*)mod) : 0ull);
    TerminateProcess(GetCurrentProcess(), er ? er->ExceptionCode : 1);
    return EXCEPTION_EXECUTE_HANDLER;
}

int WINAPI wWinMain(HINSTANCE, HINSTANCE, LPWSTR, int)
{
    SetErrorMode(SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX | SEM_NOOPENFILEERRORBOX);
    SetUnhandledExceptionFilter(CrashFilter);
    dbx::LogInit();
    Args a = Parse();
    dbx::Log(L"shellhost start (%s): %s", ElevationLine().c_str(), GetCommandLineW());
    if (a.dump) return DumpMode(a);
    if (a.show || !a.pipe.empty()) return XamlMode(a);
    dbx::Log(L"no mode given; exiting");
    return 2;
}

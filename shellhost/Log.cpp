#include "pch.h"
#include "Log.h"
#include <cstdarg>
#include <share.h>
#include <psapi.h>

namespace dbx
{
    static std::mutex g_logLock;
    static FILE* g_log = nullptr;
    static LARGE_INTEGER g_freq{}, g_t0{};

    void LogInit(const wchar_t* fileOverride)
    {
        QueryPerformanceFrequency(&g_freq);
        QueryPerformanceCounter(&g_t0);
        std::wstring path;
        if (fileOverride && *fileOverride) path = fileOverride;
        else
        {
            wchar_t buf[MAX_PATH]{};
            if (GetEnvironmentVariableW(L"LOCALAPPDATA", buf, MAX_PATH))
            {
                path = std::wstring(buf) + L"\\Deskboxes";
                CreateDirectoryW(path.c_str(), nullptr);
                path += L"\\logs";
                CreateDirectoryW(path.c_str(), nullptr);
                path += L"\\shellhost.log";
            }
        }
        // Shared: a second host (tests) and readers can open it while a host runs.
        if (!path.empty()) g_log = _wfsopen(path.c_str(), L"a, ccs=UTF-8", _SH_DENYNO);
    }

    double NowMs()
    {
        LARGE_INTEGER t; QueryPerformanceCounter(&t);
        if (!g_freq.QuadPart) QueryPerformanceFrequency(&g_freq);
        return double(t.QuadPart - g_t0.QuadPart) * 1000.0 / double(g_freq.QuadPart);
    }

    void Log(const wchar_t* fmt, ...)
    {
        wchar_t buf[2048];
        va_list ap; va_start(ap, fmt);
        _vsnwprintf_s(buf, _TRUNCATE, fmt, ap);
        va_end(ap);
        wchar_t line[2200];
        swprintf_s(line, L"[%9.1f] [%5lu] %s\n", NowMs(), GetCurrentThreadId(), buf);
        OutputDebugStringW(line);
        std::lock_guard lk(g_logLock);
        if (g_log) { fputws(line, g_log); fflush(g_log); }
    }

    std::wstring Utf8ToWide(const std::string& s)
    {
        if (s.empty()) return {};
        int n = MultiByteToWideChar(CP_UTF8, 0, s.data(), (int)s.size(), nullptr, 0);
        std::wstring w(n, L'\0');
        MultiByteToWideChar(CP_UTF8, 0, s.data(), (int)s.size(), w.data(), n);
        return w;
    }
    std::string WideToUtf8(const std::wstring& w)
    {
        if (w.empty()) return {};
        int n = WideCharToMultiByte(CP_UTF8, 0, w.data(), (int)w.size(), nullptr, 0, nullptr, nullptr);
        std::string s(n, '\0');
        WideCharToMultiByte(CP_UTF8, 0, w.data(), (int)w.size(), s.data(), n, nullptr, nullptr);
        return s;
    }

    void TrimMemorySoon()
    {
        static winrt::Microsoft::UI::Dispatching::DispatcherQueueTimer timer{ nullptr };
        if (!timer)
        {
            auto q = winrt::Microsoft::UI::Dispatching::DispatcherQueue::GetForCurrentThread();
            if (!q) return;
            timer = q.CreateTimer();
            timer.IsRepeating(false);
            timer.Interval(std::chrono::seconds(4));
            timer.Tick([](auto&&, auto&&)
            {
                PROCESS_MEMORY_COUNTERS pmc{ sizeof(pmc) };
                GetProcessMemoryInfo(GetCurrentProcess(), &pmc, sizeof(pmc));
                SetProcessWorkingSetSize(GetCurrentProcess(), (SIZE_T)-1, (SIZE_T)-1);
                Log(L"memory trimmed (working set was %u MB)", (unsigned)(pmc.WorkingSetSize >> 20));
            });
        }
        timer.Stop();
        timer.Start();
    }
}


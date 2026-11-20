#pragma once
#include <string>

namespace dbx
{
    // Rolling-free simple log: %LOCALAPPDATA%\Deskboxes\logs\shellhost.log (+ OutputDebugString).
    void LogInit(const wchar_t* fileOverride = nullptr);
    void Log(const wchar_t* fmt, ...);
    double NowMs(); // monotonic ms since process start
    std::wstring Utf8ToWide(const std::string& s);
    std::string WideToUtf8(const std::wstring& s);
    // Give the memory touched by a menu or window back to Windows once it has been closed for a
    // few seconds (the next menu takes back what it needs; it opened in ~20 ms after a trim).
    // Call on the UI thread.
    void TrimMemorySoon();
}

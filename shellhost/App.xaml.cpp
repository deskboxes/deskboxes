#include "pch.h"
#include "App.xaml.h"
#include "Log.h"

namespace dbx { std::function<void()> g_onLaunched; }

namespace winrt::DeskboxesShellHost::implementation
{
    App::App()
    {
        UnhandledException([](auto&&, Microsoft::UI::Xaml::UnhandledExceptionEventArgs const& e) {
            dbx::Log(L"XAML unhandled exception 0x%08X: %s", (uint32_t)e.Exception(), e.Message().c_str());
        });
        // DESKBOXES_THEME=light|dark: a test copy in the other theme (the core reads it too).
        wchar_t t[8]{};
        if (GetEnvironmentVariableW(L"DESKBOXES_THEME", t, 8))
        {
            if (!wcscmp(t, L"light")) RequestedTheme(Microsoft::UI::Xaml::ApplicationTheme::Light);
            else if (!wcscmp(t, L"dark")) RequestedTheme(Microsoft::UI::Xaml::ApplicationTheme::Dark);
        }
    }

    void App::OnLaunched(Microsoft::UI::Xaml::LaunchActivatedEventArgs const&)
    {
        // The host has no main window: Main.cpp decides what to show (a menu, the settings
        // window, or nothing while it waits on the pipe).
        if (dbx::g_onLaunched) dbx::g_onLaunched();
    }
}

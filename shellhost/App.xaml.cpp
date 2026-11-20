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
    }

    void App::OnLaunched(Microsoft::UI::Xaml::LaunchActivatedEventArgs const&)
    {
        // The host has no main window: Main.cpp decides what to show (a menu, the settings
        // window, or nothing while it waits on the pipe).
        if (dbx::g_onLaunched) dbx::g_onLaunched();
    }
}

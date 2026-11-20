#pragma once
#include "App.xaml.g.h"

namespace dbx { extern std::function<void()> g_onLaunched; }

namespace winrt::DeskboxesShellHost::implementation
{
    struct App : AppT<App>
    {
        App();
        void OnLaunched(Microsoft::UI::Xaml::LaunchActivatedEventArgs const&);
    };
}

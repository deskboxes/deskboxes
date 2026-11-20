#pragma once
#include "MenuModel.h"
#include <functional>

namespace dbx
{
    struct MenuViewEvents
    {
        std::function<void(double msVisible)> opened;          // flyout visible (ms since ShowMenuView)
        std::function<void(const std::wstring& cmd)> command;   // Deskboxes-handled entry chosen (rename, view.*, ...)
        std::function<void(const std::wstring& invoked)> done;  // closed; invoked entry id or empty when dismissed
    };

    // Shows a built menu as a WinUI CommandBarFlyout (command row + list), the same control
    // family Explorer uses on 26H2. Must be called on the XAML UI thread.
    void ShowMenuView(std::shared_ptr<BuiltMenu> menu, MenuViewEvents events);
    // Close the open menu (as a click elsewhere would); its `done` follows.
    void HideMenuView();
}

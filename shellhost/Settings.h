#pragma once
#include <functional>
#include <winrt/Windows.Data.Json.h>

namespace dbx
{
    // The settings window (WinUI, Mica backdrop). `data` comes from the core: strings, colors,
    // app settings, the default box style and the boxes. Changes go back through `send` as
    // { "scope": "app" | "default" | "box", "box": id, "key": ..., "value": ... }.
    // open = true shows (or raises) the window; false only refreshes it if it is open.
    void ShowSettings(winrt::Windows::Data::Json::JsonObject const& data, bool open,
                      std::function<void(winrt::Windows::Data::Json::JsonObject)> send);

    // A small color picker window (a tab's or a box's own color). `data`: title, current
    // ("#RRGGBB" or ""), ok/cancel texts, target (scope/box/key). Each change is sent at once
    // as { scope: "pick", box, key, value, final: false } (shown live on the desktop); OK sends
    // final = true, Cancel / closing sends the starting color back with final = true and
    // cancel = true.
    void ShowColorPicker(winrt::Windows::Data::Json::JsonObject const& data,
                         std::function<void(winrt::Windows::Data::Json::JsonObject)> send);

    // A dialog as Windows' ContentDialog looks (a small Mica window without a title bar,
    // centered on the pointer's monitor, fading and scaling in): `data` = { dialog (id), kind:
    // "text" | "confirm", title, body, value, placeholder, ok, cancel }. The answer goes back as
    // { scope: "dialog", dialog, ok, value } (Esc, Cancel or closing: ok = false).
    void ShowDialog(winrt::Windows::Data::Json::JsonObject const& data,
                    std::function<void(winrt::Windows::Data::Json::JsonObject)> send);
    // Settings, a color picker or a dialog is open (the host is not recycled meanwhile).
    bool HostWindowsOpen();
}

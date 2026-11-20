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
}

// The settings window: WinUI 3 with a real Mica backdrop, laid out like Windows Settings (pages
// in a navigation pane; cards with a label on the left and the control on the right). Built from a XAML string so theme
// resources follow light/dark; events are wired by element name. All values live in the core;
// every change is sent back at once and applied live on the desktop.

#include "pch.h"
#include "Settings.h"
#include "Log.h"
#include <algorithm>
#include <cmath>

namespace mux = winrt::Microsoft::UI::Xaml;
namespace muxc = winrt::Microsoft::UI::Xaml::Controls;
using namespace winrt::Windows::Data::Json;

namespace dbx
{
    namespace
    {
        struct State
        {
            mux::Window window{ nullptr };
            HWND hwnd = nullptr;
            JsonObject data{ nullptr };
            std::function<void(JsonObject)> send;
            bool wiring = false; // building the content: control events are not user changes
            std::wstring page = L"home";    // shown page (navigation tag)
            std::wstring builtPage;          // page of the content now in the window
            std::set<long long> expanded;    // box groups open on the Boxes page
            long long focusBox = 0;          // box to bring into view on the next build
            bool lookOpen = false;           // the Look entry is open in the navigation
        };
        State g;

        std::wstring Esc(const std::wstring& s)
        {
            std::wstring o;
            for (wchar_t c : s)
            {
                switch (c)
                {
                case L'&': o += L"&amp;"; break;
                case L'<': o += L"&lt;"; break;
                case L'>': o += L"&gt;"; break;
                case L'"': o += L"&quot;"; break;
                default: o += c;
                }
            }
            return o;
        }

        std::wstring Str(const wchar_t* key)
        {
            auto s = g.data.GetNamedObject(L"strings", JsonObject());
            return Esc(std::wstring(s.GetNamedString(key, key).c_str()));
        }

        std::wstring Num(double v)
        {
            wchar_t b[32]; swprintf_s(b, L"%g", v); return b;
        }

        // One settings card: optional glyph, title and description on the left, control right.
        std::wstring Card(const std::wstring& glyph, const std::wstring& title, const std::wstring& desc, const std::wstring& control)
        {
            std::wstring x = LR"(<Border Background="{ThemeResource CardBackgroundFillColorDefaultBrush}" BorderBrush="{ThemeResource CardStrokeColorDefaultBrush}" BorderThickness="1" CornerRadius="{StaticResource ControlCornerRadius}" Padding="16,10" MinHeight="64">
  <Grid ColumnSpacing="16"><Grid.ColumnDefinitions><ColumnDefinition Width="Auto"/><ColumnDefinition Width="*"/><ColumnDefinition Width="Auto"/></Grid.ColumnDefinitions>)";
            if (!glyph.empty()) x += L"<FontIcon Glyph=\"" + glyph + L"\" FontSize=\"20\" VerticalAlignment=\"Center\"/>";
            x += L"<StackPanel Grid.Column=\"1\" VerticalAlignment=\"Center\"><TextBlock Text=\"" + title + L"\"/>";
            if (!desc.empty())
                x += L"<TextBlock Text=\"" + desc + L"\" Style=\"{StaticResource CaptionTextBlockStyle}\" Foreground=\"{ThemeResource TextFillColorSecondaryBrush}\" TextWrapping=\"Wrap\"/>";
            x += L"</StackPanel><Grid Grid.Column=\"2\" VerticalAlignment=\"Center\">" + control + L"</Grid></Grid></Border>";
            return x;
        }

        std::wstring Section(const std::wstring& title)
        {
            return L"<TextBlock Text=\"" + title + L"\" Style=\"{StaticResource BodyStrongTextBlockStyle}\" Margin=\"1,24,0,6\"/>";
        }

        // Color picker: Automatic + the presets, each with its swatch. Tag = "" or "#RRGGBB".
        std::wstring ColorBox(const std::wstring& name, const std::wstring& current)
        {
            std::wstring x = L"<ComboBox x:Name=\"" + name + L"\" MinWidth=\"220\">";
            x += L"<ComboBoxItem Tag=\"\"" + std::wstring(current.empty() ? L" IsSelected=\"True\"" : L"") + L"><StackPanel Orientation=\"Horizontal\" Spacing=\"10\"><FontIcon Glyph=\"&#xE790;\" FontSize=\"14\"/><TextBlock Text=\"" + Str(L"colorAuto") + L"\"/></StackPanel></ComboBoxItem>";
            for (auto const& c : g.data.GetNamedArray(L"colors", JsonArray()))
            {
                auto o = c.GetObject();
                std::wstring rgb = o.GetNamedString(L"rgb").c_str();
                x += L"<ComboBoxItem Tag=\"" + rgb + L"\"" + (_wcsicmp(rgb.c_str(), current.c_str()) == 0 ? L" IsSelected=\"True\"" : L"") +
                     L"><StackPanel Orientation=\"Horizontal\" Spacing=\"10\"><Ellipse Width=\"14\" Height=\"14\" Fill=\"" + rgb +
                     L"\" Stroke=\"{ThemeResource ControlStrongStrokeColorDefaultBrush}\" StrokeThickness=\"1\"/><TextBlock Text=\"" + Esc(std::wstring(o.GetNamedString(L"name").c_str())) + L"\"/></StackPanel></ComboBoxItem>";
            }
            return x + L"</ComboBox>";
        }

        std::wstring SliderX(const std::wstring& name, double value, double max)
        {
            return L"<Slider x:Name=\"" + name + L"\" Minimum=\"0\" Maximum=\"" + Num(max) + L"\" StepFrequency=\"5\" TickFrequency=\"5\" SnapsTo=\"StepValues\" Width=\"220\" Value=\"" + Num(value) + L"\"/>";
        }

        // The three style cards (color, transparency, blur) for the default style or one box.
        std::wstring StyleCards(const std::wstring& prefix, JsonObject const& st)
        {
            std::wstring color = st.GetNamedString(L"color", L"").c_str();
            double transp = std::round((1.0 - st.GetNamedNumber(L"opacity", 0.6)) * 100.0);
            double blur = st.GetNamedNumber(L"blur", 60);
            return Card(L"&#xE790;", Str(L"color"), L"", ColorBox(prefix + L"Color", color)) +
                   Card(L"&#xE81E;", Str(L"transparency"), L"", SliderX(prefix + L"Opacity", transp, 100)) +
                   Card(L"&#xE8B9;", Str(L"blur"), L"", SliderX(prefix + L"Blur", blur, 80));
        }

        std::wstring Sel(bool on) { return on ? L" IsSelected=\"True\"" : L""; }

        std::wstring Opt(const std::wstring& tag, const std::wstring& text, bool sel)
        {
            return L"<ComboBoxItem Tag=\"" + tag + L"\"" + Sel(sel) + L" Content=\"" + text + L"\"/>";
        }

        std::wstring Label(const std::wstring& text)
        {
            return L"<TextBlock Text=\"" + text + L"\" VerticalAlignment=\"Center\"/>";
        }

        // A switch as Windows Settings shows it: "On"/"Off" on its left, the switch at the card's
        // right edge (a plain ToggleSwitch is 154 wide with its text on the right, so the switch
        // sits left of the other controls).
        std::wstring Toggle(const std::wstring& name, bool on)
        {
            return L"<StackPanel Orientation=\"Horizontal\" Spacing=\"12\" HorizontalAlignment=\"Right\">"
                   L"<TextBlock x:Name=\"" + name + L"State\" Text=\"" + Str(on ? L"toggleOn" : L"toggleOff") + L"\" VerticalAlignment=\"Center\"/>"
                   L"<ToggleSwitch x:Name=\"" + name + L"\" OnContent=\"\" OffContent=\"\" MinWidth=\"0\" Width=\"40\" Margin=\"0\"" +
                   (on ? L" IsOn=\"True\"" : L"") + L"/></StackPanel>";
        }

        std::wstring Note(const std::wstring& text)
        {
            return L"<TextBlock Text=\"" + text + L"\" Foreground=\"{ThemeResource TextFillColorSecondaryBrush}\" TextWrapping=\"Wrap\" Margin=\"1,0,0,8\"/>";
        }

        std::wstring IconButton(const std::wstring& name, const wchar_t* glyph, const std::wstring& tip, bool enabled)
        {
            return L"<Button x:Name=\"" + name + L"\" Background=\"Transparent\" BorderThickness=\"0\" Padding=\"9\" ToolTipService.ToolTip=\"" + tip + L"\"" +
                   (enabled ? L"" : L" IsEnabled=\"False\"") + L"><FontIcon Glyph=\"" + glyph + L"\" FontSize=\"14\"/></Button>";
        }

        // Tabs icons can be sorted into (Tag = tab id).
        std::wstring TargetBox(const std::wstring& name, double current, const std::wstring& none)
        {
            std::wstring x = L"<ComboBox x:Name=\"" + name + L"\" MinWidth=\"240\">";
            if (!none.empty()) x += Opt(L"0", none, current == 0);
            for (auto const& t : g.data.GetNamedArray(L"targets", JsonArray()))
            {
                auto o = t.GetObject();
                double id = o.GetNamedNumber(L"id");
                x += Opt(std::to_wstring((long long)id), Esc(std::wstring(o.GetNamedString(L"name").c_str())), id == current);
            }
            return x + L"</ComboBox>";
        }

        // One rule: [x] If <kind> <condition>   / put into <tab>          [up][down][delete]
        std::wstring RuleCard(JsonObject const& r, uint32_t idx, uint32_t n)
        {
            std::wstring p = L"R" + std::to_wstring((long long)r.GetNamedNumber(L"id"));
            std::wstring kind = r.GetNamedString(L"kind", L"type").c_str();
            std::wstring op = r.GetNamedString(L"op", L"").c_str();
            std::wstring val = Esc(std::wstring(r.GetNamedString(L"value", L"").c_str()));
            std::wstring x = LR"(<Border Background="{ThemeResource CardBackgroundFillColorDefaultBrush}" BorderBrush="{ThemeResource CardStrokeColorDefaultBrush}" BorderThickness="1" CornerRadius="{StaticResource ControlCornerRadius}" Padding="14,10">
  <Grid ColumnSpacing="12"><Grid.ColumnDefinitions><ColumnDefinition Width="Auto"/><ColumnDefinition Width="*"/><ColumnDefinition Width="Auto"/></Grid.ColumnDefinitions>)";
            x += L"<CheckBox x:Name=\"" + p + L"On\" MinWidth=\"0\" VerticalAlignment=\"Center\"" + (r.GetNamedBoolean(L"enabled", true) ? L" IsChecked=\"True\"" : L"") + L"/>";
            x += L"<StackPanel Grid.Column=\"1\" Spacing=\"8\"><StackPanel Orientation=\"Horizontal\" Spacing=\"8\">" + Label(Str(L"ruleIf"));
            x += L"<ComboBox x:Name=\"" + p + L"Kind\" MinWidth=\"130\">" + Opt(L"type", Str(L"kindType"), kind == L"type") + Opt(L"name", Str(L"kindName"), kind == L"name") +
                 Opt(L"ext", Str(L"kindExt"), kind == L"ext") + Opt(L"age", Str(L"kindAge"), kind == L"age") + Opt(L"size", Str(L"kindSize"), kind == L"size") + L"</ComboBox>";
            if (kind == L"name")
            {
                x += L"<ComboBox x:Name=\"" + p + L"Op\" MinWidth=\"130\">" + Opt(L"contains", Str(L"opContains"), op == L"contains") + Opt(L"starts", Str(L"opStarts"), op == L"starts") +
                     Opt(L"ends", Str(L"opEnds"), op == L"ends") + Opt(L"is", Str(L"opIs"), op == L"is") + L"</ComboBox>";
                x += L"<TextBox x:Name=\"" + p + L"Val\" Width=\"160\" PlaceholderText=\"" + Str(L"textHint") + L"\" Text=\"" + val + L"\"/>";
            }
            else if (kind == L"ext")
                x += L"<TextBox x:Name=\"" + p + L"Val\" Width=\"200\" PlaceholderText=\"" + Str(L"extHint") + L"\" Text=\"" + val + L"\"/>";
            else if (kind == L"age")
            {
                x += L"<ComboBox x:Name=\"" + p + L"Op\" MinWidth=\"120\">" + Opt(L"newer", Str(L"opNewer"), op != L"older") + Opt(L"older", Str(L"opOlder"), op == L"older") + L"</ComboBox>";
                x += L"<TextBox x:Name=\"" + p + L"Val\" Width=\"64\" Text=\"" + val + L"\"/>" + Label(Str(op == L"older" ? L"days" : L"daysNewer"));
            }
            else if (kind == L"size")
            {
                x += L"<ComboBox x:Name=\"" + p + L"Op\" MinWidth=\"120\">" + Opt(L"larger", Str(L"opLarger"), op != L"smaller") + Opt(L"smaller", Str(L"opSmaller"), op == L"smaller") + L"</ComboBox>";
                x += L"<TextBox x:Name=\"" + p + L"Val\" Width=\"72\" Text=\"" + val + L"\"/>" + Label(Str(L"mb"));
            }
            else
            {
                x += L"<ComboBox x:Name=\"" + p + L"Op\" MinWidth=\"240\">";
                for (auto const& c : g.data.GetNamedArray(L"categories", JsonArray()))
                {
                    auto o = c.GetObject();
                    std::wstring id = o.GetNamedString(L"id").c_str();
                    x += Opt(id, Esc(std::wstring(o.GetNamedString(L"name").c_str())), id == op);
                }
                x += L"</ComboBox>";
            }
            x += L"</StackPanel><StackPanel Orientation=\"Horizontal\" Spacing=\"8\">" + Label(Str(L"ruleThen")) + TargetBox(p + L"Target", r.GetNamedNumber(L"target", 0), L"") + L"</StackPanel></StackPanel>";
            x += L"<StackPanel Grid.Column=\"2\" Orientation=\"Horizontal\" VerticalAlignment=\"Center\">" + IconButton(p + L"Up", L"&#xE70E;", Str(L"moveUp"), idx > 0) +
                 IconButton(p + L"Down", L"&#xE70D;", Str(L"moveDown"), idx + 1 < n) + IconButton(p + L"Del", L"&#xE74D;", Str(L"deleteRule"), true) + L"</StackPanel>";
            return x + L"</Grid></Border>";
        }

        std::wstring PageGeneral()
        {
            auto app = g.data.GetNamedObject(L"app", JsonObject());
            std::wstring lang = app.GetNamedString(L"language", L"").c_str();
            std::wstring x;
            x += Card(L"&#xF2B7;", Str(L"language"), Str(L"languageDesc"),
                      L"<ComboBox x:Name=\"Language\" MinWidth=\"220\">" + Opt(L"", Str(L"langAuto"), lang.empty()) + Opt(L"en", L"English", lang == L"en") + Opt(L"pl", L"Polski", lang == L"pl") + L"</ComboBox>");
            x += Card(L"&#xE823;", Str(L"autostart"), Str(L"autostartDesc"), Toggle(L"Autostart", app.GetNamedBoolean(L"autostart", false)));
            // Updates: the switch, what the updater is doing, Check now / Install now.
            {
                auto up = g.data.GetNamedObject(L"update", JsonObject());
                auto st = up.GetNamedObject(L"status", JsonObject());
                std::wstring state = st.GetNamedString(L"state", L"").c_str(), latest = st.GetNamedString(L"latest", L"").c_str();
                auto fill = [](std::wstring s, const wchar_t* k, const std::wstring& v) { auto i = s.find(k); if (i != std::wstring::npos) s.replace(i, wcslen(k), v); return s; };
                std::wstring text = state == L"checking" ? Str(L"updChecking")
                                  : state == L"downloading" ? fill(Str(L"updDownloading"), L"{v}", Esc(latest))
                                  : state == L"ready" ? fill(Str(L"updReady"), L"{v}", Esc(latest))
                                  : state == L"error" ? Str(L"updError")
                                  : state == L"dev" ? Str(L"updDev")
                                  : state == L"uptodate" ? fill(Str(L"updUpToDate"), L"{v}", Esc(std::wstring(up.GetNamedString(L"version", L"").c_str())))
                                  : Str(L"updNever");
                std::wstring when;
                if (double t = st.GetNamedNumber(L"checked", 0); t > 0 && state != L"dev")
                {
                    time_t tt = (time_t)t;
                    tm lt{};
                    localtime_s(&lt, &tt);
                    wchar_t b[64];
                    wcsftime(b, 64, L"%Y-%m-%d %H:%M", &lt);
                    when = fill(Str(L"updChecked"), L"{t}", b);
                }
                std::wstring more = state == L"ready" ? Str(L"updReadyDesc")
                                  : state == L"error" ? [&] {
                                        std::wstring k = st.GetNamedString(L"error", L"").c_str();
                                        return Str(k == L"net" ? L"updErrNet" : k == L"none" ? L"updErrNone" : k == L"server" ? L"updErrServer" : k == L"file" ? L"updErrFile" : L"updErrOther");
                                    }()
                                  : state == L"dev" ? Str(L"updDevDesc")
                                  : L"";
                if (!more.empty() && !when.empty()) more += L" ";
                when = more + when;
                bool dev = state == L"dev", busy = state == L"checking" || state == L"downloading";
                x += Section(Str(L"updates"));
                x += Card(L"&#xE895;", Str(L"updAuto"), Str(L"updAutoDesc"), Toggle(L"UpdateAuto", up.GetNamedBoolean(L"auto", true)));
                std::wstring btns = L"<StackPanel Orientation=\"Horizontal\" Spacing=\"8\">";
                if (state == L"ready") btns += L"<Button x:Name=\"UpdateInstall\" Style=\"{StaticResource AccentButtonStyle}\" Content=\"" + Str(L"updInstall") + L"\" MinWidth=\"120\"/>";
                btns += L"<Button x:Name=\"UpdateNow\" Content=\"" + Str(L"updNow") + L"\" MinWidth=\"120\"" + (dev || busy ? L" IsEnabled=\"False\"" : L"") + L"/></StackPanel>";
                x += Card(state == L"error" ? L"&#xE783;" : state == L"ready" ? L"&#xE896;" : L"&#xE946;", text, when, btns);
            }
            x += Section(Str(L"about"));
            x += Card(L"&#xE7E8;", Str(L"disable"), Str(L"disableDesc"), L"<Button x:Name=\"Disable\" Content=\"" + Str(L"disableButton") + L"\" MinWidth=\"120\"/>");
            x += L"<TextBlock Text=\"Deskboxes " + Esc(std::wstring(app.GetNamedString(L"version", L"").c_str())) + L"\" Style=\"{StaticResource CaptionTextBlockStyle}\" Foreground=\"{ThemeResource TextFillColorSecondaryBrush}\" Margin=\"1,12,0,0\"/>";
            return x;
        }

        std::wstring PageBoxes()
        {
            std::wstring x;
            auto boxes = g.data.GetNamedArray(L"boxes", JsonArray());
            if (boxes.Size() == 0) x += Note(Str(L"noBoxes"));
            for (auto const& b : boxes)
            {
                auto o = b.GetObject();
                long long id = (long long)o.GetNamedNumber(L"id");
                std::wstring p = L"Box" + std::to_wstring(id);
                std::wstring title = Esc(std::wstring(o.GetNamedString(L"title", L"").c_str()));
                x += L"<Expander x:Name=\"" + p + L"Exp\" HorizontalAlignment=\"Stretch\" HorizontalContentAlignment=\"Stretch\" Margin=\"0,0,0,2\"" + (g.expanded.count(id) ? L" IsExpanded=\"True\"" : L"") + L">";
                x += L"<Expander.Header><Grid ColumnSpacing=\"16\" Padding=\"0,10\"><Grid.ColumnDefinitions><ColumnDefinition Width=\"Auto\"/><ColumnDefinition Width=\"*\"/></Grid.ColumnDefinitions>"
                     L"<FontIcon Glyph=\"&#xE8A9;\" FontSize=\"20\"/><TextBlock Grid.Column=\"1\" Text=\"" + title + L"\" VerticalAlignment=\"Center\"/></Grid></Expander.Header>";
                x += L"<StackPanel Spacing=\"4\">";
                x += Card(L"&#xE8AC;", Str(L"boxTitle"), L"", L"<TextBox x:Name=\"" + p + L"Title\" Text=\"" + title + L"\" MinWidth=\"220\"/>");
                x += StyleCards(p, o.GetNamedObject(L"style", JsonObject()));
                x += Card(L"&#xE74D;", Str(L"remove"), Str(L"removeDesc"), L"<Button x:Name=\"" + p + L"Remove\" Content=\"" + Str(L"removeButton") + L"\" MinWidth=\"120\"/>");
                x += L"</StackPanel></Expander>";
            }
            return x;
        }

        std::wstring PageColors()
        {
            std::wstring x = StyleCards(L"Def", g.data.GetNamedObject(L"default", JsonObject()));
            x += Card(L"&#xE8B3;", Str(L"applyAll"), Str(L"applyAllDesc"), L"<Button x:Name=\"ApplyAll\" Content=\"" + Str(L"apply") + L"\" MinWidth=\"120\"/>");
            return x;
        }

        std::wstring PageRules()
        {
            auto so = g.data.GetNamedObject(L"sorting", JsonObject());
            bool anyTarget = g.data.GetNamedArray(L"targets", JsonArray()).Size() > 0;
            std::wstring x;
            if (!anyTarget) x += Note(Str(L"noBoxesRules"));
            x += Card(L"&#xE8CB;", Str(L"sortNow"), Str(L"sortNowDesc"), L"<Button x:Name=\"SortNow\" Style=\"{StaticResource AccentButtonStyle}\" Content=\"" + Str(L"sortNowButton") + L"\" MinWidth=\"120\"/>");
            x += Card(L"&#xE72E;", Str(L"keepSorted"), Str(L"keepSortedDesc"), Toggle(L"KeepSorted", so.GetNamedBoolean(L"keep", false)));
            x += Card(L"&#xE8F4;", Str(L"newTo"), Str(L"newToDesc"), TargetBox(L"NewTo", so.GetNamedNumber(L"newTo", 0), Str(L"newToNone")));
            x += Section(Str(L"rulesList"));
            auto rules = so.GetNamedArray(L"rules", JsonArray());
            if (rules.Size() == 0) x += Note(Str(L"noRules"));
            for (uint32_t i = 0; i < rules.Size(); i++) x += RuleCard(rules.GetObjectAt(i), i, rules.Size());
            x += L"<Button x:Name=\"AddRule\" Margin=\"0,8,0,0\"" + std::wstring(anyTarget ? L"" : L" IsEnabled=\"False\"") +
                 L"><StackPanel Orientation=\"Horizontal\" Spacing=\"8\"><FontIcon Glyph=\"&#xE710;\" FontSize=\"14\"/><TextBlock Text=\"" + Str(L"addRule") + L"\"/></StackPanel></Button>";
            return x;
        }

        std::wstring PageTabs()
        {
            auto tb = g.data.GetNamedObject(L"tabs", JsonObject());
            std::wstring ts = tb.GetNamedString(L"style", L"pills").c_str();
            std::wstring x = Card(L"&#xE771;", Str(L"tabStyle"), Str(L"tabStyleDesc"),
                      L"<ComboBox x:Name=\"TabStyle\" MinWidth=\"220\">" + Opt(L"pills", Str(L"tabPills"), ts == L"pills") + Opt(L"underline", Str(L"tabUnderline"), ts == L"underline") +
                          Opt(L"segments", Str(L"tabSegments"), ts == L"segments") + L"</ComboBox>");
            x += Card(L"&#xE7C9;", Str(L"tabHover"), Str(L"tabHoverDesc"), Toggle(L"TabHover", tb.GetNamedBoolean(L"hoverSwitch", false)));
            x += Card(L"&#xE71B;", Str(L"tabCombine"), Str(L"tabCombineDesc"), Toggle(L"TabCombine", tb.GetNamedBoolean(L"combine", true)));
            return x;
        }

        // A hex alpha + RGB brush string: "#AARRGGBB".
        std::wstring Argb(double a, const wchar_t* rgb)
        {
            wchar_t b[16]; swprintf_s(b, L"#%02X%s", (int)std::lround(std::clamp(a, 0.0, 1.0) * 255.0), rgb); return b;
        }

        // A tiny picture of a preset: two boxes on a wallpaper, one with a single title (its
        // position) and one with two tabs (the tab style), each with the title bar and corners.
        std::wstring PresetPreview(JsonObject const& p, bool selected)
        {
            std::wstring id = p.GetNamedString(L"id").c_str();
            std::wstring bar = p.GetNamedString(L"titleBar").c_str();
            std::wstring align = p.GetNamedString(L"align").c_str();
            std::wstring tabs = p.GetNamedString(L"tabs").c_str();
            double band = p.GetNamedNumber(L"band"), size = p.GetNamedNumber(L"titleSize"), rad = p.GetNamedNumber(L"radius") * 0.7;
            double fs = size * 0.72, hh = std::round(size * 1.65);
            std::wstring r = Num(rad), hdrBg = bar == L"band" ? Argb(band, L"000000") : bar == L"solid" ? std::wstring(L"#80000000") : std::wstring(L"Transparent");
            std::wstring line = bar == L"line" ? L" BorderBrush=\"#40FFFFFF\" BorderThickness=\"0,0,0,1\"" : L"";
            auto text = [&](const std::wstring& t, bool bold, double op)
            {
                return L"<TextBlock Text=\"" + t + L"\" FontSize=\"" + Num(fs) + L"\"" + (bold ? L" FontWeight=\"SemiBold\"" : L"") + L" Foreground=\"White\" Opacity=\"" + Num(op) + L"\" VerticalAlignment=\"Center\"/>";
            };
            auto icons = L"<StackPanel Grid.Row=\"1\" Orientation=\"Horizontal\" Spacing=\"5\" Margin=\"7,7,0,0\"><Rectangle Width=\"9\" Height=\"9\" RadiusX=\"2\" RadiusY=\"2\" Fill=\"#B3FFFFFF\"/><Rectangle Width=\"9\" Height=\"9\" RadiusX=\"2\" RadiusY=\"2\" Fill=\"#80FFFFFF\"/><Rectangle Width=\"9\" Height=\"9\" RadiusX=\"2\" RadiusY=\"2\" Fill=\"#B3FFFFFF\"/></StackPanel>";
            auto box = [&](const std::wstring& col, const std::wstring& header)
            {
                return L"<Border Grid.Column=\"" + col + L"\" CornerRadius=\"" + r + L"\" Background=\"#B8202020\" BorderBrush=\"#30FFFFFF\" BorderThickness=\"1\"><Grid><Grid.RowDefinitions><RowDefinition Height=\"" + Num(hh) +
                       L"\"/><RowDefinition Height=\"*\"/></Grid.RowDefinitions><Border Background=\"" + hdrBg + L"\" CornerRadius=\"" + r + L"," + r + L",0,0\"" + line + L">" + header + L"</Border>" + icons + L"</Grid></Border>";
            };
            std::wstring single = L"<Grid Margin=\"6,0\">" + std::wstring(L"<TextBlock Text=\"") + Str(L"previewTab1") + L"\" FontSize=\"" + Num(fs) + L"\" FontWeight=\"SemiBold\" Foreground=\"White\" VerticalAlignment=\"Center\" HorizontalAlignment=\"" +
                                 (align == L"left" ? L"Left" : L"Center") + L"\"/></Grid>";
            std::wstring t1 = Str(L"previewTab1"), t2 = Str(L"previewTab2");
            std::wstring tabRow;
            if (tabs == L"underline")
                tabRow = L"<StackPanel Orientation=\"Horizontal\" Spacing=\"8\" Margin=\"6,0\" VerticalAlignment=\"Center\"><Grid>" + text(t1, true, 1) +
                         L"<Rectangle Height=\"2\" RadiusX=\"1\" RadiusY=\"1\" VerticalAlignment=\"Bottom\" Margin=\"0,0,0,-4\" Fill=\"{ThemeResource AccentFillColorDefaultBrush}\"/></Grid>" + text(t2, false, 0.62) + L"</StackPanel>";
            else if (tabs == L"segments")
                tabRow = L"<Border Background=\"#40000000\" CornerRadius=\"4\" Padding=\"2\" Margin=\"4,0\" HorizontalAlignment=\"Left\" VerticalAlignment=\"Center\"><StackPanel Orientation=\"Horizontal\" Spacing=\"2\"><Border Background=\"#2EFFFFFF\" CornerRadius=\"3\" Padding=\"5,0\">" +
                         text(t1, true, 1) + L"</Border><Border Padding=\"5,0\">" + text(t2, false, 0.62) + L"</Border></StackPanel></Border>";
            else
                tabRow = L"<StackPanel Orientation=\"Horizontal\" Spacing=\"2\" Margin=\"4,0\" VerticalAlignment=\"Center\"><Border Background=\"#33FFFFFF\" CornerRadius=\"4\" Padding=\"5,1\">" + text(t1, true, 1) +
                         L"</Border><Border Padding=\"5,1\">" + text(t2, false, 0.62) + L"</Border></StackPanel>";
            std::wstring x = L"<Button x:Name=\"Preset_" + id + L"\" Padding=\"6\" Background=\"Transparent\" BorderThickness=\"2\" CornerRadius=\"8\" HorizontalAlignment=\"Stretch\" HorizontalContentAlignment=\"Stretch\" BorderBrush=\"" +
                             (selected ? L"{ThemeResource AccentFillColorDefaultBrush}" : L"Transparent") + L"\">";
            x += L"<StackPanel Spacing=\"6\"><Grid Height=\"92\" CornerRadius=\"5\" Padding=\"10\" ColumnSpacing=\"8\"><Grid.Background><LinearGradientBrush StartPoint=\"0,0\" EndPoint=\"1,1\"><GradientStop Color=\"#FF2F4F7F\" Offset=\"0\"/><GradientStop Color=\"#FFB4643A\" Offset=\"1\"/></LinearGradientBrush></Grid.Background>"
                 L"<Grid.ColumnDefinitions><ColumnDefinition Width=\"*\"/><ColumnDefinition Width=\"1.25*\"/></Grid.ColumnDefinitions>";
            x += box(L"0", single) + box(L"1", tabRow);
            x += L"</Grid><TextBlock Text=\"" + Str((L"preset_" + id).c_str()) + L"\" HorizontalAlignment=\"Center\"/></StackPanel></Button>";
            return x;
        }

        // A size with presets: a slider for the exact value and a list of named sizes ("Custom"
        // when the slider is between them).
        std::wstring SizePair(const std::wstring& name, double value, std::vector<std::pair<std::wstring, double>> const& presets, double mn, double mx)
        {
            std::wstring items;
            bool any = false;
            for (auto const& p : presets)
            {
                bool s = std::fabs(p.second - value) < 0.01;
                any |= s;
                items += Opt(Num(p.second), p.first, s);
            }
            items += Opt(L"custom", Str(L"sizeCustom"), !any);
            return L"<StackPanel Orientation=\"Horizontal\" Spacing=\"12\"><Slider x:Name=\"" + name + L"Slider\" Minimum=\"" + Num(mn) + L"\" Maximum=\"" + Num(mx) +
                   L"\" StepFrequency=\"1\" SnapsTo=\"StepValues\" Width=\"160\" VerticalAlignment=\"Center\" Value=\"" + Num(value) + L"\"/><ComboBox x:Name=\"" + name + L"Preset\" MinWidth=\"190\">" + items + L"</ComboBox></StackPanel>";
        }

        std::wstring PageLook()
        {
            auto lk = g.data.GetNamedObject(L"look", JsonObject());
            std::wstring cur = lk.GetNamedString(L"preset", L"").c_str();
            std::wstring bar = lk.GetNamedString(L"titleBar", L"band").c_str();
            std::wstring align = lk.GetNamedString(L"align", L"center").c_str();
            auto presets = lk.GetNamedArray(L"presets", JsonArray());
            std::wstring x = Note(Str(L"presetsDesc"));
            uint32_t rows = (presets.Size() + 2) / 3;
            x += L"<Grid ColumnSpacing=\"8\" RowSpacing=\"8\"><Grid.ColumnDefinitions><ColumnDefinition Width=\"*\"/><ColumnDefinition Width=\"*\"/><ColumnDefinition Width=\"*\"/></Grid.ColumnDefinitions><Grid.RowDefinitions>";
            for (uint32_t i = 0; i < rows; i++) x += L"<RowDefinition Height=\"Auto\"/>";
            x += L"</Grid.RowDefinitions>";
            for (uint32_t i = 0; i < presets.Size(); i++)
            {
                auto p = presets.GetObjectAt(i);
                x += L"<Grid Grid.Row=\"" + std::to_wstring(i / 3) + L"\" Grid.Column=\"" + std::to_wstring(i % 3) + L"\">" + PresetPreview(p, cur == p.GetNamedString(L"id").c_str()) + L"</Grid>";
            }
            x += L"</Grid>";
            x += Section(Str(L"fineTune"));
            x += Card(L"&#xE737;", Str(L"titleBar"), Str(L"titleBarDesc"),
                      L"<ComboBox x:Name=\"LookBar\" MinWidth=\"220\">" + Opt(L"band", Str(L"barBand"), bar == L"band") + Opt(L"solid", Str(L"barSolid"), bar == L"solid") +
                          Opt(L"line", Str(L"barLine"), bar == L"line") + Opt(L"clear", Str(L"barClear"), bar == L"clear") + L"</ComboBox>");
            x += Card(L"&#xE706;", Str(L"bandStrength"), L"",
                      L"<Slider x:Name=\"LookBand\" Minimum=\"0\" Maximum=\"60\" StepFrequency=\"2\" SnapsTo=\"StepValues\" Width=\"220\" Value=\"" + Num(std::round(lk.GetNamedNumber(L"band", 0.22) * 100.0)) + L"\"" +
                          (bar == L"band" ? L"" : L" IsEnabled=\"False\"") + L"/>");
            x += Card(L"&#xE8E3;", Str(L"titleAlign"), Str(L"titleAlignDesc"),
                      L"<ComboBox x:Name=\"LookAlign\" MinWidth=\"220\">" + Opt(L"center", Str(L"alignCenter"), align == L"center") + Opt(L"left", Str(L"alignLeft"), align == L"left") + L"</ComboBox>");
            x += Card(L"&#xE8E9;", Str(L"titleSize"), Str(L"titleSizeDesc"),
                      SizePair(L"LookSize", lk.GetNamedNumber(L"titleSize", 13), { { Str(L"sizeSmall"), 12 }, { Str(L"sizeNormal"), 13 }, { Str(L"sizeLarge"), 15 }, { Str(L"sizeXLarge"), 17 } }, 10, 22));
            x += Card(L"&#xE71A;", Str(L"corners"), Str(L"cornersDesc"),
                      SizePair(L"LookRadius", lk.GetNamedNumber(L"radius", 8), { { Str(L"cornerSquare"), 0 }, { Str(L"cornerSlight"), 4 }, { Str(L"cornerNormal"), 8 }, { Str(L"cornerRound"), 14 } }, 0, 20));
            x += Card(L"&#xE7B3;", Str(L"titlesHover"), Str(L"titlesHoverDesc"), Toggle(L"TitlesHover", lk.GetNamedBoolean(L"titlesHover", false)));
            x += Card(L"&#xE8CB;", Str(L"scrollHover"), Str(L"scrollHoverDesc"), Toggle(L"ScrollHover", lk.GetNamedBoolean(L"scrollHover", false)));
            std::wstring font = lk.GetNamedString(L"titleFont", L"").c_str(), weight = lk.GetNamedString(L"titleWeight", L"semibold").c_str(), ol = lk.GetNamedString(L"outline", L"").c_str();
            std::wstring fonts = L"<ComboBox x:Name=\"LookFont\" MinWidth=\"220\">" + Opt(L"", Str(L"fontDefault"), font.empty());
            for (const wchar_t* f : { L"Segoe UI Variable Display", L"Bahnschrift", L"Georgia", L"Consolas", L"Segoe Print", L"Comic Sans MS" })
                fonts += L"<ComboBoxItem Tag=\"" + std::wstring(f) + L"\"" + Sel(font == f) + L"><TextBlock Text=\"" + f + L"\" FontFamily=\"" + f + L"\"/></ComboBoxItem>";
            fonts += L"</ComboBox>";
            x += Card(L"&#xE8D2;", Str(L"titleFont"), Str(L"titleFontDesc"), fonts);
            x += Card(L"&#xE8DD;", Str(L"titleWeight"), Str(L"titleWeightDesc"),
                      L"<ComboBox x:Name=\"LookWeight\" MinWidth=\"220\">" + Opt(L"light", Str(L"weightLight"), weight == L"light") + Opt(L"normal", Str(L"weightNormal"), weight == L"normal") +
                          Opt(L"semibold", Str(L"weightSemibold"), weight == L"semibold") + Opt(L"bold", Str(L"weightBold"), weight == L"bold") + L"</ComboBox>");
            x += Card(L"&#xE739;", Str(L"outline"), Str(L"outlineDesc"),
                      L"<ComboBox x:Name=\"LookOutline\" MinWidth=\"220\">" + Opt(L"", Str(L"outlineFaint"), ol.empty()) + Opt(L"none", Str(L"outlineNone"), ol == L"none") +
                          Opt(L"accent", Str(L"outlineAccent"), ol == L"accent") + Opt(L"box", Str(L"outlineBox"), ol == L"box") + Opt(L"white", Str(L"outlineWhite"), ol == L"white") +
                          Opt(L"black", Str(L"outlineBlack"), ol == L"black") + L"</ComboBox>");
            x += Card(L"&#xE7A8;", Str(L"outlineWidth"), L"",
                      L"<Slider x:Name=\"LookOutlineW\" Minimum=\"0.5\" Maximum=\"4\" StepFrequency=\"0.5\" SnapsTo=\"StepValues\" Width=\"220\" Value=\"" + Num(lk.GetNamedNumber(L"outlineW", 1.0)) + L"\"" +
                          (ol == L"none" ? L" IsEnabled=\"False\"" : L"") + L"/>");
            x += Card(L"&#xE712;", Str(L"titleButtons"), Str(L"titleButtonsDesc"), Toggle(L"TitleButtons", lk.GetNamedBoolean(L"titleButtons", false)));
            return x;
        }

        std::wstring PageTouches()
        {
            auto ex = g.data.GetNamedObject(L"extras", JsonObject());
            std::wstring x = Note(Str(L"touchesDesc"));
            x += Card(L"&#xE8CB;", Str(L"smoothScroll"), Str(L"smoothScrollDesc"), Toggle(L"ExSmooth", ex.GetNamedBoolean(L"smoothScroll", true)));
            x += Card(L"&#xE8B3;", Str(L"dragCount"), Str(L"dragCountDesc"), Toggle(L"ExCount", ex.GetNamedBoolean(L"dragCount", true)));
            x += Card(L"&#xE7C4;", Str(L"liftBox"), Str(L"liftBoxDesc"), Toggle(L"ExLift", ex.GetNamedBoolean(L"liftBox", true)));
            x += Card(L"&#xE71E;", Str(L"ctrlWheel"), Str(L"ctrlWheelDesc"), Toggle(L"ExWheel", ex.GetNamedBoolean(L"ctrlWheel", true)));
            return x;
        }

        std::wstring PageRollup()
        {
            auto ru = g.data.GetNamedObject(L"rollup", JsonObject());
            std::wstring rv = ru.GetNamedString(L"reveal", L"hover").c_str();
            std::wstring x = Card(L"&#xE70E;", Str(L"rollupEdges"), Str(L"rollupEdgesDesc"), Toggle(L"RollEdges", ru.GetNamedBoolean(L"edges", true)));
            x += Card(L"&#xE8B0;", Str(L"reveal"), Str(L"revealDesc"),
                      L"<ComboBox x:Name=\"RollReveal\" MinWidth=\"220\">" + Opt(L"hover", Str(L"revealHover"), rv == L"hover") + Opt(L"delay", Str(L"revealDelay"), rv == L"delay") +
                          Opt(L"click", Str(L"revealClick"), rv == L"click") + L"</ComboBox>");
            x += Card(L"&#xE8B5;", Str(L"dblclick"), Str(L"dblclickDesc"), Toggle(L"RollDbl", ru.GetNamedBoolean(L"doubleClick", true)));
            return x;
        }

        // Tint choices: for all boxes ("" = none) or one box ("" = as all boxes, "off" = none).
        std::wstring TintBox(const std::wstring& name, const std::wstring& cur, bool perBox)
        {
            std::wstring x = L"<ComboBox x:Name=\"" + name + L"\" MinWidth=\"210\">";
            if (perBox) x += Opt(L"", Str(L"tintAll"), cur.empty());
            x += Opt(perBox ? L"off" : L"", Str(L"tintNone"), perBox ? cur == L"off" : cur.empty());
            x += L"<ComboBoxItem Tag=\"box\"" + Sel(cur == L"box") + L"><StackPanel Orientation=\"Horizontal\" Spacing=\"10\"><FontIcon Glyph=\"&#xE8A9;\" FontSize=\"14\"/><TextBlock Text=\"" + Str(L"tintBox") + L"\"/></StackPanel></ComboBoxItem>";
            for (auto const& c : g.data.GetNamedArray(L"colors", JsonArray()))
            {
                auto o = c.GetObject();
                std::wstring rgb = o.GetNamedString(L"rgb").c_str();
                x += L"<ComboBoxItem Tag=\"" + rgb + L"\"" + Sel(_wcsicmp(rgb.c_str(), cur.c_str()) == 0) +
                     L"><StackPanel Orientation=\"Horizontal\" Spacing=\"10\"><Ellipse Width=\"14\" Height=\"14\" Fill=\"" + rgb +
                     L"\" Stroke=\"{ThemeResource ControlStrongStrokeColorDefaultBrush}\" StrokeThickness=\"1\"/><TextBlock Text=\"" + Esc(std::wstring(o.GetNamedString(L"name").c_str())) + L"\"/></StackPanel></ComboBoxItem>";
            }
            return x + L"</ComboBox>";
        }

        std::wstring PageIcons()
        {
            auto ic = g.data.GetNamedObject(L"icons", JsonObject());
            std::wstring x = Card(L"&#xE790;", Str(L"tint"), Str(L"tintDesc"), TintBox(L"IconTint", ic.GetNamedString(L"tint", L"").c_str(), false));
            x += Card(L"&#xE706;", Str(L"tintStrength"), L"",
                      L"<Slider x:Name=\"IconTintS\" Minimum=\"10\" Maximum=\"100\" StepFrequency=\"5\" SnapsTo=\"StepValues\" Width=\"220\" Value=\"" + Num(std::round(ic.GetNamedNumber(L"tintStrength", 0.7) * 100.0)) + L"\"/>");
            x += Card(L"&#xE81E;", Str(L"blend"), Str(L"blendDesc"), Toggle(L"IconBlend", ic.GetNamedBoolean(L"blend", false)));
            x += Card(L"&#xE8B9;", Str(L"blendStrength"), L"",
                      L"<Slider x:Name=\"IconBlendS\" Minimum=\"10\" Maximum=\"100\" StepFrequency=\"5\" SnapsTo=\"StepValues\" Width=\"220\" Value=\"" + Num(std::round(ic.GetNamedNumber(L"blendStrength", 0.8) * 100.0)) + L"\"/>");
            x += Card(L"&#xE7F4;", Str(L"iconsDesktop"), Str(L"iconsDesktopDesc"), Toggle(L"IconDesk", ic.GetNamedBoolean(L"desktop", false)));
            x += Card(L"&#xE8A9;", Str(L"blendBoxes"), Str(L"blendBoxesDesc"), Toggle(L"IconBlendBoxes", ic.GetNamedBoolean(L"blendBoxes", true)));
            x += Card(L"&#xE7C9;", Str(L"fullHover"), Str(L"fullHoverDesc"), Toggle(L"IconFullHover", ic.GetNamedBoolean(L"fullHover", true)));
            x += Card(L"&#xE7F8;", Str(L"fullActive"), Str(L"fullActiveDesc"), Toggle(L"IconFullActive", ic.GetNamedBoolean(L"fullActive", true)));
            x += Section(Str(L"eachBox"));
            auto boxes = g.data.GetNamedArray(L"boxes", JsonArray());
            if (boxes.Size() == 0) x += Note(Str(L"noBoxes"));
            for (auto const& b : boxes)
            {
                auto o = b.GetObject();
                std::wstring id = std::to_wstring((long long)o.GetNamedNumber(L"id"));
                std::wstring bl = o.GetNamedString(L"blend", L"").c_str();
                std::wstring ctl = L"<StackPanel Orientation=\"Horizontal\" Spacing=\"8\">" + TintBox(L"Tint" + id, o.GetNamedString(L"tint", L"").c_str(), true) +
                                   L"<ComboBox x:Name=\"Blend" + id + L"\" MinWidth=\"210\">" + Opt(L"", Str(L"blendAll"), bl.empty()) + Opt(L"on", Str(L"blendOn"), bl == L"on") + Opt(L"off", Str(L"blendOff"), bl == L"off") + L"</ComboBox></StackPanel>";
                x += Card(L"&#xE8A9;", Esc(std::wstring(o.GetNamedString(L"title", L"").c_str())), L"", ctl);
            }
            return x;
        }

        std::wstring PageAz()
        {
            auto az = g.data.GetNamedObject(L"az", JsonObject());
            std::wstring x = Card(L"&#xE8C4;", Str(L"azBar"), Str(L"azBarDesc"), Toggle(L"AzBar", az.GetNamedBoolean(L"letterBar", true)));
            x += Card(L"&#xE8EC;", Str(L"azTags"), Str(L"azTagsDesc"), Toggle(L"AzTags", az.GetNamedBoolean(L"letterTags", true)));
            x += Section(Str(L"azBoxes"));
            auto boxes = g.data.GetNamedArray(L"boxes", JsonArray());
            if (boxes.Size() == 0) x += Note(Str(L"noBoxes"));
            for (auto const& b : boxes)
            {
                auto o = b.GetObject();
                std::wstring id = std::to_wstring((long long)o.GetNamedNumber(L"id"));
                x += Card(L"&#xE8A9;", Esc(std::wstring(o.GetNamedString(L"title", L"").c_str())), L"", Toggle(L"Az" + id, o.GetNamedBoolean(L"az", false)));
            }
            return x;
        }

        std::wstring PageHide()
        {
            auto h = g.data.GetNamedObject(L"hide", JsonObject());
            std::wstring x = Card(L"&#xE91B;", Str(L"hideDbl"), Str(L"hideDblDesc"), Toggle(L"HideDbl", h.GetNamedBoolean(L"doubleClick", false)));
            x += Section(Str(L"hideBoxes"));
            auto boxes = g.data.GetNamedArray(L"boxes", JsonArray());
            if (boxes.Size() == 0) x += Note(Str(L"noBoxes"));
            for (auto const& b : boxes)
            {
                auto o = b.GetObject();
                std::wstring id = std::to_wstring((long long)o.GetNamedNumber(L"id"));
                x += Card(L"&#xE8A9;", Esc(std::wstring(o.GetNamedString(L"title", L"").c_str())), L"", Toggle(L"Stay" + id, o.GetNamedBoolean(L"stayVisible", false)));
            }
            return x;
        }

        std::wstring PageFolder()
        {
            auto fb = g.data.GetNamedObject(L"folderBoxes", JsonObject());
            std::wstring x;
            int i = 0;
            for (auto const& e : fb.GetNamedArray(L"known", JsonArray()))
            {
                auto o = e.GetObject();
                x += Card(L"&#xE8B7;", Esc(std::wstring(o.GetNamedString(L"name").c_str())), Esc(std::wstring(o.GetNamedString(L"path").c_str())),
                          L"<Button x:Name=\"FbNew" + std::to_wstring(i++) + L"\" Content=\"" + Str(L"fbMake") + L"\" MinWidth=\"160\"/>");
            }
            x += Section(Str(L"fbList"));
            auto list = fb.GetNamedArray(L"list", JsonArray());
            if (list.Size() == 0) x += Note(Str(L"fbNone"));
            for (auto const& e : list)
            {
                auto o = e.GetObject();
                x += Card(L"&#xE8A9;", Esc(std::wstring(o.GetNamedString(L"title").c_str())), Esc(std::wstring(o.GetNamedString(L"path").c_str())), L"");
            }
            return x;
        }

        std::wstring PageAbove()
        {
            auto a = g.data.GetNamedObject(L"above", JsonObject());
            std::wstring hk = a.GetNamedString(L"hotkey", L"win+shift+d").c_str(), corner = a.GetNamedString(L"corner", L"").c_str();
            std::wstring keys = L"<ComboBox x:Name=\"AboveKey\" MinWidth=\"220\">" + Opt(L"", Str(L"aboveNone"), hk.empty()) +
                                Opt(L"win+shift+d", L"Win + Shift + D", hk == L"win+shift+d") + Opt(L"ctrl+alt+d", L"Ctrl + Alt + D", hk == L"ctrl+alt+d") +
                                Opt(L"ctrl+shift+space", L"Ctrl + Shift + Space", hk == L"ctrl+shift+space") + Opt(L"ctrl+alt+b", L"Ctrl + Alt + B", hk == L"ctrl+alt+b") + L"</ComboBox>";
            std::wstring x = Card(L"&#xE765;", Str(L"aboveKey"), Str(L"aboveKeyDesc"), keys);
            if (!hk.empty() && !a.GetNamedBoolean(L"hotkeyOk", true))
                x += L"<InfoBar IsOpen=\"True\" IsClosable=\"False\" Severity=\"Warning\" Message=\"" + Str(L"aboveKeyTaken") + L"\"/>";
            std::wstring corners = L"<ComboBox x:Name=\"AboveCorner\" MinWidth=\"220\">" + Opt(L"", Str(L"aboveNone"), corner.empty()) +
                                   Opt(L"tl", Str(L"cornerTL"), corner == L"tl") + Opt(L"tr", Str(L"cornerTR"), corner == L"tr") +
                                   Opt(L"bl", Str(L"cornerBL"), corner == L"bl") + Opt(L"br", Str(L"cornerBR"), corner == L"br") + L"</ComboBox>";
            x += Card(L"&#xE7C4;", Str(L"aboveCorner"), Str(L"aboveCornerDesc"), corners);
            x += Card(L"&#xE706;", Str(L"aboveDim"), Str(L"aboveDimDesc"), Toggle(L"AboveDim", a.GetNamedBoolean(L"dim", true)));
            x += Card(L"&#xE768;", Str(L"aboveTry"), Str(L"aboveTryDesc"), L"<Button x:Name=\"AboveNow\" Style=\"{StaticResource AccentButtonStyle}\" Content=\"" + Str(L"aboveTry") + L"\" MinWidth=\"120\"/>");
            return x;
        }

        std::wstring PageSaved()
        {
            auto sv = g.data.GetNamedObject(L"saved", JsonObject());
            std::wstring x = Card(L"&#xE74E;", Str(L"saveNow"), Str(L"saveNowDesc"), L"<Button x:Name=\"SaveNow\" Style=\"{StaticResource AccentButtonStyle}\" Content=\"" + Str(L"saveNowButton") + L"\" MinWidth=\"120\"/>");
            x += Card(L"&#xE700;", Str(L"savedInMenu"), Str(L"savedInMenuDesc"), Toggle(L"SavedMenu", sv.GetNamedBoolean(L"inMenu", true)));
            auto list = [&](const wchar_t* key, const wchar_t* title, bool del)
            {
                x += Section(Str(title));
                auto a = sv.GetNamedArray(key, JsonArray());
                if (a.Size() == 0) x += Note(Str(L"noSaved"));
                for (auto const& e : a)
                {
                    auto o = e.GetObject();
                    std::wstring id = std::to_wstring((long long)o.GetNamedNumber(L"id"));
                    std::wstring ctl = L"<StackPanel Orientation=\"Horizontal\" Spacing=\"4\"><Button x:Name=\"Restore" + id + L"\" Content=\"" + Str(L"restoreButton") + L"\" MinWidth=\"120\"/>";
                    ctl += del ? IconButton(L"DelSaved" + id, L"&#xE74D;", Str(L"deleteSaved"), true) : L"<Border Width=\"32\"/>";
                    ctl += L"</StackPanel>";
                    x += Card(L"&#xE81C;", Esc(std::wstring(o.GetNamedString(L"when").c_str())), Str(L"boxesN") + std::to_wstring((long long)o.GetNamedNumber(L"boxes")), ctl);
                }
            };
            list(L"list", L"savedList", true);
            list(L"backups", L"backupsList", false);
            return x;
        }

        std::wstring PageSearch()
        {
            auto se = g.data.GetNamedObject(L"search", JsonObject());
            std::wstring x = Card(L"&#xE721;", Str(L"searchField"), Str(L"searchFieldDesc"), Toggle(L"SearchField", se.GetNamedBoolean(L"field", true)));
            x += Card(L"&#xE765;", Str(L"typeSearch"), Str(L"typeSearchDesc"), Toggle(L"TypeSearch", se.GetNamedBoolean(L"typeToSearch", true)));
            return x;
        }

        std::wstring PageLocks()
        {
            auto app = g.data.GetNamedObject(L"app", JsonObject());
            return Card(L"&#xE72E;", Str(L"lockAll"), Str(L"lockAllShort"), Toggle(L"LockAll", app.GetNamedBoolean(L"lockAll", false)));
        }

        std::wstring PagePrivacy()
        {
            auto app = g.data.GetNamedObject(L"app", JsonObject());
            return Card(L"&#xED1A;", Str(L"privacyAllow"), Str(L"privacyAllowDesc"), Toggle(L"Privacy", app.GetNamedBoolean(L"privacy", true)));
        }

        // A feature's picture, filling a rounded frame: res/settings/NAME.jpg in the dark theme,
        // NAME_l.jpg in the light one (drawn pictures, made by tools/art). `corner`: the CornerRadius.
        std::wstring Pic(const std::wstring& name, const std::wstring& size, const wchar_t* corner = L"8")
        {
            auto img = [&](const wchar_t* theme, const std::wstring& file)
            { return std::wstring(L"<ResourceDictionary x:Key=\"") + theme + L"\"><BitmapImage x:Key=\"Pic\" UriSource=\"ms-appx:///settings/" + file + L".jpg\"/></ResourceDictionary>"; };
            return L"<Border " + size + L" CornerRadius=\"" + corner + L"\" BorderBrush=\"{ThemeResource CardStrokeColorDefaultBrush}\" BorderThickness=\"1\">"
                   L"<Border.Resources><ResourceDictionary><ResourceDictionary.ThemeDictionaries>" + img(L"Default", name) + img(L"Dark", name) + img(L"Light", name + L"_l") +
                   L"</ResourceDictionary.ThemeDictionaries></ResourceDictionary></Border.Resources>"
                   L"<Border.Background><ImageBrush ImageSource=\"{ThemeResource Pic}\" Stretch=\"UniformToFill\" AlignmentY=\"Center\"/></Border.Background></Border>";
        }

        // A key as a keycap.
        std::wstring Key(const std::wstring& k)
        {
            return L"<Border Background=\"{ThemeResource ControlFillColorDefaultBrush}\" BorderBrush=\"{ThemeResource ControlStrokeColorDefaultBrush}\" BorderThickness=\"1,1,1,2\" CornerRadius=\"4\" Padding=\"8,2\" MinWidth=\"28\">"
                   L"<TextBlock Text=\"" + k + L"\" FontSize=\"12\" HorizontalAlignment=\"Center\"/></Border>";
        }

        // Home: what Deskboxes is (a picture of it), quick actions, every feature as a tile with
        // its picture (click: its page; a switch where the feature has one) and the shortcuts.
        struct Tile { const wchar_t* page; const wchar_t* pic; const wchar_t* title; const wchar_t* desc; const wchar_t* scope; const wchar_t* key; };
        const Tile kTiles[] = {
            { L"tabs", L"tabs", L"pageTabs", L"tileTabs", nullptr, nullptr },
            { L"rollup", L"rollup", L"pageRollup", L"tileRollup", nullptr, nullptr },
            { L"rules", L"rules", L"pageRules", L"tileRules", nullptr, nullptr },
            { L"az", L"az", L"pageAz", L"tileAz", nullptr, nullptr },
            { L"icons", L"icons", L"tileIconsTitle", L"tileIcons", L"icons", L"blend" },
            { L"colors", L"colors", L"pageColors", L"tileColors", nullptr, nullptr },
            { L"folder", L"folder", L"pageFolder", L"tileFolder", nullptr, nullptr },
            { L"above", L"above", L"pageAbove", L"tileAbove", nullptr, nullptr },
            { L"hide", L"hide", L"pageHide", L"tileHide", L"hide", L"doubleClick" },
            { L"saved", L"saved", L"pageSaved", L"tileSaved", nullptr, nullptr },
            { L"search", L"search", L"pageSearch", L"tileSearch", L"search", L"field" },
            { L"locks", L"locks", L"pageLocks", L"tileLocks", L"app", L"lockAll" },
            { L"privacy", L"privacy", L"pagePrivacy", L"tilePrivacy", L"app", L"privacy" },
        };

        bool TileOn(const Tile& t)
        {
            if (!t.scope) return false;
            std::wstring sc = t.scope, k = t.key;
            if (sc == L"app") return g.data.GetNamedObject(L"app", JsonObject()).GetNamedBoolean(k == L"lockAll" ? L"lockAll" : L"privacy", k != L"lockAll");
            bool def = sc == L"search";
            return g.data.GetNamedObject(t.scope, JsonObject()).GetNamedBoolean(t.key, def);
        }

        std::wstring PageHome()
        {
            auto app = g.data.GetNamedObject(L"app", JsonObject());
            size_t boxes = g.data.GetNamedArray(L"boxes", JsonArray()).Size();
            // The picture with the name over it.
            std::wstring x = L"<Grid Height=\"230\" Margin=\"0,0,0,8\">" + Pic(L"home", L"", L"8") +
                             L"<Border CornerRadius=\"8\"><Border.Background><LinearGradientBrush StartPoint=\"0,0.5\" EndPoint=\"1,0.5\">"
                             L"<GradientStop Color=\"#F0141414\" Offset=\"0\"/><GradientStop Color=\"#C8141414\" Offset=\"0.42\"/><GradientStop Color=\"#00141414\" Offset=\"0.8\"/></LinearGradientBrush></Border.Background></Border>"
                             L"<StackPanel VerticalAlignment=\"Center\" HorizontalAlignment=\"Left\" MaxWidth=\"380\" Margin=\"28,0,24,0\" Spacing=\"2\">"
                             L"<TextBlock Text=\"Deskboxes\" Style=\"{StaticResource TitleLargeTextBlockStyle}\" Foreground=\"White\"/>"
                             L"<TextBlock Text=\"" + Str(L"homeTag") + L"\" Foreground=\"#E6FFFFFF\" TextWrapping=\"Wrap\"/>"
                             L"<TextBlock Text=\"" + Esc(std::wstring(app.GetNamedString(L"version", L"").c_str())) + L"  ·  " + Str(L"boxesN") + std::to_wstring(boxes) + L"\" Foreground=\"#B3FFFFFF\" Style=\"{StaticResource CaptionTextBlockStyle}\" Margin=\"0,4,0,0\"/>"
                             L"</StackPanel></Grid>";
            // Quick actions.
            auto action = [](const wchar_t* name, const wchar_t* glyph, const wchar_t* text, bool accent)
            {
                return std::wstring(L"<Button x:Name=\"") + name + L"\"" + (accent ? L" Style=\"{StaticResource AccentButtonStyle}\"" : L"") +
                       L" Padding=\"14,8\"><StackPanel Orientation=\"Horizontal\" Spacing=\"8\"><FontIcon Glyph=\"" + glyph + L"\" FontSize=\"14\"/><TextBlock Text=\"" + Str(text) + L"\"/></StackPanel></Button>";
            };
            x += L"<StackPanel Orientation=\"Horizontal\" Spacing=\"8\" Margin=\"0,4,0,0\">" + action(L"HomeSort", L"&#xE8CB;", L"sortNowButtonLong", true) +
                 action(L"HomeSave", L"&#xE74E;", L"saveNow", false) + action(L"HomeRules", L"&#xE71C;", L"pageRules", false) + L"</StackPanel>";
            // Features.
            x += Section(Str(L"navFeatures"));
            x += L"<VariableSizedWrapGrid Orientation=\"Horizontal\" ItemWidth=\"230\" ItemHeight=\"206\">";
            for (auto const& t : kTiles)
            {
                std::wstring tag = t.page;
                x += L"<Grid Margin=\"0,0,8,8\">"
                     L"<Button x:Name=\"Tile_" + tag + L"\" Padding=\"0\" HorizontalAlignment=\"Stretch\" VerticalAlignment=\"Stretch\" HorizontalContentAlignment=\"Stretch\" VerticalContentAlignment=\"Stretch\""
                     L" Background=\"{ThemeResource CardBackgroundFillColorDefaultBrush}\" BorderBrush=\"{ThemeResource CardStrokeColorDefaultBrush}\" CornerRadius=\"8\">"
                     L"<Grid><Grid.RowDefinitions><RowDefinition Height=\"124\"/><RowDefinition Height=\"*\"/></Grid.RowDefinitions>" +
                     Pic(std::wstring(L"t_") + t.pic, L"", L"7,7,0,0") +
                     L"<StackPanel Grid.Row=\"1\" Padding=\"14,10,14,10\" Spacing=\"2\">"
                     L"<TextBlock Text=\"" + Str(t.title) + L"\" Style=\"{StaticResource BodyStrongTextBlockStyle}\" Margin=\"0,0,52,0\" TextTrimming=\"CharacterEllipsis\"/>"
                     L"<TextBlock Text=\"" + Str(t.desc) + L"\" Style=\"{StaticResource CaptionTextBlockStyle}\" Foreground=\"{ThemeResource TextFillColorSecondaryBrush}\" TextWrapping=\"Wrap\" MaxLines=\"2\" TextTrimming=\"CharacterEllipsis\"/>"
                     L"</StackPanel></Grid></Button>";
                // The switch sits over the tile (not in the button, so switching it doesn't open the page).
                if (t.scope)
                    x += L"<ToggleSwitch x:Name=\"HomeT_" + tag + L"\" OnContent=\"\" OffContent=\"\" MinWidth=\"0\" Width=\"40\" HorizontalAlignment=\"Right\" VerticalAlignment=\"Top\" Margin=\"0,130,10,0\"" +
                         std::wstring(TileOn(t) ? L" IsOn=\"True\"" : L"") + L"/>";
                else
                    x += L"<FontIcon Glyph=\"&#xE76C;\" FontSize=\"12\" HorizontalAlignment=\"Right\" VerticalAlignment=\"Top\" Margin=\"0,142,16,0\" Foreground=\"{ThemeResource TextFillColorSecondaryBrush}\" IsHitTestVisible=\"False\"/>";
                x += L"</Grid>";
            }
            x += L"</VariableSizedWrapGrid>";
            // Shortcuts.
            x += Section(Str(L"shortcutsHead"));
            auto row = [](const std::wstring& what, const std::wstring& keys)
            {
                return L"<Grid Padding=\"16,10\" ColumnSpacing=\"16\"><Grid.ColumnDefinitions><ColumnDefinition Width=\"*\"/><ColumnDefinition Width=\"Auto\"/></Grid.ColumnDefinitions>"
                       L"<TextBlock Text=\"" + what + L"\" VerticalAlignment=\"Center\"/><StackPanel Grid.Column=\"1\" Orientation=\"Horizontal\" Spacing=\"4\" VerticalAlignment=\"Center\">" + keys + L"</StackPanel></Grid>";
            };
            auto words = [](const wchar_t* key)
            {
                return L"<TextBlock Text=\"" + Str(key) + L"\" Foreground=\"{ThemeResource TextFillColorSecondaryBrush}\" VerticalAlignment=\"Center\"/>";
            };
            auto rollup = g.data.GetNamedObject(L"rollup", JsonObject());
            bool hide = g.data.GetNamedObject(L"hide", JsonObject()).GetNamedBoolean(L"doubleClick", false);
            std::wstring rows = row(Str(L"scSearch"), Key(L"Ctrl") + Key(L"F")) + row(Str(L"scType"), words(L"scTypeHow")) +
                                row(Str(L"scUndo"), Key(L"Ctrl") + Key(L"Z")) + row(Str(L"scRedo"), Key(L"Ctrl") + Key(L"Y")) +
                                row(Str(L"scRename"), Key(L"F2"));
            if (rollup.GetNamedBoolean(L"doubleClick", true)) rows += row(Str(L"scRoll"), words(L"scRollHow"));
            if (hide) rows += row(Str(L"scHide"), words(L"scHideHow"));
            {
                std::wstring hk = g.data.GetNamedObject(L"above", JsonObject()).GetNamedString(L"hotkey", L"win+shift+d").c_str();
                if (!hk.empty())
                {
                    std::wstring caps;
                    size_t at = 0;
                    while (at <= hk.size())
                    {
                        size_t e = hk.find(L'+', at);
                        if (e == std::wstring::npos) e = hk.size();
                        std::wstring k = hk.substr(at, e - at);
                        if (!k.empty()) k[0] = towupper(k[0]);
                        caps += Key(k);
                        at = e + 1;
                    }
                    rows += row(Str(L"scAbove"), caps);
                }
            }
            rows += row(Str(L"scBoxMenu"), words(L"scBoxMenuHow"));
            x += L"<Border Background=\"{ThemeResource CardBackgroundFillColorDefaultBrush}\" BorderBrush=\"{ThemeResource CardStrokeColorDefaultBrush}\" BorderThickness=\"1\" CornerRadius=\"8\" Padding=\"0,4\"><StackPanel>" + rows + L"</StackPanel></Border>";
            return x;
        }

        // The pages, in the navigation's order. `group`: under the Look entry; `feature`: under
        // the Features heading, with a picture and what it does at the top (`hero`).
        struct Page { const wchar_t* tag; const wchar_t* title; const wchar_t* glyph; std::wstring (*build)(); bool group; bool feature; const wchar_t* hero; const wchar_t* pic; };
        const Page kPages[] = {
            { L"home", L"pageHome", L"&#xE80F;", PageHome, false, false, nullptr, nullptr },
            { L"general", L"pageGeneral", L"&#xE713;", PageGeneral, false, false, nullptr, nullptr },
            { L"look", L"pageStyles", L"&#xE737;", PageLook, true, false, L"heroLook", L"look" },
            { L"colors", L"pageColors", L"&#xE790;", PageColors, true, false, L"colorsDesc", L"colors" },
            { L"icons", L"pageIcons", L"&#xE8B9;", PageIcons, true, false, L"iconsDesc", L"icons" },
            { L"touches", L"pageTouches", L"&#xE945;", PageTouches, true, false, nullptr, nullptr },
            { L"boxes", L"pageBoxes", L"&#xE8A9;", PageBoxes, false, false, L"boxesDesc", L"boxes" },
            { L"tabs", L"pageTabs", L"&#xE8F1;", PageTabs, false, true, L"heroTabs", L"tabs" },
            { L"rollup", L"pageRollup", L"&#xE70E;", PageRollup, false, true, L"heroRollup", L"rollup" },
            { L"rules", L"pageRules", L"&#xE8CB;", PageRules, false, true, L"rulesIntro", L"rules" },
            { L"az", L"pageAz", L"&#xE8D2;", PageAz, false, true, L"heroAz", L"az" },
            { L"folder", L"pageFolder", L"&#xE8B7;", PageFolder, false, true, L"heroFolder", L"folder" },
            { L"above", L"pageAbove", L"&#xE8A0;", PageAbove, false, true, L"heroAbove", L"above" },
            { L"hide", L"pageHide", L"&#xE91B;", PageHide, false, true, L"heroHide", L"hide" },
            { L"saved", L"pageSaved", L"&#xE81C;", PageSaved, false, true, L"heroSaved", L"saved" },
            { L"search", L"pageSearch", L"&#xE721;", PageSearch, false, true, L"heroSearch", L"search" },
            { L"locks", L"pageLocks", L"&#xE72E;", PageLocks, false, true, L"heroLocks", L"locks" },
            { L"privacy", L"pagePrivacy", L"&#xED1A;", PagePrivacy, false, true, L"heroPrivacy", L"privacy" },
        };

        // A page's top: a picture of the feature on a desktop, and what it does beside it.
        std::wstring Hero(const Page* p)
        {
            if (!p->pic)
                return L"<TextBlock Text=\"" + Str(p->hero) + L"\" TextWrapping=\"Wrap\" Foreground=\"{ThemeResource TextFillColorSecondaryBrush}\" Margin=\"0,0,0,16\"/>";
            return L"<Grid ColumnSpacing=\"24\" Margin=\"0,0,0,20\"><Grid.ColumnDefinitions><ColumnDefinition Width=\"Auto\"/><ColumnDefinition Width=\"*\"/></Grid.ColumnDefinitions>" +
                   Pic(p->pic, L"Width=\"336\" Height=\"189\"") +
                   L"<StackPanel Grid.Column=\"1\" VerticalAlignment=\"Center\" Spacing=\"6\"><FontIcon Glyph=\"" + std::wstring(p->glyph) + L"\" FontSize=\"24\" HorizontalAlignment=\"Left\" Foreground=\"{ThemeResource AccentTextFillColorPrimaryBrush}\"/>"
                   L"<TextBlock Text=\"" + Str(p->hero) + L"\" TextWrapping=\"Wrap\" Style=\"{StaticResource BodyTextBlockStyle}\"/></StackPanel></Grid>";
        }

        const Page* CurrentPage()
        {
            const Page* cur = &kPages[0];
            for (auto const& p : kPages)
                if (g.page == p.tag) cur = &p;
            return cur;
        }

        // A page: its title and cards in a scroller. `standalone`: a XAML document of its own (a
        // page switch puts it into the navigation view that is there). `animate`: the cards come
        // in one after another, rising a little, as Windows Settings pages do.
        std::wstring PageXaml(const Page* cur, bool standalone, bool animate)
        {
            std::wstring x = L"<ScrollViewer x:Name=\"Scroll\"";
            if (standalone)
                x += L" xmlns=\"http://schemas.microsoft.com/winfx/2006/xaml/presentation\" xmlns:x=\"http://schemas.microsoft.com/winfx/2006/xaml\"";
            x += L"><StackPanel Spacing=\"4\" Margin=\"36,12,36,36\" MaxWidth=\"1000\">";
            if (animate)
                x += L"<StackPanel.ChildrenTransitions><TransitionCollection><EntranceThemeTransition FromVerticalOffset=\"36\" IsStaggeringEnabled=\"True\"/></TransitionCollection></StackPanel.ChildrenTransitions>";
            x += L"<TextBlock Text=\"" + Str(cur->title) + L"\" Style=\"{StaticResource TitleTextBlockStyle}\" Margin=\"0,0,0,12\"/>";
            if (cur->hero) x += Hero(cur);
            x += cur->build();
            return x + L"</StackPanel></ScrollViewer>";
        }

        std::wstring BuildXaml()
        {
            const Page* cur = CurrentPage();
            std::wstring x = LR"(<Grid xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation" xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml">
<Grid.RowDefinitions><RowDefinition Height="Auto"/><RowDefinition Height="*"/></Grid.RowDefinitions>
<Grid x:Name="TitleBar" Height="48" Background="Transparent"><StackPanel Orientation="Horizontal" Spacing="12" Margin="16,0,0,0" VerticalAlignment="Center">
<Image Source="ms-appx:///deskboxes.png" Width="16" Height="16"/><TextBlock Text=")" + Str(L"title") + LR"(" Style="{StaticResource CaptionTextBlockStyle}" VerticalAlignment="Center"/></StackPanel></Grid>
<NavigationView x:Name="Nav" Grid.Row="1" PaneDisplayMode="Left" IsBackButtonVisible="Collapsed" IsSettingsVisible="False" IsPaneToggleButtonVisible="False" OpenPaneLength="250" IsTitleBarAutoPaddingEnabled="False">
<NavigationView.MenuItems>)";
            auto item = [cur](const Page& p)
            {
                return L"<NavigationViewItem Tag=\"" + std::wstring(p.tag) + L"\" Content=\"" + Str(p.title) + L"\"" + Sel(&p == cur) +
                       L"><NavigationViewItem.Icon><FontIcon Glyph=\"" + p.glyph + L"\"/></NavigationViewItem.Icon></NavigationViewItem>";
            };
            bool groupDone = false, headerDone = false;
            for (auto const& p : kPages)
            {
                if (p.group)
                {
                    if (groupDone) continue;
                    groupDone = true;
                    // Look: an entry that opens to its pages (open while one of them is shown).
                    x += L"<NavigationViewItem Tag=\"\" Content=\"" + Str(L"pageLook") + L"\" SelectsOnInvoked=\"False\"" +
                         std::wstring(cur->group || g.lookOpen ? L" IsExpanded=\"True\"" : L"") +
                         L"><NavigationViewItem.Icon><FontIcon Glyph=\"&#xE771;\"/></NavigationViewItem.Icon><NavigationViewItem.MenuItems>";
                    for (auto const& c : kPages)
                        if (c.group) x += item(c);
                    x += L"</NavigationViewItem.MenuItems></NavigationViewItem>";
                    continue;
                }
                if (p.feature && !headerDone)
                {
                    headerDone = true;
                    x += L"<NavigationViewItemHeader Content=\"" + Str(L"navFeatures") + L"\"/>";
                }
                x += item(p);
            }
            x += L"</NavigationView.MenuItems>" + PageXaml(cur, false, false) + L"</NavigationView></Grid>";
            return x;
        }

        void Send(const wchar_t* scope, double id, const wchar_t* key, IJsonValue const& value)
        {
            if (g.wiring || !g.send) return;
            JsonObject o;
            o.SetNamedValue(L"scope", JsonValue::CreateStringValue(scope));
            o.SetNamedValue(L"box", JsonValue::CreateNumberValue(id));
            o.SetNamedValue(L"key", JsonValue::CreateStringValue(key));
            o.SetNamedValue(L"value", value);
            g.send(o);
        }

        template <class T> T Find(mux::FrameworkElement const& root, const std::wstring& name)
        {
            auto o = root.FindName(name);
            return o ? o.try_as<T>() : T{ nullptr };
        }

        std::wstring Tag(muxc::ComboBox const& c, const wchar_t* fallback)
        {
            auto it = c.SelectedItem().try_as<muxc::ComboBoxItem>();
            return it ? std::wstring(winrt::unbox_value_or<winrt::hstring>(it.Tag(), fallback).c_str()) : fallback;
        }

        void OnCombo(mux::FrameworkElement const& root, const std::wstring& name, std::function<void(std::wstring)> f)
        {
            if (auto c = Find<muxc::ComboBox>(root, name))
                c.SelectionChanged([c, f](auto&&, auto&&) { f(Tag(c, L"")); });
        }

        void OnToggle(mux::FrameworkElement const& root, const std::wstring& name, const wchar_t* scope, const wchar_t* key, double id = 0)
        {
            if (auto t = Find<muxc::ToggleSwitch>(root, name))
            {
                auto state = Find<muxc::TextBlock>(root, name + L"State");
                t.Toggled([t, state, scope, key, id](auto&&, auto&&)
                {
                    if (state) state.Text(winrt::hstring(Str(t.IsOn() ? L"toggleOn" : L"toggleOff")));
                    Send(scope, id, key, JsonValue::CreateBooleanValue(t.IsOn()));
                });
            }
        }

        void OnClick(mux::FrameworkElement const& root, const std::wstring& name, const wchar_t* scope, double id, const wchar_t* key)
        {
            if (auto b = Find<muxc::Button>(root, name))
                b.Click([scope, id, key](auto&&, auto&&) { Send(scope, id, key, JsonValue::CreateBooleanValue(true)); });
        }

        // A text field sends its text when it loses focus or on Enter (only if it changed).
        void OnText(mux::FrameworkElement const& root, const std::wstring& name, const wchar_t* scope, double id, const wchar_t* key)
        {
            auto t = Find<muxc::TextBox>(root, name);
            if (!t) return;
            auto orig = std::make_shared<std::wstring>(t.Text().c_str());
            auto commit = [t, orig, scope, id, key]
            {
                std::wstring now = t.Text().c_str();
                if (now == *orig) return;
                *orig = now;
                Send(scope, id, key, JsonValue::CreateStringValue(now));
            };
            t.LostFocus([commit](auto&&, auto&&) { commit(); });
            t.KeyDown([commit](auto&&, mux::Input::KeyRoutedEventArgs const& e) { if (e.Key() == winrt::Windows::System::VirtualKey::Enter) commit(); });
        }

        void WireStyle(mux::FrameworkElement const& root, const std::wstring& prefix, const wchar_t* scope, double id)
        {
            std::wstring sc = scope;
            OnCombo(root, prefix + L"Color", [sc, id](std::wstring v) { Send(sc.c_str(), id, L"color", JsonValue::CreateStringValue(v)); });
            if (auto s = Find<muxc::Slider>(root, prefix + L"Opacity"))
                s.ValueChanged([sc, id](auto&&, mux::Controls::Primitives::RangeBaseValueChangedEventArgs const& e)
                {
                    Send(sc.c_str(), id, L"opacity", JsonValue::CreateNumberValue(1.0 - e.NewValue() / 100.0));
                });
            if (auto s = Find<muxc::Slider>(root, prefix + L"Blur"))
                s.ValueChanged([sc, id](auto&&, mux::Controls::Primitives::RangeBaseValueChangedEventArgs const& e)
                {
                    Send(sc.c_str(), id, L"blur", JsonValue::CreateNumberValue(e.NewValue()));
                });
        }

        void Build();

        // A fine-tuning change: the preset no longer matches (the next refresh says if it does).
        void UnmarkPresets(mux::FrameworkElement const& root)
        {
            for (auto const& p : g.data.GetNamedObject(L"look", JsonObject()).GetNamedArray(L"presets", JsonArray()))
                if (auto b = Find<muxc::Button>(root, L"Preset_" + std::wstring(p.GetObject().GetNamedString(L"id").c_str())))
                    b.BorderBrush(mux::Media::SolidColorBrush(winrt::Windows::UI::Colors::Transparent()));
        }

        void WireSizePair(mux::FrameworkElement const& root, const std::wstring& name, const wchar_t* key)
        {
            auto sl = Find<muxc::Slider>(root, name + L"Slider");
            auto cb = Find<muxc::ComboBox>(root, name + L"Preset");
            if (!sl || !cb) return;
            auto sync = std::make_shared<bool>(false);
            cb.SelectionChanged([sl, cb, sync](auto&&, auto&&)
            {
                if (*sync) return;
                auto t = Tag(cb, L"custom");
                if (t == L"custom") return;
                *sync = true;
                sl.Value(_wtof(t.c_str()));
                *sync = false;
            });
            mux::FrameworkElement r = root;
            sl.ValueChanged([cb, sync, key, r](auto&&, mux::Controls::Primitives::RangeBaseValueChangedEventArgs const& e)
            {
                Send(L"look", 0, key, JsonValue::CreateNumberValue(e.NewValue()));
                UnmarkPresets(r);
                if (*sync) return;
                *sync = true;
                auto items = cb.Items();
                int sel = (int)items.Size() - 1;
                for (uint32_t i = 0; i < items.Size(); i++)
                {
                    auto it = items.GetAt(i).try_as<muxc::ComboBoxItem>();
                    std::wstring t = it ? winrt::unbox_value_or<winrt::hstring>(it.Tag(), L"custom").c_str() : L"custom";
                    if (t != L"custom" && std::fabs(_wtof(t.c_str()) - e.NewValue()) < 0.01) { sel = (int)i; break; }
                }
                cb.SelectedIndex(sel);
                *sync = false;
            });
        }

        void SwitchPage();

        // Open a page from inside a page (a Home tile): select its navigation entry, which
        // switches the page; an entry under the closed Look group: build the window anew.
        void GoTo(const std::wstring& tag)
        {
            g.page = tag;
            auto root = g.window ? g.window.Content().try_as<mux::FrameworkElement>() : mux::FrameworkElement{ nullptr };
            auto nav = root ? Find<muxc::NavigationView>(root, L"Nav") : muxc::NavigationView{ nullptr };
            if (nav)
                for (auto const& o : nav.MenuItems())
                    if (auto it = o.try_as<muxc::NavigationViewItem>())
                        if (winrt::unbox_value_or<winrt::hstring>(it.Tag(), L"") == tag) { nav.SelectedItem(it); return; }
            for (auto const& p : kPages)
                if (tag == p.tag && p.group) g.lookOpen = true;
            g.window.DispatcherQueue().TryEnqueue([] { Build(); });
        }

        void Wire(mux::FrameworkElement const& root)
        {
            if (auto nav = Find<muxc::NavigationView>(root, L"Nav"))
                nav.SelectionChanged([](auto&&, muxc::NavigationViewSelectionChangedEventArgs const& e)
                {
                    if (g.wiring) return;
                    auto it = e.SelectedItem().try_as<muxc::NavigationViewItem>();
                    if (!it) return;
                    std::wstring tag = winrt::unbox_value_or<winrt::hstring>(it.Tag(), L"").c_str();
                    if (tag.empty()) return;
                    g.page = tag;
                    g.window.DispatcherQueue().TryEnqueue([] { SwitchPage(); });
                });
            if (auto nav = Find<muxc::NavigationView>(root, L"Nav"))
            {
                nav.Expanding([](auto&&, auto&&) { g.lookOpen = true; });
                nav.Collapsed([](auto&&, auto&&) { g.lookOpen = false; });
            }
            // Home: tiles open their page; switches; quick actions.
            for (auto const& t : kTiles)
            {
                std::wstring tag = t.page;
                if (auto b = Find<muxc::Button>(root, L"Tile_" + tag))
                    b.Click([tag](auto&&, auto&&) { GoTo(tag); });
                if (t.scope) OnToggle(root, L"HomeT_" + tag, t.scope, t.key);
            }
            OnClick(root, L"HomeSort", L"sorting", 0, L"now");
            OnClick(root, L"HomeSave", L"saved", 0, L"now");
            if (auto b = Find<muxc::Button>(root, L"HomeRules")) b.Click([](auto&&, auto&&) { GoTo(L"rules"); });
            // General
            OnToggle(root, L"Autostart", L"app", L"autostart");
            OnToggle(root, L"UpdateAuto", L"update", L"auto");
            OnClick(root, L"UpdateNow", L"update", 0, L"now");
            OnClick(root, L"UpdateInstall", L"update", 0, L"install");
            OnToggle(root, L"LockAll", L"app", L"lockAll");
            OnToggle(root, L"Privacy", L"app", L"privacy");
            OnCombo(root, L"Language", [](std::wstring v) { Send(L"app", 0, L"language", JsonValue::CreateStringValue(v)); });
            OnClick(root, L"Disable", L"app", 0, L"disable");
            // Boxes
            WireStyle(root, L"Def", L"default", 0);
            OnClick(root, L"ApplyAll", L"default", 0, L"applyAll");
            for (auto const& bx : g.data.GetNamedArray(L"boxes", JsonArray()))
            {
                double id = bx.GetObject().GetNamedNumber(L"id");
                std::wstring p = L"Box" + std::to_wstring((long long)id);
                WireStyle(root, p, L"box", id);
                OnText(root, p + L"Title", L"box", id, L"title");
                OnClick(root, p + L"Remove", L"box", id, L"remove");
                if (auto ex = Find<muxc::Expander>(root, p + L"Exp"))
                {
                    long long key = (long long)id;
                    ex.Expanding([key](auto&&, auto&&) { g.expanded.insert(key); });
                    ex.Collapsed([key](auto&&, auto&&) { g.expanded.erase(key); });
                }
            }
            // Look
            for (auto const& pv : g.data.GetNamedObject(L"look", JsonObject()).GetNamedArray(L"presets", JsonArray()))
            {
                std::wstring id = pv.GetObject().GetNamedString(L"id").c_str();
                if (auto b = Find<muxc::Button>(root, L"Preset_" + id))
                    b.Click([id](auto&&, auto&&) { Send(L"look", 0, L"preset", JsonValue::CreateStringValue(id)); });
            }
            {
                mux::FrameworkElement r = root;
                OnCombo(root, L"LookBar", [r](std::wstring v)
                {
                    Send(L"look", 0, L"titleBar", JsonValue::CreateStringValue(v));
                    if (auto s = Find<muxc::Slider>(r, L"LookBand")) s.IsEnabled(v == L"band");
                    UnmarkPresets(r);
                });
                OnCombo(root, L"LookAlign", [r](std::wstring v) { Send(L"look", 0, L"align", JsonValue::CreateStringValue(v)); UnmarkPresets(r); });
                if (auto s = Find<muxc::Slider>(root, L"LookBand"))
                    s.ValueChanged([r](auto&&, mux::Controls::Primitives::RangeBaseValueChangedEventArgs const& e)
                    {
                        Send(L"look", 0, L"band", JsonValue::CreateNumberValue(e.NewValue() / 100.0));
                        UnmarkPresets(r);
                    });
            }
            WireSizePair(root, L"LookSize", L"titleSize");
            WireSizePair(root, L"LookRadius", L"radius");
            // Sorting rules
            OnClick(root, L"SortNow", L"sorting", 0, L"now");
            OnToggle(root, L"KeepSorted", L"sorting", L"keep");
            OnCombo(root, L"NewTo", [](std::wstring v) { Send(L"sorting", 0, L"newTo", JsonValue::CreateNumberValue(v.empty() ? 0 : _wtof(v.c_str()))); });
            OnClick(root, L"AddRule", L"rules", 0, L"add");
            for (auto const& rv : g.data.GetNamedObject(L"sorting", JsonObject()).GetNamedArray(L"rules", JsonArray()))
            {
                double id = rv.GetObject().GetNamedNumber(L"id");
                std::wstring p = L"R" + std::to_wstring((long long)id);
                if (auto c = Find<muxc::CheckBox>(root, p + L"On"))
                    c.Click([c, id](auto&&, auto&&) { auto v = c.IsChecked(); Send(L"rules", id, L"enabled", JsonValue::CreateBooleanValue(v && v.Value())); });
                OnCombo(root, p + L"Kind", [id](std::wstring v) { Send(L"rules", id, L"kind", JsonValue::CreateStringValue(v)); });
                OnCombo(root, p + L"Op", [id](std::wstring v) { Send(L"rules", id, L"op", JsonValue::CreateStringValue(v)); });
                OnText(root, p + L"Val", L"rules", id, L"value");
                OnCombo(root, p + L"Target", [id](std::wstring v) { Send(L"rules", id, L"target", JsonValue::CreateNumberValue(_wtof(v.c_str()))); });
                OnClick(root, p + L"Up", L"rules", id, L"up");
                OnClick(root, p + L"Down", L"rules", id, L"down");
                OnClick(root, p + L"Del", L"rules", id, L"delete");
            }
            // Tabs
            OnCombo(root, L"TabStyle", [](std::wstring v) { Send(L"tabs", 0, L"style", JsonValue::CreateStringValue(v)); });
            OnToggle(root, L"TabHover", L"tabs", L"hoverSwitch");
            OnToggle(root, L"ExSmooth", L"extras", L"smoothScroll");
            OnToggle(root, L"ExCount", L"extras", L"dragCount");
            OnToggle(root, L"ExLift", L"extras", L"liftBox");
            OnToggle(root, L"ExWheel", L"extras", L"ctrlWheel");
            OnToggle(root, L"TabCombine", L"tabs", L"combine");
            // A–Z index
            OnToggle(root, L"AzBar", L"az", L"letterBar");
            OnToggle(root, L"AzTags", L"az", L"letterTags");
            for (auto const& bx : g.data.GetNamedArray(L"boxes", JsonArray()))
            {
                double id = bx.GetObject().GetNamedNumber(L"id");
                OnToggle(root, L"Az" + std::to_wstring((long long)id), L"box", L"az", id);
                OnToggle(root, L"Stay" + std::to_wstring((long long)id), L"box", L"stayVisible", id);
            }
            OnToggle(root, L"HideDbl", L"hide", L"doubleClick");
            {
                auto known = g.data.GetNamedObject(L"folderBoxes", JsonObject()).GetNamedArray(L"known", JsonArray());
                for (uint32_t i = 0; i < known.Size(); i++)
                    if (auto b = Find<muxc::Button>(root, L"FbNew" + std::to_wstring(i)))
                    {
                        winrt::hstring path = known.GetObjectAt(i).GetNamedString(L"path");
                        b.Click([path](auto&&, auto&&) { Send(L"folderbox", 0, L"new", JsonValue::CreateStringValue(path)); });
                    }
            }
            OnCombo(root, L"AboveKey", [](std::wstring v) { Send(L"above", 0, L"hotkey", JsonValue::CreateStringValue(v)); });
            OnCombo(root, L"AboveCorner", [](std::wstring v) { Send(L"above", 0, L"corner", JsonValue::CreateStringValue(v)); });
            OnToggle(root, L"AboveDim", L"above", L"dim");
            if (auto b = Find<muxc::Button>(root, L"AboveNow"))
                b.Click([](auto&&, auto&&)
                {
                    // The core puts its window in front: let it.
                    AllowSetForegroundWindow(ASFW_ANY);
                    Send(L"above", 0, L"now", JsonValue::CreateBooleanValue(true));
                });
            // Saved layouts
            OnClick(root, L"SaveNow", L"saved", 0, L"now");
            OnToggle(root, L"SavedMenu", L"saved", L"inMenu");
            for (const wchar_t* k : { L"list", L"backups" })
                for (auto const& e : g.data.GetNamedObject(L"saved", JsonObject()).GetNamedArray(k, JsonArray()))
                {
                    double id = e.GetObject().GetNamedNumber(L"id");
                    std::wstring s = std::to_wstring((long long)id);
                    OnClick(root, L"Restore" + s, L"saved", id, L"restore");
                    OnClick(root, L"DelSaved" + s, L"saved", id, L"delete");
                }
            // Icons
            OnCombo(root, L"IconTint", [](std::wstring v) { Send(L"icons", 0, L"tint", JsonValue::CreateStringValue(v)); });
            OnToggle(root, L"IconBlend", L"icons", L"blend");
            OnToggle(root, L"IconDesk", L"icons", L"desktop");
            OnToggle(root, L"IconBlendBoxes", L"icons", L"blendBoxes");
            OnToggle(root, L"IconFullHover", L"icons", L"fullHover");
            OnToggle(root, L"IconFullActive", L"icons", L"fullActive");
            OnToggle(root, L"TitlesHover", L"look", L"titlesHover");
            OnToggle(root, L"TitleButtons", L"look", L"titleButtons");
            OnCombo(root, L"LookFont", [](std::wstring v) { Send(L"look", 0, L"titleFont", JsonValue::CreateStringValue(v)); });
            OnCombo(root, L"LookWeight", [](std::wstring v) { Send(L"look", 0, L"titleWeight", JsonValue::CreateStringValue(v)); });
            {
                mux::FrameworkElement r = root;
                OnCombo(root, L"LookOutline", [r](std::wstring v)
                {
                    Send(L"look", 0, L"outline", JsonValue::CreateStringValue(v));
                    if (auto s = Find<muxc::Slider>(r, L"LookOutlineW")) s.IsEnabled(v != L"none");
                });
                if (auto s = Find<muxc::Slider>(root, L"LookOutlineW"))
                    s.ValueChanged([](auto&&, mux::Controls::Primitives::RangeBaseValueChangedEventArgs const& e) { Send(L"look", 0, L"outlineW", JsonValue::CreateNumberValue(e.NewValue())); });
            }
            OnToggle(root, L"ScrollHover", L"look", L"scrollHover");
            if (auto s = Find<muxc::Slider>(root, L"IconTintS"))
                s.ValueChanged([](auto&&, mux::Controls::Primitives::RangeBaseValueChangedEventArgs const& e) { Send(L"icons", 0, L"tintStrength", JsonValue::CreateNumberValue(e.NewValue() / 100.0)); });
            if (auto s = Find<muxc::Slider>(root, L"IconBlendS"))
                s.ValueChanged([](auto&&, mux::Controls::Primitives::RangeBaseValueChangedEventArgs const& e) { Send(L"icons", 0, L"blendStrength", JsonValue::CreateNumberValue(e.NewValue() / 100.0)); });
            for (auto const& bx : g.data.GetNamedArray(L"boxes", JsonArray()))
            {
                double id = bx.GetObject().GetNamedNumber(L"id");
                std::wstring n = std::to_wstring((long long)id);
                OnCombo(root, L"Tint" + n, [id](std::wstring v) { Send(L"box", id, L"tint", JsonValue::CreateStringValue(v)); });
                OnCombo(root, L"Blend" + n, [id](std::wstring v) { Send(L"box", id, L"blend", JsonValue::CreateStringValue(v)); });
            }
            // Search
            OnToggle(root, L"SearchField", L"search", L"field");
            OnToggle(root, L"TypeSearch", L"search", L"typeToSearch");
            // Roll-up
            OnToggle(root, L"RollEdges", L"rollup", L"edges");
            OnToggle(root, L"RollDbl", L"rollup", L"doubleClick");
            OnCombo(root, L"RollReveal", [](std::wstring v) { Send(L"rollup", 0, L"reveal", JsonValue::CreateStringValue(v)); });
        }

        void Build();

        // Another page: only the page is replaced (the navigation keeps its selection marker
        // gliding to the new entry), and its cards come in animated.
        void SwitchPage()
        {
            if (!g.window) return;
            auto old = g.window.Content().try_as<mux::FrameworkElement>();
            auto nav = old ? Find<muxc::NavigationView>(old, L"Nav") : muxc::NavigationView{ nullptr };
            if (!nav) return Build();
            mux::FrameworkElement page{ nullptr };
            try { page = mux::Markup::XamlReader::Load(PageXaml(CurrentPage(), true, true)).as<mux::FrameworkElement>(); }
            catch (winrt::hresult_error const& e) { Log(L"settings: page XAML failed: %s", e.message().c_str()); return Build(); }
            g.wiring = true;
            g.builtPage = g.page;
            nav.Content(page);
            Wire(page);
            page.Loaded([page](auto&&, auto&&)
            {
                page.DispatcherQueue().TryEnqueue(winrt::Microsoft::UI::Dispatching::DispatcherQueuePriority::Low, [] { g.wiring = false; });
            });
        }

        void Build()
        {
            if (!g.window) return;
            // The same page keeps its scroll position across refreshes; another page starts at the top.
            double scroll = 0;
            if (g.page == g.builtPage)
                if (auto old = g.window.Content().try_as<mux::FrameworkElement>())
                    if (auto nav = Find<muxc::NavigationView>(old, L"Nav"))
                        if (auto sv = nav.Content().try_as<muxc::ScrollViewer>()) scroll = sv.VerticalOffset();
            mux::FrameworkElement root{ nullptr };
            try { root = mux::Markup::XamlReader::Load(BuildXaml()).as<mux::FrameworkElement>(); }
            catch (winrt::hresult_error const& e) { Log(L"settings: XAML failed: %s", e.message().c_str()); return; }
            g.wiring = true;
            g.builtPage = g.page;
            g.window.Content(root);
            g.window.SetTitleBar(Find<mux::UIElement>(root, L"TitleBar"));
            Wire(root);
            // Opened from a box's menu: that box's group open and in view.
            muxc::Expander focus{ nullptr };
            if (g.focusBox)
                focus = Find<muxc::Expander>(root, L"Box" + std::to_wstring(g.focusBox) + L"Exp");
            g.focusBox = 0;
            // Control events raised while the tree loads (initial selection) are not changes.
            root.Loaded([root, scroll, focus](auto&&, auto&&)
            {
                root.DispatcherQueue().TryEnqueue(winrt::Microsoft::UI::Dispatching::DispatcherQueuePriority::Low, [root, scroll, focus]
                {
                    if (focus) focus.StartBringIntoView();
                    else if (auto sv = Find<muxc::ScrollViewer>(root, L"Scroll")) sv.ChangeView(nullptr, scroll, nullptr, true);
                    g.wiring = false;
                });
            });
        }

        void MakeWindow()
        {
            g.window = mux::Window();
            g.window.SystemBackdrop(mux::Media::MicaBackdrop());
            g.window.ExtendsContentIntoTitleBar(true);
            auto aw = g.window.AppWindow();
            g.hwnd = winrt::Microsoft::UI::GetWindowFromWindowId(aw.Id());
            // The app icon (resource 1, res/deskboxes.rc) on the title bar and the taskbar.
            if (HICON ico = (HICON)LoadImageW(GetModuleHandleW(nullptr), MAKEINTRESOURCEW(1), IMAGE_ICON, 0, 0, LR_DEFAULTSIZE))
                aw.SetIcon(winrt::Microsoft::UI::GetIconIdFromIcon(ico));
            aw.TitleBar().PreferredHeightOption(winrt::Microsoft::UI::Windowing::TitleBarHeightOption::Tall);
            // ~ Windows Settings' default size, centered on the work area of the pointer's monitor.
            POINT pt{}; GetCursorPos(&pt);
            HMONITOR mon = MonitorFromPoint(pt, MONITOR_DEFAULTTOPRIMARY);
            MONITORINFO mi{ sizeof(mi) }; GetMonitorInfoW(mon, &mi);
            UINT dpiX = 96, dpiY = 96;
            using GetDpiForMonitorFn = HRESULT(WINAPI*)(HMONITOR, int, UINT*, UINT*);
            if (auto shcore = LoadLibraryW(L"shcore.dll"))
                if (auto f = reinterpret_cast<GetDpiForMonitorFn>(GetProcAddress(shcore, "GetDpiForMonitor"))) f(mon, 0, &dpiX, &dpiY);
            int w = MulDiv(1040, dpiX, 96), h = MulDiv(820, dpiX, 96);
            auto& wa = mi.rcWork;
            w = std::min<int>(w, wa.right - wa.left); h = std::min<int>(h, wa.bottom - wa.top);
            aw.MoveAndResize({ wa.left + (wa.right - wa.left - w) / 2, wa.top + (wa.bottom - wa.top - h) / 2, w, h });
            g.window.Closed([](auto&&, auto&&) { g.window = nullptr; g.hwnd = nullptr; g.builtPage.clear(); TrimMemorySoon(); });
        }
    }

    namespace
    {
        struct Picker
        {
            mux::Window window{ nullptr };
            std::function<void(JsonObject)> send;
            double box = 0;
            std::wstring key, orig;
            bool done = false;
        };
        Picker gp;

        std::wstring Hex(winrt::Windows::UI::Color c)
        {
            wchar_t b[16]; swprintf_s(b, L"#%02X%02X%02X", c.R, c.G, c.B); return b;
        }

        void PickSend(const std::wstring& value, bool fin, bool cancel)
        {
            if (!gp.send) return;
            JsonObject o;
            o.SetNamedValue(L"scope", JsonValue::CreateStringValue(L"pick"));
            o.SetNamedValue(L"box", JsonValue::CreateNumberValue(gp.box));
            o.SetNamedValue(L"key", JsonValue::CreateStringValue(gp.key));
            o.SetNamedValue(L"value", JsonValue::CreateStringValue(value));
            o.SetNamedValue(L"final", JsonValue::CreateBooleanValue(fin));
            o.SetNamedValue(L"cancel", JsonValue::CreateBooleanValue(cancel));
            gp.send(o);
        }
    }

    void ShowColorPicker(JsonObject const& data, std::function<void(JsonObject)> send)
    {
        if (gp.window && !gp.done) { PickSend(gp.orig, true, true); gp.done = true; gp.window.Close(); }
        gp = Picker{};
        gp.send = std::move(send);
        gp.box = data.GetNamedNumber(L"box", 0);
        gp.key = data.GetNamedString(L"key", L"").c_str();
        gp.orig = data.GetNamedString(L"current", L"").c_str();
        std::wstring start = gp.orig.empty() ? std::wstring(data.GetNamedString(L"auto", L"#808080").c_str()) : gp.orig;
        auto esc = [](winrt::hstring const& s) { return Esc(std::wstring(s.c_str())); };
        std::wstring x = LR"(<Grid xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation" xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml">
<Grid.RowDefinitions><RowDefinition Height="Auto"/><RowDefinition Height="*"/><RowDefinition Height="Auto"/></Grid.RowDefinitions>
<Grid x:Name="TitleBar" Height="40" Background="Transparent"><StackPanel Orientation="Horizontal" Spacing="12" Margin="16,0,0,0" VerticalAlignment="Center">
<Image Source="ms-appx:///deskboxes.png" Width="16" Height="16"/><TextBlock Text=")" + esc(data.GetNamedString(L"title", L"")) + LR"(" Style="{StaticResource CaptionTextBlockStyle}" VerticalAlignment="Center"/></StackPanel></Grid>
<ColorPicker x:Name="Picker" Grid.Row="1" Margin="24,4,24,12" HorizontalAlignment="Center" ColorSpectrumShape="Ring" IsAlphaEnabled="False" IsMoreButtonVisible="False" IsColorChannelTextInputVisible="False" IsHexInputVisible="True" IsColorPreviewVisible="True" Color=")" + start + LR"("/>
<Grid Grid.Row="2" Padding="24,16" ColumnSpacing="8" Background="{ThemeResource LayerOnMicaBaseAltFillColorDefaultBrush}" BorderBrush="{ThemeResource CardStrokeColorDefaultBrush}" BorderThickness="0,1,0,0">
<Grid.ColumnDefinitions><ColumnDefinition Width="*"/><ColumnDefinition Width="*"/></Grid.ColumnDefinitions>
<Button x:Name="Ok" Style="{StaticResource AccentButtonStyle}" HorizontalAlignment="Stretch" Content=")" + esc(data.GetNamedString(L"ok", L"OK")) + LR"("/>
<Button x:Name="Cancel" Grid.Column="1" HorizontalAlignment="Stretch" Content=")" + esc(data.GetNamedString(L"cancel", L"Cancel")) + LR"("/>
</Grid></Grid>)";
        mux::FrameworkElement root{ nullptr };
        try { root = mux::Markup::XamlReader::Load(x).as<mux::FrameworkElement>(); }
        catch (winrt::hresult_error const& e) { Log(L"color picker: XAML failed: %s", e.message().c_str()); return; }
        gp.window = mux::Window();
        gp.window.SystemBackdrop(mux::Media::MicaBackdrop());
        gp.window.ExtendsContentIntoTitleBar(true);
        gp.window.Title(data.GetNamedString(L"title", L"Deskboxes"));
        gp.window.Content(root);
        gp.window.SetTitleBar(Find<mux::UIElement>(root, L"TitleBar"));
        auto aw = gp.window.AppWindow();
        HWND hwnd = winrt::Microsoft::UI::GetWindowFromWindowId(aw.Id());
        if (HICON ico = (HICON)LoadImageW(GetModuleHandleW(nullptr), MAKEINTRESOURCEW(1), IMAGE_ICON, 0, 0, LR_DEFAULTSIZE))
            aw.SetIcon(winrt::Microsoft::UI::GetIconIdFromIcon(ico));
        if (auto op = aw.Presenter().try_as<winrt::Microsoft::UI::Windowing::OverlappedPresenter>())
        {
            op.IsResizable(false);
            op.IsMaximizable(false);
            op.IsMinimizable(false);
        }
        // Next to the pointer (where the menu was), kept on its monitor's work area.
        POINT pt{}; GetCursorPos(&pt);
        HMONITOR mon = MonitorFromPoint(pt, MONITOR_DEFAULTTOPRIMARY);
        MONITORINFO mi{ sizeof(mi) }; GetMonitorInfoW(mon, &mi);
        UINT dpi = GetDpiForWindow(hwnd); if (!dpi) dpi = 96;
        int w = MulDiv(380, dpi, 96), h = MulDiv(560, dpi, 96);
        auto& wa = mi.rcWork;
        int left = std::clamp<int>(pt.x + 16, wa.left, wa.right - w), top = std::clamp<int>(pt.y - h / 3, wa.top, wa.bottom - h);
        aw.MoveAndResize({ left, top, w, h });
        if (auto pk = Find<muxc::ColorPicker>(root, L"Picker"))
            pk.ColorChanged([](auto&&, muxc::ColorChangedEventArgs const& e) { if (!gp.done) PickSend(Hex(e.NewColor()), false, false); });
        if (auto b = Find<muxc::Button>(root, L"Ok"))
            b.Click([root](auto&&, auto&&)
            {
                auto pk = Find<muxc::ColorPicker>(root, L"Picker");
                if (pk && !gp.done) PickSend(Hex(pk.Color()), true, false);
                gp.done = true;
                if (gp.window) gp.window.Close();
            });
        if (auto b = Find<muxc::Button>(root, L"Cancel"))
            b.Click([](auto&&, auto&&)
            {
                if (!gp.done) PickSend(gp.orig, true, true);
                gp.done = true;
                if (gp.window) gp.window.Close();
            });
        root.KeyDown([](auto&&, mux::Input::KeyRoutedEventArgs const& e)
        {
            if (e.Key() == winrt::Windows::System::VirtualKey::Escape)
            {
                if (!gp.done) PickSend(gp.orig, true, true);
                gp.done = true;
                if (gp.window) gp.window.Close();
            }
        });
        gp.window.Closed([](auto&&, auto&&)
        {
            if (!gp.done) PickSend(gp.orig, true, true);
            gp.done = true;
            gp.window = nullptr;
            TrimMemorySoon();
        });
        gp.window.Activate();
        SetForegroundWindow(hwnd);
        Log(L"color picker opened for %s (box %g)", gp.key.c_str(), gp.box);
    }

    void ShowSettings(JsonObject const& data, bool open, std::function<void(JsonObject)> send)
    {
        g.data = data;
        g.send = std::move(send);
        if (!g.window && !open) return;
        bool fresh = !g.window;
        if (fresh) MakeWindow();
        if (open)
        {
            // Opened for a page (Sorting rules...) or a box (its group in Boxes).
            if (data.HasKey(L"page") && data.GetNamedValue(L"page").ValueType() == JsonValueType::String)
                g.page = data.GetNamedString(L"page").c_str();
            if (data.HasKey(L"focusBox") && data.GetNamedValue(L"focusBox").ValueType() == JsonValueType::Number)
            {
                g.focusBox = (long long)data.GetNamedNumber(L"focusBox");
                g.expanded.insert(g.focusBox);
                g.page = L"boxes";
            }
        }
        auto title = g.data.GetNamedObject(L"strings", JsonObject()).GetNamedString(L"title", L"Deskboxes");
        g.window.Title(title);
        Build();
        if (open)
        {
            g.window.Activate();
            if (IsIconic(g.hwnd)) ShowWindow(g.hwnd, SW_RESTORE);
            SetForegroundWindow(g.hwnd);
        }
        Log(L"settings %s (%u boxes)", fresh ? L"opened" : L"refreshed", g.data.GetNamedArray(L"boxes", JsonArray()).Size());
    }
}

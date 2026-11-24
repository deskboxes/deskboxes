// The settings window: WinUI 3 with a real Mica backdrop, laid out like Windows Settings (pages
// in a navigation pane; cards with a label on the left and the control on the right). Built from a XAML string so theme
// resources follow light/dark; events are wired by element name. All values live in the core;
// every change is sent back at once and applied live on the desktop.

#include "pch.h"
#include "Settings.h"
#include "Log.h"
#include <algorithm>
#include <cmath>
#include <limits>
#include <dwmapi.h>

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
            std::wstring lastSig;            // the data last built, without the download progress
            // A page switch waiting for the old page to fade out (kept here: a timer nothing
            // holds is released at once and never ticks).
            winrt::Microsoft::UI::Dispatching::DispatcherQueueTimer switchTimer{ nullptr };
            // Finding a setting (the box at the top of the navigation): every page's cards and
            // headings, gathered by building the pages' text once (`indexing`).
            struct Entry { std::wstring page, title, desc; bool isPage = false; };
            std::vector<Entry> index;
            bool indexing = false;
            std::wstring indexPage;
            std::vector<Entry> hits;          // the suggestions shown, in their order
            std::wstring findTitle;           // a card to bring into view once its page is shown
            int findTries = 0;
            winrt::Microsoft::UI::Dispatching::DispatcherQueueTimer findTimer{ nullptr };
            winrt::Microsoft::UI::Dispatching::DispatcherQueueTimer flashTimer{ nullptr };
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

        // Esc's text back (for showing it outside XAML markup, and for comparing).
        std::wstring Unesc(std::wstring s)
        {
            const std::pair<const wchar_t*, const wchar_t*> ents[] = { { L"&lt;", L"<" }, { L"&gt;", L">" }, { L"&quot;", L"\"" }, { L"&apos;", L"'" }, { L"&amp;", L"&" } };
            for (auto const& [from, to] : ents)
                for (size_t i = s.find(from); i != std::wstring::npos; i = s.find(from, i + wcslen(to)))
                    s.replace(i, wcslen(from), to);
            return s;
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
            if (g.indexing) g.index.push_back({ g.indexPage, Unesc(title), Unesc(desc) });
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
            if (g.indexing) g.index.push_back({ g.indexPage, Unesc(title), L"" });
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
                 Opt(L"link", Str(L"kindLink"), kind == L"link") + Opt(L"ext", Str(L"kindExt"), kind == L"ext") + Opt(L"age", Str(L"kindAge"), kind == L"age") +
                 Opt(L"size", Str(L"kindSize"), kind == L"size") + L"</ComboBox>";
            if (kind == L"name" || kind == L"link")
            {
                bool link = kind == L"link";
                x += L"<ComboBox x:Name=\"" + p + L"Op\" MinWidth=\"130\">" + Opt(L"contains", Str(L"opContains"), op == L"contains") + Opt(L"starts", Str(L"opStarts"), op == L"starts") +
                     Opt(L"ends", Str(L"opEnds"), op == L"ends") + Opt(L"is", Str(L"opIs"), op == L"is") + L"</ComboBox>";
                // (A shortcut target is hard to guess: the hover note shows what one looks like.)
                x += L"<TextBox x:Name=\"" + p + L"Val\" Width=\"" + std::wstring(link ? L"200" : L"160") + L"\" PlaceholderText=\"" + Str(link ? L"linkHint" : L"textHint") + L"\"" +
                     (link ? L" ToolTipService.ToolTip=\"" + Str(L"linkNote") + L"\"" : std::wstring()) + L" Text=\"" + val + L"\"/>";
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
            x += Section(Str(L"about"));
            x += Card(L"&#xE7E8;", Str(L"disable"), Str(L"disableDesc"), L"<Button x:Name=\"Disable\" Content=\"" + Str(L"disableButton") + L"\" MinWidth=\"120\"/>");
            x += L"<TextBlock Text=\"Deskboxes " + Esc(std::wstring(app.GetNamedString(L"version", L"").c_str())) + L"\" Style=\"{StaticResource CaptionTextBlockStyle}\" Foreground=\"{ThemeResource TextFillColorSecondaryBrush}\" Margin=\"1,12,0,0\"/>";
            return x;
        }

        std::wstring Fill(std::wstring s, const wchar_t* k, const std::wstring& v)
        {
            auto i = s.find(k);
            if (i != std::wstring::npos) s.replace(i, wcslen(k), v);
            return s;
        }

        // "today, 14:02", "yesterday, 9:15", or the date in the user's format and the time.
        std::wstring WhenText(double unix)
        {
            ULONGLONG t = (ULONGLONG)unix * 10000000ULL + 116444736000000000ULL;
            FILETIME ft{ (DWORD)t, (DWORD)(t >> 32) }, lft{};
            SYSTEMTIME st{}, now{};
            FileTimeToLocalFileTime(&ft, &lft);
            FileTimeToSystemTime(&lft, &st);
            GetLocalTime(&now);
            wchar_t tm[64] = {}, dt[128] = {};
            GetTimeFormatEx(LOCALE_NAME_USER_DEFAULT, TIME_NOSECONDS, &st, nullptr, tm, 64);
            // Days apart, by calendar date.
            auto day = [](SYSTEMTIME s)
            {
                s.wHour = s.wMinute = s.wSecond = s.wMilliseconds = 0;
                FILETIME f{};
                SystemTimeToFileTime(&s, &f);
                return (((ULONGLONG)f.dwHighDateTime << 32) | f.dwLowDateTime) / 864000000000ULL;
            };
            ULONGLONG a = day(st), b = day(now);
            if (a == b) return Str(L"updToday") + L", " + tm;
            if (a + 1 == b) return Str(L"updYesterday") + L", " + tm;
            GetDateFormatEx(LOCALE_NAME_USER_DEFAULT, DATE_SHORTDATE, &st, nullptr, dt, 128, nullptr);
            return std::wstring(dt) + L", " + tm;
        }

        // "2026-11-19" in the user's long date format.
        std::wstring LongDate(const std::wstring& iso)
        {
            SYSTEMTIME st{};
            if (iso.size() < 10 || swscanf_s(iso.c_str(), L"%hu-%hu-%hu", &st.wYear, &st.wMonth, &st.wDay) != 3) return L"";
            wchar_t dt[128] = {};
            if (!GetDateFormatEx(LOCALE_NAME_USER_DEFAULT, DATE_LONGDATE, &st, nullptr, dt, 128, nullptr)) return iso.substr(0, 10);
            return dt;
        }

        // Release notes (Markdown as written on GitHub) as text blocks: headings, bullets and
        // paragraphs; emphasis marks and link targets are left out.
        std::wstring NotesXaml(const std::wstring& md)
        {
            auto inl = [](const std::wstring& t)
            {
                std::wstring o;
                for (size_t i = 0; i < t.size(); i++)
                {
                    wchar_t c = t[i];
                    if (c == L'*' || c == L'`') continue;
                    if (c == L'_' && i + 1 < t.size() && t[i + 1] == L'_') { i++; continue; }
                    if (c == L'[')
                    {
                        size_t close = t.find(L"](", i);
                        size_t end = close == std::wstring::npos ? close : t.find(L')', close);
                        if (end != std::wstring::npos) { o += t.substr(i + 1, close - i - 1); i = end; continue; }
                    }
                    o += c;
                }
                return Esc(o);
            };
            std::wstring x, para;
            auto flush = [&]
            {
                if (!para.empty())
                    x += L"<TextBlock Text=\"" + inl(para) + L"\" TextWrapping=\"Wrap\" IsTextSelectionEnabled=\"True\"/>";
                para.clear();
            };
            size_t at = 0;
            int lines = 0;
            while (at <= md.size() && lines < 150)
            {
                size_t e = md.find(L'\n', at);
                if (e == std::wstring::npos) e = md.size();
                std::wstring ln = md.substr(at, e - at);
                at = e + 1;
                lines++;
                while (!ln.empty() && (ln.back() == L'\r' || ln.back() == L' ')) ln.pop_back();
                size_t lead = ln.find_first_not_of(L' ');
                if (lead == std::wstring::npos) { flush(); continue; }
                std::wstring t = ln.substr(lead);
                // Not for a text page: tables (the installer hashes), HTML, pictures and rules;
                // and the hashes' own heading.
                if (t[0] == L'|' || t[0] == L'<' || t.rfind(L"![", 0) == 0 || t == L"---" || t == L"***" || t == L"___" ||
                    (t[0] == L'#' && t.find(L"Installer hash") != std::wstring::npos))
                {
                    flush();
                    continue;
                }
                if (t[0] == L'#')
                {
                    flush();
                    size_t f = t.find_first_not_of(L"# ");
                    if (f == std::wstring::npos) continue;
                    x += L"<TextBlock Text=\"" + inl(t.substr(f)) + L"\" Style=\"{StaticResource BodyStrongTextBlockStyle}\" TextWrapping=\"Wrap\" Margin=\"0,8,0,0\"/>";
                }
                else if (t.size() > 1 && (t[0] == L'-' || t[0] == L'*' || t[0] == L'+' || t[0] == 0x2022) && t[1] == L' ')
                {
                    flush();
                    x += L"<Grid ColumnSpacing=\"10\" Margin=\"" + std::to_wstring(lead >= 2 ? 22 : 4) + L",0,0,0\"><Grid.ColumnDefinitions><ColumnDefinition Width=\"Auto\"/><ColumnDefinition Width=\"*\"/></Grid.ColumnDefinitions>"
                         L"<TextBlock Text=\"&#x2022;\" Foreground=\"{ThemeResource TextFillColorSecondaryBrush}\"/><TextBlock Grid.Column=\"1\" Text=\"" + inl(t.substr(2)) + L"\" TextWrapping=\"Wrap\" IsTextSelectionEnabled=\"True\"/></Grid>";
                }
                else
                    para += (para.empty() ? L"" : L" ") + t;
            }
            flush();
            return x;
        }

        // Updates, as Windows Update shows them: what the updater is doing (with its progress and
        // "Restart now"), what changed in the newest version, and the options.
        std::wstring PageUpdates()
        {
            auto up = g.data.GetNamedObject(L"update", JsonObject());
            auto st = up.GetNamedObject(L"status", JsonObject());
            std::wstring state = st.GetNamedString(L"state", L"").c_str();
            std::wstring latest = Esc(std::wstring(st.GetNamedString(L"latest", L"").c_str()));
            std::wstring version = Esc(std::wstring(up.GetNamedString(L"version", L"").c_str()));
            bool dev = state == L"dev", busy = state == L"checking" || state == L"downloading", ready = state == L"ready";
            std::wstring title = state == L"checking" ? Str(L"updChecking")
                               : state == L"downloading" ? Fill(Str(L"updDownloading"), L"{v}", latest)
                               : ready ? Fill(Str(L"updReady"), L"{v}", latest)
                               : state == L"error" ? Str(L"updError")
                               : dev ? Str(L"updDev")
                               : state == L"uptodate" ? Str(L"updUpToDateTitle")
                               : Str(L"updNever");
            std::wstring more;
            if (ready) more = Str(L"updReadyDesc");
            else if (state == L"uptodate") more = Fill(Str(L"updUpToDate"), L"{v}", version);
            else if (dev) more = Str(L"updDevDesc");
            else if (state == L"error")
            {
                std::wstring k = st.GetNamedString(L"error", L"").c_str();
                more = Str(k == L"net" ? L"updErrNet" : k == L"none" ? L"updErrNone" : k == L"server" ? L"updErrServer" : k == L"file" ? L"updErrFile" : L"updErrOther");
            }
            double checked = st.GetNamedNumber(L"checked", 0);
            std::wstring when = checked > 0 && !dev ? Fill(Str(L"updChecked"), L"{t}", WhenText(checked)) : L"";
            // The picture on the left: a turning ring while busy, otherwise what the state is.
            std::wstring icon;
            if (busy)
                icon = L"<ProgressRing IsActive=\"True\" Width=\"36\" Height=\"36\"/>";
            else
            {
                const wchar_t* glyph = ready ? L"&#xE896;" : state == L"uptodate" ? L"&#xE930;" : state == L"error" ? L"&#xE7BA;" : L"&#xE895;";
                const wchar_t* brush = ready ? L"AccentTextFillColorPrimaryBrush" : state == L"uptodate" ? L"SystemFillColorSuccessBrush" : state == L"error" ? L"SystemFillColorCautionBrush" : L"TextFillColorSecondaryBrush";
                icon = std::wstring(L"<FontIcon Glyph=\"") + glyph + L"\" FontSize=\"36\" Foreground=\"{ThemeResource " + brush + L"}\"/>";
            }
            std::wstring x = LR"(<Border Background="{ThemeResource CardBackgroundFillColorDefaultBrush}" BorderBrush="{ThemeResource CardStrokeColorDefaultBrush}" BorderThickness="1" CornerRadius="8" Padding="20,18">
  <Grid ColumnSpacing="18"><Grid.ColumnDefinitions><ColumnDefinition Width="Auto"/><ColumnDefinition Width="*"/><ColumnDefinition Width="Auto"/></Grid.ColumnDefinitions>)";
            x += L"<Grid Width=\"48\" Height=\"48\" VerticalAlignment=\"Center\">" + icon + L"</Grid>";
            x += L"<StackPanel Grid.Column=\"1\" VerticalAlignment=\"Center\" Spacing=\"2\"><TextBlock Text=\"" + title + L"\" Style=\"{StaticResource SubtitleTextBlockStyle}\" TextWrapping=\"Wrap\"/>";
            if (!more.empty()) x += L"<TextBlock Text=\"" + more + L"\" Foreground=\"{ThemeResource TextFillColorSecondaryBrush}\" TextWrapping=\"Wrap\"/>";
            if (!when.empty()) x += L"<TextBlock Text=\"" + when + L"\" Style=\"{StaticResource CaptionTextBlockStyle}\" Foreground=\"{ThemeResource TextFillColorSecondaryBrush}\"/>";
            if (state == L"downloading")
            {
                double pct = std::round(st.GetNamedNumber(L"progress", 0) * 100.0);
                x += L"<Grid ColumnSpacing=\"12\" Margin=\"0,10,0,0\"><Grid.ColumnDefinitions><ColumnDefinition Width=\"*\"/><ColumnDefinition Width=\"Auto\"/></Grid.ColumnDefinitions>"
                     L"<ProgressBar x:Name=\"UpdBar\" Maximum=\"100\" Value=\"" + Num(pct) + L"\" VerticalAlignment=\"Center\"" + (pct <= 0 ? L" IsIndeterminate=\"True\"" : L"") + L"/>"
                     L"<TextBlock x:Name=\"UpdPct\" Grid.Column=\"1\" Text=\"" + Num(pct) + L"%\" Style=\"{StaticResource CaptionTextBlockStyle}\" MinWidth=\"32\" TextAlignment=\"Right\"/></Grid>";
            }
            x += L"</StackPanel><StackPanel Grid.Column=\"2\" Orientation=\"Horizontal\" Spacing=\"8\" VerticalAlignment=\"Center\">";
            if (ready) x += L"<Button x:Name=\"UpdateInstall\" Style=\"{StaticResource AccentButtonStyle}\" Content=\"" + Str(L"updRestart") + L"\" MinWidth=\"120\"/>";
            else x += L"<Button x:Name=\"UpdateNow\" Content=\"" + Str(L"updNow") + L"\" MinWidth=\"120\"" + (dev || busy ? L" IsEnabled=\"False\"" : L"") + L"/>";
            x += L"</StackPanel></Grid></Border>";

            // What's new.
            std::wstring notes = st.GetNamedString(L"notes", L"").c_str();
            std::wstring nv = Esc(std::wstring(st.GetNamedString(L"notes_version", L"").c_str()));
            std::wstring date = LongDate(st.GetNamedString(L"notes_date", L"").c_str());
            std::wstring head = Str(L"updWhatsNew");
            x += Section(nv.empty() ? Str(L"updWhatsNewHead") : Fill(head, L"{v}", nv));
            x += LR"(<Border Background="{ThemeResource CardBackgroundFillColorDefaultBrush}" BorderBrush="{ThemeResource CardStrokeColorDefaultBrush}" BorderThickness="1" CornerRadius="8" Padding="20,14,20,16"><StackPanel Spacing="6">)";
            if (!date.empty()) x += L"<TextBlock Text=\"" + Esc(date) + L"\" Style=\"{StaticResource CaptionTextBlockStyle}\" Foreground=\"{ThemeResource TextFillColorSecondaryBrush}\"/>";
            std::wstring body = NotesXaml(notes);
            x += body.empty() ? L"<TextBlock Text=\"" + Str(L"updNoNotes") + L"\" Foreground=\"{ThemeResource TextFillColorSecondaryBrush}\" TextWrapping=\"Wrap\"/>" : body;
            x += L"</StackPanel></Border>";

            // Options.
            x += Section(Str(L"updOptions"));
            x += Card(L"&#xE895;", Str(L"updAuto"), Str(L"updAutoDesc"), Toggle(L"UpdateAuto", up.GetNamedBoolean(L"auto", true)));
            x += Card(L"&#xE946;", Str(L"updInstalled"), L"Deskboxes " + version,
                      L"<HyperlinkButton Content=\"" + Str(L"updAllReleases") + L"\" NavigateUri=\"https://github.com/deskboxes/deskboxes/releases\"/>");
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
            // Without rules, Sort now makes boxes by type (with a rule for each) instead of doing nothing.
            bool none = so.GetNamedArray(L"rules", JsonArray()).Size() == 0;
            x += Card(L"&#xE8CB;", Str(none ? L"sortTemplate" : L"sortNow"), Str(none ? L"sortTemplateDesc" : L"sortNowDesc"), L"<Button x:Name=\"SortNow\" Style=\"{StaticResource AccentButtonStyle}\" Content=\"" + Str(none ? L"sortTemplateButton" : L"sortNowButton") + L"\" MinWidth=\"120\"/>");
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
                      L"<Slider x:Name=\"IconTintS\" Minimum=\"10\" Maximum=\"100\" StepFrequency=\"5\" SnapsTo=\"StepValues\" Width=\"220\" Value=\"" + Num(std::round(ic.GetNamedNumber(L"tintStrength", 1.0) * 100.0)) + L"\"/>");
            x += Card(L"&#xE7F4;", Str(L"iconsDesktop"), Str(L"iconsDesktopDesc"), Toggle(L"IconDesk", ic.GetNamedBoolean(L"desktop", false)));
            x += Section(Str(L"eachBox"));
            auto boxes = g.data.GetNamedArray(L"boxes", JsonArray());
            if (boxes.Size() == 0) x += Note(Str(L"noBoxes"));
            for (auto const& b : boxes)
            {
                auto o = b.GetObject();
                std::wstring id = std::to_wstring((long long)o.GetNamedNumber(L"id"));
                x += Card(L"&#xE8A9;", Esc(std::wstring(o.GetNamedString(L"title", L"").c_str())), L"", TintBox(L"Tint" + id, o.GetNamedString(L"tint", L"").c_str(), true));
            }
            return x;
        }

        // Blend into wallpaper: its own page (it was on Icons, under the tint, and the Home tile
        // named after it opened Icons, round 11).
        std::wstring PageBlend()
        {
            auto ic = g.data.GetNamedObject(L"icons", JsonObject());
            std::wstring x = Card(L"&#xE81E;", Str(L"blend"), Str(L"blendDesc"), Toggle(L"IconBlend", ic.GetNamedBoolean(L"blend", false)));
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
                x += Card(L"&#xE8A9;", Esc(std::wstring(o.GetNamedString(L"title", L"").c_str())), L"",
                          L"<ComboBox x:Name=\"Blend" + id + L"\" MinWidth=\"210\">" + Opt(L"", Str(L"blendAll"), bl.empty()) + Opt(L"on", Str(L"blendOn"), bl == L"on") + Opt(L"off", Str(L"blendOff"), bl == L"off") + L"</ComboBox>");
            }
            return x;
        }

        // About: what Deskboxes is, how it works and why it is quick (round 11).
        std::wstring PageAbout()
        {
            auto app = g.data.GetNamedObject(L"app", JsonObject());
            std::wstring x = Note(Str(L"aboutIntro"));
            auto part = [&](const wchar_t* glyph, const wchar_t* head, const wchar_t* body)
            {
                x += L"<Border Background=\"{ThemeResource CardBackgroundFillColorDefaultBrush}\" BorderBrush=\"{ThemeResource CardStrokeColorDefaultBrush}\" BorderThickness=\"1\" CornerRadius=\"8\" Padding=\"20,16\">"
                     L"<Grid ColumnSpacing=\"16\"><Grid.ColumnDefinitions><ColumnDefinition Width=\"Auto\"/><ColumnDefinition Width=\"*\"/></Grid.ColumnDefinitions>"
                     L"<FontIcon Glyph=\"" + std::wstring(glyph) + L"\" FontSize=\"20\" VerticalAlignment=\"Top\" Margin=\"0,2,0,0\" Foreground=\"{ThemeResource AccentTextFillColorPrimaryBrush}\"/>"
                     L"<StackPanel Grid.Column=\"1\" Spacing=\"4\"><TextBlock Text=\"" + Str(head) + L"\" Style=\"{StaticResource BodyStrongTextBlockStyle}\"/>"
                     L"<TextBlock Text=\"" + Str(body) + L"\" TextWrapping=\"Wrap\" Foreground=\"{ThemeResource TextFillColorSecondaryBrush}\"/></StackPanel></Grid></Border>";
            };
            x += Section(Str(L"aboutHowHead"));
            part(L"&#xE8A9;", L"aboutFilesHead", L"aboutFiles");
            part(L"&#xE7F4;", L"aboutDrawHead", L"aboutDraw");
            part(L"&#xE8B7;", L"aboutShellHead", L"aboutShell");
            x += Section(Str(L"aboutFastHead"));
            part(L"&#xE945;", L"aboutRustHead", L"aboutRust");
            part(L"&#xE9D9;", L"aboutGpuHead", L"aboutGpu");
            part(L"&#xE916;", L"aboutLazyHead", L"aboutLazy");
            part(L"&#xE72E;", L"aboutSafeHead", L"aboutSafe");
            x += L"<TextBlock Text=\"Deskboxes " + Esc(std::wstring(app.GetNamedString(L"version", L"").c_str())) + L"\" Style=\"{StaticResource CaptionTextBlockStyle}\" Foreground=\"{ThemeResource TextFillColorSecondaryBrush}\" Margin=\"1,12,0,0\"/>";
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

        // Desktop pages: on or off, the pages themselves, how to go from page to page, the slide.
        std::wstring PagePages()
        {
            auto p = g.data.GetNamedObject(L"pages", JsonObject());
            auto on = [&](const wchar_t* k) { return p.GetNamedBoolean(k, true); };
            std::wstring x = Card(L"&#xE7C4;", Str(L"pagesOn"), Str(L"pagesOnDesc"), Toggle(L"PgOn", on(L"enabled")));
            // The pages, in order: show, rename, move, delete; and a new one.
            x += Section(Str(L"pagesList"));
            auto list = p.GetNamedArray(L"list", JsonArray());
            double cur = p.GetNamedNumber(L"current", 0);
            uint32_t n = list.Size();
            for (uint32_t i = 0; i < n; ++i)
            {
                auto o = list.GetObjectAt(i);
                double id = o.GetNamedNumber(L"id");
                std::wstring s = std::to_wstring((long long)id);
                std::wstring desc = Fill(Fill(Str(L"pagesCount"), L"{b}", std::to_wstring((long long)o.GetNamedNumber(L"boxes"))), L"{i}", std::to_wstring((long long)o.GetNamedNumber(L"icons")));
                if (id == cur) desc = Str(L"pagesCurrent") + L"  \x00B7  " + desc;
                std::wstring ctl = L"<StackPanel Orientation=\"Horizontal\" Spacing=\"4\">";
                ctl += id == cur ? L"<Border Width=\"80\"/>" : L"<Button x:Name=\"PgGo" + s + L"\" Content=\"" + Str(L"pagesGo") + L"\" MinWidth=\"80\"/>";
                ctl += IconButton(L"PgRen" + s, L"&#xE8AC;", Str(L"pagesRename"), true);
                ctl += IconButton(L"PgUp" + s, L"&#xE70E;", Str(L"pagesUp"), i > 0);
                ctl += IconButton(L"PgDown" + s, L"&#xE70D;", Str(L"pagesDown"), i + 1 < n);
                ctl += IconButton(L"PgDel" + s, L"&#xE74D;", Str(L"pagesDelete"), n > 1);
                ctl += L"</StackPanel>";
                x += Card(id == cur ? L"&#xE7C4;" : L"&#xE737;", std::to_wstring(i + 1) + L".  " + Esc(std::wstring(o.GetNamedString(L"name", L"").c_str())), desc, ctl);
            }
            x += L"<Button x:Name=\"PgNew\" Margin=\"0,4,0,0\" Padding=\"14,8\"><StackPanel Orientation=\"Horizontal\" Spacing=\"8\"><FontIcon Glyph=\"&#xE710;\" FontSize=\"14\"/><TextBlock Text=\"" + Str(L"pagesNew") + L"\"/></StackPanel></Button>";
            std::wstring nf = p.GetNamedString(L"newFiles", L"current").c_str();
            x += Card(L"&#xE8E5;", Str(L"pagesNewFiles"), Str(L"pagesNewFilesDesc"),
                      L"<ComboBox x:Name=\"PgNewFiles\" MinWidth=\"220\">" + Opt(L"current", Str(L"pagesNewCurrent"), nf != L"first") + Opt(L"first", Str(L"pagesNewFirst"), nf == L"first") + L"</ComboBox>");
            // Going from page to page.
            x += Section(Str(L"pagesSwitch"));
            x += Card(L"&#xE765;", Str(L"pagesAltKeys"), Str(L"pagesAltKeysDesc"), Toggle(L"PgAlt", on(L"altKeys")));
            x += Card(L"&#xE7C9;", Str(L"pagesSwipe"), Str(L"pagesSwipeDesc"), Toggle(L"PgSwipe", on(L"swipe")));
            x += Card(L"&#xE962;", Str(L"pagesMiddle"), L"", Toggle(L"PgMiddle", on(L"middleDrag")));
            x += Card(L"&#xE962;", Str(L"pagesBoth"), L"", Toggle(L"PgBoth", on(L"bothDrag")));
            x += Card(L"&#xE8A0;", Str(L"pagesEdgePull"), L"", Toggle(L"PgEdgePull", on(L"edgePull")));
            x += Card(L"&#xE8AB;", Str(L"pagesEdgeDrag"), Str(L"pagesEdgeDragDesc"), Toggle(L"PgEdgeDrag", on(L"edgeDrag")));
            // The slide.
            x += Section(Str(L"pagesLook"));
            x += Card(L"&#xE916;", Str(L"pagesAnimate"), Str(L"pagesAnimateDesc"), Toggle(L"PgAnimate", on(L"animate")));
            x += Card(L"&#xE945;", Str(L"pagesBounce"), L"", Toggle(L"PgBounce", on(L"bounce")));
            x += Card(L"&#xE8EC;", Str(L"pagesShowName"), Str(L"pagesShowNameDesc"), Toggle(L"PgShowName", on(L"showName")));
            // Several screens (only once a second one was seen).
            if (p.GetNamedNumber(L"screens", 1) > 1)
            {
                x += Section(Str(L"pagesScreens"));
                x += Note(Str(L"pagesScreensNote"));
                x += Card(L"&#xE7F4;", Str(L"pagesScreenNotice"), Str(L"pagesScreenNoticeDesc"), Toggle(L"PgScreenNotice", on(L"screenNotice")));
            }
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
            // Everything to one file, and back (another PC).
            x += Section(Str(L"transfer"));
            x += Card(L"&#xEDE1;", Str(L"exportAll"), Str(L"exportAllDesc"), L"<Button x:Name=\"ExportAll\" Content=\"" + Str(L"exportButton") + L"\" MinWidth=\"120\"/>");
            x += Card(L"&#xE8B5;", Str(L"importAll"), Str(L"importAllDesc"), L"<Button x:Name=\"ImportAll\" Content=\"" + Str(L"importButton") + L"\" MinWidth=\"120\"/>");
            return x;
        }

        std::wstring PageSearch()
        {
            auto se = g.data.GetNamedObject(L"search", JsonObject());
            std::wstring x = Card(L"&#xE721;", Str(L"searchField"), Str(L"searchFieldDesc"), Toggle(L"SearchField", se.GetNamedBoolean(L"field", true)));
            x += Card(L"&#xE765;", Str(L"typeSearch"), Str(L"typeSearchDesc"), Toggle(L"TypeSearch", se.GetNamedBoolean(L"typeToSearch", true)));
            x += Card(L"&#xE8FD;", Str(L"searchResults"), Str(L"searchResultsDesc"), Toggle(L"SearchResults", se.GetNamedBoolean(L"results", true)));
            x += Card(L"&#xED1A;", Str(L"searchHideToggle"), Str(L"searchHideToggleDesc"), Toggle(L"SearchHideToggle", se.GetNamedBoolean(L"hideToggle", true)));
            std::wstring pos = se.GetNamedString(L"position", L"bottom").c_str();
            x += Card(L"&#xE8A0;", Str(L"searchPosition"), Str(L"searchPositionDesc"),
                      L"<ComboBox x:Name=\"SearchPosition\" MinWidth=\"220\">" + Opt(L"bottom", Str(L"searchBottom"), pos != L"top") + Opt(L"top", Str(L"searchTop"), pos == L"top") + L"</ComboBox>");
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

        std::wstring ExeDir()
        {
            wchar_t p[MAX_PATH]{};
            GetModuleFileNameW(nullptr, p, MAX_PATH);
            std::wstring s = p;
            return s.substr(0, s.find_last_of(L'\\'));
        }

        // The clip of a feature (settings/CLIP.mp4, CLIP_l.mp4 in the light theme), if there is one.
        bool HasClip(const wchar_t* clip)
        {
            return clip && GetFileAttributesW((ExeDir() + L"\\settings\\" + clip + L".mp4").c_str()) != INVALID_FILE_ATTRIBUTES;
        }

        // A feature's picture, filling a rounded frame: res/settings/NAME.jpg in the dark theme,
        // NAME_l.jpg in the light one (drawn scenes, made by tools/art). `corner`: the CornerRadius.
        // `clip` (with `key`, naming the elements Clip_KEY and ClipG_KEY): the feature's clip, which
        // WireClip plays over the picture while the pointer is on it; its first frame is the picture.
        std::wstring Pic(const std::wstring& name, const std::wstring& size, const wchar_t* corner = L"8", const wchar_t* clip = nullptr, const std::wstring& key = L"")
        {
            auto img = [&](const wchar_t* theme, const std::wstring& file)
            { return std::wstring(L"<ResourceDictionary x:Key=\"") + theme + L"\"><BitmapImage x:Key=\"Pic\" UriSource=\"ms-appx:///settings/" + file + L".jpg\"/></ResourceDictionary>"; };
            bool anim = HasClip(clip);
            // The picture; with a clip, it lies over the video (named Still_KEY) covering exactly
            // what the video does (inside the outline, which is drawn apart over both), so nothing
            // moves when one gives way to the other.
            std::wstring x = L"<Border " + (anim ? L"x:Name=\"Still_" + key + L"\" IsHitTestVisible=\"False\" Margin=\"1\"" : size) + L" CornerRadius=\"" + corner + L"\"" +
                   (anim ? L"" : L" BorderBrush=\"{ThemeResource CardStrokeColorDefaultBrush}\" BorderThickness=\"1\"") + L">"
                   L"<Border.Resources><ResourceDictionary><ResourceDictionary.ThemeDictionaries>" + img(L"Default", name) + img(L"Dark", name) + img(L"Light", name + L"_l") +
                   L"</ResourceDictionary.ThemeDictionaries></ResourceDictionary></Border.Resources>"
                   L"<Border.Background><ImageBrush ImageSource=\"{ThemeResource Pic}\" Stretch=\"UniformToFill\" AlignmentY=\"Center\"/></Border.Background></Border>";
            if (!anim) return x;
            // The corners go to WireClip in the tag (video is not clipped by a CornerRadius): CLIP;top;bottom.
            // The video stays shown under the picture all along: a video element that turns visible
            // shows black until its next frame (the flash on hover), so the picture fades instead.
            std::wstring c = corner;
            std::wstring top = c.substr(0, c.find(L',')), bottom = c.find(L',') == std::wstring::npos ? c : c.substr(c.rfind(L',') + 1);
            // The video is transparent until it has a frame to show (ClipState::Cover): before
            // that it is black, which showed while the picture over it was still being decoded.
            return L"<Grid x:Name=\"ClipG_" + key + L"\" " + size + L">" +
                   L"<Border Margin=\"1\" CornerRadius=\"" + corner + L"\" Background=\"{ThemeResource CardBackgroundFillColorSecondaryBrush}\"/>"
                   L"<Grid Margin=\"1\" IsHitTestVisible=\"False\"><MediaPlayerElement x:Name=\"Clip_" + key + L"\" Tag=\"" + clip + L";" + top + L";" + bottom + L"\" Stretch=\"UniformToFill\""
                   L" Opacity=\"0\" AreTransportControlsEnabled=\"False\" AutoPlay=\"False\"/></Grid>" + x +
                   L"<Border IsHitTestVisible=\"False\" CornerRadius=\"" + corner + L"\" BorderBrush=\"{ThemeResource CardStrokeColorDefaultBrush}\" BorderThickness=\"1\"/></Grid>";
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
            { L"blend", L"blend", L"blend", L"tileBlend", L"icons", L"blend" },
            { L"colors", L"colors", L"pageColors", L"tileColors", nullptr, nullptr },
            { L"folder", L"folder", L"pageFolder", L"tileFolder", nullptr, nullptr },
            { L"above", L"above", L"pageAbove", L"tileAbove", nullptr, nullptr },
            { L"hide", L"hide", L"pageHide", L"tileHide", L"hide", L"doubleClick" },
            { L"saved", L"saved", L"pageSaved", L"tileSaved", nullptr, nullptr },
            { L"pages", L"pages", L"pagePages", L"tilePages", L"pages", L"enabled" },
            { L"search", L"search", L"pageSearch", L"tileSearch", L"search", L"field" },
            { L"locks", L"locks", L"pageLocks", L"tileLocks", L"app", L"lockAll" },
            { L"privacy", L"privacy", L"pagePrivacy", L"tilePrivacy", L"app", L"privacy" },
        };

        bool TileOn(const Tile& t)
        {
            if (!t.scope) return false;
            std::wstring sc = t.scope, k = t.key;
            if (sc == L"app") return g.data.GetNamedObject(L"app", JsonObject()).GetNamedBoolean(k == L"lockAll" ? L"lockAll" : L"privacy", k != L"lockAll");
            bool def = sc == L"search" || sc == L"pages";
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
            // A downloaded update: say so here too, with its button.
            {
                auto st = g.data.GetNamedObject(L"update", JsonObject()).GetNamedObject(L"status", JsonObject());
                if (st.GetNamedString(L"state", L"") == L"ready")
                    x += L"<InfoBar IsOpen=\"True\" IsClosable=\"False\" Severity=\"Informational\" Margin=\"0,0,0,8\" Title=\"" +
                         Fill(Str(L"updReady"), L"{v}", Esc(st.GetNamedString(L"latest", L"").c_str())) + L"\" Message=\"" + Str(L"updReadyDesc") +
                         L"\"><InfoBar.ActionButton><Button x:Name=\"HomeUpdate\" Style=\"{StaticResource AccentButtonStyle}\" Content=\"" + Str(L"updRestart") + L"\"/></InfoBar.ActionButton></InfoBar>";
            }
            // Quick actions.
            auto action = [](const wchar_t* name, const wchar_t* glyph, const wchar_t* text, bool accent)
            {
                return std::wstring(L"<Button x:Name=\"") + name + L"\"" + (accent ? L" Style=\"{StaticResource AccentButtonStyle}\"" : L"") +
                       L" Padding=\"14,8\"><StackPanel Orientation=\"Horizontal\" Spacing=\"8\"><FontIcon Glyph=\"" + glyph + L"\" FontSize=\"14\"/><TextBlock Text=\"" + Str(text) + L"\"/></StackPanel></Button>";
            };
            x += L"<StackPanel Orientation=\"Horizontal\" Spacing=\"8\" Margin=\"0,4,0,0\">" + action(L"HomeSort", L"&#xE8CB;", g.data.GetNamedObject(L"sorting", JsonObject()).GetNamedArray(L"rules", JsonArray()).Size() ? L"sortNowButtonLong" : L"sortTemplateButtonLong", true) +
                 action(L"HomeSave", L"&#xE74E;", L"saveNow", false) + action(L"HomeRules", L"&#xE71C;", L"pageRules", false) + L"</StackPanel>";
            // Features.
            x += Section(Str(L"navFeatures"));
            // The tiles fill the row (Wire sets ItemWidth from the width; at least 230 each), so
            // their right edge lines up with the banner's. The -8 takes back the last one's gap.
            x += L"<VariableSizedWrapGrid x:Name=\"Tiles\" Orientation=\"Horizontal\" ItemWidth=\"230\" ItemHeight=\"206\" Margin=\"0,0,-8,0\">";
            for (auto const& t : kTiles)
            {
                std::wstring tag = t.page;
                // The button is the tile's background and click target; the picture and the text lie
                // over it and let clicks through. The switch (not in the button, so switching it doesn't
                // open the page) has its own column in the title row, so it never covers the
                // description however the text wraps (it was placed over the tile at a fixed spot and
                // ran into the description with larger text, round 10).
                x += L"<Grid x:Name=\"TileG_" + tag + L"\" Margin=\"0,0,8,8\">"
                     L"<Button x:Name=\"Tile_" + tag + L"\" Padding=\"0\" HorizontalAlignment=\"Stretch\" VerticalAlignment=\"Stretch\""
                     L" Background=\"{ThemeResource CardBackgroundFillColorDefaultBrush}\" BorderBrush=\"{ThemeResource CardStrokeColorDefaultBrush}\" CornerRadius=\"8\"/>"
                     L"<Grid Margin=\"1\"><Grid.RowDefinitions><RowDefinition Height=\"123\"/><RowDefinition Height=\"Auto\"/><RowDefinition Height=\"*\"/></Grid.RowDefinitions>"
                     L"<Grid IsHitTestVisible=\"False\">" + Pic(std::wstring(L"t_") + t.pic, L"", L"7,7,0,0", t.pic, L"t_" + tag) + L"</Grid>"
                     L"<Grid Grid.Row=\"1\" Margin=\"14,10,10,0\" ColumnSpacing=\"8\"><Grid.ColumnDefinitions><ColumnDefinition Width=\"*\"/><ColumnDefinition Width=\"Auto\"/></Grid.ColumnDefinitions>"
                     L"<TextBlock Text=\"" + Str(t.title) + L"\" Style=\"{StaticResource BodyStrongTextBlockStyle}\" VerticalAlignment=\"Center\" TextTrimming=\"CharacterEllipsis\" IsHitTestVisible=\"False\"/>";
                if (t.scope)
                    x += L"<ToggleSwitch x:Name=\"HomeT_" + tag + L"\" Grid.Column=\"1\" OnContent=\"\" OffContent=\"\" MinWidth=\"0\" Width=\"40\" VerticalAlignment=\"Center\" Margin=\"0,-12,0,-12\"" +
                         std::wstring(TileOn(t) ? L" IsOn=\"True\"" : L"") + L"/>";
                else
                    x += L"<FontIcon Grid.Column=\"1\" Glyph=\"&#xE76C;\" FontSize=\"12\" VerticalAlignment=\"Center\" Margin=\"0,0,6,0\" Foreground=\"{ThemeResource TextFillColorSecondaryBrush}\" IsHitTestVisible=\"False\"/>";
                x += L"</Grid><TextBlock Grid.Row=\"2\" Margin=\"14,2,14,10\" Text=\"" + Str(t.desc) + L"\" Style=\"{StaticResource CaptionTextBlockStyle}\" Foreground=\"{ThemeResource TextFillColorSecondaryBrush}\""
                     L" TextWrapping=\"Wrap\" MaxLines=\"2\" TextTrimming=\"CharacterEllipsis\" IsHitTestVisible=\"False\"/></Grid></Grid>";
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
            // Searching is typing; Ctrl+F does the same and comes second (round 11: two rows read as
            // if Ctrl+F were for searching from inside an app).
            std::wstring rows = row(Str(L"scSearch"), words(L"scTypeHow") + words(L"scOr") + Key(L"Ctrl") + Key(L"F")) +
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
            if (g.data.GetNamedObject(L"pages", JsonObject()).GetNamedBoolean(L"enabled", true) && g.data.GetNamedObject(L"pages", JsonObject()).GetNamedBoolean(L"altKeys", true))
                rows += row(Str(L"scPages"), Key(L"Alt") + Key(L"&#x2190;") + Key(L"&#x2192;"));
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
            { L"updates", L"updates", L"&#xE895;", PageUpdates, false, false, nullptr, nullptr },
            { L"look", L"pageStyles", L"&#xE737;", PageLook, true, false, L"heroLook", L"look" },
            { L"colors", L"pageColors", L"&#xE790;", PageColors, true, false, L"colorsDesc", L"colors" },
            { L"icons", L"pageIcons", L"&#xE8B9;", PageIcons, true, false, L"iconsDesc", L"icons" },
            { L"touches", L"pageTouches", L"&#xE945;", PageTouches, true, false, nullptr, nullptr },
            { L"boxes", L"pageBoxes", L"&#xE8A9;", PageBoxes, false, false, L"boxesDesc", L"boxes" },
            { L"tabs", L"pageTabs", L"&#xE8F1;", PageTabs, false, true, L"heroTabs", L"tabs" },
            { L"rollup", L"pageRollup", L"&#xE70E;", PageRollup, false, true, L"heroRollup", L"rollup" },
            { L"rules", L"pageRules", L"&#xE8CB;", PageRules, false, true, L"rulesIntro", L"rules" },
            { L"az", L"pageAz", L"&#xE8D2;", PageAz, false, true, L"heroAz", L"az" },
            { L"blend", L"blend", L"&#xE81E;", PageBlend, false, true, L"heroBlend", L"blend" },
            { L"folder", L"pageFolder", L"&#xE8B7;", PageFolder, false, true, L"heroFolder", L"folder" },
            { L"above", L"pageAbove", L"&#xE8A0;", PageAbove, false, true, L"heroAbove", L"above" },
            { L"hide", L"pageHide", L"&#xE91B;", PageHide, false, true, L"heroHide", L"hide" },
            { L"saved", L"pageSaved", L"&#xE81C;", PageSaved, false, true, L"heroSaved", L"saved" },
            { L"pages", L"pagePages", L"&#xE7C4;", PagePages, false, true, L"heroPages", L"pages" },
            { L"search", L"pageSearch", L"&#xE721;", PageSearch, false, true, L"heroSearch", L"search" },
            { L"locks", L"pageLocks", L"&#xE72E;", PageLocks, false, true, L"heroLocks", L"locks" },
            { L"privacy", L"pagePrivacy", L"&#xED1A;", PagePrivacy, false, true, L"heroPrivacy", L"privacy" },
            { L"about", L"pageAbout", L"&#xE946;", PageAbout, false, false, nullptr, nullptr },
        };

        // A page's top: a picture of the feature on a desktop, and what it does beside it.
        std::wstring Hero(const Page* p)
        {
            if (!p->pic)
                return L"<TextBlock Text=\"" + Str(p->hero) + L"\" TextWrapping=\"Wrap\" Foreground=\"{ThemeResource TextFillColorSecondaryBrush}\" Margin=\"0,0,0,16\"/>";
            // The picture as wide as the page (its clip playing; see WireClip), scaled down with
            // the window, and what the feature does under it.
            return L"<StackPanel Spacing=\"14\" Margin=\"0,0,0,20\"><Viewbox StretchDirection=\"DownOnly\" HorizontalAlignment=\"Left\" MaxWidth=\"928\">" +
                   Pic(p->pic, L"Width=\"928\" Height=\"522\"", L"8", p->pic, L"hero") +
                   L"</Viewbox><Grid ColumnSpacing=\"14\"><Grid.ColumnDefinitions><ColumnDefinition Width=\"Auto\"/><ColumnDefinition Width=\"*\"/></Grid.ColumnDefinitions>"
                   L"<FontIcon Glyph=\"" + std::wstring(p->glyph) + L"\" FontSize=\"24\" VerticalAlignment=\"Top\" Margin=\"0,2,0,0\" Foreground=\"{ThemeResource AccentTextFillColorPrimaryBrush}\"/>"
                   L"<TextBlock Grid.Column=\"1\" Text=\"" + Str(p->hero) + L"\" TextWrapping=\"Wrap\" Style=\"{StaticResource BodyTextBlockStyle}\"/></Grid></StackPanel>";
        }

        const Page* CurrentPage()
        {
            const Page* cur = &kPages[0];
            for (auto const& p : kPages)
                if (g.page == p.tag) cur = &p;
            return cur;
        }

        // A page: its title, fixed at the top as in Windows Settings, over its cards in a
        // scroller. `standalone`: a XAML document of its own (a page switch puts it into the
        // navigation view that is there). `animate`: the cards start hidden and SwitchPage brings
        // them in as Windows Settings does (the title changes at once; the cards rise into place).
        // (One animation for the whole page: per-card entrance transitions started late, after the
        // page had already been drawn once, so it blinked and the pictures popped.)
        std::wstring PageXaml(const Page* cur, bool standalone, bool animate)
        {
            std::wstring x = L"<Grid";
            if (standalone)
                x += L" xmlns=\"http://schemas.microsoft.com/winfx/2006/xaml/presentation\" xmlns:x=\"http://schemas.microsoft.com/winfx/2006/xaml\"";
            x += L"><Grid.RowDefinitions><RowDefinition Height=\"Auto\"/><RowDefinition Height=\"*\"/></Grid.RowDefinitions>"
                 L"<TextBlock x:Name=\"PageTitle\" Text=\"" + Str(cur->title) + L"\" Style=\"{StaticResource TitleTextBlockStyle}\" Margin=\"36,12,36,16\" MaxWidth=\"1000\" HorizontalAlignment=\"Stretch\" TextTrimming=\"CharacterEllipsis\"/>"
                 L"<ScrollViewer x:Name=\"Scroll\" Grid.Row=\"1\">";
            // In a Grid: straight in the ScrollViewer, a page narrower than MaxWidth was placed by
            // its own width and then stretched, so it sat off to the right in a wide window.
            x += L"<Grid x:Name=\"PageMove\"><StackPanel x:Name=\"PageBody\" Spacing=\"4\" Margin=\"36,0,36,36\" MaxWidth=\"1000\"";
            x += animate ? L" Opacity=\"0\">" : L">";
            if (animate)
                x += L"<StackPanel.OpacityTransition><ScalarTransition Duration=\"0:0:0.083\"/></StackPanel.OpacityTransition>";
            if (cur->hero) x += Hero(cur);
            x += cur->build();
            return x + L"</StackPanel></Grid></ScrollViewer></Grid>";
        }

        std::wstring BuildXaml()
        {
            const Page* cur = CurrentPage();
            std::wstring x = LR"(<Grid xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation" xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml">
<Grid.RowDefinitions><RowDefinition Height="Auto"/><RowDefinition Height="*"/></Grid.RowDefinitions>
<Grid x:Name="TitleBar" Height="48" Background="Transparent"><StackPanel Orientation="Horizontal" Spacing="12" Margin="16,0,0,0" VerticalAlignment="Center">
<Image Source="ms-appx:///deskboxes.png" Width="16" Height="16"/><TextBlock Text=")" + Str(L"title") + LR"(" Style="{StaticResource CaptionTextBlockStyle}" VerticalAlignment="Center"/></StackPanel></Grid>
<NavigationView x:Name="Nav" Grid.Row="1" PaneDisplayMode="Left" IsBackButtonVisible="Collapsed" IsSettingsVisible="False" IsPaneToggleButtonVisible="False" OpenPaneLength="250" IsTitleBarAutoPaddingEnabled="False">
<NavigationView.AutoSuggestBox><AutoSuggestBox x:Name="Find" PlaceholderText=")" + Str(L"findPlaceholder") + LR"(" QueryIcon="Find" AutomationProperties.Name=")" + Str(L"findPlaceholder") + LR"("/></NavigationView.AutoSuggestBox>
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
                if (std::wstring(p.tag) == L"about") continue;
                x += item(p);
            }
            // About sits at the bottom of the pane, as in Windows' own apps.
            x += L"</NavigationView.MenuItems><NavigationView.FooterMenuItems>";
            for (auto const& p : kPages)
                if (std::wstring(p.tag) == L"about") x += item(p);
            x += L"</NavigationView.FooterMenuItems>" + PageXaml(cur, false, false) + L"</NavigationView></Grid>";
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
                        if (winrt::unbox_value_or<winrt::hstring>(it.Tag(), L"") == tag)
                        {
                            // (An entry under Look stayed marked as selected beside the new one.)
                            for (auto const& p : nav.MenuItems())
                                if (auto grp = p.try_as<muxc::NavigationViewItem>())
                                    for (auto const& c : grp.MenuItems())
                                        if (auto ci = c.try_as<muxc::NavigationViewItem>()) ci.IsSelected(false);
                            nav.SelectedItem(it);
                            return;
                        }
            for (auto const& p : kPages)
                if (tag == p.tag && p.group) g.lookOpen = true;
            g.window.DispatcherQueue().TryEnqueue([] { Build(); });
        }

        // ------------------------------------------------------------ finding a setting

        std::wstring Lower(std::wstring s)
        {
            if (!s.empty()) CharLowerBuffW(s.data(), (DWORD)s.size());
            return s;
        }

        const wchar_t* PageTitleKey(const std::wstring& tag)
        {
            for (auto const& p : kPages)
                if (tag == p.tag) return p.title;
            return L"";
        }

        // Every page's cards and headings (Home's tiles only lead to pages: left out).
        void BuildIndex()
        {
            g.index.clear();
            g.indexing = true;
            for (auto const& p : kPages)
            {
                if (std::wstring(p.tag) == L"home") continue;
                g.indexPage = p.tag;
                g.index.push_back({ p.tag, Unesc(Str(p.title)), L"", true });
                try { p.build(); }
                catch (...) {}
            }
            g.indexing = false;
        }

        // What a suggestion says: the setting, and its page after it.
        std::wstring Label(const State::Entry& e)
        {
            return e.isPage ? e.title : e.title + L"  ·  " + Unesc(Str(PageTitleKey(e.page)));
        }

        // Settings whose words have all of the query's words in them (names first, then pages,
        // then descriptions), at most 12, each once.
        std::vector<State::Entry> Search(const std::wstring& query)
        {
            std::vector<std::wstring> words;
            {
                std::wstring q = Lower(query), w;
                for (wchar_t c : q + L" ")
                    if (iswspace(c)) { if (!w.empty()) words.push_back(w); w.clear(); }
                    else w += c;
            }
            std::vector<State::Entry> out;
            if (words.empty()) return out;
            if (g.index.empty()) BuildIndex();
            std::vector<std::pair<int, size_t>> ranked;
            for (size_t i = 0; i < g.index.size(); i++)
            {
                auto const& e = g.index[i];
                std::wstring title = Lower(e.title), all = title + L" " + Lower(e.desc) + L" " + Lower(Unesc(Str(PageTitleKey(e.page))));
                bool every = std::all_of(words.begin(), words.end(), [&](auto const& w) { return all.find(w) != std::wstring::npos; });
                if (!every) continue;
                bool inTitle = std::all_of(words.begin(), words.end(), [&](auto const& w) { return title.find(w) != std::wstring::npos; });
                int rank = title.rfind(words[0], 0) == 0 ? 0 : inTitle ? 1 : e.isPage ? 2 : 3;
                ranked.push_back({ rank, i });
            }
            std::stable_sort(ranked.begin(), ranked.end(), [](auto const& a, auto const& b) { return a.first < b.first; });
            std::set<std::wstring> seen;
            for (auto const& [rank, i] : ranked)
            {
                auto const& e = g.index[i];
                if (e.title.empty() || !seen.insert(e.page + L"|" + e.title).second) continue;
                out.push_back(e);
                if (out.size() == 12) break;
            }
            return out;
        }

        muxc::TextBlock FindText(mux::DependencyObject const& o, const std::wstring& text)
        {
            if (auto t = o.try_as<muxc::TextBlock>(); t && t.Text() == text) return t;
            int n = mux::Media::VisualTreeHelper::GetChildrenCount(o);
            for (int i = 0; i < n; i++)
                if (auto t = FindText(mux::Media::VisualTreeHelper::GetChild(o, i), text)) return t;
            return nullptr;
        }

        // The card being looked for, once its page is there: brought into view and outlined in
        // the accent color for a moment. False while the page is not there yet.
        bool ShowFound()
        {
            if (g.findTitle.empty()) return true;
            if (!g.window || g.builtPage != g.page) return false;
            auto root = g.window.Content().try_as<mux::FrameworkElement>();
            auto nav = root ? Find<muxc::NavigationView>(root, L"Nav") : muxc::NavigationView{ nullptr };
            auto page = nav ? nav.Content().try_as<mux::FrameworkElement>() : mux::FrameworkElement{ nullptr };
            auto body = page ? Find<mux::FrameworkElement>(page, L"PageBody") : mux::FrameworkElement{ nullptr };
            auto text = body ? FindText(body, g.findTitle) : muxc::TextBlock{ nullptr };
            if (!text) return false;
            g.findTitle.clear();
            // Its card: the Border around it (a heading has none).
            muxc::Border card{ nullptr };
            for (mux::DependencyObject o = mux::Media::VisualTreeHelper::GetParent(text); o && o != body; o = mux::Media::VisualTreeHelper::GetParent(o))
                if ((card = o.try_as<muxc::Border>())) break;
            mux::UIElement target = card ? card.as<mux::UIElement>() : text.as<mux::UIElement>();
            mux::BringIntoViewOptions opt;
            opt.AnimationDesired(true);
            opt.VerticalAlignmentRatio(0.3);
            target.StartBringIntoView(opt);
            if (card)
            {
                auto oldBrush = card.BorderBrush();
                auto oldThick = card.BorderThickness();
                auto accent = winrt::Windows::UI::ViewManagement::UISettings().GetColorValue(winrt::Windows::UI::ViewManagement::UIColorType::Accent);
                card.BorderBrush(mux::Media::SolidColorBrush(accent));
                card.BorderThickness(mux::ThicknessHelper::FromUniformLength(2));
                winrt::weak_ref<muxc::Border> weak{ card };
                if (g.flashTimer) g.flashTimer.Stop();
                g.flashTimer = g.window.DispatcherQueue().CreateTimer();
                g.flashTimer.Interval(std::chrono::milliseconds(1800));
                g.flashTimer.IsRepeating(false);
                g.flashTimer.Tick([weak, oldBrush, oldThick](auto&&, auto&&)
                {
                    if (auto c = weak.get()) { c.BorderBrush(oldBrush); c.BorderThickness(oldThick); }
                });
                g.flashTimer.Start();
            }
            return true;
        }

        // A suggestion chosen: its page, then its card.
        void OpenFound(const State::Entry& e)
        {
            Log(L"settings: found '%s' on %s", e.title.c_str(), e.page.c_str());
            g.findTitle = e.isPage ? L"" : e.title;
            g.findTries = 0;
            if (e.page != g.builtPage) GoTo(e.page);
            if (g.findTitle.empty() || ShowFound()) return;
            if (!g.findTimer)
            {
                g.findTimer = g.window.DispatcherQueue().CreateTimer();
                g.findTimer.Interval(std::chrono::milliseconds(80));
                g.findTimer.Tick([](auto&&, auto&&)
                {
                    // (The page fades in over a moment; given up after a few seconds.)
                    if (ShowFound() || ++g.findTries > 50) { g.findTimer.Stop(); g.findTitle.clear(); }
                });
            }
            g.findTimer.Start();
        }

        void WireFind(mux::FrameworkElement const& root)
        {
            auto f = Find<muxc::AutoSuggestBox>(root, L"Find");
            if (!f) return;
            f.TextChanged([](muxc::AutoSuggestBox const& s, muxc::AutoSuggestBoxTextChangedEventArgs const& e)
            {
                if (e.Reason() != muxc::AutoSuggestionBoxTextChangeReason::UserInput) return;
                g.hits = Search(s.Text().c_str());
                auto items = winrt::single_threaded_vector<winrt::Windows::Foundation::IInspectable>();
                for (auto const& h : g.hits) items.Append(winrt::box_value(winrt::hstring(Label(h))));
                if (g.hits.empty() && !s.Text().empty()) items.Append(winrt::box_value(winrt::hstring(Unesc(Str(L"findNone")))));
                s.ItemsSource(items);
            });
            f.QuerySubmitted([](muxc::AutoSuggestBox const& s, muxc::AutoSuggestBoxQuerySubmittedEventArgs const& e)
            {
                int pick = g.hits.empty() ? -1 : 0;
                if (auto c = e.ChosenSuggestion())
                {
                    std::wstring t = winrt::unbox_value_or<winrt::hstring>(c, L"").c_str();
                    pick = -1;
                    for (size_t i = 0; i < g.hits.size(); i++)
                        if (Label(g.hits[i]) == t) pick = (int)i;
                }
                if (pick < 0) return;
                auto hit = g.hits[pick];
                s.Text(L"");
                s.ItemsSource(nullptr);
                OpenFound(hit);
            });
            // Going through the suggestions with the arrow keys puts the setting's name in the box.
            f.SuggestionChosen([](muxc::AutoSuggestBox const& s, muxc::AutoSuggestBoxSuggestionChosenEventArgs const& e)
            {
                std::wstring t = winrt::unbox_value_or<winrt::hstring>(e.SelectedItem(), L"").c_str();
                for (auto const& h : g.hits)
                    if (Label(h) == t) { s.Text(winrt::hstring(h.title)); return; }
            });
        }

        // A clip's file as a file:/// URI, anything but letters, digits and -._~: escaped (a space
        // in the user's name).
        std::wstring FileUri(const std::wstring& file)
        {
            std::wstring u = L"file:///";
            for (wchar_t ch : file)
            {
                if (ch == L'\\') u += L'/';
                else if ((ch < 128 && iswalnum(ch)) || wcschr(L"-._~:", ch)) u += ch;
                else
                {
                    char utf8[8]{};
                    int n = WideCharToMultiByte(CP_UTF8, 0, &ch, 1, utf8, sizeof utf8, nullptr, nullptr);
                    for (int i = 0; i < n; i++) { wchar_t h[4]; swprintf_s(h, L"%%%02X", (unsigned char)utf8[i]); u += h; }
                }
            }
            return u;
        }

        // Show a clip that was just started from its beginning, once it runs (a frame in, so no
        // visible jump from the picture, which is its first frame): the picture over it fades out.
        // Polled, for the player says nothing when a frame arrives; `want` going false
        // (the pointer left) stops it. The timer is kept in the clip's state: a timer nothing holds
        // is released at once and never ticks (the clips then played unseen).
        struct ClipState
        {
            bool want = false;
            int ticks = 0;
            winrt::Microsoft::UI::Dispatching::DispatcherQueueTimer timer{ nullptr };
            // The picture over the video (Still_KEY in Pic).
            winrt::weak_ref<mux::UIElement> still;
            // The video itself: transparent until it has shown a frame (before that it is black,
            // which showed through while the picture over it was still loading).
            winrt::weak_ref<mux::UIElement> player;
            // The tick the video was made visible (under the picture), -1 before.
            int shownAt = -1;
            void Cover(bool on)
            {
                if (auto s = still.get()) s.Opacity(on ? 1.0 : 0.0);
            }
        };

        void RevealClip(winrt::weak_ref<muxc::MediaPlayerElement> weak, std::shared_ptr<ClipState> st)
        {
            namespace wmp = winrt::Windows::Media::Playback;
            if (!st->timer)
            {
                st->timer = g.window.DispatcherQueue().CreateTimer();
                st->timer.Interval(std::chrono::milliseconds(30));
                std::weak_ptr<ClipState> ws = st;
                st->timer.Tick([weak, ws](winrt::Microsoft::UI::Dispatching::DispatcherQueueTimer const& t, auto&&)
                    {
                        auto st = ws.lock();
                        auto e = weak.get();
                        // (Up to ~30 s: a first clip can be slow to open on a slow PC.)
                        if (!st || !e || !st->want || ++st->ticks > 1000) { t.Stop(); return; }
                        auto mp = e.MediaPlayer();
                        if (!mp) return;
                        auto s = mp.PlaybackSession();
                        // First the video turns visible under the picture (a video element that
                        // was transparent takes a few frames to show again: the picture fading
                        // at once let the page show through); then the picture fades.
                        if (st->shownAt < 0)
                        {
                            if (s.PlaybackState() == wmp::MediaPlaybackState::Playing && s.Position() >= std::chrono::milliseconds(40))
                            {
                                if (auto p = st->player.get()) p.Opacity(1.0);
                                st->shownAt = st->ticks;
                            }
                        }
                        else if (st->ticks - st->shownAt >= 4)
                        {
                            st->Cover(false);
                            t.Stop();
                        }
                    });
            }
            st->ticks = 0;
            st->shownAt = -1;
            st->timer.Stop();
            st->timer.Start();
        }

        // The clip's player on its element, made (paused, on the first frame) if it has none yet:
        // the element comes with an empty player of its own; ours is the one with a source.
        bool LoadClip(muxc::MediaPlayerElement const& e, const std::wstring& clip)
        {
            namespace wmp = winrt::Windows::Media::Playback;
            if (auto had = e.MediaPlayer(); had && had.Source()) return true;
            std::wstring file = ExeDir() + L"\\settings\\" + clip + (e.ActualTheme() == mux::ElementTheme::Light ? L"_l" : L"") + L".mp4";
            if (GetFileAttributesW(file.c_str()) == INVALID_FILE_ATTRIBUTES) return false;
            try
            {
                wmp::MediaPlayer mp;
                mp.CommandManager().IsEnabled(false);
                mp.IsMuted(true);
                mp.IsLoopingEnabled(true);
                mp.AutoPlay(false);
                mp.MediaFailed([clip](auto&&, wmp::MediaPlayerFailedEventArgs const& f)
                    { Log(L"settings: clip %s failed (%d, 0x%08X): %s", clip.c_str(), (int)f.Error(), (unsigned)f.ExtendedErrorCode().value, f.ErrorMessage().c_str()); });
                e.SetMediaPlayer(mp);
                mp.Source(winrt::Windows::Media::Core::MediaSource::CreateFromUri(winrt::Windows::Foundation::Uri(FileUri(file))));
                return true;
            }
            catch (winrt::hresult_error const& x) { Log(L"settings: clip %s: %s", clip.c_str(), x.message().c_str()); }
            return false;
        }

        // Start (or restart from the beginning) a feature's clip on its element; see WireClip.
        void StartClip(muxc::MediaPlayerElement const& e, const std::wstring& clip, std::shared_ptr<ClipState> want)
        {
            if (!winrt::Windows::UI::ViewManagement::UISettings().AnimationsEnabled())
            {
                static bool told = false;
                if (!told) Log(L"settings: Windows' animation effects are off: feature clips stay still");
                told = true;
                return;
            }
            if (!LoadClip(e, clip)) return;
            want->want = true;
            want->Cover(true);
            auto mp = e.MediaPlayer();
            if (mp.PlaybackSession().Position() > std::chrono::milliseconds(0))
                mp.PlaybackSession().Position(std::chrono::seconds(0));
            mp.Play();
            RevealClip(e, want);
        }

        // A feature's clip (see Pic), muted and looping over the picture, which is its first frame.
        // On a tile it plays while the pointer is on `hover` and goes when it leaves; at the top of
        // a feature's own page (`autoplay`) it plays from the start for as long as the page shows.
        // The picture over it fades out once it runs. Nothing plays when Windows' animation effects
        // are off. The player is made when it first plays and kept out of the system's media controls.
        void WireClip(mux::FrameworkElement const& hover, muxc::MediaPlayerElement const& el, bool autoplay = false)
        {
            if (!hover || !el) return;
            std::wstring tag = winrt::unbox_value_or<winrt::hstring>(el.Tag(), L"").c_str();
            size_t a = tag.find(L';'), b = tag.rfind(L';');
            if (a == std::wstring::npos || a == b) return;
            std::wstring clip = tag.substr(0, a);
            float rt = std::max(0.f, (float)_wtof(tag.substr(a + 1, b - a - 1).c_str()) - 1), rb = std::max(0.f, (float)_wtof(tag.substr(b + 1).c_str()) - 1);
            winrt::weak_ref<muxc::MediaPlayerElement> weak = el;
            // The corners are cut on the frame around the player (an element with an
            // OpacityTransition gives no composition visual), found once it is laid out.
            el.SizeChanged([weak, rt, rb](auto&&, auto&&)
                {
                    auto e = weak.get();
                    if (!e) return;
                    auto frame = mux::Media::VisualTreeHelper::GetParent(e).try_as<mux::UIElement>();
                    if (!frame) return;
                    auto vis = mux::Hosting::ElementCompositionPreview::GetElementVisual(frame);
                    vis.Clip(vis.Compositor().CreateRectangleClip(0, 0, (float)e.ActualWidth(), (float)e.ActualHeight(), { rt, rt }, { rt, rt }, { rb, rb }, { rb, rb }));
                });
            auto still = el.FindName(L"Still_" + std::wstring(el.Name()).substr(5)).try_as<mux::UIElement>();
            if (!still) return;
            mux::ScalarTransition fade;
            fade.Duration(std::chrono::milliseconds(autoplay ? 260 : 160));
            still.OpacityTransition(fade);
            auto want = std::make_shared<ClipState>();
            want->still = still;
            want->player = el;
            auto stop = [weak, want]
            {
                want->want = false;
                if (want->timer) want->timer.Stop();
                want->Cover(true);
                auto e = weak.get();
                if (!e) return;
                if (auto mp = e.MediaPlayer()) mp.Pause();
            };
            // Shown: a page's own clip plays; a tile's is loaded, so a pointer on it starts it at once.
            el.Loaded([weak, clip, want, autoplay](auto&&, auto&&)
                {
                    auto e = weak.get();
                    if (!e) return;
                    if (autoplay) StartClip(e, clip, want);
                    else if (winrt::Windows::UI::ViewManagement::UISettings().AnimationsEnabled()) LoadClip(e, clip);
                });
            // The page went (another one was picked, or the window closed): let the file go.
            el.Unloaded([weak, stop](auto&&, auto&&)
                {
                    stop();
                    if (auto e = weak.get())
                        if (auto mp = e.MediaPlayer()) mp.Source(nullptr);
                });
            if (autoplay) return;
            // The tile's button marks pointer events handled, so these listen to handled ones too.
            auto play = [weak, clip, want](winrt::Windows::Foundation::IInspectable const&, mux::Input::PointerRoutedEventArgs const&)
                {
                    if (auto e = weak.get()) StartClip(e, clip, want);
                };
            hover.AddHandler(mux::UIElement::PointerEnteredEvent(), winrt::box_value(mux::Input::PointerEventHandler(play)), true);
            auto leave = [stop](winrt::Windows::Foundation::IInspectable const&, mux::Input::PointerRoutedEventArgs const&) { stop(); };
            for (auto ev : { mux::UIElement::PointerExitedEvent(), mux::UIElement::PointerCanceledEvent(), mux::UIElement::PointerCaptureLostEvent() })
                hover.AddHandler(ev, winrt::box_value(mux::Input::PointerEventHandler(leave)), true);
        }

        void Wire(mux::FrameworkElement const& root)
        {
            for (auto const& t : kTiles)
                WireClip(Find<mux::FrameworkElement>(root, L"TileG_" + std::wstring(t.page)), Find<muxc::MediaPlayerElement>(root, L"Clip_t_" + std::wstring(t.page)));
            WireClip(Find<mux::FrameworkElement>(root, L"ClipG_hero"), Find<muxc::MediaPlayerElement>(root, L"Clip_hero"), true);
            if (auto tiles = Find<muxc::VariableSizedWrapGrid>(root, L"Tiles"))
                tiles.SizeChanged([](winrt::Windows::Foundation::IInspectable const& s, mux::SizeChangedEventArgs const& e)
                    {
                        auto w = s.as<muxc::VariableSizedWrapGrid>();
                        double width = e.NewSize().Width;
                        int n = std::max(1, (int)(width / 230));
                        double item = std::floor(width / n);
                        if (std::abs(w.ItemWidth() - item) > 0.5) w.ItemWidth(item);
                    });
            WireFind(root);
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
            OnClick(root, L"HomeUpdate", L"update", 0, L"install");
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
            // Desktop pages
            OnToggle(root, L"PgOn", L"pages", L"enabled");
            OnToggle(root, L"PgAlt", L"pages", L"altKeys");
            OnToggle(root, L"PgSwipe", L"pages", L"swipe");
            OnToggle(root, L"PgMiddle", L"pages", L"middleDrag");
            OnToggle(root, L"PgBoth", L"pages", L"bothDrag");
            OnToggle(root, L"PgEdgePull", L"pages", L"edgePull");
            OnToggle(root, L"PgEdgeDrag", L"pages", L"edgeDrag");
            OnToggle(root, L"PgAnimate", L"pages", L"animate");
            OnToggle(root, L"PgBounce", L"pages", L"bounce");
            OnToggle(root, L"PgShowName", L"pages", L"showName");
            OnToggle(root, L"PgScreenNotice", L"pages", L"screenNotice");
            OnCombo(root, L"PgNewFiles", [](std::wstring v) { Send(L"pages", 0, L"newFiles", JsonValue::CreateStringValue(v)); });
            OnClick(root, L"PgNew", L"pages", 0, L"new");
            for (auto const& e : g.data.GetNamedObject(L"pages", JsonObject()).GetNamedArray(L"list", JsonArray()))
            {
                double id = e.GetObject().GetNamedNumber(L"id");
                std::wstring s = std::to_wstring((long long)id);
                using P = std::pair<const wchar_t*, const wchar_t*>;
                for (auto [name, key] : { P{ L"PgGo", L"go" }, P{ L"PgRen", L"rename" }, P{ L"PgUp", L"up" }, P{ L"PgDown", L"down" }, P{ L"PgDel", L"delete" } })
                    if (auto b = Find<muxc::Button>(root, name + s))
                        b.Click([key = key, id](auto&&, auto&&) { Send(L"pages", 0, key, JsonValue::CreateNumberValue(id)); });
            }
            // Saved layouts
            OnClick(root, L"SaveNow", L"saved", 0, L"now");
            OnClick(root, L"ExportAll", L"transfer", 0, L"export");
            OnClick(root, L"ImportAll", L"transfer", 0, L"import");
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
            OnToggle(root, L"SearchResults", L"search", L"results");
            OnToggle(root, L"SearchHideToggle", L"search", L"hideToggle");
            OnCombo(root, L"SearchPosition", [](std::wstring v) { Send(L"search", 0, L"position", JsonValue::CreateStringValue(v)); });
            // Roll-up
            OnToggle(root, L"RollEdges", L"rollup", L"edges");
            OnToggle(root, L"RollDbl", L"rollup", L"doubleClick");
            OnCombo(root, L"RollReveal", [](std::wstring v) { Send(L"rollup", 0, L"reveal", JsonValue::CreateStringValue(v)); });
        }

        void Build();

        // The new page's cards rise into place as Windows Settings' do: from a little below,
        // quickly at first and then settling (its decelerate curve), fading in at the start.
        // `move` is the frame around `body`: an element with an OpacityTransition (the body) must
        // not have its composition visual taken (WinUI ends the process).
        void Rise(mux::UIElement const& move, mux::UIElement const& body)
        {
            auto vis = mux::Hosting::ElementCompositionPreview::GetElementVisual(move);
            auto comp = vis.Compositor();
            auto a = comp.CreateVector3KeyFrameAnimation();
            auto ease = comp.CreateCubicBezierEasingFunction({ 0.1f, 0.9f }, { 0.2f, 1.0f });
            a.InsertKeyFrame(0.0f, { 0.0f, 120.0f, 0.0f });
            a.InsertKeyFrame(1.0f, { 0.0f, 0.0f, 0.0f }, ease);
            a.Duration(std::chrono::milliseconds(380));
            vis.StartAnimation(L"Translation", a);
            body.Opacity(1.0);
        }

        // Another page: only the page is replaced (the navigation keeps its selection marker
        // gliding to the new entry). As in Windows Settings the title changes at once, the old
        // cards fade out quickly, and then the new ones rise into place (see Rise).
        // The page to show next, made while the one before fades out.
        mux::FrameworkElement MakePage(bool anim)
        {
            try { return mux::Markup::XamlReader::Load(PageXaml(CurrentPage(), true, anim)).as<mux::FrameworkElement>(); }
            catch (winrt::hresult_error const& e) { Log(L"settings: page XAML failed: %s", e.message().c_str()); }
            return nullptr;
        }

        void ShowPage(mux::FrameworkElement page, bool anim)
        {
            if (!g.window) return;
            auto old = g.window.Content().try_as<mux::FrameworkElement>();
            auto nav = old ? Find<muxc::NavigationView>(old, L"Nav") : muxc::NavigationView{ nullptr };
            if (!nav || !page) return Build();
            auto body = anim ? Find<mux::UIElement>(page, L"PageBody") : mux::UIElement{ nullptr };
            auto move = anim ? Find<mux::UIElement>(page, L"PageMove") : mux::UIElement{ nullptr };
            if (body && move)
            {
                mux::Hosting::ElementCompositionPreview::SetIsTranslationEnabled(move, true);
                mux::Hosting::ElementCompositionPreview::GetElementVisual(move).Properties().InsertVector3(L"Translation", { 0.0f, 120.0f, 0.0f });
            }
            else body = nullptr;
            g.wiring = true;
            g.builtPage = g.page;
            nav.Content(page);
            Wire(page);
            // (Weak: the page's own handler holding it, or its child, would keep every page
            // switched away from alive.)
            winrt::weak_ref<mux::UIElement> weakBody{ body }, weakMove{ move };
            page.Loaded([weakBody, weakMove](auto&& sender, auto&&)
            {
                // Laid out and still invisible: now it comes in.
                auto body = weakBody.get();
                auto move = weakMove.get();
                if (body && move) Rise(move, body);
                auto page = sender.template as<mux::FrameworkElement>();
                page.DispatcherQueue().TryEnqueue(winrt::Microsoft::UI::Dispatching::DispatcherQueuePriority::Low, [] { g.wiring = false; });
            });
        }

        // The page that comes next, waiting for the old one to fade out (see SwitchPage).
        mux::FrameworkElement g_nextPage{ nullptr };

        void SwitchPage()
        {
            if (!g.window) return;
            // Picked again while it shows (selection raised twice): nothing to do.
            if (g.page == g.builtPage && !(g.switchTimer && g.switchTimer.IsRunning())) return;
            // Windows' "Animation effects" off: the page just appears.
            BOOL anim = TRUE;
            SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION, 0, &anim, 0);
            auto root = g.window.Content().try_as<mux::FrameworkElement>();
            auto nav = root ? Find<muxc::NavigationView>(root, L"Nav") : muxc::NavigationView{ nullptr };
            auto shown = nav ? nav.Content().try_as<mux::FrameworkElement>() : mux::FrameworkElement{ nullptr };
            auto oldBody = shown ? Find<mux::UIElement>(shown, L"PageBody") : mux::UIElement{ nullptr };
            if (!anim || !oldBody) return ShowPage(MakePage(anim != FALSE), anim != FALSE);
            // As in Windows Settings: the title at once; the old cards fade out quickly; the new
            // page (made meanwhile) once they are gone.
            if (auto t = Find<muxc::TextBlock>(shown, L"PageTitle")) t.Text(g.data.GetNamedObject(L"strings", JsonObject()).GetNamedString(CurrentPage()->title, CurrentPage()->title));
            mux::ScalarTransition fade;
            fade.Duration(std::chrono::milliseconds(90));
            oldBody.OpacityTransition(fade);
            oldBody.Opacity(0.0);
            if (!g.switchTimer)
            {
                g.switchTimer = g.window.DispatcherQueue().CreateTimer();
                g.switchTimer.IsRepeating(false);
                g.switchTimer.Tick([](auto&&, auto&&)
                    {
                        auto page = g_nextPage;
                        g_nextPage = nullptr;
                        ShowPage(page, true);
                    });
            }
            g.switchTimer.Stop();
            auto t0 = std::chrono::steady_clock::now();
            g_nextPage = MakePage(true);
            // What is left of the fade after making the page (at least a frame).
            auto spent = std::chrono::duration_cast<std::chrono::milliseconds>(std::chrono::steady_clock::now() - t0);
            g.switchTimer.Interval(std::max(std::chrono::milliseconds(16), std::chrono::milliseconds(95) - spent));
            g.switchTimer.Start();
        }

        void Build()
        {
            if (!g.window) return;
            // The same page keeps its scroll position across refreshes; another page starts at the top.
            double scroll = 0;
            if (g.page == g.builtPage)
                if (auto old = g.window.Content().try_as<mux::FrameworkElement>())
                    if (auto nav = Find<muxc::NavigationView>(old, L"Nav"))
                        if (auto shown = nav.Content().try_as<mux::FrameworkElement>())
                            if (auto sv = Find<muxc::ScrollViewer>(shown, L"Scroll")) scroll = sv.VerticalOffset();
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
            winrt::weak_ref<mux::FrameworkElement> weakRoot{ root };
            winrt::weak_ref<muxc::Expander> weakFocus{ focus };
            root.Loaded([weakRoot, scroll, weakFocus](auto&&, auto&&)
            {
                auto root = weakRoot.get();
                if (!root) return;
                root.DispatcherQueue().TryEnqueue(winrt::Microsoft::UI::Dispatching::DispatcherQueuePriority::Low, [weakRoot, scroll, weakFocus]
                {
                    auto root = weakRoot.get();
                    if (auto focus = weakFocus.get()) focus.StartBringIntoView();
                    else if (auto sv = root ? Find<muxc::ScrollViewer>(root, L"Scroll") : muxc::ScrollViewer{ nullptr }) sv.ChangeView(nullptr, scroll, nullptr, true);
                    g.wiring = false;
                });
            });
        }

        // Keeps the settings window at least 640 x 500 (at 100% scale), like Windows Settings keeps
        // room for its navigation and a card.
        LRESULT CALLBACK MinSizeProc(HWND hwnd, UINT msg, WPARAM wp, LPARAM lp, UINT_PTR id, DWORD_PTR)
        {
            if (msg == WM_GETMINMAXINFO)
            {
                LRESULT r = DefSubclassProc(hwnd, msg, wp, lp);
                UINT dpi = GetDpiForWindow(hwnd);
                auto mmi = reinterpret_cast<MINMAXINFO*>(lp);
                mmi->ptMinTrackSize.x = std::max<LONG>(mmi->ptMinTrackSize.x, MulDiv(640, dpi, 96));
                mmi->ptMinTrackSize.y = std::max<LONG>(mmi->ptMinTrackSize.y, MulDiv(500, dpi, 96));
                return r;
            }
            if (msg == WM_NCDESTROY) RemoveWindowSubclass(hwnd, MinSizeProc, id);
            return DefSubclassProc(hwnd, msg, wp, lp);
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
            // Not smaller than the navigation and a settings card need.
            SetWindowSubclass(g.hwnd, MinSizeProc, 1, 0);
            g.window.Closed([](auto&&, auto&&)
                {
                    if (g.switchTimer) g.switchTimer.Stop();
                    g.switchTimer = nullptr;
                    g_nextPage = nullptr;
                    g.window = nullptr; g.hwnd = nullptr; g.builtPage.clear(); TrimMemorySoon();
                });
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
            b.Click([weakRoot = winrt::weak_ref<mux::FrameworkElement>(root)](auto&&, auto&&)
            {
                auto root = weakRoot.get();
                auto pk = root ? Find<muxc::ColorPicker>(root, L"Picker") : muxc::ColorPicker{ nullptr };
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

    namespace
    {
        struct Dialog
        {
            mux::Window window{ nullptr };
            std::function<void(JsonObject)> send;
            double id = 0;
            bool text = false;
            bool done = false;
        };
        Dialog gd;

        void DialogSend(bool ok, const std::wstring& value)
        {
            if (gd.done || !gd.send) return;
            gd.done = true;
            JsonObject o;
            o.SetNamedValue(L"scope", JsonValue::CreateStringValue(L"dialog"));
            o.SetNamedValue(L"dialog", JsonValue::CreateNumberValue(gd.id));
            o.SetNamedValue(L"ok", JsonValue::CreateBooleanValue(ok));
            o.SetNamedValue(L"value", JsonValue::CreateStringValue(value));
            gd.send(o);
        }

        std::wstring Trimmed(std::wstring s)
        {
            while (!s.empty() && iswspace(s.back())) s.pop_back();
            size_t a = 0;
            while (a < s.size() && iswspace(s[a])) ++a;
            return s.substr(a);
        }

        // OK (a name field must not be empty) or Cancel: answered, closed.
        void DialogEnd(mux::FrameworkElement const& root, bool ok)
        {
            std::wstring v;
            if (auto f = Find<muxc::TextBox>(root, L"Field")) v = Trimmed(f.Text().c_str());
            if (ok && gd.text && v.empty()) return;
            DialogSend(ok, v);
            if (gd.window) gd.window.Close();
        }
    }

    void ShowDialog(JsonObject const& data, std::function<void(JsonObject)> send)
    {
        if (gd.window && !gd.done) { DialogSend(false, L""); gd.window.Close(); }
        gd = Dialog{};
        gd.send = std::move(send);
        gd.id = data.GetNamedNumber(L"dialog", 0);
        gd.text = data.GetNamedString(L"kind", L"confirm") == L"text";
        auto s = [&](const wchar_t* k) { return Esc(std::wstring(data.GetNamedString(k, L"").c_str())); };
        std::wstring body = s(L"body");
        // ContentDialog's look: the title (Subtitle), the text, the field; the buttons in a band
        // at the bottom. It comes in as ContentDialog does: fading in, from 1.05 times its size.
        std::wstring x = LR"(<Grid xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation" xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml" x:Name="Root" Opacity="0" RenderTransformOrigin="0.5,0.5">
<Grid.RenderTransform><ScaleTransform x:Name="Sc" ScaleX="1.05" ScaleY="1.05"/></Grid.RenderTransform>
<Grid.Triggers><EventTrigger RoutedEvent="FrameworkElement.Loaded"><BeginStoryboard><Storyboard>
<DoubleAnimation Storyboard.TargetName="Root" Storyboard.TargetProperty="Opacity" To="1" Duration="0:0:0.167"/>
<DoubleAnimation Storyboard.TargetName="Sc" Storyboard.TargetProperty="ScaleX" To="1" Duration="0:0:0.35"><DoubleAnimation.EasingFunction><ExponentialEase EasingMode="EaseOut" Exponent="6"/></DoubleAnimation.EasingFunction></DoubleAnimation>
<DoubleAnimation Storyboard.TargetName="Sc" Storyboard.TargetProperty="ScaleY" To="1" Duration="0:0:0.35"><DoubleAnimation.EasingFunction><ExponentialEase EasingMode="EaseOut" Exponent="6"/></DoubleAnimation.EasingFunction></DoubleAnimation>
</Storyboard></BeginStoryboard></EventTrigger></Grid.Triggers>
<Grid.RowDefinitions><RowDefinition Height="*"/><RowDefinition Height="Auto"/></Grid.RowDefinitions>
<StackPanel x:Name="Content" Padding="24,24,24,24" Spacing="12">
<TextBlock Text=")" + s(L"title") + LR"(" Style="{StaticResource SubtitleTextBlockStyle}" TextWrapping="Wrap"/>)";
        if (!body.empty()) x += L"<TextBlock Text=\"" + body + L"\" TextWrapping=\"Wrap\"/>";
        if (gd.text) x += L"<TextBox x:Name=\"Field\" Text=\"" + s(L"value") + L"\" PlaceholderText=\"" + s(L"placeholder") + L"\" MaxLength=\"60\" Margin=\"0,4,0,0\"/>";
        x += LR"(</StackPanel>
<Grid x:Name="Band" Grid.Row="1" Padding="24" ColumnSpacing="8" Background="{ThemeResource LayerOnMicaBaseAltFillColorDefaultBrush}" BorderBrush="{ThemeResource CardStrokeColorDefaultBrush}" BorderThickness="0,1,0,0">
<Grid.ColumnDefinitions><ColumnDefinition Width="*"/><ColumnDefinition Width="*"/></Grid.ColumnDefinitions>)";
        // No cancel text: a message, with OK alone (on the right, as a ContentDialog's).
        if (s(L"cancel").empty())
            x += LR"(<Button x:Name="Ok" Grid.Column="1" Style="{StaticResource AccentButtonStyle}" HorizontalAlignment="Stretch" Content=")" + s(L"ok") + LR"("/>)";
        else
            x += LR"(<Button x:Name="Ok" Style="{StaticResource AccentButtonStyle}" HorizontalAlignment="Stretch" Content=")" + s(L"ok") + LR"("/>
<Button x:Name="Cancel" Grid.Column="1" HorizontalAlignment="Stretch" Content=")" + s(L"cancel") + LR"("/>)";
        x += LR"(</Grid></Grid>)";
        mux::FrameworkElement root{ nullptr };
        try { root = mux::Markup::XamlReader::Load(x).as<mux::FrameworkElement>(); }
        catch (winrt::hresult_error const& e) { Log(L"dialog: XAML failed: %s", e.message().c_str()); DialogSend(false, L""); return; }
        gd.window = mux::Window();
        gd.window.SystemBackdrop(mux::Media::MicaBackdrop());
        gd.window.Title(data.GetNamedString(L"title", L"Deskboxes"));
        gd.window.Content(root);
        auto aw = gd.window.AppWindow();
        HWND hwnd = winrt::Microsoft::UI::GetWindowFromWindowId(aw.Id());
        if (HICON ico = (HICON)LoadImageW(GetModuleHandleW(nullptr), MAKEINTRESOURCEW(1), IMAGE_ICON, 0, 0, LR_DEFAULTSIZE))
            aw.SetIcon(winrt::Microsoft::UI::GetIconIdFromIcon(ico));
        if (auto op = aw.Presenter().try_as<winrt::Microsoft::UI::Windowing::OverlappedPresenter>())
        {
            op.IsResizable(false);
            op.IsMaximizable(false);
            op.IsMinimizable(false);
            op.IsAlwaysOnTop(true);
            // No title bar, as a ContentDialog (the border and rounded corners stay).
            op.SetBorderAndTitleBar(true, false);
        }
        // (Without a caption Windows leaves the corners square: rounded, as a dialog's.)
        DWM_WINDOW_CORNER_PREFERENCE corner = DWMWCP_ROUND;
        DwmSetWindowAttribute(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, &corner, sizeof(corner));
        // Centered on the pointer's monitor (where the menu was), as wide as a ContentDialog.
        POINT pt{}; GetCursorPos(&pt);
        HMONITOR mon = MonitorFromPoint(pt, MONITOR_DEFAULTTOPRIMARY);
        MONITORINFO mi{ sizeof(mi) }; GetMonitorInfoW(mon, &mi);
        UINT dpi = GetDpiForWindow(hwnd); if (!dpi) dpi = 96;
        auto& wa = mi.rcWork;
        auto place = [hwnd, wa, dpi](double hDip)
        {
            int w = MulDiv(440, dpi, 96), h = (int)std::ceil(hDip * dpi / 96.0);
            if (auto win = gd.window) win.AppWindow().MoveAndResize({ wa.left + (wa.right - wa.left - w) / 2, wa.top + (wa.bottom - wa.top - h) / 2, w, h });
        };
        place(gd.text ? 216 : (body.empty() ? 160 : 196));
        // Handlers hold the root weakly: a strong capture in an element's own handler (or a
        // child's) is a cycle that would keep each closed dialog's tree alive.
        winrt::weak_ref<mux::FrameworkElement> weakRoot{ root };
        // Its real height once laid out (the text wraps).
        root.Loaded([weakRoot, place](auto&&, auto&&)
        {
            auto root = weakRoot.get();
            if (!root) return;
            root.Measure({ 440, std::numeric_limits<float>::infinity() });
            double h = root.DesiredSize().Height;
            if (h > 60) place(h);
            if (auto f = Find<muxc::TextBox>(root, L"Field"))
            {
                f.Focus(mux::FocusState::Programmatic);
                f.SelectAll();
            }
            else if (auto b = Find<muxc::Button>(root, L"Ok")) b.Focus(mux::FocusState::Programmatic);
        });
        auto ok = Find<muxc::Button>(root, L"Ok");
        if (auto f = Find<muxc::TextBox>(root, L"Field"))
        {
            winrt::weak_ref<muxc::Button> weakOk{ ok };
            f.TextChanged([weakOk](auto&& s, auto&&) { auto t = s.template as<muxc::TextBox>(); if (auto ok = weakOk.get(); ok && t) ok.IsEnabled(!Trimmed(t.Text().c_str()).empty()); });
        }
        if (ok) ok.Click([weakRoot](auto&&, auto&&) { if (auto root = weakRoot.get()) DialogEnd(root, true); });
        if (auto b = Find<muxc::Button>(root, L"Cancel"))
            b.Click([weakRoot](auto&&, auto&&) { if (auto root = weakRoot.get()) DialogEnd(root, false); });
        root.KeyDown([weakRoot](auto&&, mux::Input::KeyRoutedEventArgs const& e)
        {
            auto root = weakRoot.get();
            if (!root) return;
            if (e.Key() == winrt::Windows::System::VirtualKey::Escape) { e.Handled(true); DialogEnd(root, false); }
            else if (e.Key() == winrt::Windows::System::VirtualKey::Enter)
            {
                // (Enter on Cancel is Cancel's own click.)
                auto focused = mux::Input::FocusManager::GetFocusedElement(root.XamlRoot()).try_as<muxc::Button>();
                if (focused && focused.Name() == L"Cancel") return;
                e.Handled(true);
                DialogEnd(root, true);
            }
        });
        gd.window.Closed([](auto&&, auto&&)
        {
            DialogSend(false, L"");
            gd.window = nullptr;
            TrimMemorySoon();
        });
        gd.window.Activate();
        SetForegroundWindow(hwnd);
        Log(L"dialog %g opened", gd.id);
    }

    void ShowSettings(JsonObject const& data, bool open, std::function<void(JsonObject)> send)
    {
        g.data = data;
        g.send = std::move(send);
        if (!g.window && !open) return;
        // Only the download went on: move the progress bar instead of building the page again
        // (a rebuild per percent would restart the ring and flicker).
        {
            auto copy = JsonObject::Parse(data.Stringify());
            auto st = copy.GetNamedObject(L"update", JsonObject()).GetNamedObject(L"status", JsonObject());
            double progress = st.GetNamedNumber(L"progress", 0);
            if (st.HasKey(L"progress")) st.SetNamedValue(L"progress", JsonValue::CreateNumberValue(0));
            std::wstring sig = copy.Stringify().c_str();
            bool same = !open && g.window && sig == g.lastSig;
            g.lastSig = sig;
            if (same)
            {
                auto root = g.window.Content().try_as<mux::FrameworkElement>();
                // A page put in by a page switch is a XAML document of its own (its own names).
                if (auto nav = root ? Find<muxc::NavigationView>(root, L"Nav") : muxc::NavigationView{ nullptr })
                    if (auto pg = nav.Content().try_as<mux::FrameworkElement>(); pg && Find<muxc::ProgressBar>(pg, L"UpdBar")) root = pg;
                if (root)
                {
                    double pct = std::round(progress * 100.0);
                    if (auto bar = Find<muxc::ProgressBar>(root, L"UpdBar"))
                    {
                        bar.IsIndeterminate(pct <= 0);
                        bar.Value(pct);
                    }
                    if (auto t = Find<muxc::TextBlock>(root, L"UpdPct")) t.Text(Num(pct) + L"%");
                }
                return;
            }
        }
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

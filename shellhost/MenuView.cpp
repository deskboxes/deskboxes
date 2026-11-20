// The context menu view: a WinUI CommandBarFlyout, the control Explorer itself uses on
// Windows 11 26H2 (verified with UI Automation: PopupWindowSiteBridge windows, AppBarButtons,
// AutomationId = canonical verb). Primary commands = the command row; secondary commands = the
// list. The flyout is a windowed popup anchored to a 1x1, fully transparent context-menu window
// at the cursor.

#include "pch.h"
#include "MenuView.h"
#include "Log.h"
#include <winrt/Windows.Storage.Streams.h>
#include <winrt/Microsoft.UI.Xaml.Automation.h>
#include <shcore.h>

using namespace winrt;
using namespace winrt::Microsoft::UI::Xaml;
namespace mux = winrt::Microsoft::UI::Xaml;
namespace muxc = winrt::Microsoft::UI::Xaml::Controls;

namespace dbx
{
    // ---- icons ---------------------------------------------------------------------------
    static std::vector<uint8_t> DibPixels(HBITMAP bmp, int& w, int& h)
    {
        BITMAP bm{};
        if (!GetObjectW(bmp, sizeof(bm), &bm)) return {};
        w = bm.bmWidth; h = std::abs(bm.bmHeight);
        BITMAPINFO bi{}; bi.bmiHeader.biSize = sizeof(bi.bmiHeader);
        bi.bmiHeader.biWidth = w; bi.bmiHeader.biHeight = -h; bi.bmiHeader.biPlanes = 1;
        bi.bmiHeader.biBitCount = 32; bi.bmiHeader.biCompression = BI_RGB;
        std::vector<uint8_t> px((size_t)w * h * 4);
        HDC dc = GetDC(nullptr);
        int ok = GetDIBits(dc, bmp, 0, h, px.data(), &bi, DIB_RGB_COLORS);
        ReleaseDC(nullptr, dc);
        if (!ok) return {};
        return px;
    }

    static mux::Media::ImageSource ToImage(std::vector<uint8_t>& px, int w, int h, bool premultiply)
    {
        if (px.empty() || w <= 0 || h <= 0) return nullptr;
        if (premultiply)
            for (size_t i = 0; i < px.size(); i += 4)
            {
                uint8_t a = px[i + 3];
                px[i] = (uint8_t)(px[i] * a / 255); px[i + 1] = (uint8_t)(px[i + 1] * a / 255); px[i + 2] = (uint8_t)(px[i + 2] * a / 255);
            }
        mux::Media::Imaging::WriteableBitmap wb(w, h);
        memcpy(wb.PixelBuffer().data(), px.data(), px.size());
        wb.Invalidate();
        return wb;
    }

    static mux::Media::ImageSource ImageFromIcon(HICON icon)
    {
        ICONINFO ii{};
        if (!icon || !GetIconInfo(icon, &ii)) return nullptr;
        int w = 0, h = 0;
        auto px = ii.hbmColor ? DibPixels(ii.hbmColor, w, h) : std::vector<uint8_t>{};
        bool anyAlpha = false;
        for (size_t i = 3; i < px.size(); i += 4) if (px[i]) { anyAlpha = true; break; }
        if (!px.empty() && !anyAlpha && ii.hbmMask)
        {
            // Old-style icon: transparency comes from the AND mask.
            int mw = 0, mh = 0;
            auto mask = DibPixels(ii.hbmMask, mw, mh);
            for (int y = 0; y < h; ++y) for (int x = 0; x < w; ++x)
            {
                size_t i = ((size_t)y * w + x) * 4;
                px[i + 3] = (mask.size() > i && mask[i] == 0) ? 255 : 0;
            }
        }
        if (ii.hbmColor) DeleteObject(ii.hbmColor);
        if (ii.hbmMask) DeleteObject(ii.hbmMask);
        return ToImage(px, w, h, true);
    }

    static bool IsImageFile(const std::wstring& p)
    {
        const wchar_t* x = PathFindExtensionW(p.c_str());
        return x && (_wcsicmp(x, L".png") == 0 || _wcsicmp(x, L".jpg") == 0 || _wcsicmp(x, L".jpeg") == 0 || _wcsicmp(x, L".bmp") == 0);
    }

    // File -> IRandomAccessStream synchronously (no blocking wait on the UI thread).
    static winrt::Windows::Storage::Streams::IRandomAccessStream StreamFromFile(const std::wstring& path)
    {
        winrt::com_ptr<IStream> fs;
        if (FAILED(SHCreateStreamOnFileEx(path.c_str(), STGM_READ | STGM_SHARE_DENY_NONE, 0, FALSE, nullptr, fs.put()))) return nullptr;
        winrt::Windows::Storage::Streams::IRandomAccessStream ras{ nullptr };
        if (FAILED(CreateRandomAccessStreamOverStream(fs.get(), BSOS_DEFAULT, winrt::guid_of<winrt::Windows::Storage::Streams::IRandomAccessStream>(), winrt::put_abi(ras)))) return nullptr;
        return ras;
    }

    static mux::Media::ImageSource BitmapFromFile(const std::wstring& path)
    {
        auto ras = StreamFromFile(path);
        if (!ras) { Log(L"image unreadable: %s", path.c_str()); return nullptr; }
        mux::Media::Imaging::BitmapImage bi;
        bi.SetSourceAsync(ras);
        return bi;
    }

    // Packaged app icons resolve to a plated "Name.scale-NNN.png"; small shell icons use the
    // unplated target-size asset next to it, as the shell does ("Name.targetsize-16_altform-
    // unplated.png", "-lightunplated" in light theme). Falls back to the resolved file.
    static bool DarkTheme();
    static std::wstring TargetSizeVariant(const std::wstring& resolved, int size)
    {
        auto slash = resolved.find_last_of(L'\\');
        auto dot = resolved.find(L'.', slash == std::wstring::npos ? 0 : slash + 1);
        if (dot == std::wstring::npos) return resolved;
        std::wstring base = resolved.substr(0, dot), ext = PathFindExtensionW(resolved.c_str());
        const wchar_t* unplated = DarkTheme() ? L"_altform-unplated" : L"_altform-lightunplated";
        for (int t : { size, 16, 20, 24, 32 })
            for (const wchar_t* alt : { unplated, L"_altform-unplated", L"" })
            {
                std::wstring c = base + L".targetsize-" + std::to_wstring(t) + alt + ext;
                if (GetFileAttributesW(c.c_str()) != INVALID_FILE_ATTRIBUTES) return c;
            }
        return resolved;
    }

    // "path,-id" | "path,index" | "path" (.ico) | "@{...}" (indirect) -> image of `size` pixels
    static mux::Media::ImageSource ImageFromRef(const std::wstring& refIn, int size)
    {
        if (refIn.empty()) return nullptr;
        std::wstring ref = refIn;
        if (ref[0] == L'@')
        {
            // Packaged apps: "@{PackageFullName?ms-resource://...}" (IAssocHandler adds ",index").
            if (auto c = ref.find_last_of(L','); c != std::wstring::npos && ref.find(L'}', c) == std::wstring::npos && ref.back() != L'}')
                ref.erase(c);
            ref = LoadIndirect(ref);
            if (IsImageFile(ref)) return BitmapFromFile(TargetSizeVariant(ref, size));
        }
        int index = 0;
        std::wstring path = ref;
        auto comma = ref.find_last_of(L',');
        if (comma != std::wstring::npos)
        {
            std::wstring tail = ref.substr(comma + 1);
            if (!tail.empty() && (iswdigit(tail[0]) || tail[0] == L'-')) { index = _wtoi(tail.c_str()); path = ref.substr(0, comma); }
        }
        path.erase(std::remove(path.begin(), path.end(), L'"'), path.end());
        wchar_t exp[MAX_PATH * 2]{};
        ExpandEnvironmentStringsW(path.c_str(), exp, ARRAYSIZE(exp));
        HICON icon = nullptr;
        if (SHDefExtractIconW(exp, index, 0, &icon, nullptr, MAKELONG(size, size)) != S_OK || !icon)
            icon = (HICON)LoadImageW(nullptr, exp, IMAGE_ICON, size, size, LR_LOADFROMFILE);
        if (!icon) return nullptr;
        auto img = ImageFromIcon(icon);
        DestroyIcon(icon);
        return img;
    }

    // ---- Explorer's own icons ------------------------------------------------------------
    // Explorer draws its verbs with SVGs installed in its package, one per command id and per
    // theme / contrast (menu_policy.json "iconAssets"). We load the installed files at runtime;
    // nothing is copied into Deskboxes.
    static bool DarkTheme()
    {
        return mux::Application::Current().RequestedTheme() == mux::ApplicationTheme::Dark;
    }

    static std::wstring Lower(std::wstring s) { CharLowerBuffW(s.data(), (DWORD)s.size()); return s; }

    static const std::map<std::wstring, std::wstring>& AssetIndex()
    {
        static std::map<std::wstring, std::wstring> idx;
        static bool done = false;
        if (done) return idx;
        done = true;
        const auto& pol = GetPolicy();
        if (pol.iconAssetPackage.empty()) return idx;
        std::wstring root;
        try
        {
            winrt::Windows::Management::Deployment::PackageManager pm;
            auto it = pm.FindPackagesForUser(L"", pol.iconAssetPackage).First();
            if (it.HasCurrent()) root = it.Current().InstalledLocation().Path().c_str();
        }
        catch (winrt::hresult_error const& e) { Log(L"icon assets: package query failed: %s", e.message().c_str()); }
        if (root.empty()) { Log(L"icon assets: package %s not installed; using fallbacks", pol.iconAssetPackage.c_str()); return idx; }
        HIGHCONTRAST hc{ sizeof(hc) };
        SystemParametersInfoW(SPI_GETHIGHCONTRAST, sizeof(hc), &hc, 0);
        std::wstring sub;
        if (hc.dwFlags & HCF_HIGHCONTRASTON)
        {
            COLORREF w = GetSysColor(COLOR_WINDOW);
            sub = (GetRValue(w) + GetGValue(w) + GetBValue(w)) < 384 ? L"contrast-black" : L"contrast-white";
        }
        else sub = std::wstring(L"contrast-standard\\theme-") + (DarkTheme() ? L"dark" : L"light");
        std::wstring dir = root + L"\\" + pol.iconAssetPath + L"\\" + sub;
        WIN32_FIND_DATAW fd{};
        HANDLE h = FindFirstFileW((dir + L"\\*.svg").c_str(), &fd);
        if (h != INVALID_HANDLE_VALUE)
        {
            do
            {
                std::wstring n = fd.cFileName;
                idx[Lower(n.substr(0, n.size() - 4))] = dir + L"\\" + n;
            } while (FindNextFileW(h, &fd));
            FindClose(h);
        }
        Log(L"icon assets: %zu in %s", idx.size(), dir.c_str());
        return idx;
    }

    static std::wstring AssetPathFor(const std::wstring& id)
    {
        const auto& idx = AssetIndex();
        if (idx.empty() || id.empty()) return {};
        const auto& names = GetPolicy().iconAssetNames;
        std::vector<std::wstring> keys;
        if (auto it = names.find(id); it != names.end()) keys.push_back(it->second);
        keys.push_back(id);
        keys.push_back(L"windows." + id);
        for (auto& k : keys)
            if (auto it = idx.find(Lower(k)); it != idx.end()) return it->second;
        return {};
    }

    // WinUI rasterizes an SVG without anti-aliasing at its own 16 px: hard, binary edges where
    // Explorer's menu has soft ones (compared pixel by pixel, 2026-11-19). Rasterized at 4x and
    // scaled down by the image, the edges come out anti-aliased like Explorer's.
    static mux::Media::ImageSource SvgFromFile(const std::wstring& path, int px)
    {
        static std::map<std::wstring, mux::Media::ImageSource> cache;
        if (auto it = cache.find(path); it != cache.end()) return it->second;
        mux::Media::ImageSource result{ nullptr };
        if (auto ras = StreamFromFile(path))
        {
            mux::Media::Imaging::SvgImageSource svg;
            svg.RasterizePixelWidth(px * 4.0);
            svg.RasterizePixelHeight(px * 4.0);
            svg.SetSourceAsync(ras);
            result = svg;
        }
        else Log(L"icon asset unreadable: %s", path.c_str());
        cache.insert_or_assign(path, result);
        return result;
    }

    // Icon of the item itself (Explorer shows it on the default verb: folder, exe, shortcut target app, document type).
    static mux::Media::ImageSource ItemIcon(const std::wstring& parsing)
    {
        SHFILEINFOW sfi{};
        PIDLIST_ABSOLUTE pidl = nullptr;
        if (FAILED(SHParseDisplayName(parsing.c_str(), nullptr, &pidl, 0, nullptr))) return nullptr;
        DWORD_PTR ok = SHGetFileInfoW((LPCWSTR)pidl, 0, &sfi, sizeof(sfi), SHGFI_PIDL | SHGFI_ICON | SHGFI_SMALLICON);
        CoTaskMemFree(pidl);
        if (!ok || !sfi.hIcon) return nullptr;
        auto img = ImageFromIcon(sfi.hIcon);
        DestroyIcon(sfi.hIcon);
        return img;
    }

    static mux::Media::ImageSource ImageFromParsingName(const std::wstring& name, int px);

    // Icon on the default verb ("Open"). Explorer shows the default app's icon: Photos for .png,
    // Notepad for .txt, File Explorer's for folders (docs/fidelity/explorer/png.png, txt.png,
    // folder.png). The app comes from the association (ASSOCSTR_APPICONREFERENCE: an AUMID).
    // Without one (.exe, .lnk, Win32 handlers here) the item's own icon is used, which is what
    // the .exe / .lnk / .docx captures show.
    static mux::Media::ImageSource DefaultVerbIcon(const std::wstring& item, int px)
    {
        DWORD attrs = GetFileAttributesW(item.c_str());
        std::wstring assoc = (attrs != INVALID_FILE_ATTRIBUTES && (attrs & FILE_ATTRIBUTE_DIRECTORY)) ? L"Folder" : PathFindExtensionW(item.c_str());
        if (!assoc.empty())
        {
            wchar_t buf[512]{}; DWORD n = ARRAYSIZE(buf);
            if (SUCCEEDED(AssocQueryStringW(ASSOCF_NONE, ASSOCSTR_APPICONREFERENCE, assoc.c_str(), nullptr, buf, &n)) && *buf)
            {
                std::wstring ref = buf;
                mux::Media::ImageSource img{ nullptr };
                // Observed forms: "@{PackageFullName?ms-resource://...png}" (packaged app), an AUMID,
                // "path,index", and "%1" (Folder: the item's own icon).
                if (ref[0] == L'@' || ref.find(L'\\') != std::wstring::npos)
                    img = ImageFromRef(ref, px);
                else if (ref.find(L'!') != std::wstring::npos)
                    img = ImageFromParsingName(L"shell:AppsFolder\\" + ref, px);
                if (img) return img;
            }
        }
        return ItemIcon(item);
    }

    // `itemPath`: the primary item when `e` is the default verb of an item menu.
    static muxc::IconElement IconFor(const MenuEntry& e, int px, const std::wstring* itemPath = nullptr)
    {
        if (e.color.size() == 7 && e.color[0] == L'#')
        {
            // A color swatch: a filled circle in the color, with a thin ring so very dark or very
            // light colors still show on the menu background.
            auto v = std::wcstoul(e.color.c_str() + 1, nullptr, 16);
            winrt::Windows::UI::Color c{ 255, (uint8_t)(v >> 16), (uint8_t)(v >> 8), (uint8_t)v };
            muxc::FontIcon fi; fi.Glyph(L"\uEA3B"); // StatusCircleInner: filled circle
            fi.Foreground(mux::Media::SolidColorBrush(c));
            return fi;
        }
        mux::Media::ImageSource src{ nullptr };
        // New > Folder: Explorer's context menu shows the folder icon itself (yellow, as on the
        // desktop); its outlined "newfolder" asset belongs to the toolbar.
        if (_wcsicmp(e.id.c_str(), L"NewFolder") == 0)
        {
            SHSTOCKICONINFO sii{ sizeof(sii) };
            if (SUCCEEDED(SHGetStockIconInfo(SIID_FOLDER, SHGSI_ICONLOCATION, &sii)))
                src = ImageFromRef(std::wstring(sii.szPath) + L"," + std::to_wstring(sii.iIcon), px);
        }
        if (!src && itemPath && e.isDefault) src = DefaultVerbIcon(*itemPath, px);
        if (!src && e.source != Source::Packaged)
            if (auto p = AssetPathFor(e.id); !p.empty()) src = SvgFromFile(p, px);
        if (!src && !e.iconRef.empty()) src = ImageFromRef(e.iconRef, px);
        if (!src && e.classicBitmap)
        {
            int w = 0, h = 0;
            auto pixels = DibPixels(e.classicBitmap, w, h);
            src = ToImage(pixels, w, h, false); // menu bitmaps are premultiplied 32bpp already
        }
        if (src)
        {
            muxc::ImageIcon ii; ii.Source(src);
            return ii;
        }
        if (!e.glyph.empty())
        {
            muxc::FontIcon fi; fi.Glyph(e.glyph);
            return fi;
        }
        return nullptr;
    }

    // ---- view state --------------------------------------------------------------------
    struct View
    {
        std::shared_ptr<BuiltMenu> m;
        mux::Window window{ nullptr };
        HWND hwnd = nullptr;
        muxc::CommandBarFlyout flyout{ nullptr };
        MenuViewEvents ev;
        std::function<void()> pending;     // the chosen action, run after the flyout closed
        std::wstring invoked;
        std::unique_ptr<NewMenu> newMenu;
        std::vector<OpenWithEntry> openWith;
        int iconPx = 16;
        std::map<const MenuEntry*, std::wstring> keys; // access keys after de-duplication
    };
    static std::shared_ptr<View> g_view;

    // ---- dismissal ---------------------------------------------------------------------
    // The flyout closes by itself when its window loses the activation. A menu that never got
    // it (a click into another app right after the right-click, before Windows let our window
    // come to the front) never would: it stayed up, half behind that app, and blocked every
    // later menu. So while a menu is open, a press anywhere outside our windows, or another app
    // coming to the front, closes it, as Explorer's menus close.
    static HHOOK g_mouseHook = nullptr;
    static HWINEVENTHOOK g_fgHook = nullptr;

    static bool IsOurs(HWND h)
    {
        DWORD pid = 0;
        if (h) GetWindowThreadProcessId(h, &pid);
        return pid == GetCurrentProcessId();
    }

    static void DismissSoon()
    {
        auto v = g_view;
        if (!v || !v->window) return;
        v->window.DispatcherQueue().TryEnqueue([wv = std::weak_ptr<View>(v)]
        {
            auto v = wv.lock();
            if (v && v->flyout) { try { v->flyout.Hide(); } catch (winrt::hresult_error const&) {} }
        });
    }

    static LRESULT CALLBACK DismissMouseProc(int code, WPARAM w, LPARAM l)
    {
        if (code == HC_ACTION && (w == WM_LBUTTONDOWN || w == WM_RBUTTONDOWN || w == WM_MBUTTONDOWN || w == WM_XBUTTONDOWN))
        {
            auto* m = reinterpret_cast<MSLLHOOKSTRUCT*>(l);
            if (!IsOurs(WindowFromPoint(m->pt))) DismissSoon();
        }
        return CallNextHookEx(nullptr, code, w, l);
    }

    static void CALLBACK DismissForegroundProc(HWINEVENTHOOK, DWORD, HWND h, LONG, LONG, DWORD, DWORD)
    {
        if (h && !IsOurs(h)) DismissSoon();
    }

    static void WatchDismiss(bool on)
    {
        if (on)
        {
            if (!g_mouseHook) g_mouseHook = SetWindowsHookExW(WH_MOUSE_LL, DismissMouseProc, GetModuleHandleW(nullptr), 0);
            if (!g_fgHook) g_fgHook = SetWinEventHook(EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND, nullptr, DismissForegroundProc, 0, 0, WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS);
        }
        else
        {
            if (g_mouseHook) { UnhookWindowsHookEx(g_mouseHook); g_mouseHook = nullptr; }
            if (g_fgHook) { UnhookWinEvent(g_fgHook); g_fgHook = nullptr; }
        }
    }

    void HideMenuView()
    {
        DismissSoon();
    }

    // Access keys (KeyTips). Explorer numbers keys that occur more than once in one menu, in
    // display order, command row first: Copy "C1" ... Customize menu "C2" (verified in the
    // captures docs/fidelity/explorer/*.png). Unique keys stay single letters.
    static void AssignAccessKeys(const std::vector<MenuEntry*>& order)
    {
        auto v = g_view;
        std::map<std::wstring, int> count, seen;
        for (auto* e : order) if (!e->separator && !e->accessKey.empty()) count[e->accessKey]++;
        for (auto* e : order)
        {
            if (e->separator || e->accessKey.empty()) continue;
            const auto& k = e->accessKey;
            v->keys[e] = count[k] > 1 ? k + std::to_wstring(++seen[k]) : k;
        }
    }
    static std::wstring KeyOf(const MenuEntry* e)
    {
        auto it = g_view->keys.find(e);
        return it == g_view->keys.end() ? std::wstring() : it->second;
    }

    static void Choose(const std::wstring& id, std::function<void()> action)
    {
        auto v = g_view;
        if (!v) return;
        v->invoked = id;
        v->pending = std::move(action);
        v->flyout.Hide();
    }

    static void LaunchUri(const std::wstring& uri)
    {
        SHELLEXECUTEINFOW sei{ sizeof(sei) };
        sei.fMask = SEE_MASK_NOASYNC; sei.lpVerb = L"open"; sei.lpFile = uri.c_str(); sei.nShow = SW_SHOWNORMAL;
        ShellExecuteExW(&sei);
    }

    static void Invoke(MenuEntry* e)
    {
        auto v = g_view;
        std::shared_ptr<BuiltMenu> m = v->m;
        HWND owner = v->hwnd;
        POINT pt = m->req.pt;
        switch (e->source)
        {
        case Source::Classic:
            if (_wcsicmp(e->id.c_str(), L"rename") == 0)
                Choose(e->id, [ev = g_view->ev] { if (ev.command) ev.command(L"rename"); }); // the core edits in place
            else if (_wcsicmp(e->id.c_str(), L"delete") == 0)
                Choose(e->id, [ev = g_view->ev] { if (ev.command) ev.command(L"delete"); }); // the core deletes, so Undo can bring it back
            else
                Choose(e->id, [m, e, owner, pt] { HRESULT hr = InvokeClassic(*m, *e, owner, pt); Log(L"classic %s -> 0x%08X", e->id.c_str(), hr); });
            break;
        case Source::Packaged:
        case Source::CommandStore:
            Choose(e->id, [m, e] { InvokeExplorerCommand(*m, *e); });
            break;
        case Source::Synthetic:
            if (e->id == L"showMoreOptions")
                Choose(e->id, [m, owner, pt] { ShowClassicMenu(*m, owner, pt); });
            else // Customize menu / Manage extensions: the Settings page (URI UNVERIFIED)
                Choose(e->id, [] { LaunchUri(GetPolicy().customizeMenuUri); });
            break;
        case Source::Deskboxes:
            // While our window still has the foreground (the click on the entry): let the core take
            // it, so an in-place title edit gets the keyboard once the menu has closed.
            AllowSetForegroundWindow(ASFW_ANY);
            Choose(e->id, [id = e->id, ev = g_view->ev] { if (ev.command) ev.command(id); });
            break;
        }
    }

    // Policy value without indirect-string resolution (ids, URIs).
    static std::wstring PolicyRaw(const wchar_t* key)
    {
        auto& s = GetPolicy().strings;
        auto it = s.find(key);
        return it == s.end() ? std::wstring() : it->second;
    }

    // Group policy "Do not show 'Search the Microsoft Store' in Open with" (NoUseStoreOpenWith).
    static bool StoreOpenWithDisabled()
    {
        for (HKEY root : { HKEY_LOCAL_MACHINE, HKEY_CURRENT_USER })
        {
            DWORD v = 0, sz = sizeof(v);
            if (RegGetValueW(root, L"SOFTWARE\\Policies\\Microsoft\\Windows\\Explorer", L"NoUseStoreOpenWith", RRF_RT_REG_DWORD, nullptr, &v, &sz) == ERROR_SUCCESS && v) return true;
        }
        return false;
    }

    // Icon of any shell item by parsing name (e.g. an app: shell:AppsFolder\<AUMID>).
    static mux::Media::ImageSource ImageFromParsingName(const std::wstring& name, int px)
    {
        winrt::com_ptr<IShellItemImageFactory> f;
        if (FAILED(SHCreateItemFromParsingName(name.c_str(), nullptr, IID_PPV_ARGS(f.put())))) { Log(L"no shell item %s", name.c_str()); return nullptr; }
        HBITMAP bmp = nullptr;
        if (FAILED(f->GetImage({ px, px }, SIIGBF_ICONONLY, &bmp)) || !bmp) return nullptr;
        int w = 0, h = 0;
        auto pixels = DibPixels(bmp, w, h);
        DeleteObject(bmp);
        return ToImage(pixels, w, h, true);
    }

    static void AddItems(muxc::MenuFlyout const& f, std::vector<MenuEntry>& entries);

    // Windows' New submenu lists Folder, Shortcut and the file types without a divider
    // (docs/fidelity/explorer/*.New.png); the classic menu it comes from has one after Shortcut.
    static void DropSeparators(std::vector<MenuEntry>& entries)
    {
        entries.erase(std::remove_if(entries.begin(), entries.end(), [](const MenuEntry& x) { return x.separator; }), entries.end());
    }

    static void FillSubmenu(muxc::MenuFlyout const& f, MenuEntry* e)
    {
        auto v = g_view;
        auto& m = *v->m;
        f.Items().Clear();
        const bool openWith = VerbIn(GetPolicy().alwaysSubmenu, e->id) && _wcsicmp(e->id.c_str(), L"openas") == 0;
        const bool shellNew = !m.req.background && _wcsicmp(e->id.c_str(), L"New") == 0 && e->source == Source::Classic;
        if (openWith)
        {
            // Windows' "Open with" (docs/fidelity/explorer/*.openas.png): the recommended
            // handlers sorted by name, "Search the Microsoft Store" with the Store's icon, then
            // "Choose another app" without an icon; no separators. Labels are Windows' own
            // (shell32 string table, menu_policy.json).
            v->openWith = OpenWithHandlers(m);
            std::stable_sort(v->openWith.begin(), v->openWith.end(), [](const OpenWithEntry& x, const OpenWithEntry& y)
            {
                return CompareStringEx(LOCALE_NAME_USER_DEFAULT, LINGUISTIC_IGNORECASE, x.name.c_str(), -1, y.name.c_str(), -1, nullptr, nullptr, 0) == CSTR_LESS_THAN;
            });
            auto mp = v->m; HWND owner = v->hwnd;
            for (auto& h : v->openWith)
            {
                muxc::MenuFlyoutItem it; it.Text(h.name);
                MenuEntry tmp; tmp.iconRef = h.iconRef;
                if (auto ic = IconFor(tmp, v->iconPx)) it.Icon(ic);
                IAssocHandler* hp = h.handler.get();
                it.Click([hp, mp](auto&&, auto&&) { Choose(L"openwith", [hp, mp] { InvokeOpenWith(*mp, hp); }); });
                f.Items().Append(it);
            }
            auto labeled = [&](const wchar_t* key) {
                muxc::MenuFlyoutItem it;
                std::wstring raw = PolicyString(key, L""), ak;
                if (raw.empty()) Log(L"policy string %s unresolved", key);
                it.Text(StripMnemonic(raw, &ak));
                if (!ak.empty()) it.AccessKey(ak);
                return it;
            };
            const wchar_t* ext = m.req.items.empty() ? L"" : PathFindExtensionW(m.req.items.front().c_str());
            if (!StoreOpenWithDisabled() && ext && *ext)
            {
                auto store = labeled(L"openWithStore");
                if (auto img = ImageFromParsingName(L"shell:AppsFolder\\" + PolicyRaw(L"openWithStoreApp"), v->iconPx))
                {
                    muxc::ImageIcon ii; ii.Source(img); store.Icon(ii);
                }
                // URI form UNVERIFIED (no capture of the page it opens); see REPORT.md.
                std::wstring uri = PolicyRaw(L"openWithStoreUri");
                if (auto at = uri.find(L"{ext}"); at != std::wstring::npos) uri.replace(at, 5, ext + 1);
                store.Click([uri](auto&&, auto&&) { Choose(L"openwithstore", [uri] { LaunchUri(uri); }); });
                f.Items().Append(store);
            }
            auto other = labeled(L"chooseAnotherApp");
            other.Click([mp, owner](auto&&, auto&&) { Choose(L"openas", [mp, owner] { InvokeClassicVerb(*mp, L"openas", owner, mp->req.pt); }); });
            f.Items().Append(other);
            return;
        }
        if (shellNew)
        {
            v->newMenu = BuildNewMenu(m.siteFolder.get(), v->hwnd);
            DropSeparators(v->newMenu->items);
            AddItems(f, v->newMenu->items);
            return;
        }
        switch (e->source)
        {
        case Source::Classic: LoadClassicSubmenu(m, *e); break;
        case Source::Packaged:
        case Source::CommandStore: LoadChildren(*e, m.items.get()); break;
        default: break;
        }
        if (_wcsicmp(e->id.c_str(), L"New") == 0) DropSeparators(e->children);
        AddItems(f, e->children);
    }

    static void AddItems(muxc::MenuFlyout const& f, std::vector<MenuEntry>& entries)
    {
        auto v = g_view;
        std::vector<MenuEntry*> order;
        for (auto& c : entries) order.push_back(&c);
        AssignAccessKeys(order);
        for (auto& c : entries)
        {
            MenuEntry* e = &c;
            if (e->separator) { f.Items().Append(muxc::MenuFlyoutSeparator()); continue; }
            if (e->hasSubmenu)
            {
                // Nested submenus are filled when built (classic: one level; commands: their children).
                muxc::MenuFlyoutSubItem sub; sub.Text(e->text);
                if (auto ic = IconFor(*e, v->iconPx)) sub.Icon(ic);
                if (auto k = KeyOf(e); !k.empty()) sub.AccessKey(k);
                muxc::MenuFlyout tmp;
                FillSubmenu(tmp, e);
                while (tmp.Items().Size()) { auto x = tmp.Items().GetAt(0); tmp.Items().RemoveAt(0); sub.Items().Append(x); }
                f.Items().Append(sub);
                continue;
            }
            // Check marks: radio groups (Sort by, icon size) and toggles (Auto arrange ...).
            muxc::MenuFlyoutItem it{ nullptr };
            if (e->radio) { muxc::RadioMenuFlyoutItem r; r.GroupName(std::to_wstring((uintptr_t)&entries)); r.IsChecked(e->checked); it = r; }
            else if (e->checkable) { muxc::ToggleMenuFlyoutItem t; t.IsChecked(e->checked); it = t; }
            else it = muxc::MenuFlyoutItem();
            it.Text(e->text);
            if (auto ic = IconFor(*e, v->iconPx)) it.Icon(ic);
            if (!e->accel.empty()) it.KeyboardAcceleratorTextOverride(e->accel);
            if (auto k = KeyOf(e); !k.empty()) { it.AccessKey(k); it.KeyTipPlacementMode(mux::Input::KeyTipPlacementMode::Right); }
            it.IsEnabled(!e->disabled);
            // Subcommands of an extension or a classic submenu: invoke through their owner.
            bool newItem = v->newMenu && !v->newMenu->items.empty() && e >= &v->newMenu->items.front() && e <= &v->newMenu->items.back();
            if (newItem)
            {
                HWND owner = v->hwnd;
                it.Click([e, owner](auto&&, auto&&) { auto vv = g_view; Choose(L"new", [vv, e, owner] { InvokeNewMenu(*vv->newMenu, *e, owner); }); });
            }
            else
                it.Click([e](auto&&, auto&&) { Invoke(e); });
            f.Items().Append(it);
        }
    }

    static muxc::AppBarButton MakeButton(MenuEntry* e, bool inRow)
    {
        auto v = g_view;
        muxc::AppBarButton b;
        b.Label(e->text);
        const std::wstring* item = v->m->req.background || v->m->req.items.empty() ? nullptr : &v->m->req.items.front();
        if (auto ic = IconFor(*e, v->iconPx, item)) b.Icon(ic);
        if (!e->accel.empty()) b.KeyboardAcceleratorTextOverride(e->accel);
        if (auto k = KeyOf(e); !k.empty()) b.AccessKey(k);
        mux::Automation::AutomationProperties::SetAutomationId(b, e->id);
        // Explorer: row KeyTips above the buttons (also when the row is at the bottom), list
        // KeyTips at the right edge (docs/fidelity/explorer/desktop_docx.png, folder.png).
        b.KeyTipPlacementMode(inRow ? mux::Input::KeyTipPlacementMode::Top : mux::Input::KeyTipPlacementMode::Right);
        b.IsEnabled(!e->disabled);
        if (e->hasSubmenu)
        {
            muxc::MenuFlyout sub;
            sub.Placement(muxc::Primitives::FlyoutPlacementMode::RightEdgeAlignedTop);
            // The sender, not a captured `sub`: a flyout holding a handler that holds the flyout
            // is a reference cycle, and every menu stayed in memory (2026-11-19).
            sub.Opening([e](auto&& s, auto&&) { auto sub = s.template as<muxc::MenuFlyout>(); if (sub.Items().Size() == 0) FillSubmenu(sub, e); });
            b.Flyout(sub);
        }
        else
            b.Click([e](auto&&, auto&&) { Invoke(e); });
        return b;
    }

    // KeyTips (menu opened from the keyboard), as Explorer shows them: row and list together.
    // The list lives in the CommandBar's overflow presenter, a separate popup; KeyTip display mode
    // only covers the access-key scope that holds focus (the row's popup root), so the row's root
    // is declared a scope and the list's buttons are attached to it with AccessKeyScopeOwner.
    // Runs after the popups are in the tree (XamlRoot is null while Opened is raised).
    static void ShowKeyTips(View& v)
    {
        mux::DependencyObject rowRoot{ nullptr };
        auto primary = v.flyout.PrimaryCommands();
        auto secondary = v.flyout.SecondaryCommands();
        mux::DependencyObject start = primary.Size() ? primary.GetAt(0).as<mux::DependencyObject>()
                                     : secondary.Size() ? secondary.GetAt(0).as<mux::DependencyObject>() : nullptr;
        for (auto p = start; p; p = mux::Media::VisualTreeHelper::GetParent(p)) rowRoot = p;
        auto owner = rowRoot ? rowRoot.try_as<mux::UIElement>() : nullptr;
        if (!owner) { Log(L"KeyTips: menu not in a live tree; not shown"); return; }
        owner.IsAccessKeyScope(true);
        if (primary.Size())
            for (auto const& c : secondary) c.as<mux::UIElement>().AccessKeyScopeOwner(owner);
        mux::Input::AccessKeyManager::EnterDisplayMode(owner.XamlRoot());
    }

    // CommandBarFlyout's list (CommandBarOverflowPresenter) has MaxHeight 480 in WinUI's own style,
    // then scrolls. Explorer's menus stay under it; ours carry Deskboxes' entries too and went
    // over it on the desktop (Customize menu was cut off, 2026-11-19). Lift it to the work area.
    static void LiftOverflowCap(View& v)
    {
        auto sec = v.flyout.SecondaryCommands();
        if (!sec.Size()) return;
        MONITORINFO mi{ sizeof(mi) };
        GetMonitorInfoW(MonitorFromPoint(v.m->req.pt, MONITOR_DEFAULTTONEAREST), &mi);
        UINT dpi = GetDpiForWindow(v.hwnd);
        double maxH = (mi.rcWork.bottom - mi.rcWork.top) * 96.0 / (dpi ? dpi : 96) - 16.0;
        for (mux::DependencyObject p = sec.GetAt(sec.Size() - 1).as<mux::DependencyObject>(); p; p = mux::Media::VisualTreeHelper::GetParent(p))
        {
            if (auto op = p.try_as<muxc::CommandBarOverflowPresenter>())
            {
                if (op.MaxHeight() < maxH) op.MaxHeight(maxH);
                return;
            }
        }
        Log(L"menu list cap: overflow presenter not found");
    }

    // Where the menu opens, as Explorer's does: its top-left corner at the pointer; above the
    // pointer (bottom-left corner there) when there is no room below, and to the left of it
    // when there is no room on the right. The menu's size is estimated from its entries (the
    // same metrics WinUI uses: 56 px command row, 32 px items, 9 px separators, 8 px padding).
    static mux::Controls::Primitives::FlyoutPlacementMode MenuPlacement(View& v)
    {
        using P = mux::Controls::Primitives::FlyoutPlacementMode;
        MONITORINFO mi{ sizeof(mi) };
        GetMonitorInfoW(MonitorFromPoint(v.m->req.pt, MONITOR_DEFAULTTONEAREST), &mi);
        UINT dpi = GetDpiForWindow(v.hwnd);
        double k = (dpi ? dpi : 96) / 96.0;
        double h = 8.0 + (v.m->row.empty() ? 0.0 : 56.0);
        for (auto& e : v.m->list) h += e.separator ? 9.0 : 32.0;
        double w = v.m->row.empty() ? 260.0 : (std::max)(260.0, 65.0 * v.m->row.size());
        POINT pt = v.m->req.pt;
        bool below = (mi.rcWork.bottom - pt.y) >= h * k || (pt.y - mi.rcWork.top) < h * k && (mi.rcWork.bottom - pt.y) >= (pt.y - mi.rcWork.top);
        bool right = (mi.rcWork.right - pt.x) >= w * k;
        if (below) return right ? P::BottomEdgeAlignedLeft : P::BottomEdgeAlignedRight;
        return right ? P::TopEdgeAlignedLeft : P::TopEdgeAlignedRight;
    }

    void ShowMenuView(std::shared_ptr<BuiltMenu> menu, MenuViewEvents events)
    {
        auto v = std::make_shared<View>();
        g_view = v;
        v->m = menu;
        v->ev = std::move(events);
        auto t0 = NowMs();

        // Host window: a 1x1 context-menu window at the anchor point, fully transparent.
        v->window = mux::Window();
        muxc::Grid root;
        // The flyout's target: a 1x1 element at the window's top-left (the window itself is
        // clamped larger than 1x1, and a placement below it opened the menu far under the pointer).
        muxc::Border anchor;
        anchor.Width(1);
        anchor.Height(1);
        anchor.HorizontalAlignment(mux::HorizontalAlignment::Left);
        anchor.VerticalAlignment(mux::VerticalAlignment::Top);
        root.Children().Append(anchor);
        v->window.Content(root);
        auto aw = v->window.AppWindow();
        aw.SetPresenter(winrt::Microsoft::UI::Windowing::OverlappedPresenter::CreateForContextMenu());
        aw.IsShownInSwitchers(false);
        v->hwnd = winrt::Microsoft::UI::GetWindowFromWindowId(aw.Id());
        SetWindowLongPtrW(v->hwnd, GWL_EXSTYLE, GetWindowLongPtrW(v->hwnd, GWL_EXSTYLE) | WS_EX_LAYERED | WS_EX_TOOLWINDOW);
        SetLayeredWindowAttributes(v->hwnd, 0, 0, LWA_ALPHA);
        POINT pt = menu->req.pt;
        auto place = MenuPlacement(*v);
        UINT dpi = GetDpiForWindow(v->hwnd);
        // Opening upward, the flyout keeps its (empty) icon-row space under the list: 52 px at
        // 100 % between the menu's visible bottom and the pointer. Shift the anchor down by that
        // so the bottom-left corner sits at the pointer, as Explorer's does.
        using PM = mux::Controls::Primitives::FlyoutPlacementMode;
        bool up = place == PM::TopEdgeAlignedLeft || place == PM::TopEdgeAlignedRight;
        int dy = up && menu->row.empty() ? MulDiv(52, dpi ? dpi : 96, 96) : 0;
        aw.MoveAndResize({ pt.x, pt.y + dy, 1, 1 });
        v->iconPx = MulDiv(16, dpi ? dpi : 96, 96);

        // The flyout.
        v->flyout = muxc::CommandBarFlyout();
        v->flyout.AlwaysExpanded(true);
        v->flyout.ShouldConstrainToRootBounds(false);
        {
            std::vector<MenuEntry*> order;
            for (auto& e : menu->row) order.push_back(&e);
            for (auto& e : menu->list) order.push_back(&e);
            AssignAccessKeys(order);
        }
        // Explorer separates the row's buttons with AppBarSeparators: its captures show the
        // divider lines and a 65 px button pitch (56 px button + separator) where plain
        // buttons give 60 px; the list then widens to the row (316 px with five buttons).
        for (auto& e : menu->row)
        {
            if (v->flyout.PrimaryCommands().Size()) v->flyout.PrimaryCommands().Append(muxc::AppBarSeparator());
            v->flyout.PrimaryCommands().Append(MakeButton(&e, true));
        }
        for (auto& e : menu->list)
        {
            if (e.separator) { v->flyout.SecondaryCommands().Append(muxc::AppBarSeparator()); continue; }
            v->flyout.SecondaryCommands().Append(MakeButton(&e, false));
        }
        // Handlers hold the view weakly: the view holds the flyout and the window, and a strong
        // capture kept every closed menu alive with its XAML tree and shell handlers (the host
        // grew by tens of MB per menu; 2026-11-19).
        std::weak_ptr<View> wv = v;
        v->flyout.Opened([wv, t0](auto&&, auto&&)
        {
            auto v = wv.lock();
            if (!v) return;
            double ms = NowMs() - t0;
            Log(L"menu view opened in %.1f ms (model %.1f ms)", ms, v->m->msTotal);
            if (v->ev.opened) v->ev.opened(ms);
            // Building the menu leaves the wait cursor up (shell handlers set it) until the mouse
            // moves: the menu is ready, so show the arrow now, and let the pointer's window set
            // its own cursor (a move to where it already is).
            SetCursor(LoadCursorW(nullptr, IDC_ARROW));
            POINT cp{};
            if (GetCursorPos(&cp)) SetCursorPos(cp.x, cp.y);
            v->window.DispatcherQueue().TryEnqueue(winrt::Microsoft::UI::Dispatching::DispatcherQueuePriority::Low, [v] { LiftOverflowCap(*v); });
            if (v->m->req.keyboard)
                v->window.DispatcherQueue().TryEnqueue(winrt::Microsoft::UI::Dispatching::DispatcherQueuePriority::Low, [v] { ShowKeyTips(*v); });
        });
        v->flyout.Closed([wv](auto&&, auto&&)
        {
            auto v = wv.lock();
            if (!v) return;
            WatchDismiss(false);
            if (v->pending) v->pending();
            auto id = v->invoked;
            auto done = v->ev.done;
            // A keyboard-opened menu put its XamlRoot in access-key display mode: leave it before
            // the window goes. Close the window on the next dispatcher turn, not inside the
            // flyout's own Closed event (the host crashed after closing a keyboard-opened menu
            // when both happened here; 2026-11-19).
            if (v->m->req.keyboard)
            {
                try { mux::Input::AccessKeyManager::ExitDisplayMode(); }
                catch (winrt::hresult_error const& e) { Log(L"KeyTips: ExitDisplayMode failed 0x%08X", (unsigned)e.code().value); }
            }
            v->window.DispatcherQueue().TryEnqueue([v]
            {
                // Tear the view down: the window, the flyout and its commands, the shell objects.
                v->window.Close();
                try
                {
                    v->flyout.PrimaryCommands().Clear();
                    v->flyout.SecondaryCommands().Clear();
                }
                catch (winrt::hresult_error const&) {}
                v->flyout = nullptr;
                v->window = nullptr;
                v->newMenu.reset();
                v->openWith.clear();
                v->keys.clear();
                v->pending = nullptr;
                if (g_view == v) g_view.reset();
                TrimMemorySoon();
            });
            if (done) done(id);
        });

        // Show once the root is in the live tree (it has a XamlRoot then); Window.Activated can
        // arrive earlier and ShowAt fails with "This element does not have a XamlRoot".
        bool shown = false;
        root.Loaded([wv, shown, anchor, place](auto&&, auto&&) mutable
        {
            auto v = wv.lock();
            if (shown || !v) return;
            shown = true;
            mux::Controls::Primitives::FlyoutShowOptions o;
            o.Placement(place);
            o.ShowMode(mux::Controls::Primitives::FlyoutShowMode::Standard);
            v->flyout.ShowAt(anchor, o);
        });
        v->window.Activate();
        SetForegroundWindow(v->hwnd);
        WatchDismiss(true);
    }
}

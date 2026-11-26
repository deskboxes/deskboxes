#include "pch.h"
#include "MenuModel.h"
#include "Log.h"
#include <future>
#include <sstream>

using namespace winrt;

namespace dbx
{
    // All calls in this file run on the host's single shell STA worker thread, so the
    // cached IExplorerCommand instances stay in their apartment.

    static std::vector<PackagedRegistration> g_regs;
    static bool g_regsLoaded = false;
    static std::map<std::wstring, com_ptr<IClassFactory>> g_cfCache; // key: CLSID string

    static std::wstring ReadFileUtf8(const std::wstring& path)
    {
        HANDLE f = CreateFileW(path.c_str(), GENERIC_READ, FILE_SHARE_READ, nullptr, OPEN_EXISTING, 0, nullptr);
        if (f == INVALID_HANDLE_VALUE) return {};
        LARGE_INTEGER sz{}; GetFileSizeEx(f, &sz);
        std::string buf((size_t)sz.QuadPart, '\0');
        DWORD rd = 0; ReadFile(f, buf.data(), (DWORD)buf.size(), &rd, nullptr);
        CloseHandle(f);
        if (buf.size() >= 3 && (unsigned char)buf[0] == 0xEF) buf.erase(0, 3);
        return Utf8ToWide(buf);
    }

    // The packages' context-menu verbs (0.4-0.8 s: every package's manifest is read). No shell
    // objects: StartPackagedScan runs it on a thread of its own while the host starts.
    static std::vector<PackagedRegistration> ScanPackaged(bool* ok = nullptr)
    {
        std::vector<PackagedRegistration> regs;
        auto t0 = NowMs();
        if (ok) *ok = true;
        try
        {
            Windows::Management::Deployment::PackageManager pm;
            for (auto const& pkg : pm.FindPackagesForUser(L""))
            {
                std::wstring loc;
                try { loc = pkg.InstalledLocation().Path().c_str(); } catch (...) { continue; }
                std::wstring xml = ReadFileUtf8(loc + L"\\AppxManifest.xml");
                if (xml.find(L"FileExplorerContextMenus") == std::wstring::npos) continue;
                Windows::Data::Xml::Dom::XmlDocument doc;
                try { doc.LoadXml(xml); } catch (...) { Log(L"manifest parse failed: %s", loc.c_str()); continue; }
                std::wstring family = pkg.Id().FamilyName().c_str();
                std::wstring full = pkg.Id().FullName().c_str();
                std::wstring pkgDisplay;
                try { pkgDisplay = pkg.DisplayName().c_str(); } catch (...) {}
                // Sort key: the display name of the app that declares the extension (its
                // Start-menu name, e.g. "Notepad", "Photos"); packages whose app has no app-list
                // entry fall back to the package display name. Verified against Windows' order on
                // .docx/.png/background menus (26300).
                std::map<std::wstring, std::wstring> appNames; // AUMID -> display name
                try
                {
                    for (auto const& ale : pkg.GetAppListEntries())
                        appNames[ale.AppUserModelId().c_str()] = ale.DisplayInfo().DisplayName().c_str();
                }
                catch (...) {}
                auto types = doc.SelectNodes(L"//*[local-name()='FileExplorerContextMenus']/*[local-name()='ItemType']");
                for (auto const& t : types)
                {
                    auto te = t.as<Windows::Data::Xml::Dom::XmlElement>();
                    std::wstring display = pkgDisplay;
                    if (auto app = te.SelectSingleNode(L"ancestor::*[local-name()='Application']"))
                    {
                        std::wstring aumid = family + L"!" + std::wstring(app.as<Windows::Data::Xml::Dom::XmlElement>().GetAttribute(L"Id").c_str());
                        if (auto it = appNames.find(aumid); it != appNames.end() && !it->second.empty()) display = it->second;
                    }
                    std::wstring type = te.GetAttribute(L"Type").c_str();
                    for (auto const& v : te.SelectNodes(L"*[local-name()='Verb']"))
                    {
                        auto ve = v.as<Windows::Data::Xml::Dom::XmlElement>();
                        PackagedRegistration r;
                        r.family = family; r.fullName = full; r.displayName = display; r.itemType = type;
                        r.verbId = ve.GetAttribute(L"Id").c_str();
                        std::wstring cl = ve.GetAttribute(L"Clsid").c_str();
                        if (cl.empty() || cl[0] != L'{') cl = L"{" + cl + L"}";
                        if (FAILED(CLSIDFromString(cl.c_str(), &r.clsid))) continue;
                        regs.push_back(std::move(r));
                    }
                }
            }
        }
        catch (hresult_error const& e) { Log(L"PackageManager failed: 0x%08X %s", (uint32_t)e.code(), e.message().c_str()); if (ok) *ok = false; }
        // Explorer orders extension commands by the package's (localized) display name.
        // Verified on 26300 against three menus (desktop background, .docx, .png), where
        // full-name order would put Notepad after the PowerToys packages. The exact string
        // comparison Explorer uses for non-ASCII names is UNVERIFIED (linguistic, case-insensitive here).
        std::stable_sort(regs.begin(), regs.end(), [](auto& a, auto& b) {
            int c = CompareStringEx(LOCALE_NAME_USER_DEFAULT, LINGUISTIC_IGNORECASE, a.displayName.c_str(), -1, b.displayName.c_str(), -1, nullptr, nullptr, 0);
            if (c != CSTR_EQUAL) return c == CSTR_LESS_THAN;
            return _wcsicmp(a.fullName.c_str(), b.fullName.c_str()) < 0;
        });
        Log(L"packaged registrations: %zu in %.1f ms", regs.size(), NowMs() - t0);
        return regs;
    }

    // ---- The list kept from last time ----------------------------------------------------
    // The scan took 0.4 s of every host start (and the first menu waited for it). The list is
    // kept in %LOCALAPPDATA%\Deskboxes\packaged-menus.cache: a host starts with it, and the scan
    // runs meanwhile only to check it (Windows has no cheap, complete "packages changed" stamp:
    // the per-user repository key misses a quarter of the packages here). One line per verb,
    // tab-separated; the first line names the format and the language (display names, order).
    static std::wstring CachePath()
    {
        wchar_t buf[MAX_PATH]{};
        if (!GetEnvironmentVariableW(L"LOCALAPPDATA", buf, MAX_PATH)) return {};
        return std::wstring(buf) + L"\\Deskboxes\\packaged-menus.cache";
    }

    static std::wstring CacheHeader()
    {
        wchar_t loc[LOCALE_NAME_MAX_LENGTH]{};
        GetUserDefaultLocaleName(loc, LOCALE_NAME_MAX_LENGTH);
        return L"deskboxes packaged menus 1 " + std::to_wstring(GetUserDefaultUILanguage()) + L" " + loc;
    }

    static bool ReadCache(std::vector<PackagedRegistration>& out)
    {
        std::wstring path = CachePath();
        if (path.empty()) return false;
        std::wstring text = ReadFileUtf8(path);
        std::wistringstream in(text);
        std::wstring line;
        auto next = [&] { if (!std::getline(in, line)) return false; if (!line.empty() && line.back() == L'\r') line.pop_back(); return true; };
        if (!next() || line != CacheHeader()) return false;
        while (next())
        {
            if (line.empty()) continue;
            std::vector<std::wstring> f;
            for (size_t a = 0;;)
            {
                size_t b = line.find(L'\t', a);
                f.push_back(line.substr(a, b == std::wstring::npos ? std::wstring::npos : b - a));
                if (b == std::wstring::npos) break;
                a = b + 1;
            }
            PackagedRegistration r;
            if (f.size() != 6 || FAILED(CLSIDFromString(f[5].c_str(), &r.clsid))) return false;
            r.family = f[0]; r.fullName = f[1]; r.displayName = f[2]; r.itemType = f[3]; r.verbId = f[4];
            out.push_back(std::move(r));
        }
        return true;
    }

    static void WriteCache(const std::vector<PackagedRegistration>& regs)
    {
        std::wstring path = CachePath();
        if (path.empty()) return;
        auto clean = [](std::wstring s) { for (auto& c : s) if (c == L'\t' || c == L'\r' || c == L'\n') c = L' '; return s; };
        std::wstring s = CacheHeader() + L"\n";
        for (auto& r : regs)
        {
            wchar_t g[64]; StringFromGUID2(r.clsid, g, 64);
            s += clean(r.family) + L"\t" + clean(r.fullName) + L"\t" + clean(r.displayName) + L"\t" + clean(r.itemType) + L"\t" + clean(r.verbId) + L"\t" + g + L"\n";
        }
        std::string body = WideToUtf8(s);
        std::wstring tmp = path + L".tmp";
        HANDLE f = CreateFileW(tmp.c_str(), GENERIC_WRITE, 0, nullptr, CREATE_ALWAYS, FILE_ATTRIBUTE_NORMAL, nullptr);
        if (f == INVALID_HANDLE_VALUE) return;
        DWORD wr = 0;
        BOOL ok = WriteFile(f, body.data(), (DWORD)body.size(), &wr, nullptr) && wr == body.size();
        CloseHandle(f);
        if (!ok || !MoveFileExW(tmp.c_str(), path.c_str(), MOVEFILE_REPLACE_EXISTING)) DeleteFileW(tmp.c_str());
    }

    static bool SameRegs(const std::vector<PackagedRegistration>& a, const std::vector<PackagedRegistration>& b)
    {
        if (a.size() != b.size()) return false;
        for (size_t i = 0; i < a.size(); ++i)
            if (a[i].family != b[i].family || a[i].fullName != b[i].fullName || a[i].displayName != b[i].displayName ||
                a[i].itemType != b[i].itemType || a[i].verbId != b[i].verbId || !IsEqualCLSID(a[i].clsid, b[i].clsid))
                return false;
        return true;
    }

    static std::shared_future<std::vector<PackagedRegistration>> g_scan;
    static bool g_scanStarted = false;

    void StartPackagedScan(std::function<void()> ready, std::function<void()> changed)
    {
        if (g_regsLoaded || g_scanStarted) return;
        g_scanStarted = true;
        auto ui = winrt::Microsoft::UI::Dispatching::DispatcherQueue::GetForCurrentThread();
        double t0 = NowMs();
        std::vector<PackagedRegistration> cached;
        const bool hit = ReadCache(cached);
        auto promise = std::make_shared<std::promise<std::vector<PackagedRegistration>>>();
        if (hit)
        {
            g_regs = std::move(cached);
            g_regsLoaded = true;
            Log(L"packaged registrations: %zu from the cache in %.1f ms (a scan checks them meanwhile)", g_regs.size(), NowMs() - t0);
        }
        else g_scan = promise->get_future().share();
        std::thread([ui, hit, promise, ready, changed]
        {
            // Background work: menus and Settings come first.
            SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL);
            winrt::init_apartment(winrt::apartment_type::multi_threaded);
            bool ok = true;
            auto v = ScanPackaged(&ok);
            winrt::uninit_apartment();
            if (!hit) promise->set_value(v);
            ui.TryEnqueue([v = std::move(v), ok, hit, ready, changed]() mutable
            {
                if (!hit)
                {
                    // (A menu may have taken it already: LoadPackagedRegistrations.)
                    if (!g_regsLoaded) { g_regs = v; g_regsLoaded = true; }
                    g_scan = {};
                    if (ok) WriteCache(v);
                    if (ready) ready();
                    return;
                }
                if (!ok || SameRegs(v, g_regs)) return;
                Log(L"packaged registrations changed since the cache (%zu -> %zu): menus use the new ones", g_regs.size(), v.size());
                g_regs = std::move(v);
                WriteCache(g_regs);
                if (changed) changed();
            });
        }).detach();
        if (hit && ready) ready();
    }

    void LoadPackagedRegistrations(bool force)
    {
        if (g_regsLoaded && !force) return;
        if (!force && g_scan.valid())
        {
            double t0 = NowMs();
            g_regs = g_scan.get();
            if (NowMs() - t0 > 5) Log(L"packaged registrations: waited %.1f ms for the scan", NowMs() - t0);
        }
        else g_regs = ScanPackaged();
        g_scan = {};
        g_regsLoaded = true;
    }

    bool CommandsAlive(const BuiltMenu& m)
    {
        for (auto& e : m.packaged)
        {
            if (!e.cmd) continue;
            EXPCMDFLAGS f = 0;
            HRESULT hr = e.cmd->GetFlags(&f);
            if (hr == RPC_E_DISCONNECTED || hr == RPC_E_SERVER_DIED_DNE || hr == RPC_E_SERVER_DIED || hr == HRESULT_FROM_WIN32(RPC_S_SERVER_UNAVAILABLE) || hr == HRESULT_FROM_WIN32(RPC_S_CALL_FAILED))
            {
                Log(L"menu built ahead: %s no longer answers (0x%08X); built again", e.text.c_str(), (unsigned)hr);
                return false;
            }
        }
        return true;
    }

    std::vector<PackagedRegistration> PackagedFor(const std::vector<std::wstring>& itemTypes)
    {
        LoadPackagedRegistrations();
        std::vector<PackagedRegistration> out;
        std::set<std::wstring> seen; // one entry per (package, CLSID), as Explorer shows it once
        for (auto& r : g_regs)
        {
            bool match = false;
            for (auto& t : itemTypes) if (_wcsicmp(t.c_str(), r.itemType.c_str()) == 0) { match = true; break; }
            if (!match) continue;
            wchar_t g[64]; StringFromGUID2(r.clsid, g, 64);
            std::wstring key = r.family + L"|" + g;
            if (seen.insert(key).second) out.push_back(r);
        }
        return out;
    }

    // A NEW command instance per menu, like Explorer: some handlers keep per-instance state
    // (verified: a handler appended its subcommands again on every query of a reused
    // instance). The class factory is cached instead, so a warm menu still avoids the
    // expensive part (starting / connecting to the handler's server).
    com_ptr<IExplorerCommand> GetCommand(const CLSID& clsid)
    {
        wchar_t g[64]; StringFromGUID2(clsid, g, 64);
        auto t0 = NowMs();
        com_ptr<IClassFactory> cf;
        if (auto it = g_cfCache.find(g); it != g_cfCache.end()) cf = it->second;
        else
        {
            HRESULT hr = CoGetClassObject(clsid, CLSCTX_LOCAL_SERVER | CLSCTX_INPROC_SERVER, nullptr, IID_PPV_ARGS(cf.put()));
            if (FAILED(hr)) { Log(L"class object %s failed 0x%08X", g, hr); return nullptr; }
            g_cfCache[g] = cf;
        }
        com_ptr<IExplorerCommand> c;
        HRESULT hr = cf->CreateInstance(nullptr, IID_PPV_ARGS(c.put()));
        if (FAILED(hr))
        {
            // The server went away (RPC_E_DISCONNECTED etc.): reconnect once.
            g_cfCache.erase(g); cf = nullptr;
            hr = CoGetClassObject(clsid, CLSCTX_LOCAL_SERVER | CLSCTX_INPROC_SERVER, nullptr, IID_PPV_ARGS(cf.put()));
            if (SUCCEEDED(hr)) { g_cfCache[g] = cf; hr = cf->CreateInstance(nullptr, IID_PPV_ARGS(c.put())); }
            if (FAILED(hr)) { Log(L"activate %s failed 0x%08X", g, hr); return nullptr; }
        }
        double ms = NowMs() - t0;
        if (ms > 20) Log(L"activated %s in %.1f ms", g, ms);
        return c;
    }

    static void DropCommand(IExplorerCommand*) {} // instances are per menu; nothing cached to evict

    com_ptr<IExplorerCommand> CommandStoreHandler(const std::wstring& name, std::wstring* iconRef)
    {
        std::wstring key = L"SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Explorer\\CommandStore\\shell\\" + name;
        wchar_t clsid[64]{}; DWORD sz = sizeof(clsid);
        if (RegGetValueW(HKEY_LOCAL_MACHINE, key.c_str(), L"ExplorerCommandHandler", RRF_RT_REG_SZ, nullptr, clsid, &sz) != ERROR_SUCCESS)
            return nullptr;
        if (iconRef)
        {
            wchar_t icon[MAX_PATH]{}; DWORD isz = sizeof(icon);
            if (RegGetValueW(HKEY_LOCAL_MACHINE, key.c_str(), L"Icon", RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ, nullptr, icon, &isz) == ERROR_SUCCESS) *iconRef = icon;
        }
        CLSID c{};
        if (FAILED(CLSIDFromString(clsid, &c))) return nullptr;
        return GetCommand(c);
    }

    static std::wstring TakeCoString(PWSTR p) { std::wstring s = p ? p : L""; CoTaskMemFree(p); return s; }

    static std::wstring GuidStr(REFGUID g) { wchar_t b[64]; StringFromGUID2(g, b, 64); return b; }

    // ---- View site --------------------------------------------------------------------
    // Explorer gives background commands no item array; handlers find the folder through
    // their site (IObjectWithSite -> IServiceProvider -> IFolderView::GetFolder). Deskboxes
    // is the view here, so it provides that site for the folder it shows. Every query is
    // logged: only what handlers actually ask for is implemented.
    // non_agile: calls from the handlers' processes must arrive on this STA (the shell objects
    // used here live in it); an agile object gets them on RPC threads (RPC_E_WRONG_THREAD).
    struct ViewSite : winrt::implements<ViewSite, IServiceProvider, IFolderView, winrt::non_agile>
    {
        com_ptr<IShellItem> folder;
        explicit ViewSite(IShellItem* f) { folder.copy_from(f); }

        IFACEMETHODIMP QueryService(REFGUID svc, REFIID riid, void** ppv) override
        {
            *ppv = nullptr;
            HRESULT hr = E_NOINTERFACE;
            if (svc == SID_SFolderView || svc == SID_STopLevelBrowser || svc == SID_SShellBrowser)
                hr = QueryInterface(riid, ppv);
            Log(L"site QueryService(%s, %s) -> 0x%08X", GuidStr(svc).c_str(), GuidStr(riid).c_str(), hr);
            return hr;
        }
        // IFolderView
        IFACEMETHODIMP GetCurrentViewMode(UINT* m) override { *m = FVM_ICON; return S_OK; }
        IFACEMETHODIMP SetCurrentViewMode(UINT) override { return E_NOTIMPL; }
        IFACEMETHODIMP GetFolder(REFIID riid, void** ppv) override
        {
            *ppv = nullptr;
            HRESULT hr = E_NOINTERFACE;
            if (riid == __uuidof(IShellItem) || riid == __uuidof(IShellItem2)) hr = folder->QueryInterface(riid, ppv);
            else if (riid == __uuidof(IShellFolder) || riid == __uuidof(IShellFolder2) || riid == __uuidof(IPersistFolder2) || riid == __uuidof(IPersistFolder) || riid == __uuidof(IPersistIDList))
            {
                if (riid == __uuidof(IShellFolder) || riid == __uuidof(IShellFolder2))
                    hr = folder->BindToHandler(nullptr, BHID_SFObject, riid, ppv);
                else
                {
                    com_ptr<IShellFolder> sf;
                    hr = folder->BindToHandler(nullptr, BHID_SFObject, IID_PPV_ARGS(sf.put()));
                    if (SUCCEEDED(hr)) hr = sf->QueryInterface(riid, ppv);
                }
            }
            Log(L"site GetFolder(%s) -> 0x%08X", GuidStr(riid).c_str(), hr);
            return hr;
        }
        IFACEMETHODIMP Item(int, PITEMID_CHILD*) override { return E_INVALIDARG; }
        IFACEMETHODIMP ItemCount(UINT, int* n) override { *n = 0; return S_OK; }
        IFACEMETHODIMP Items(UINT flags, REFIID riid, void** ppv) override { *ppv = nullptr; Log(L"site Items(0x%X, %s)", flags, GuidStr(riid).c_str()); return E_NOINTERFACE; }
        IFACEMETHODIMP GetSelectionMarkedItem(int* i) override { *i = -1; return S_FALSE; }
        IFACEMETHODIMP GetFocusedItem(int* i) override { *i = -1; return S_FALSE; }
        IFACEMETHODIMP GetItemPosition(PCUITEMID_CHILD, POINT*) override { return E_NOTIMPL; }
        IFACEMETHODIMP GetSpacing(POINT*) override { return E_NOTIMPL; }
        IFACEMETHODIMP GetDefaultSpacing(POINT*) override { return E_NOTIMPL; }
        IFACEMETHODIMP GetAutoArrange() override { return S_FALSE; }
        IFACEMETHODIMP SelectItem(int, DWORD) override { return E_NOTIMPL; }
        IFACEMETHODIMP SelectAndPositionItems(UINT, PCUITEMID_CHILD_ARRAY, POINT*, DWORD) override { return E_NOTIMPL; }
    };

    void SetCommandSite(IExplorerCommand* c, IShellItem* folder)
    {
        if (!c || !folder) return;
        com_ptr<IObjectWithSite> ows;
        if (FAILED(c->QueryInterface(IID_PPV_ARGS(ows.put())))) return;
        auto site = winrt::make<ViewSite>(folder);
        ows->SetSite(site.as<IServiceProvider>().get());
    }

    bool ReadExplorerCommand(IExplorerCommand* c, IShellItemArray* items, MenuEntry& e, bool loadChildren)
    {
        EXPCMDSTATE st = ECS_ENABLED;
        // Same order as Explorer: a fast query first, the slow one only when the handler
        // asks for it (E_PENDING). Some handlers are stateful and answer differently otherwise.
        HRESULT hr = c->GetState(items, FALSE, &st);
        if (hr == E_PENDING) { st = ECS_ENABLED; hr = c->GetState(items, TRUE, &st); }
        if (hr == RPC_E_DISCONNECTED || hr == RPC_E_SERVER_DIED_DNE || hr == HRESULT_FROM_WIN32(RPC_S_SERVER_UNAVAILABLE))
        { DropCommand(c); return false; } // surrogate went away; re-activated next time
        if (SUCCEEDED(hr) && (st & ECS_HIDDEN)) { Log(L"command hidden by GetState (items %p)", items); return false; } // title is not meaningful when hidden
        if (FAILED(hr)) Log(L"GetState failed 0x%08X (items %p); shown enabled", hr, items);
        PWSTR p = nullptr;
        if (FAILED(c->GetTitle(items, &p))) return false;
        std::wstring raw = TakeCoString(p);
        e.text = StripMnemonic(raw, &e.accessKey);
        p = nullptr;
        if (SUCCEEDED(c->GetIcon(items, &p))) e.iconRef = TakeCoString(p);
        EXPCMDFLAGS fl = 0; c->GetFlags(&fl);
        e.hasSubmenu = (fl & ECF_HASSUBCOMMANDS) != 0;
        e.separator = (fl & ECF_ISSEPARATOR) != 0;
        e.shield = (fl & ECF_HASLUASHIELD) != 0;
        e.disabled = SUCCEEDED(hr) && (st & ECS_DISABLED);
        e.checked = SUCCEEDED(hr) && (st & ECS_CHECKED);
        e.radio = SUCCEEDED(hr) && (st & ECS_RADIOCHECK);
        GUID canon{};
        if (SUCCEEDED(c->GetCanonicalName(&canon)) && canon != GUID_NULL && e.id.empty())
        { wchar_t g[64]; StringFromGUID2(canon, g, 64); e.id = g; }
        e.cmd.copy_from(c);
        if (e.hasSubmenu && loadChildren) LoadChildren(e, items);
        return true;
    }

    HRESULT InvokeExplorerCommand(BuiltMenu& m, const MenuEntry& e)
    {
        if (!e.cmd) return E_UNEXPECTED;
        // Same arguments as for GetState: the selection, or none for a background menu
        // (the handler finds the folder through its site).
        HRESULT hr = e.cmd->Invoke(m.items.get(), nullptr);
        Log(L"Invoke %s (%s) -> 0x%08X", e.text.c_str(), e.id.c_str(), hr);
        return hr;
    }

    void LoadChildren(MenuEntry& e, IShellItemArray* items)
    {
        if (e.childrenLoaded || !e.cmd) return;
        e.childrenLoaded = true;
        com_ptr<IEnumExplorerCommand> en;
        if (FAILED(e.cmd->EnumSubCommands(en.put())) || !en) return;
        IExplorerCommand* sub = nullptr; ULONG got = 0;
        while (en->Next(1, &sub, &got) == S_OK && got == 1)
        {
            MenuEntry ce; ce.source = e.source; ce.packageFamily = e.packageFamily;
            if (ReadExplorerCommand(sub, items, ce, false)) e.children.push_back(std::move(ce));
            sub->Release();
        }
    }

    std::vector<std::wstring> ItemTypesFor(const MenuRequest& r)
    {
        if (r.background) return { L"Directory\\Background" };
        if (r.items.empty()) return {};
        const std::wstring& p = r.items.front(); // Explorer evaluates for the focused item
        if (p.rfind(L"::", 0) == 0) return {};
        DWORD a = GetFileAttributesW(p.c_str());
        if (a != INVALID_FILE_ATTRIBUTES && (a & FILE_ATTRIBUTE_DIRECTORY))
        {
            if (p.size() <= 3) return { L"Drive", L"Directory" };
            return { L"Directory" };
        }
        std::vector<std::wstring> t;
        auto dot = p.find_last_of(L'.');
        auto slash = p.find_last_of(L"\\/");
        if (dot != std::wstring::npos && (slash == std::wstring::npos || dot > slash)) t.push_back(p.substr(dot));
        t.push_back(L"*");
        return t;
    }

    static bool ReadPrefDword(const std::wstring& sub, const wchar_t* value, DWORD& out)
    {
        DWORD sz = sizeof(DWORD);
        return RegGetValueW(HKEY_CURRENT_USER, sub.c_str(), value, RRF_RT_REG_DWORD, nullptr, &out, &sz) == ERROR_SUCCESS;
    }

    bool ExtensionPromoted(const std::wstring& family)
    {
        // Windows 11 26H2 Settings > Personalization > Context menu: per-extension toggle.
        // Preferences (user choice) wins over DefaultState.
        const std::wstring base = L"Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\ContextMenu\\";
        DWORD v = 0;
        if (ReadPrefDword(base + L"Preferences\\extensions\\" + family, L"Enabled", v)) return v != 0;
        if (ReadPrefDword(base + L"DefaultState\\extensions\\" + family, L"Enabled", v)) return v != 0;
        return false;
    }

    bool CustomizableMenuPresent()
    {
        if (!GetPolicy().customizable) return false;
        HKEY k = nullptr;
        // Evidence that this build has the feature even if the user never opened the page.
        bool key = RegOpenKeyExW(HKEY_CURRENT_USER, L"Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\ContextMenu", 0, KEY_READ, &k) == ERROR_SUCCESS;
        if (k) RegCloseKey(k);
        return key || GetPolicy().customizable;
    }
}

#include "pch.h"
#include "MenuModel.h"
#include "Log.h"

namespace dbx
{
    static std::wstring VerbOf(IContextMenu* cm, UINT offset)
    {
        wchar_t buf[256]{};
        // Some handlers return E_NOTIMPL or garbage; only accept a clean string.
        if (SUCCEEDED(cm->GetCommandString(offset, GCS_VERBW, nullptr, reinterpret_cast<LPSTR>(buf), ARRAYSIZE(buf))) && buf[0] && iswprint(buf[0]))
            return buf;
        char abuf[256]{};
        if (SUCCEEDED(cm->GetCommandString(offset, GCS_VERBA, nullptr, abuf, ARRAYSIZE(abuf))) && abuf[0] && isprint((unsigned char)abuf[0]))
            return Utf8ToWide(abuf);
        return {};
    }

    static void InitSubmenu(IContextMenu* cm, HMENU sub, UINT index)
    {
        // Dynamic submenus (Open with, Send to, New) are filled on WM_INITMENUPOPUP.
        winrt::com_ptr<IContextMenu3> cm3;
        winrt::com_ptr<IContextMenu2> cm2;
        LRESULT lr = 0;
        if (SUCCEEDED(cm->QueryInterface(IID_PPV_ARGS(cm3.put()))))
            cm3->HandleMenuMsg2(WM_INITMENUPOPUP, (WPARAM)sub, MAKELPARAM(index, FALSE), &lr);
        else if (SUCCEEDED(cm->QueryInterface(IID_PPV_ARGS(cm2.put()))))
            cm2->HandleMenuMsg(WM_INITMENUPOPUP, (WPARAM)sub, MAKELPARAM(index, FALSE));
    }

    void ReadClassicMenu(IContextMenu* cm, HMENU h, UINT idFirst, std::vector<MenuEntry>& out, int depth)
    {
        int n = GetMenuItemCount(h);
        for (int i = 0; i < n; ++i)
        {
            MENUITEMINFOW mii{ sizeof(mii) };
            mii.fMask = MIIM_FTYPE | MIIM_STATE | MIIM_ID | MIIM_SUBMENU | MIIM_STRING | MIIM_BITMAP;
            wchar_t text[512]{};
            mii.dwTypeData = text; mii.cch = ARRAYSIZE(text) - 1;
            if (!GetMenuItemInfoW(h, i, TRUE, &mii)) continue;
            MenuEntry e;
            e.source = Source::Classic;
            if (mii.fType & MFT_SEPARATOR) { e.separator = true; out.push_back(std::move(e)); continue; }
            std::wstring raw = text;
            auto tab = raw.find(L'\t');
            if (tab != std::wstring::npos) e.accel = raw.substr(tab + 1);
            e.text = StripMnemonic(raw, &e.accessKey);
            e.classicId = mii.wID;
            e.disabled = (mii.fState & MFS_DISABLED) != 0;
            e.checked = (mii.fState & MFS_CHECKED) != 0;
            e.radio = (mii.fType & MFT_RADIOCHECK) != 0;
            e.isDefault = (mii.fState & MFS_DEFAULT) != 0;
            e.classicBitmap = (mii.hbmpItem && mii.hbmpItem != HBMMENU_CALLBACK) ? mii.hbmpItem : nullptr;
            if (mii.fType & MFT_OWNERDRAW) e.id = L"<ownerdraw>";
            if (!mii.hSubMenu && mii.wID >= idFirst && mii.wID <= 0x7FFF)
                e.id = VerbOf(cm, mii.wID - idFirst);
            else if (mii.hSubMenu && mii.wID >= idFirst && mii.wID <= 0x7FFF)
                e.id = VerbOf(cm, mii.wID - idFirst); // some submenu owners have a verb (e.g. "New")
            e.classicPos = i;
            if (mii.hSubMenu)
            {
                e.hasSubmenu = true;
                e.classicSub = mii.hSubMenu;
                if (depth > 0)
                {
                    InitSubmenu(cm, mii.hSubMenu, i);
                    ReadClassicMenu(cm, mii.hSubMenu, idFirst, e.children, depth - 1);
                    e.childrenLoaded = true;
                }
            }
            out.push_back(std::move(e));
        }
    }

    HRESULT BuildClassic(BuiltMenu& m, const Selection& sel)
    {
        auto t0 = NowMs();
        HRESULT hr;
        if (m.req.background)
        {
            winrt::com_ptr<IShellFolder> folder;
            hr = ResolveFolder(m.req.folder, folder, m.folderItem);
            if (FAILED(hr)) return hr;
            hr = folder->CreateViewObject(m.req.owner, IID_PPV_ARGS(m.classic.put()));
        }
        else
        {
            hr = sel.parent->GetUIObjectOf(m.req.owner, (UINT)sel.children.size(), const_cast<LPCITEMIDLIST*>(sel.children.data()), IID_IContextMenu, nullptr, m.classic.put_void());
        }
        if (FAILED(hr)) { Log(L"classic IContextMenu failed 0x%08X", hr); return hr; }

        m.hmenu = CreatePopupMenu();
        UINT flags = CMF_NORMAL | (m.req.background ? 0 : (CMF_CANRENAME | CMF_ITEMMENU));
        if (m.req.shift) flags |= CMF_EXTENDEDVERBS;
        hr = m.classic->QueryContextMenu(m.hmenu, 0, m.idFirst, m.idLast, flags);
        if (FAILED(hr)) { Log(L"QueryContextMenu failed 0x%08X", hr); return hr; }
        // Top level only here; submenus are initialised lazily (they can be slow: Open with, Send to).
        ReadClassicMenu(m.classic.get(), m.hmenu, m.idFirst, m.classicAll, 0);
        m.msClassic = NowMs() - t0;
        return S_OK;
    }

    static HRESULT InvokeInfo(BuiltMenu& m, LPCSTR verbA, LPCWSTR verbW, HWND owner, POINT pt)
    {
        CMINVOKECOMMANDINFOEX ici{ sizeof(ici) };
        ici.fMask = CMIC_MASK_UNICODE | CMIC_MASK_PTINVOKE | CMIC_MASK_ASYNCOK;
        if (GetKeyState(VK_SHIFT) < 0) ici.fMask |= CMIC_MASK_SHIFT_DOWN;
        if (GetKeyState(VK_CONTROL) < 0) ici.fMask |= CMIC_MASK_CONTROL_DOWN;
        ici.hwnd = owner;
        ici.lpVerb = verbA;
        ici.lpVerbW = verbW;
        ici.nShow = SW_SHOWNORMAL;
        ici.ptInvoke = pt;
        return m.classic ? m.classic->InvokeCommand(reinterpret_cast<CMINVOKECOMMANDINFO*>(&ici)) : E_UNEXPECTED;
    }

    HRESULT InvokeClassic(BuiltMenu& m, const MenuEntry& e, HWND owner, POINT pt)
    {
        UINT off = e.classicId - m.idFirst;
        return InvokeInfo(m, MAKEINTRESOURCEA(off), MAKEINTRESOURCEW(off), owner, pt);
    }

    HRESULT InvokeClassicVerb(BuiltMenu& m, const wchar_t* verb, HWND owner, POINT pt)
    {
        // Prefer the command id the classic menu assigned to this verb (most reliable);
        // fall back to invoking by canonical verb string.
        for (auto& e : m.classicAll)
            if (!e.separator && _wcsicmp(e.id.c_str(), verb) == 0) return InvokeClassic(m, e, owner, pt);
        std::string a = WideToUtf8(verb);
        return InvokeInfo(m, a.c_str(), verb, owner, pt);
    }

    void LoadClassicSubmenu(BuiltMenu& m, MenuEntry& e)
    {
        if (e.childrenLoaded || !e.classicSub || !m.classic) return;
        e.childrenLoaded = true;
        InitSubmenu(m.classic.get(), e.classicSub, (UINT)std::max(e.classicPos, 0));
        ReadClassicMenu(m.classic.get(), e.classicSub, m.idFirst, e.children, 0);
    }

    // ---- ShellNew: the folder's "New" submenu (CLSID_NewMenu) ------------------------
    NewMenu::~NewMenu() { if (hmenu) DestroyMenu(hmenu); }

    std::unique_ptr<NewMenu> BuildNewMenu(IShellItem* folder, HWND owner)
    {
        auto n = std::make_unique<NewMenu>();
        if (!folder) return n;
        PIDLIST_ABSOLUTE pidl = nullptr;
        if (FAILED(SHGetIDListFromObject(folder, &pidl))) return n;
        winrt::com_ptr<IShellExtInit> init;
        HRESULT hr = CoCreateInstance(CLSID_NewMenu, nullptr, CLSCTX_INPROC_SERVER, IID_PPV_ARGS(init.put()));
        if (SUCCEEDED(hr)) hr = init->Initialize(pidl, nullptr, nullptr);
        CoTaskMemFree(pidl);
        if (FAILED(hr)) { Log(L"NewMenu init failed 0x%08X", hr); return n; }
        n->cm = init.as<IContextMenu>();
        n->hmenu = CreatePopupMenu();
        hr = n->cm->QueryContextMenu(n->hmenu, 0, 1, 0x7FFF, CMF_NORMAL);
        if (FAILED(hr)) { Log(L"NewMenu QueryContextMenu failed 0x%08X", hr); return n; }
        // CLSID_NewMenu adds one "New" popup; its items appear on WM_INITMENUPOPUP.
        HMENU sub = GetSubMenu(n->hmenu, 0);
        if (!sub) return n;
        InitSubmenu(n->cm.get(), sub, 0);
        ReadClassicMenu(n->cm.get(), sub, 1, n->items, 0);
        (void)owner;
        return n;
    }

    HRESULT InvokeNewMenu(NewMenu& n, const MenuEntry& e, HWND owner)
    {
        if (!n.cm) return E_UNEXPECTED;
        CMINVOKECOMMANDINFOEX ici{ sizeof(ici) };
        ici.fMask = CMIC_MASK_UNICODE | CMIC_MASK_ASYNCOK;
        ici.hwnd = owner;
        UINT off = e.classicId - 1;
        ici.lpVerb = MAKEINTRESOURCEA(off); ici.lpVerbW = MAKEINTRESOURCEW(off);
        ici.nShow = SW_SHOWNORMAL;
        return n.cm->InvokeCommand(reinterpret_cast<CMINVOKECOMMANDINFO*>(&ici));
    }

    // ---- "Show more options": the real classic popup menu ------------------------------
    static IContextMenu* g_trackCm = nullptr;
    static LRESULT CALLBACK ClassicOwnerProc(HWND h, UINT msg, WPARAM w, LPARAM l)
    {
        if (g_trackCm && (msg == WM_INITMENUPOPUP || msg == WM_DRAWITEM || msg == WM_MEASUREITEM || msg == WM_MENUCHAR || msg == WM_MENUSELECT))
        {
            winrt::com_ptr<IContextMenu3> cm3;
            winrt::com_ptr<IContextMenu2> cm2;
            LRESULT lr = 0;
            if (SUCCEEDED(g_trackCm->QueryInterface(IID_PPV_ARGS(cm3.put()))) && SUCCEEDED(cm3->HandleMenuMsg2(msg, w, l, &lr)))
                return msg == WM_MENUCHAR ? lr : (msg == WM_INITMENUPOPUP ? 0 : TRUE);
            if (SUCCEEDED(g_trackCm->QueryInterface(IID_PPV_ARGS(cm2.put()))) && SUCCEEDED(cm2->HandleMenuMsg(msg, w, l)))
                return msg == WM_INITMENUPOPUP ? 0 : TRUE;
        }
        return DefWindowProcW(h, msg, w, l);
    }

    UINT ShowClassicMenu(BuiltMenu& m, HWND /*owner*/, POINT pt)
    {
        static HWND s_owner = [] {
            WNDCLASSW wc{}; wc.lpfnWndProc = ClassicOwnerProc; wc.hInstance = GetModuleHandleW(nullptr); wc.lpszClassName = L"DeskboxesClassicMenuOwner";
            RegisterClassW(&wc);
            return CreateWindowExW(WS_EX_TOOLWINDOW, wc.lpszClassName, L"", WS_POPUP, 0, 0, 0, 0, nullptr, nullptr, wc.hInstance, nullptr);
        }();
        g_trackCm = m.classic.get();
        SetForegroundWindow(s_owner);
        UINT cmd = TrackPopupMenuEx(m.hmenu, TPM_RETURNCMD | TPM_RIGHTBUTTON, pt.x, pt.y, s_owner, nullptr);
        PostMessageW(s_owner, WM_NULL, 0, 0);
        g_trackCm = nullptr;
        if (cmd >= m.idFirst && cmd <= m.idLast)
        {
            MenuEntry e; e.classicId = cmd;
            HRESULT hr = InvokeClassic(m, e, s_owner, pt);
            Log(L"classic menu invoked id %u -> 0x%08X", cmd, hr);
        }
        return cmd;
    }
}

#include "pch.h"
#include "MenuModel.h"
#include "Log.h"

namespace dbx
{
    Selection::~Selection()
    {
        for (auto p : abs) CoTaskMemFree(p);
        for (auto p : ownedChildren) CoTaskMemFree(p);
    }

    BuiltMenu::~BuiltMenu()
    {
        if (hmenu) DestroyMenu(hmenu);
    }

    DWORD WindowsBuild(DWORD* ubr)
    {
        DWORD build = 0, sz = sizeof(DWORD), u = 0;
        wchar_t buf[32]{}; DWORD bsz = sizeof(buf);
        if (RegGetValueW(HKEY_LOCAL_MACHINE, L"SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion", L"CurrentBuild", RRF_RT_REG_SZ, nullptr, buf, &bsz) == ERROR_SUCCESS)
            build = (DWORD)_wtoi(buf);
        RegGetValueW(HKEY_LOCAL_MACHINE, L"SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion", L"UBR", RRF_RT_REG_DWORD, nullptr, &u, &sz);
        if (ubr) *ubr = u;
        return build;
    }

    std::wstring StripMnemonic(const std::wstring& s, std::wstring* accessKey)
    {
        std::wstring out; out.reserve(s.size());
        for (size_t i = 0; i < s.size(); ++i)
        {
            if (s[i] == L'\t') break; // classic accelerator suffix, handled separately
            if (s[i] == L'&')
            {
                if (i + 1 < s.size() && s[i + 1] == L'&') { out += L'&'; ++i; continue; }
                if (i + 1 < s.size() && accessKey && accessKey->empty()) *accessKey = std::wstring(1, (wchar_t)towupper(s[i + 1]));
                continue;
            }
            out += s[i];
        }
        return out;
    }

    std::wstring LoadIndirect(const std::wstring& ref)
    {
        if (ref.empty() || ref[0] != L'@') return ref;
        wchar_t buf[512]{};
        if (SUCCEEDED(SHLoadIndirectString(ref.c_str(), buf, ARRAYSIZE(buf), nullptr))) return buf;
        return {};
    }

    StaticVerbInfo QueryStaticVerb(IShellFolder* parent, PCUITEMID_CHILD child, const std::wstring& verb)
    {
        StaticVerbInfo r;
        if (!parent || !child || verb.empty()) return r;
        winrt::com_ptr<IQueryAssociations> qa;
        if (FAILED(parent->GetUIObjectOf(nullptr, 1, &child, IID_IQueryAssociations, nullptr, qa.put_void()))) return r;
        HKEY cls = nullptr;
        if (FAILED(qa->GetKey(0, ASSOCKEY_SHELLEXECCLASS, nullptr, &cls)) || !cls) return r;
        HKEY v = nullptr;
        if (RegOpenKeyExW(cls, (L"shell\\" + verb).c_str(), 0, KEY_READ, &v) == ERROR_SUCCESS)
        {
            r.found = true;
            r.legacyDisable = RegQueryValueExW(v, L"LegacyDisable", nullptr, nullptr, nullptr, nullptr) == ERROR_SUCCESS;
            RegCloseKey(v);
        }
        RegCloseKey(cls);
        return r;
    }

    std::vector<OpenWithEntry> OpenWithHandlers(const BuiltMenu& m)
    {
        std::vector<OpenWithEntry> out;
        if (m.req.items.empty()) return out;
        const std::wstring& p = m.req.items.front();
        const wchar_t* ext = PathFindExtensionW(p.c_str());
        if (!ext || !*ext) return out;
        winrt::com_ptr<IEnumAssocHandlers> en;
        if (FAILED(SHAssocEnumHandlers(ext, ASSOC_FILTER_RECOMMENDED, en.put())) || !en) return out;
        IAssocHandler* h = nullptr; ULONG got = 0;
        while (en->Next(1, &h, &got) == S_OK && got == 1)
        {
            OpenWithEntry e;
            PWSTR name = nullptr;
            if (SUCCEEDED(h->GetUIName(&name)) && name) { e.name = name; CoTaskMemFree(name); }
            PWSTR icon = nullptr; int idx = 0;
            if (SUCCEEDED(h->GetIconLocation(&icon, &idx)) && icon) { e.iconRef = std::wstring(icon) + L"," + std::to_wstring(idx); CoTaskMemFree(icon); }
            e.handler.attach(h);
            if (!e.name.empty()) out.push_back(std::move(e));
        }
        return out;
    }

    HRESULT InvokeOpenWith(BuiltMenu& m, IAssocHandler* h)
    {
        if (!h || !m.items) return E_UNEXPECTED;
        winrt::com_ptr<IDataObject> dobj;
        HRESULT hr = m.items->BindToHandler(nullptr, BHID_DataObject, IID_PPV_ARGS(dobj.put()));
        if (SUCCEEDED(hr)) hr = h->Invoke(dobj.get());
        Log(L"Open with handler invoke -> 0x%08X", hr);
        return hr;
    }

    HRESULT ResolveFolder(const std::wstring& parsing, winrt::com_ptr<IShellFolder>& folder, winrt::com_ptr<IShellItem>& item)
    {
        winrt::com_ptr<IShellFolder> desktop;
        HRESULT hr = SHGetDesktopFolder(desktop.put());
        if (FAILED(hr)) return hr;
        if (parsing.empty() || _wcsicmp(parsing.c_str(), L"::desktop") == 0)
        {
            folder = desktop;
            return SHGetKnownFolderItem(FOLDERID_Desktop, KF_FLAG_DEFAULT, nullptr, IID_PPV_ARGS(item.put()));
        }
        PIDLIST_ABSOLUTE pidl = nullptr;
        hr = SHParseDisplayName(parsing.c_str(), nullptr, &pidl, 0, nullptr);
        if (FAILED(hr)) return hr;
        hr = desktop->BindToObject(pidl, nullptr, IID_PPV_ARGS(folder.put()));
        if (SUCCEEDED(hr)) hr = SHCreateItemFromIDList(pidl, IID_PPV_ARGS(item.put()));
        CoTaskMemFree(pidl);
        return hr;
    }

    // Resolve the selection to (parent IShellFolder, child PIDLs).
    // - All items share a parent (the common case: everything on the user's Desktop, or a
    //   fixture folder): SHBindToParent on the first item.
    // - Mixed parents (user + Public desktop): the desktop root merges both, so enumerate the
    //   desktop root once and match children by SHGDN_FORPARSING.
    HRESULT ResolveSelection(const MenuRequest& r, Selection& sel)
    {
        // Parse each parent folder once and the names relative to it: a full SHParseDisplayName
        // per path cost ~1.5 ms (148 ms for 100 items, measured 2026-11-19); the relative parse
        // gives the same absolute ID lists. Any failure falls back to the full parse.
        struct Parent { PIDLIST_ABSOLUTE pidl = nullptr; winrt::com_ptr<IShellFolder> folder; bool failed = false; };
        std::map<std::wstring, Parent> parents;
        for (auto& p : r.items)
        {
            PIDLIST_ABSOLUTE pidl = nullptr;
            auto slash = p.find_last_of(L'\\');
            if (slash != std::wstring::npos && slash + 1 < p.size())
            {
                std::wstring dir = p.substr(0, slash);
                if (dir.size() == 2 && dir[1] == L':') dir += L'\\';
                auto& pe = parents[dir];
                if (!pe.pidl && !pe.failed)
                {
                    pe.failed = FAILED(SHParseDisplayName(dir.c_str(), nullptr, &pe.pidl, 0, nullptr))
                        || FAILED(SHBindToObject(nullptr, pe.pidl, nullptr, IID_PPV_ARGS(pe.folder.put())));
                }
                if (!pe.failed)
                {
                    std::wstring name = p.substr(slash + 1);
                    PIDLIST_RELATIVE rel = nullptr; ULONG eaten = 0;
                    if (SUCCEEDED(pe.folder->ParseDisplayName(nullptr, nullptr, name.data(), &eaten, &rel, nullptr)) && rel)
                    {
                        pidl = ILCombine(pe.pidl, rel);
                        CoTaskMemFree(rel);
                    }
                }
            }
            if (!pidl)
            {
                HRESULT hr = SHParseDisplayName(p.c_str(), nullptr, &pidl, SFGAO_FILESYSTEM, nullptr);
                if (FAILED(hr))
                {
                    Log(L"SHParseDisplayName(%s) failed 0x%08X", p.c_str(), hr);
                    for (auto& [k, pe] : parents) if (pe.pidl) CoTaskMemFree(pe.pidl);
                    return hr;
                }
            }
            sel.abs.push_back(pidl);
        }
        for (auto& [k, pe] : parents) if (pe.pidl) CoTaskMemFree(pe.pidl);
        if (sel.abs.empty()) return E_INVALIDARG;

        bool sameParent = true;
        for (size_t i = 1; i < sel.abs.size() && sameParent; ++i)
        {
            PIDLIST_ABSOLUTE a = ILClone(sel.abs[0]), b = ILClone(sel.abs[i]);
            ILRemoveLastID(a); ILRemoveLastID(b);
            sameParent = ILIsEqual(a, b) != FALSE;
            ILFree(a); ILFree(b);
        }
        if (sameParent)
        {
            PCUITEMID_CHILD child = nullptr;
            HRESULT hr = SHBindToParent(sel.abs[0], IID_PPV_ARGS(sel.parent.put()), &child);
            if (FAILED(hr)) return hr;
            for (auto a : sel.abs) sel.children.push_back(ILFindLastID(a));
            return S_OK;
        }

        winrt::com_ptr<IShellFolder> desktop;
        HRESULT hr = SHGetDesktopFolder(desktop.put());
        if (FAILED(hr)) return hr;
        winrt::com_ptr<IEnumIDList> en;
        hr = desktop->EnumObjects(nullptr, SHCONTF_FOLDERS | SHCONTF_NONFOLDERS | SHCONTF_INCLUDEHIDDEN, en.put());
        if (hr != S_OK) return FAILED(hr) ? hr : E_FAIL;
        std::map<std::wstring, PITEMID_CHILD> byName;
        PITEMID_CHILD c = nullptr;
        while (en->Next(1, &c, nullptr) == S_OK)
        {
            STRRET sr{}; wchar_t buf[MAX_PATH * 2]{};
            if (SUCCEEDED(desktop->GetDisplayNameOf(c, SHGDN_FORPARSING, &sr)) && SUCCEEDED(StrRetToBufW(&sr, c, buf, ARRAYSIZE(buf))))
            {
                std::wstring k = buf; CharLowerBuffW(k.data(), (DWORD)k.size());
                byName[k] = c;
            }
            else CoTaskMemFree(c);
        }
        sel.parent = desktop;
        for (auto& p : r.items)
        {
            std::wstring k = p; CharLowerBuffW(k.data(), (DWORD)k.size());
            auto it = byName.find(k);
            if (it == byName.end()) { Log(L"mixed selection: %s not a desktop child", p.c_str()); hr = E_INVALIDARG; break; }
            sel.children.push_back(it->second);
            sel.ownedChildren.push_back(it->second);
            byName.erase(it);
        }
        for (auto& kv : byName) CoTaskMemFree(kv.second);
        return hr;
    }
}

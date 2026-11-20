#include "pch.h"
#include "MenuModel.h"
#include "Log.h"

using namespace winrt;

namespace dbx
{
    static MenuEntry Sep() { MenuEntry e; e.separator = true; e.source = Source::Synthetic; e.id = L"separator"; return e; }

    static void PushSep(std::vector<MenuEntry>& v)
    {
        if (!v.empty() && !v.back().separator) v.push_back(Sep());
    }
    static void TrimSeps(std::vector<MenuEntry>& v)
    {
        while (!v.empty() && v.back().separator) v.pop_back();
        while (!v.empty() && v.front().separator) v.erase(v.begin());
        for (size_t i = 1; i < v.size();)
            if (v[i].separator && v[i - 1].separator) v.erase(v.begin() + i); else ++i;
    }

    static void Decorate(MenuEntry& e, const Policy& pol)
    {
        if (e.accel.empty())
        {
            if (e.isDefault) e.accel = L"Enter";
            else if (auto it = pol.accelerators.find(e.id); it != pol.accelerators.end() && !e.id.empty())
            {
                // Office registers "Open" (capitalised) as its default verb; Enter belongs to the default only.
                if (_wcsicmp(e.id.c_str(), L"open") != 0) e.accel = it->second;
            }
        }
        if (e.iconRef.empty() && !e.classicBitmap)
            if (auto it = pol.glyphs.find(e.id); it != pol.glyphs.end()) e.glyph = it->second;
    }

    static std::vector<MenuEntry>::iterator FindId(std::vector<MenuEntry>& v, const std::wstring& id)
    {
        return std::find_if(v.begin(), v.end(), [&](auto& e) { return !e.separator && _wcsicmp(e.id.c_str(), id.c_str()) == 0; });
    }

    static bool IsFolderSelection(const MenuRequest& r)
    {
        if (r.items.empty()) return false;
        DWORD a = GetFileAttributesW(r.items.front().c_str());
        return a != INVALID_FILE_ATTRIBUTES && (a & FILE_ATTRIBUTE_DIRECTORY);
    }

    static void InsertExtras(BuiltMenu& m, const std::wstring& placement, std::vector<MenuEntry>& into, std::vector<MenuEntry>::iterator at)
    {
        std::vector<MenuEntry> add;
        for (auto& x : m.req.extra)
            if (x.packageFamily == placement) add.push_back(x); // packageFamily carries the placement for extras
        into.insert(at, add.begin(), add.end());
    }

    // Windows' localized text for the desktop view's own entries, read from the view's menu
    // resource (shell32 MENU 215 on 26H2). The core implements these commands.
    static std::vector<MenuEntry> DesktopOwnEntries(const MenuRequest& r, const Policy& pol)
    {
        std::vector<MenuEntry> out;
        if (pol.desktopOwn.empty() || pol.ownMenuResource.empty()) return out;
        static HMODULE mod = LoadLibraryExW(pol.ownMenuResource.c_str(), nullptr, LOAD_LIBRARY_AS_DATAFILE | LOAD_LIBRARY_AS_IMAGE_RESOURCE);
        if (!mod) { Log(L"cannot load %s for own entries", pol.ownMenuResource.c_str()); return out; }
        HMENU menu = LoadMenuW(mod, MAKEINTRESOURCEW(pol.ownMenuId));
        if (!menu) { Log(L"menu %u not found in %s", pol.ownMenuId, pol.ownMenuResource.c_str()); return out; }
        const bool files = IsClipboardFormatAvailable(CF_HDROP) != FALSE;
        int group = 1;
        for (auto& oe : pol.desktopOwn)
        {
            if (oe.when == L"clipboardHasFiles" && !files) continue;
            if (oe.when == L"coreUndo" && !r.coreUndo) continue;
            if (oe.when == L"coreRedo" && !r.coreRedo) continue;
            wchar_t text[256]{};
            if (oe.command)
                GetMenuStringW(menu, oe.command, text, ARRAYSIZE(text), MF_BYCOMMAND);
            else if (!oe.position.empty())
            {
                HMENU h = menu;
                for (size_t i = 0; i + 1 < oe.position.size() && h; ++i) h = GetSubMenu(h, oe.position[i]);
                if (h) GetMenuStringW(h, oe.position.back(), text, ARRAYSIZE(text), MF_BYPOSITION);
            }
            // What Undo / Redo would do, named as Explorer names it ("Undo Move").
            const std::wstring& named = oe.id == L"undo" ? r.coreUndoText : oe.id == L"redo" ? r.coreRedoText : std::wstring();
            if (!named.empty())
            {
                const wchar_t* tab = wcschr(text, L'\t'); // keep Windows' "\tCtrl+Z"
                std::wstring t = named + (tab ? std::wstring(tab) : std::wstring());
                wcsncpy_s(text, t.c_str(), _TRUNCATE);
            }
            if (!text[0]) { Log(L"own entry %s: no text in resource", oe.id.c_str()); continue; }
            if (oe.group != group && !out.empty()) out.push_back(Sep());
            group = oe.group;
            MenuEntry e; e.source = Source::Deskboxes; e.id = oe.id;
            e.text = StripMnemonic(text, &e.accessKey);
            e.hasSubmenu = oe.submenu;
            // The core supplies the View / Sort by children (they describe its own boxes' state).
            if (auto it = r.ownChildren.find(oe.id); oe.submenu && it != r.ownChildren.end()) e.children = it->second;
            else if (oe.submenu && !oe.position.empty())
            {
                // The core did not supply them: show Windows' own entries from the same resource
                // submenu, disabled (Phase 1 does not implement them; listed in REPORT.md).
                HMENU h = menu;
                for (size_t i = 0; i < oe.position.size() && h; ++i) h = GetSubMenu(h, oe.position[i]);
                for (int i = 0, n = h ? GetMenuItemCount(h) : 0; i < n; ++i)
                {
                    MENUITEMINFOW mi{ sizeof(mi) }; mi.fMask = MIIM_FTYPE | MIIM_ID;
                    GetMenuItemInfoW(h, i, TRUE, &mi);
                    if (mi.fType & MFT_SEPARATOR) { e.children.push_back(Sep()); continue; }
                    wchar_t ct[256]{};
                    GetMenuStringW(h, i, ct, ARRAYSIZE(ct), MF_BYPOSITION);
                    if (!ct[0]) continue;
                    MenuEntry c; c.source = Source::Deskboxes; c.id = oe.id + L"." + std::to_wstring(mi.wID);
                    c.text = StripMnemonic(ct, &c.accessKey);
                    c.disabled = true;
                    e.children.push_back(std::move(c));
                }
            }
            e.childrenLoaded = true;
            Decorate(e, pol);
            out.push_back(std::move(e));
        }
        DestroyMenu(menu);
        if (!out.empty()) out.push_back(Sep());
        return out;
    }

    std::unique_ptr<BuiltMenu> BuildMenu(const MenuRequest& r)
    {
        auto t0 = NowMs();
        auto m = std::make_unique<BuiltMenu>();
        m->req = r;
        const Policy& pol = GetPolicy();
        m->customizable = CustomizableMenuPresent();

        Selection sel;
        if (!r.background)
        {
            if (FAILED(ResolveSelection(r, sel))) return m;
            SHCreateShellItemArrayFromIDLists((UINT)sel.abs.size(), const_cast<PCIDLIST_ABSOLUTE_ARRAY>(reinterpret_cast<PIDLIST_ABSOLUTE*>(sel.abs.data())), m->items.put());
        }
        m->msResolve = NowMs() - t0;
        if (FAILED(BuildClassic(*m, sel))) return m;
        // Background menus: Explorer passes NO item array to extension commands; they find the
        // folder through their site (our ViewSite). Verified on 26300: with the folder as an
        // item array instead, PowerRename and AMD hide themselves; with neither, the Terminal
        // entries hide. Only "no array + site" reproduces Windows' desktop background menu.
        // (m->items stays empty for the background.)
        IShellItem* siteFolder = r.background ? m->folderItem.get() : nullptr;
        winrt::com_ptr<IShellItem> parentItem;
        if (!r.background && sel.parent)
        {
            // Item menus: the site is the folder the items are shown in.
            winrt::com_ptr<IPersistFolder2> pf;
            PIDLIST_ABSOLUTE pidl = nullptr;
            if (SUCCEEDED(sel.parent->QueryInterface(IID_PPV_ARGS(pf.put()))) && SUCCEEDED(pf->GetCurFolder(&pidl)))
            {
                SHCreateItemFromIDList(pidl, IID_PPV_ARGS(parentItem.put()));
                CoTaskMemFree(pidl);
            }
            siteFolder = parentItem.get();
        }
        m->siteFolder.copy_from(siteFolder);

        // ---- 1. command row (item menus) --------------------------------------------
        const bool folders = IsFolderSelection(r);
        if (!r.background)
        {
            for (auto& v : pol.rowVerbs)
            {
                bool store = false;
                for (auto& s : pol.storeItems) if (s.row && s.name == v) store = true;
                if (store)
                {
                    std::wstring icon;
                    if (auto c = CommandStoreHandler(v, &icon))
                    {
                        MenuEntry e; e.source = Source::CommandStore; e.id = v;
                        if (ReadExplorerCommand(c.get(), m->items.get(), e, false))
                        {
                            e.id = v; if (e.iconRef.empty()) e.iconRef = icon;
                            Decorate(e, pol); e.iconRef.clear(); // row uses glyphs
                            if (auto g = pol.glyphs.find(v); g != pol.glyphs.end()) e.glyph = g->second;
                            m->row.push_back(std::move(e));
                        }
                    }
                    continue;
                }
                auto it = FindId(m->classicAll, v);
                if (it == m->classicAll.end()) continue;
                MenuEntry e = *it; e.classicBitmap = nullptr; Decorate(e, pol);
                if (auto g = pol.glyphs.find(v); g != pol.glyphs.end()) e.glyph = g->second;
                m->row.push_back(std::move(e));
            }
        }

        // ---- 2. promoted classic verbs, in the classic menu's own order --------------
        const auto& promoted = r.background ? pol.backgroundPromoted : pol.itemPromoted;
        std::vector<MenuEntry> list;
        IShellFolder* primaryParent = r.background ? nullptr : sel.parent.get();
        PCUITEMID_CHILD primary = (!r.background && !sel.children.empty()) ? sel.children[0] : nullptr;
        auto legacyDisabled = [&](const MenuEntry& e) {
            return pol.honorLegacyDisable && primary && QueryStaticVerb(primaryParent, primary, e.id).legacyDisable;
        };
        bool defaultDropped = false;
        for (auto& e : m->classicAll)
        {
            if (e.separator) { if (r.background) PushSep(list); continue; }
            if (e.id.empty() || !VerbIn(promoted, e.id) || VerbIn(pol.rowVerbs, e.id)) continue;
            if (legacyDisabled(e)) { if (e.isDefault) defaultDropped = true; continue; }
            MenuEntry c = e;
            if (r.background) c.isDefault = false; // background menus show no "Enter" accelerator
            if (VerbIn(pol.alwaysSubmenu, c.id))
            {
                // Windows builds its own "Open with" submenu (handlers from the association
                // database), so the classic "Open with..." dialog item becomes a submenu too.
                c.hasSubmenu = true; c.childrenLoaded = false; c.children.clear();
                while (!c.text.empty() && (c.text.back() == L'.' || c.text.back() == L'…')) c.text.pop_back();
                // An item's "New" verb (Office registers one) shows Windows' ShellNew submenu, and
                // with it Windows' own label and access key ("Ne&w" -> W, desktop_docx.png), not
                // the verb's ("&New" -> N).
                if (!r.background && _wcsicmp(c.id.c_str(), L"New") == 0)
                {
                    std::wstring raw = PolicyString(L"shellNewLabel", L"");
                    if (!raw.empty()) { c.accessKey.clear(); c.text = StripMnemonic(raw, &c.accessKey); }
                }
            }
            Decorate(c, pol);
            list.push_back(std::move(c));
        }
        if (!r.background && pol.orderByPolicy)
        {
            auto rank = [&](const MenuEntry& e) {
                for (size_t i = 0; i < promoted.size(); ++i) if (_wcsicmp(promoted[i].c_str(), e.id.c_str()) == 0) return i;
                return promoted.size();
            };
            std::stable_sort(list.begin(), list.end(), [&](const MenuEntry& a, const MenuEntry& b) { return rank(a) < rank(b); });
        }
        if (defaultDropped)
        {
            // The classic default verb is LegacyDisable'd: Windows shows the next static verb of
            // the same ProgID first, then a separator (UNVERIFIED beyond .url; see menu_policy.json).
            for (auto& e : m->classicAll)
            {
                if (e.separator || e.id.empty() || e.isDefault || VerbIn(promoted, e.id) || VerbIn(pol.rowVerbs, e.id)) continue;
                auto info = QueryStaticVerb(primaryParent, primary, e.id);
                if (!info.found || info.legacyDisable) continue;
                MenuEntry c = e; c.accel.clear(); Decorate(c, pol); c.accel.clear();
                list.insert(list.begin(), Sep());
                list.insert(list.begin(), std::move(c));
                break;
            }
        }
        TrimSeps(list);

        // ---- 3. Windows built-ins that live in the CommandStore (Compress to, ...) ----
        if (!r.background)
            for (auto& s : pol.storeItems)
            {
                if (s.row || (folders && !s.folders) || (!folders && !s.files)) continue;
                std::wstring icon;
                auto c = CommandStoreHandler(s.name, &icon);
                if (!c) continue;
                MenuEntry e; e.source = Source::CommandStore;
                if (!ReadExplorerCommand(c.get(), m->items.get(), e, false)) continue;
                e.id = s.name; if (e.iconRef.empty()) e.iconRef = icon;
                Decorate(e, pol);
                auto at = s.after.empty() ? list.end() : FindId(list, s.after);
                if (at != list.end()) ++at;
                list.insert(at, std::move(e));
            }

        // ---- 4. Deskboxes' own top entries (View / Sort by / Refresh / Undo ...) ------
        if (r.background && r.desktop)
        {
            auto own = DesktopOwnEntries(r, pol);
            list.insert(list.begin(), own.begin(), own.end());
        }
        // Background row (Paste), and entries kept for Shift-opened menus only.
        if (r.background)
        {
            for (auto& v : pol.backgroundRowVerbs)
            {
                auto it = FindId(list, v);
                if (it == list.end()) continue;
                MenuEntry e = *it;
                list.erase(it);
                e.classicBitmap = nullptr;
                if (auto g = pol.glyphs.find(v); g != pol.glyphs.end()) e.glyph = g->second;
                m->row.push_back(std::move(e));
            }
            if (!r.shift)
                for (auto& v : pol.extendedOnlyVerbs)
                    if (auto it = FindId(list, v); it != list.end()) list.erase(it);
            TrimSeps(list);
        }
        InsertExtras(*m, L"top", list, list.begin());
        {
            auto at = FindId(list, L"New");
            InsertExtras(*m, L"beforeNew", list, at == list.end() ? list.end() : at);
            auto props = FindId(list, L"properties");
            InsertExtras(*m, L"beforeProperties", list, props);
        }

        // ---- 5. third-party modern entries (packaged IExplorerCommand) ---------------
        auto t1 = NowMs();
        std::vector<MenuEntry> top, appExt;
        for (auto& reg : PackagedFor(ItemTypesFor(r)))
        {
            auto c = GetCommand(reg.clsid);
            if (!c) continue;
            SetCommandSite(c.get(), siteFolder);
            MenuEntry e; e.source = Source::Packaged; e.packageFamily = reg.family;
            if (!ReadExplorerCommand(c.get(), m->items.get(), e, false)) continue;
            if (e.id.empty()) { wchar_t g[64]; StringFromGUID2(reg.clsid, g, 64); e.id = g; }
            if (e.hasSubmenu)
            {
                // A root whose subcommands are all hidden is not shown (an empty flyout).
                LoadChildren(e, m->items.get());
                size_t visible = 0;
                for (auto& ch : e.children) if (!ch.separator) ++visible;
                if (visible == 0) { Log(L"packaged %s: no visible subcommands, hidden", e.text.c_str()); continue; }
            }
            if (!m->customizable || ExtensionPromoted(reg.family)) top.push_back(std::move(e));
            else appExt.push_back(std::move(e));
        }
        m->msPackaged = NowMs() - t1;

        PushSep(list);
        InsertExtras(*m, L"extensions", list, list.end());
        for (auto& e : top) list.push_back(std::move(e));
        if (m->customizable)
        {
            if (!appExt.empty())
            {
                MenuEntry ax; ax.source = Source::Synthetic; ax.id = L"Windows.ExtensionsFlyout";
                ax.text = StripMnemonic(PolicyString(L"appExtensions", L"App &extensions"), &ax.accessKey);
                ax.hasSubmenu = true; ax.childrenLoaded = true; ax.children = std::move(appExt);
                if (!pol.appExtensionsFooter.empty())
                {
                    // Separator before it: UNVERIFIED (the probe reads names, not gaps, in submenus).
                    MenuEntry me; me.source = Source::Synthetic; me.id = L"Windows.ManageExtensions";
                    me.text = StripMnemonic(PolicyString(pol.appExtensionsFooter.c_str(), L"Manage &extensions"), &me.accessKey);
                    Decorate(me, pol);
                    ax.children.push_back(std::move(me));
                }
                Decorate(ax, pol);
                list.push_back(std::move(ax));
            }
            PushSep(list);
            MenuEntry cm; cm.source = Source::Synthetic; cm.id = L"Windows.CustomizeMenu";
            cm.text = StripMnemonic(PolicyString(L"customizeMenu", L"&Customize menu"), &cm.accessKey);
            Decorate(cm, pol);
            list.push_back(std::move(cm));
        }
        else
        {
            PushSep(list);
            MenuEntry mo; mo.source = Source::Synthetic; mo.id = L"showMoreOptions";
            mo.text = StripMnemonic(PolicyString(L"showMoreOptions", L"Sho&w more options"), &mo.accessKey);
            Decorate(mo, pol);
            list.push_back(std::move(mo));
        }
        InsertExtras(*m, L"bottom", list, list.end());
        TrimSeps(list);
        m->list = std::move(list);
        m->msTotal = NowMs() - t0;
        return m;
    }

    static void DumpEntries(std::wstring& out, const std::vector<MenuEntry>& v, int depth)
    {
        for (auto& e : v)
        {
            std::wstring pad(2 + depth * 4, L' ');
            if (e.separator) { out += pad + L"----\n"; continue; }
            wchar_t line[1024];
            const wchar_t* src = e.source == Source::Classic ? L"classic" : e.source == Source::Packaged ? L"packaged" : e.source == Source::CommandStore ? L"store" : e.source == Source::Deskboxes ? L"deskboxes" : L"synthetic";
            swprintf_s(line, L"%-42s %-28s %-14s%s  (%s%s%s%s)\n", e.text.c_str(), e.id.c_str(), e.accel.c_str(), e.hasSubmenu ? L" >" : L"  ",
                src, e.accessKey.empty() ? L"" : (L" key=" + e.accessKey).c_str(), e.disabled ? L" disabled" : L"", e.packageFamily.empty() ? L"" : (L" " + e.packageFamily).c_str());
            out += pad + line;
            if (!e.children.empty()) DumpEntries(out, e.children, depth + 1);
        }
    }

    static winrt::Windows::Data::Json::JsonArray JsonEntries(const std::vector<MenuEntry>& v, const wchar_t* group)
    {
        using namespace winrt::Windows::Data::Json;
        JsonArray a;
        for (auto& e : v)
        {
            JsonObject o;
            o.SetNamedValue(L"group", JsonValue::CreateStringValue(group));
            if (e.separator)
            {
                o.SetNamedValue(L"name", JsonValue::CreateStringValue(L"----"));
                o.SetNamedValue(L"id", JsonValue::CreateStringValue(L"separator"));
            }
            else
            {
                o.SetNamedValue(L"name", JsonValue::CreateStringValue(e.text));
                o.SetNamedValue(L"id", JsonValue::CreateStringValue(e.id));
                o.SetNamedValue(L"accel", JsonValue::CreateStringValue(e.accel));
                o.SetNamedValue(L"accessKey", JsonValue::CreateStringValue(e.accessKey));
                o.SetNamedValue(L"submenu", JsonValue::CreateBooleanValue(e.hasSubmenu));
                o.SetNamedValue(L"enabled", JsonValue::CreateBooleanValue(!e.disabled));
                const wchar_t* src = e.source == Source::Classic ? L"classic" : e.source == Source::Packaged ? L"packaged" : e.source == Source::CommandStore ? L"store" : e.source == Source::Deskboxes ? L"deskboxes" : L"synthetic";
                o.SetNamedValue(L"source", JsonValue::CreateStringValue(src));
                if (!e.packageFamily.empty()) o.SetNamedValue(L"package", JsonValue::CreateStringValue(e.packageFamily));
                if (!e.children.empty()) o.SetNamedValue(L"children", JsonEntries(e.children, L"sub"));
            }
            a.Append(o);
        }
        return a;
    }

    std::wstring DumpMenuJson(const BuiltMenu& m, bool includeClassic)
    {
        using namespace winrt::Windows::Data::Json;
        JsonObject o;
        o.SetNamedValue(L"msResolve", JsonValue::CreateNumberValue(m.msResolve));
        o.SetNamedValue(L"msClassic", JsonValue::CreateNumberValue(m.msClassic));
        o.SetNamedValue(L"msPackaged", JsonValue::CreateNumberValue(m.msPackaged));
        o.SetNamedValue(L"msTotal", JsonValue::CreateNumberValue(m.msTotal));
        o.SetNamedValue(L"customizable", JsonValue::CreateBooleanValue(m.customizable));
        o.SetNamedValue(L"row", JsonEntries(m.row, L"row"));
        o.SetNamedValue(L"list", JsonEntries(m.list, L"list"));
        if (includeClassic) o.SetNamedValue(L"classic", JsonEntries(m.classicAll, L"classic"));
        return std::wstring(o.Stringify().c_str());
    }

    std::wstring DumpMenu(const BuiltMenu& m, bool includeClassic)
    {
        std::wstring out;
        wchar_t hdr[256];
        swprintf_s(hdr, L"timing: resolve %.1f ms, classic %.1f ms, packaged %.1f ms, total %.1f ms; customizable=%d\n", m.msResolve, m.msClassic, m.msPackaged, m.msTotal, (int)m.customizable);
        out += hdr;
        out += L"  row: ";
        for (size_t i = 0; i < m.row.size(); ++i) out += (i ? L" | " : L"") + m.row[i].text + L"[" + m.row[i].id + L"]";
        out += L"\n";
        DumpEntries(out, m.list, 0);
        if (includeClassic)
        {
            out += L"  --- classic (full) ---\n";
            DumpEntries(out, m.classicAll, 0);
        }
        return out;
    }
}

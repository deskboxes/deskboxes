#include "pch.h"
#include "MenuModel.h"
#include "Log.h"

using namespace winrt;
using namespace winrt::Windows::Data::Json;

namespace dbx
{
    static std::vector<std::wstring> StrList(JsonObject const& o, const wchar_t* k)
    {
        std::vector<std::wstring> v;
        if (o.HasKey(k)) for (auto const& x : o.GetNamedArray(k)) v.emplace_back(x.GetString().c_str());
        return v;
    }
    static void StrMap(JsonObject const& o, const wchar_t* k, std::map<std::wstring, std::wstring>& m)
    {
        if (!o.HasKey(k)) return;
        for (auto const& kv : o.GetNamedObject(k)) m[kv.Key().c_str()] = kv.Value().GetString().c_str();
    }

    static void Apply(JsonObject const& p, Policy& pol)
    {
        if (p.HasKey(L"name")) pol.name = p.GetNamedString(L"name").c_str();
        if (p.HasKey(L"customizableMenu")) pol.customizable = p.GetNamedBoolean(L"customizableMenu");
        if (p.HasKey(L"itemPromotedVerbs")) pol.itemPromoted = StrList(p, L"itemPromotedVerbs");
        if (p.HasKey(L"backgroundPromotedVerbs")) pol.backgroundPromoted = StrList(p, L"backgroundPromotedVerbs");
        if (p.HasKey(L"rowVerbs")) pol.rowVerbs = StrList(p, L"rowVerbs");
        if (p.HasKey(L"backgroundRowVerbs")) pol.backgroundRowVerbs = StrList(p, L"backgroundRowVerbs");
        if (p.HasKey(L"extendedOnlyVerbs")) pol.extendedOnlyVerbs = StrList(p, L"extendedOnlyVerbs");
        if (p.HasKey(L"alwaysSubmenuVerbs")) pol.alwaysSubmenu = StrList(p, L"alwaysSubmenuVerbs");
        if (p.HasKey(L"itemOrder")) pol.orderByPolicy = p.GetNamedString(L"itemOrder") == L"policy";
        if (p.HasKey(L"honorLegacyDisable")) pol.honorLegacyDisable = p.GetNamedBoolean(L"honorLegacyDisable");
        if (p.HasKey(L"commandStoreItems"))
        {
            pol.storeItems.clear();
            for (auto const& x : p.GetNamedArray(L"commandStoreItems"))
            {
                auto o = x.GetObject();
                Policy::StoreItem s;
                s.name = o.GetNamedString(L"name").c_str();
                s.after = o.GetNamedString(L"after", L"").c_str();
                s.row = o.GetNamedBoolean(L"row", false);
                s.files = o.GetNamedBoolean(L"files", true);
                s.folders = o.GetNamedBoolean(L"folders", true);
                pol.storeItems.push_back(s);
            }
        }
        if (p.HasKey(L"desktopOwnEntries"))
        {
            auto o = p.GetNamedObject(L"desktopOwnEntries");
            pol.ownMenuResource = o.GetNamedString(L"menuResource", L"").c_str();
            pol.ownMenuId = (UINT)o.GetNamedNumber(L"menuId", 0);
            pol.desktopOwn.clear();
            for (auto const& x : o.GetNamedArray(L"items"))
            {
                auto e = x.GetObject();
                Policy::OwnEntry oe;
                oe.id = e.GetNamedString(L"id").c_str();
                oe.when = e.GetNamedString(L"when", L"").c_str();
                oe.command = (UINT)e.GetNamedNumber(L"command", 0);
                oe.submenu = e.GetNamedBoolean(L"submenu", false);
                oe.group = (int)e.GetNamedNumber(L"group", 1);
                if (e.HasKey(L"position")) for (auto const& n : e.GetNamedArray(L"position")) oe.position.push_back((int)n.GetNumber());
                pol.desktopOwn.push_back(oe);
            }
        }
        StrMap(p, L"accelerators", pol.accelerators);
        if (p.HasKey(L"iconAssets"))
        {
            auto o = p.GetNamedObject(L"iconAssets");
            pol.iconAssetPackage = o.GetNamedString(L"package", L"").c_str();
            pol.iconAssetPath = o.GetNamedString(L"path", L"").c_str();
            StrMap(o, L"names", pol.iconAssetNames);
        }
        StrMap(p, L"glyphs", pol.glyphs);
        StrMap(p, L"strings", pol.strings);
        if (p.HasKey(L"defaultAcceleratorWhenNoOpen")) pol.defaultAcceleratorWhenNoOpen = p.GetNamedString(L"defaultAcceleratorWhenNoOpen").c_str();
        if (p.HasKey(L"appExtensionsFooter")) pol.appExtensionsFooter = p.GetNamedString(L"appExtensionsFooter").c_str();
        if (p.HasKey(L"customizeMenuUri")) pol.customizeMenuUri = p.GetNamedString(L"customizeMenuUri").c_str();
    }

    const Policy& GetPolicy()
    {
        static Policy pol = [] {
            Policy p;
            wchar_t exe[MAX_PATH]; GetModuleFileNameW(nullptr, exe, MAX_PATH);
            std::wstring path = exe; path = path.substr(0, path.find_last_of(L'\\')) + L"\\menu_policy.json";
            HANDLE f = CreateFileW(path.c_str(), GENERIC_READ, FILE_SHARE_READ, nullptr, OPEN_EXISTING, 0, nullptr);
            if (f == INVALID_HANDLE_VALUE) { Log(L"menu_policy.json missing at %s", path.c_str()); return p; }
            LARGE_INTEGER sz{}; GetFileSizeEx(f, &sz);
            std::string buf((size_t)sz.QuadPart, '\0'); DWORD rd = 0;
            ReadFile(f, buf.data(), (DWORD)buf.size(), &rd, nullptr); CloseHandle(f);
            if (buf.rfind("\xEF\xBB\xBF", 0) == 0) buf.erase(0, 3); // editors may add a UTF-8 BOM; Windows.Data.Json rejects it
            try
            {
                auto root = JsonObject::Parse(Utf8ToWide(buf));
                DWORD build = WindowsBuild();
                std::map<std::wstring, JsonObject> byName;
                JsonObject chosen{ nullptr }; double best = -1;
                for (auto const& x : root.GetNamedArray(L"profiles"))
                {
                    auto o = x.GetObject();
                    byName[o.GetNamedString(L"name").c_str()] = o;
                    double mb = o.GetNamedNumber(L"minBuild", 0);
                    if (mb <= build && mb > best) { best = mb; chosen = o; }
                }
                if (chosen)
                {
                    if (chosen.HasKey(L"inherit")) { auto it = byName.find(chosen.GetNamedString(L"inherit").c_str()); if (it != byName.end()) Apply(it->second, p); }
                    Apply(chosen, p);
                }
                Log(L"menu policy profile '%s' for build %lu", p.name.c_str(), build);
            }
            catch (hresult_error const& e) { Log(L"menu_policy.json parse error: %s", e.message().c_str()); }
            return p;
        }();
        return pol;
    }

    bool VerbIn(const std::vector<std::wstring>& list, const std::wstring& verb)
    {
        for (auto& v : list) if (_wcsicmp(v.c_str(), verb.c_str()) == 0) return true;
        return false;
    }

    std::wstring PolicyString(const wchar_t* key, const wchar_t* fallback)
    {
        auto& s = GetPolicy().strings;
        auto it = s.find(key);
        if (it != s.end())
        {
            auto v = LoadIndirect(it->second);
            if (!v.empty()) return v;
        }
        return fallback ? fallback : L"";
    }
}

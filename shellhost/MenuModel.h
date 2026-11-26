#pragma once
// Menu model shared by the builder (STA worker), the dump mode and the WinUI view.
//
// Everything machine-specific is discovered at runtime (see MenuBuilder.cpp). The only
// fixed input is Windows' presentation policy (menu_policy.json): which Windows canonical
// verbs are promoted from the classic menu, where the command row goes, which strings are
// Windows' own. See docs/ARCHITECTURE.md §12.

#include <string>
#include <vector>

namespace dbx
{
    enum class Source { Classic, Packaged, CommandStore, Deskboxes, Synthetic };

    struct MenuEntry
    {
        std::wstring id;          // canonical verb, "Windows.X", CLSID, or "deskboxes.*"
        std::wstring text;        // display text (mnemonic '&' removed)
        std::wstring accessKey;   // from the '&' mnemonic in the source text
        std::wstring accel;       // "Ctrl+Shift+C" (policy) or from a "\t" suffix in classic text
        std::wstring iconRef;     // "path,-id" / "path,index" / "@..." resource icon
        std::wstring glyph;       // Segoe Fluent Icons glyph (policy) when there is no iconRef
        std::wstring color;       // "#RRGGBB": a color swatch as the icon (Deskboxes' box colors)
        HBITMAP classicBitmap = nullptr; // MENUITEMINFO.hbmpItem (owned by the classic HMENU)
        Source source = Source::Classic;
        bool separator = false, disabled = false, checked = false, radio = false;
        bool isDefault = false, hasSubmenu = false, shield = false;
        bool checkable = false;   // toggle item (checked = state); radio = one of a group
        UINT classicId = 0;       // absolute HMENU command id (Classic)
        HMENU classicSub = nullptr; // classic submenu (filled on WM_INITMENUPOPUP when opened)
        int classicPos = -1;        // position in its parent HMENU
        winrt::com_ptr<IExplorerCommand> cmd; // Packaged / CommandStore
        std::wstring packageFamily;           // Packaged
        std::vector<MenuEntry> children;
        bool childrenLoaded = false;
    };

    struct MenuRequest
    {
        bool background = false;
        bool ownOnly = false;               // only Deskboxes' own entries (the page name's menu)
        std::wstring folder;                // parsing name of the folder (desktop or fixture)
        std::vector<std::wstring> items;    // parsing names (full paths or ::{CLSID})
        bool shift = false;
        bool desktop = false;               // background of the desktop surface (adds the view's own entries)
        bool coreUndo = false, coreRedo = false; // the core has something to undo / redo
        std::wstring coreUndoText, coreRedoText; // "Undo Move", "Redo Delete" (empty: Windows' "Undo")
        bool keyboard = false;              // opened from the keyboard (Menu key, Shift+F10): show KeyTips
        POINT pt{};                         // physical screen pixels
        HWND owner = nullptr;
        std::vector<MenuEntry> extra;       // Deskboxes' own entries (from the core)
        std::wstring extraJson;             // the same, as sent (part of the prefetch cache key)
        std::map<std::wstring, std::vector<MenuEntry>> ownChildren; // children of the view's own submenus (View, Sort by), from the core
    };

    struct BuiltMenu
    {
        MenuRequest req;
        winrt::com_ptr<IContextMenu> classic;   // keep alive for invoke / "Show more options"
        HMENU hmenu = nullptr;
        UINT idFirst = 1, idLast = 0x7FFF;
        winrt::com_ptr<IShellItemArray> items;  // selection (empty for background)
        winrt::com_ptr<IShellItem> folderItem;  // background target
        winrt::com_ptr<IShellItem> siteFolder;  // folder shown by "our view": site for commands, target of New
        std::vector<MenuEntry> classicAll;      // full classic tree (dump + diagnostics)
        std::vector<MenuEntry> row;             // command row (Cut/Copy/Rename/Share/Delete)
        std::vector<MenuEntry> list;            // the menu list (top level)
        // The shell's part (BuildShellMenu), from which AssembleMenu makes row and list per show.
        std::vector<MenuEntry> shellRow, shellList, packaged;
        bool shellOk = false;
        bool customizable = false;              // 26H2 "Customize menu" feature present
        double msResolve = 0, msClassic = 0, msPackaged = 0, msTotal = 0;
        double msAssemble = 0, builtAt = 0;     // builtAt: NowMs() when the shell's part was done
        ~BuiltMenu();
    };

    // ---- ShellUtil.cpp
    struct Selection
    {
        winrt::com_ptr<IShellFolder> parent;
        std::vector<PIDLIST_ABSOLUTE> abs;
        std::vector<PCUITEMID_CHILD> children; // point into parent's namespace
        std::vector<PITEMID_CHILD> ownedChildren;
        ~Selection();
    };
    HRESULT ResolveSelection(const MenuRequest& r, Selection& out);
    HRESULT ResolveFolder(const std::wstring& parsing, winrt::com_ptr<IShellFolder>& folder, winrt::com_ptr<IShellItem>& item);
    std::wstring StripMnemonic(const std::wstring& s, std::wstring* accessKey);
    std::wstring LoadIndirect(const std::wstring& ref);
    // Registry flags of a static verb under the item's ProgID (IQueryAssociations shell-exec class).
    struct StaticVerbInfo { bool found = false, legacyDisable = false; };
    StaticVerbInfo QueryStaticVerb(IShellFolder* parent, PCUITEMID_CHILD child, const std::wstring& verb);
    DWORD WindowsBuild(DWORD* ubr = nullptr);

    // ---- ClassicMenu.cpp
    HRESULT BuildClassic(BuiltMenu& m, const Selection& sel);
    void ReadClassicMenu(IContextMenu* cm, HMENU h, UINT idFirst, std::vector<MenuEntry>& out, int depth);
    HRESULT InvokeClassic(BuiltMenu& m, const MenuEntry& e, HWND owner, POINT pt);
    HRESULT InvokeClassicVerb(BuiltMenu& m, const wchar_t* verb, HWND owner, POINT pt);
    UINT ShowClassicMenu(BuiltMenu& m, HWND owner, POINT pt); // "Show more options"
    void LoadClassicSubmenu(BuiltMenu& m, MenuEntry& e);       // fill e.children from e.classicSub
    HRESULT InvokeExplorerCommand(BuiltMenu& m, const MenuEntry& e);
    // ---- ShellNew ("New" submenu of a folder), used for an item's "New" verb
    struct NewMenu { winrt::com_ptr<IContextMenu> cm; HMENU hmenu = nullptr; std::vector<MenuEntry> items; ~NewMenu(); };
    std::unique_ptr<NewMenu> BuildNewMenu(IShellItem* folder, HWND owner);
    HRESULT InvokeNewMenu(NewMenu& n, const MenuEntry& e, HWND owner);

    // ---- ExplorerCommands.cpp
    struct PackagedRegistration { std::wstring family, fullName, displayName, itemType, verbId; CLSID clsid{}; };
    void LoadPackagedRegistrations(bool force = false);
    // The same on a thread of its own; the first Load waits for it. With the list cached from
    // last time, that is used at once and the scan only checks it. `ready` once the list is there,
    // `changed` if the scan found it changed since the cache (both on the calling thread).
    void StartPackagedScan(std::function<void()> ready, std::function<void()> changed);
    // Every extension command of a menu built a while ago still answers (its server may have
    // ended since: an update, a crash).
    bool CommandsAlive(const BuiltMenu& m);
    std::vector<PackagedRegistration> PackagedFor(const std::vector<std::wstring>& itemTypes);
    winrt::com_ptr<IExplorerCommand> GetCommand(const CLSID& clsid);   // cached instances
    winrt::com_ptr<IExplorerCommand> CommandStoreHandler(const std::wstring& name, std::wstring* iconRef);
    bool ReadExplorerCommand(IExplorerCommand* c, IShellItemArray* items, MenuEntry& e, bool loadChildren);
    void SetCommandSite(IExplorerCommand* c, IShellItem* folder); // our view as the command's site
    void LoadChildren(MenuEntry& e, IShellItemArray* items);
    std::vector<std::wstring> ItemTypesFor(const MenuRequest& r);
    bool ExtensionPromoted(const std::wstring& packageFamily); // 26H2 HKCU preference
    bool CustomizableMenuPresent();

    // ---- "Open with" submenu (association handlers for the primary item)
    struct OpenWithEntry { std::wstring name, iconRef; winrt::com_ptr<IAssocHandler> handler; };
    std::vector<OpenWithEntry> OpenWithHandlers(const BuiltMenu& m);
    HRESULT InvokeOpenWith(BuiltMenu& m, IAssocHandler* h);

    // ---- MenuPolicy.cpp
    struct Policy
    {
        std::wstring name;
        bool customizable = false;
        std::vector<std::wstring> itemPromoted, backgroundPromoted, rowVerbs, alwaysSubmenu;
        std::vector<std::wstring> backgroundRowVerbs, extendedOnlyVerbs; // background row; shown only with Shift
        bool orderByPolicy = false;       // "itemOrder": "policy" — Windows' order, not the classic order
        bool honorLegacyDisable = false;
        struct StoreItem { std::wstring name, after; bool row = false, files = true, folders = true; };
        std::vector<StoreItem> storeItems;
        std::map<std::wstring, std::wstring> accelerators, glyphs, strings;
        std::wstring iconAssetPackage, iconAssetPath;          // Explorer's own SVG menu icons
        std::map<std::wstring, std::wstring> iconAssetNames;   // id -> asset base name, where they differ
        std::wstring defaultAcceleratorWhenNoOpen, customizeMenuUri, appExtensionsFooter;
        struct OwnEntry { std::wstring id, when; std::vector<int> position; UINT command = 0; bool submenu = false; int group = 1; };
        std::wstring ownMenuResource; UINT ownMenuId = 0;
        std::vector<OwnEntry> desktopOwn;
    };
    const Policy& GetPolicy();
    bool VerbIn(const std::vector<std::wstring>& list, const std::wstring& verb);
    std::wstring PolicyString(const wchar_t* key, const wchar_t* fallback);

    // ---- MenuBuilder.cpp
    // A menu is built in two parts. The shell's part (resolving the items, the classic handlers'
    // QueryContextMenu, the extension commands' GetState) is what takes the time (25-90 ms warm),
    // and depends only on the items / folder and Shift: it can be built ahead (a prefetch, the
    // desktop background). The rest changes with every show (Deskboxes' own entries, the desktop
    // view's View / Sort by / Paste / Undo, where extensions go): AssembleMenu, once per menu,
    // at show (well under 1 ms).
    std::unique_ptr<BuiltMenu> BuildShellMenu(const MenuRequest& r);
    void AssembleMenu(BuiltMenu& m, const MenuRequest& r);
    std::unique_ptr<BuiltMenu> BuildMenu(const MenuRequest& r); // both, at once
    std::wstring DumpMenu(const BuiltMenu& m, bool includeClassic);
    std::wstring DumpMenuJson(const BuiltMenu& m, bool includeClassic); // same shape as tools/menu-probe output
}

//! "Turn on Deskboxes" in Windows' desktop menu while Deskboxes is turned off.
//!
//! Two ways, both only while off:
//! - Windows 11's main menu: the sparse package's `IExplorerCommand` (crates/menuext), which the
//!   installer registers (it needs admin once) and which hides itself while Deskboxes is on.
//! - Everywhere else (Windows 10's menu, Windows 11's "Show more options", or when the package
//!   could not be registered): a classic verb under the user's DesktopBackground key, which the
//!   core adds when turned off and removes when it starts.
//!
//! Windows 26H2's "Customize menu" puts packaged menu extensions into an "App extensions"
//! submenu unless they are listed (Enabled=1) in its Preferences: `pin_modern_verb` lists ours,
//! once, so "Turn on Deskboxes" sits in the menu itself (the user may turn it off there later).

use crate::reg;
use std::path::{Path, PathBuf};

const VERB: &str = r"Software\Classes\DesktopBackground\Shell\DeskboxesTurnOn";

/// The menu package (crates/menuext/package/AppxManifest.xml).
pub const MENU_PACKAGE: &str = "Deskboxes.DesktopMenu";
/// Its package family name (name + hash of the manifest's publisher).
pub const MENU_PFN: &str = "Deskboxes.DesktopMenu_hq7sd8dcym9vw";
/// Windows 26H2's "Customize menu" settings: packaged extensions listed under `extensions` with
/// Enabled=1 sit in the menu itself, the others go into "App extensions".
pub const MENU_PREFS: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\ContextMenu\Preferences";

/// Where the menu package's files are registered from, if it is registered for this user
/// (Windows' own package list; PowerShell 5.1's `Get-AppxPackage` has no such property, which
/// left the menu entry in "App extensions" and asked for admin on every install).
pub fn menu_package_dir() -> Option<PathBuf> {
    use windows::core::HSTRING;
    let pm = windows::Management::Deployment::PackageManager::new().ok()?;
    let found = pm.FindPackagesByUserSecurityIdPackageFamilyName(&HSTRING::new(), &HSTRING::from(MENU_PFN)).ok()?;
    found.into_iter().find_map(|p| p.EffectiveExternalPath().ok().map(|s| s.to_string()).filter(|s| !s.is_empty()).map(PathBuf::from))
}

/// "Turn on Deskboxes" in Windows 11's menu itself, not in "App extensions" (26H2's Customize
/// menu): our package listed as enabled, if it is registered, the feature is there, and the user
/// has not decided about it already (an entry of ours that says 0 is theirs to keep).
pub fn pin_modern_verb() {
    if !reg::key_exists(MENU_PREFS) {
        return;
    }
    let key = format!(r"{MENU_PREFS}\extensions\{MENU_PFN}");
    if reg::get_dword(&key, "Enabled").is_some() {
        return;
    }
    if menu_package_dir().is_none() {
        log::info!("turn-on: the menu package is not registered");
        return;
    }
    let r = reg::set_dword(&key, "Enabled", 1);
    log::info!("turn-on: listed in Windows' menu itself (Customize menu): {r:?}");
}

/// Add the classic "Turn on Deskboxes" verb (starts `watchdog --launch --enable`).
pub fn add_verb(title: &str, watchdog: &Path, icon: &Path) {
    let r = reg::set_string(VERB, "MUIVerb", title)
        .and_then(|_| reg::set_string(VERB, "Icon", &format!("\"{}\",0", icon.display())))
        .and_then(|_| reg::set_string(&format!(r"{VERB}\command"), "", &format!("\"{}\" --launch --enable", watchdog.display())));
    log::info!("turn-on verb added: {r:?}");
}

pub fn remove_verb() {
    if reg::get_string(VERB, "MUIVerb").is_some() {
        if reg::delete_tree(VERB) {
            log::info!("turn-on verb removed");
        } else {
            log::warn!("turn-on verb: could not remove {VERB}");
        }
    }
}

#[cfg(test)]
mod tests {
    /// Prints where this user's menu package is registered from (run with --ignored).
    #[test]
    #[ignore]
    fn menu_package_dir_here() {
        println!("menu package dir: {:?}", super::menu_package_dir());
    }
}

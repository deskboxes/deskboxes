//! "Turn on Deskboxes" in Windows' desktop menu while Deskboxes is turned off.
//!
//! Two ways, both only while off:
//! - Windows 11's main menu: the sparse package's `IExplorerCommand` (crates/menuext), which the
//!   installer registers (it needs admin once) and which hides itself while Deskboxes is on.
//! - Everywhere else (Windows 10's menu, Windows 11's "Show more options", or when the package
//!   could not be registered): a classic verb under the user's DesktopBackground key, which the
//!   core adds when turned off and removes when it starts.

use crate::reg;
use std::path::Path;

const VERB: &str = r"Software\Classes\DesktopBackground\Shell\DeskboxesTurnOn";

/// Add the classic "Turn on Deskboxes" verb (starts `watchdog --launch --enable`).
pub fn add_verb(title: &str, watchdog: &Path, icon: &Path) {
    let r = reg::set_string(VERB, "MUIVerb", title)
        .and_then(|_| reg::set_string(VERB, "Icon", &format!("\"{}\",0", icon.display())))
        .and_then(|_| reg::set_string(&format!(r"{VERB}\command"), "", &format!("\"{}\" --launch --enable", watchdog.display())));
    log::info!("turn-on verb added: {r:?}");
}

pub fn remove_verb() {
    if reg::get_string(VERB, "MUIVerb").is_some() {
        reg::delete_tree(VERB);
        log::info!("turn-on verb removed");
    }
}

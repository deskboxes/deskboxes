//! Narrator and other screen readers: a UI Automation tree for the desktop's window. The boxes
//! are groups named by their titles, the icons in them and on the desktop are list items named
//! as their files, and the keyboard focus is the icon with the focus rectangle.
//!
//! UI Automation asks from threads of its own: the UI thread keeps a copy of what is shown
//! (`update`, only while a screen reader is listening) and the providers read that copy, never
//! the App.

#![allow(non_upper_case_globals)]

use crate::app::App;
use deskboxes_core::geom::Rect;
use std::collections::HashMap;
use std::sync::Mutex;
use windows::core::{implement, BSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::System::Com::SAFEARRAY;
use windows::Win32::System::Ole::{SafeArrayCreateVector, SafeArrayPutElement};
use windows::Win32::System::Variant::{VARIANT, VT_I4};
use windows::Win32::UI::Accessibility::*;

#[derive(Clone, Debug, PartialEq)]
struct Node {
    /// "b<id>" for a box, the item key for an icon.
    key: String,
    /// The box an icon is in.
    parent: Option<String>,
    name: String,
    group: bool,
    /// On the screen, in pixels: left, top, width, height.
    rect: [f64; 4],
}

#[derive(Default, Clone, PartialEq)]
struct Tree {
    name: String,
    nodes: Vec<Node>,
    focus: Option<String>,
}

/// Each surface window's tree.
static TREES: Mutex<Option<HashMap<isize, Tree>>> = Mutex::new(None);

fn with_tree<T>(hwnd: isize, f: impl FnOnce(&Tree) -> T) -> Option<T> {
    let g = TREES.lock().unwrap_or_else(|p| p.into_inner());
    g.as_ref()?.get(&hwnd).map(f)
}

/// Keep the copy of what the screen in use shows; tell a screen reader where the focus went.
pub fn update(app: &App) {
    if !unsafe { UiaClientsAreListening() }.as_bool() {
        return;
    }
    let mut o = POINT::default();
    unsafe {
        let _ = ClientToScreen(app.hwnd, &mut o);
    }
    let s = app.scale as f64;
    let px = |r: Rect| [o.x as f64 + r.x as f64 * s, o.y as f64 + r.y as f64 * s, r.w as f64 * s, r.h as f64 * s];
    let mut nodes: Vec<Node> = app.layout.boxes.iter().map(|b| Node { key: format!("b{}", b.id), parent: None, name: b.title().to_string(), group: true, rect: px(b.rect) }).collect();
    for (k, r, bx) in app.visible_cells() {
        let Some(it) = app.items.get(&k) else { continue };
        nodes.push(Node { key: k.0.clone(), parent: bx.map(|id| format!("b{id}")), name: it.display_name.clone(), group: false, rect: px(r) });
    }
    let focus = app.sel.focus.as_ref().map(|k| k.0.clone()).filter(|f| nodes.iter().any(|n| &n.key == f));
    let tree = Tree { name: "Deskboxes".into(), nodes, focus: focus.clone() };
    let h = app.hwnd.0 as isize;
    let moved = {
        let mut g = TREES.lock().unwrap_or_else(|p| p.into_inner());
        let m = g.get_or_insert_with(HashMap::new);
        let was = m.get(&h).and_then(|t| t.focus.clone());
        if m.get(&h) != Some(&tree) {
            m.insert(h, tree);
        }
        was != focus
    };
    if let (true, Some(f)) = (moved, focus) {
        let p: IRawElementProviderSimple = Elem { hwnd: h, key: Some(f) }.into();
        unsafe {
            let _ = UiaRaiseAutomationEvent(&p, UIA_AutomationFocusChangedEventId);
        }
    }
}

/// The window is gone: its tree too.
pub fn forget(hwnd: HWND) {
    let mut g = TREES.lock().unwrap_or_else(|p| p.into_inner());
    if let Some(m) = g.as_mut() {
        m.remove(&(hwnd.0 as isize));
    }
}

/// WM_GETOBJECT for a surface window: its tree's root.
pub fn get_object(hwnd: HWND, w: WPARAM, l: LPARAM) -> Option<LRESULT> {
    if l.0 as i32 != UiaRootObjectId {
        return None;
    }
    let p: IRawElementProviderSimple = Elem { hwnd: hwnd.0 as isize, key: None }.into();
    Some(unsafe { UiaReturnRawElementProvider(hwnd, w, l, &p) })
}

/// The window (`key` None) or a box or icon in it.
#[implement(IRawElementProviderSimple, IRawElementProviderFragment, IRawElementProviderFragmentRoot)]
struct Elem {
    hwnd: isize,
    key: Option<String>,
}

fn none<T>() -> windows::core::Result<T> {
    Err(windows::core::Error::empty())
}

impl Elem {
    fn node(&self) -> Option<Node> {
        let k = self.key.as_ref()?;
        with_tree(self.hwnd, |t| t.nodes.iter().find(|n| &n.key == k).cloned()).flatten()
    }
    fn make(&self, key: Option<String>) -> IRawElementProviderFragment {
        Elem { hwnd: self.hwnd, key }.into()
    }
    /// The keys of the children of `parent` (None: the window's: boxes, then desktop icons).
    fn children(&self, parent: Option<&str>) -> Vec<String> {
        with_tree(self.hwnd, |t| {
            t.nodes
                .iter()
                .filter(|n| match parent {
                    None => n.group || n.parent.is_none(),
                    Some(p) => !n.group && n.parent.as_deref() == Some(p),
                })
                .map(|n| n.key.clone())
                .collect()
        })
        .unwrap_or_default()
    }
}

impl IRawElementProviderSimple_Impl for Elem_Impl {
    fn ProviderOptions(&self) -> windows::core::Result<ProviderOptions> {
        // Called on UI Automation's threads: everything read is behind the TREES lock.
        Ok(ProviderOptions_ServerSideProvider)
    }

    fn GetPatternProvider(&self, _pattern: UIA_PATTERN_ID) -> windows::core::Result<windows::core::IUnknown> {
        none()
    }

    fn GetPropertyValue(&self, id: UIA_PROPERTY_ID) -> windows::core::Result<VARIANT> {
        let Some(n) = self.node() else {
            // The window itself (the rest comes from its host provider).
            return Ok(match id {
                UIA_NamePropertyId => VARIANT::from(BSTR::from(with_tree(self.hwnd, |t| t.name.clone()).unwrap_or_else(|| "Deskboxes".into()))),
                UIA_ControlTypePropertyId => VARIANT::from(UIA_PaneControlTypeId.0),
                _ => VARIANT::default(),
            });
        };
        let focused = with_tree(self.hwnd, |t| t.focus.as_deref() == Some(n.key.as_str())).unwrap_or(false);
        Ok(match id {
            UIA_NamePropertyId => VARIANT::from(BSTR::from(n.name.as_str())),
            UIA_ControlTypePropertyId => VARIANT::from(if n.group { UIA_GroupControlTypeId.0 } else { UIA_ListItemControlTypeId.0 }),
            UIA_AutomationIdPropertyId => VARIANT::from(BSTR::from(n.key.as_str())),
            UIA_IsKeyboardFocusablePropertyId => VARIANT::from(!n.group),
            UIA_HasKeyboardFocusPropertyId => VARIANT::from(focused),
            UIA_IsControlElementPropertyId | UIA_IsContentElementPropertyId | UIA_IsEnabledPropertyId => VARIANT::from(true),
            _ => VARIANT::default(),
        })
    }

    fn HostRawElementProvider(&self) -> windows::core::Result<IRawElementProviderSimple> {
        if self.key.is_none() {
            unsafe { UiaHostProviderFromHwnd(HWND(self.hwnd as _)) }
        } else {
            none()
        }
    }
}

impl IRawElementProviderFragment_Impl for Elem_Impl {
    fn Navigate(&self, dir: NavigateDirection) -> windows::core::Result<IRawElementProviderFragment> {
        let me = self.node();
        if self.key.is_some() && me.is_none() {
            return none();
        }
        let parent = me.as_ref().map(|n| n.parent.clone());
        match dir {
            NavigateDirection_Parent => match parent {
                None => none(),
                Some(p) => Ok(self.make(p)),
            },
            NavigateDirection_FirstChild | NavigateDirection_LastChild => {
                if me.as_ref().is_some_and(|n| !n.group) {
                    return none();
                }
                let kids = self.children(self.key.as_deref());
                let k = if dir == NavigateDirection_FirstChild { kids.first() } else { kids.last() };
                k.map(|k| self.make(Some(k.clone()))).map_or_else(none, Ok)
            }
            NavigateDirection_NextSibling | NavigateDirection_PreviousSibling => {
                let Some(p) = parent else { return none() };
                let sibs = self.children(p.as_deref());
                let Some(i) = sibs.iter().position(|k| Some(k) == self.key.as_ref()) else { return none() };
                let j = if dir == NavigateDirection_NextSibling { i.checked_add(1) } else { i.checked_sub(1) };
                j.and_then(|j| sibs.get(j)).map(|k| self.make(Some(k.clone()))).map_or_else(none, Ok)
            }
            _ => none(),
        }
    }

    fn GetRuntimeId(&self) -> windows::core::Result<*mut SAFEARRAY> {
        let Some(k) = &self.key else { return Ok(std::ptr::null_mut()) };
        // A key's hash: the same box or icon is the same element from update to update.
        let mut h: u64 = 0xcbf29ce484222325;
        for b in k.bytes() {
            h = (h ^ b as u64).wrapping_mul(0x100000001b3);
        }
        unsafe {
            let sa = SafeArrayCreateVector(VT_I4, 0, 3);
            if sa.is_null() {
                return Err(E_OUTOFMEMORY.into());
            }
            for (i, v) in [UiaAppendRuntimeId as i32, h as i32, (h >> 32) as i32].iter().enumerate() {
                let idx = i as i32;
                SafeArrayPutElement(sa, &idx, v as *const i32 as *const _)?;
            }
            Ok(sa)
        }
    }

    fn BoundingRectangle(&self) -> windows::core::Result<UiaRect> {
        match self.node() {
            Some(n) => Ok(UiaRect { left: n.rect[0], top: n.rect[1], width: n.rect[2], height: n.rect[3] }),
            None => Ok(UiaRect::default()),
        }
    }

    fn GetEmbeddedFragmentRoots(&self) -> windows::core::Result<*mut SAFEARRAY> {
        Ok(std::ptr::null_mut())
    }

    fn SetFocus(&self) -> windows::core::Result<()> {
        Ok(())
    }

    fn FragmentRoot(&self) -> windows::core::Result<IRawElementProviderFragmentRoot> {
        Ok(Elem { hwnd: self.hwnd, key: None }.into())
    }
}

impl IRawElementProviderFragmentRoot_Impl for Elem_Impl {
    fn ElementProviderFromPoint(&self, x: f64, y: f64) -> windows::core::Result<IRawElementProviderFragment> {
        let inside = |r: &[f64; 4]| x >= r[0] && y >= r[1] && x < r[0] + r[2] && y < r[1] + r[3];
        // An icon before the box it is in.
        let hit = with_tree(self.hwnd, |t| t.nodes.iter().filter(|n| inside(&n.rect)).min_by_key(|n| n.group).map(|n| n.key.clone())).flatten();
        Ok(self.make(hit))
    }

    fn GetFocus(&self) -> windows::core::Result<IRawElementProviderFragment> {
        match with_tree(self.hwnd, |t| t.focus.clone()).flatten() {
            Some(f) => Ok(self.make(Some(f))),
            None => none(),
        }
    }
}

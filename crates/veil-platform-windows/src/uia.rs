//! UI Automation queries. Every function here runs on a [`crate::com::ComWorker`] thread.

use crate::com::UiaCtx;
use std::path::{Path, PathBuf};
use veil_core::{FocusedText, UiElement};
use windows::core::BSTR;
use windows::Win32::Foundation::POINT;
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Accessibility::*;

/// Longest composer text read (characters).
const MAX_TEXT: i32 = 200_000;

pub fn control_type_name(ct: UIA_CONTROLTYPE_ID) -> &'static str {
    match ct {
        _ if ct == UIA_ButtonControlTypeId => "Button",
        _ if ct == UIA_SplitButtonControlTypeId => "SplitButton",
        _ if ct == UIA_EditControlTypeId => "Edit",
        _ if ct == UIA_DocumentControlTypeId => "Document",
        _ if ct == UIA_PaneControlTypeId => "Pane",
        _ if ct == UIA_GroupControlTypeId => "Group",
        _ if ct == UIA_CustomControlTypeId => "Custom",
        _ if ct == UIA_TextControlTypeId => "Text",
        _ if ct == UIA_ImageControlTypeId => "Image",
        _ if ct == UIA_HyperlinkControlTypeId => "Hyperlink",
        _ if ct == UIA_ListItemControlTypeId => "ListItem",
        _ if ct == UIA_ComboBoxControlTypeId => "ComboBox",
        _ if ct == UIA_MenuItemControlTypeId => "MenuItem",
        _ if ct == UIA_ToolBarControlTypeId => "ToolBar",
        _ if ct == UIA_WindowControlTypeId => "Window",
        _ => "Other",
    }
}

fn bstr(r: windows::core::Result<BSTR>) -> String {
    r.map(|b| b.to_string()).unwrap_or_default()
}

/// Facts about one element (no pattern calls: cheap).
fn describe(el: &IUIAutomationElement) -> Option<UiElement> {
    unsafe {
        let ct = el.CurrentControlType().ok()?;
        let mut name = bstr(el.CurrentName());
        let is_button = ct == UIA_ButtonControlTypeId || ct == UIA_SplitButtonControlTypeId;
        if name.trim().is_empty() && is_button {
            // Icon-only buttons sometimes carry their meaning in the help text (tooltip).
            name = bstr(el.CurrentHelpText());
        }
        let pid = el.CurrentProcessId().unwrap_or(0) as u32;
        let generic_type = [
            UIA_PaneControlTypeId,
            UIA_GroupControlTypeId,
            UIA_CustomControlTypeId,
            UIA_DocumentControlTypeId,
            UIA_TextControlTypeId,
            UIA_ImageControlTypeId,
        ]
        .contains(&ct);
        Some(UiElement {
            role: control_type_name(ct).to_string(),
            label: name.trim().to_lowercase(),
            pid,
            is_button,
            in_file_dialog: false,
            is_generic: generic_type && name.trim().is_empty(),
        })
    }
}

/// Element under a screen point. If the hit element is an icon or label inside a button, the button wins.
pub fn element_at(ctx: &UiaCtx, x: i32, y: i32, in_file_dialog: bool) -> Option<UiElement> {
    unsafe {
        let el = ctx.uia.ElementFromPoint(POINT { x, y }).ok()?;
        let mut info = describe(&el)?;
        if !info.is_button {
            if let Some(walker) = &ctx.walker {
                let mut cur = el.clone();
                for _ in 0..2 {
                    let Ok(parent) = walker.GetParentElement(&cur) else { break };
                    if let Some(p) = describe(&parent) {
                        if p.is_button {
                            info = p;
                            break;
                        }
                    }
                    cur = parent;
                }
            }
        }
        info.in_file_dialog = in_file_dialog;
        Some(info)
    }
}

/// Value or text of an element: `Some(text)` when the element exposes a value/text pattern.
fn text_of(el: &IUIAutomationElement) -> Option<String> {
    unsafe {
        if el.CurrentIsPassword().map(|b| b.as_bool()).unwrap_or(false) {
            return None;
        }
        if let Ok(vp) = el.GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId) {
            if let Ok(v) = vp.CurrentValue() {
                let v = v.to_string();
                if !v.is_empty() {
                    return Some(v);
                }
            }
        }
        if let Ok(tp) = el.GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId) {
            if let Ok(range) = tp.DocumentRange() {
                if let Ok(t) = range.GetText(MAX_TEXT) {
                    return Some(t.to_string());
                }
            }
        }
        None
    }
}

/// Text of the focused composer. Walks up a few parents for Edit/Document elements with text patterns.
pub fn focused_text(ctx: &UiaCtx, own_pid: u32) -> FocusedText {
    unsafe {
        let Ok(el) = ctx.uia.GetFocusedElement() else {
            return FocusedText::NotExposed { role: "none".into() };
        };
        if el.CurrentProcessId().map(|p| p as u32 == own_pid).unwrap_or(false) {
            return FocusedText::NotExposed { role: "veil".into() };
        }
        let role = el.CurrentControlType().map(control_type_name).unwrap_or("Other").to_string();
        if let Some(t) = text_of(&el) {
            return FocusedText::Exposed { text: normalize(&t), role };
        }
        if let Some(walker) = &ctx.walker {
            let mut cur = el;
            for _ in 0..4 {
                let Ok(parent) = walker.GetParentElement(&cur) else { break };
                let ct = parent.CurrentControlType().unwrap_or_default();
                if ct == UIA_EditControlTypeId || ct == UIA_DocumentControlTypeId {
                    if let Some(t) = text_of(&parent) {
                        if !t.trim().is_empty() {
                            return FocusedText::Exposed { text: normalize(&t), role: control_type_name(ct).into() };
                        }
                    }
                }
                cur = parent;
            }
        }
        FocusedText::NotExposed { role }
    }
}

fn normalize(t: &str) -> String {
    t.replace("\r\n", "\n").replace('\r', "\n")
}

/// Replace the focused element's value through `ValuePattern`, verified by reading it back.
pub fn set_focused_value(ctx: &UiaCtx, text: &str) -> Option<bool> {
    unsafe {
        let el = ctx.uia.GetFocusedElement().ok()?;
        let vp = el.GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId).ok()?;
        if vp.CurrentIsReadOnly().map(|b| b.as_bool()).unwrap_or(true) {
            return Some(false);
        }
        if vp.SetValue(&BSTR::from(text)).is_err() {
            return Some(false);
        }
        let back = vp.CurrentValue().map(|b| normalize(&b.to_string())).unwrap_or_default();
        Some(back.trim_end() == normalize(text).trim_end())
    }
}

fn first_by_property(
    ctx: &UiaCtx,
    root: &IUIAutomationElement,
    prop: UIA_PROPERTY_ID,
    value: VARIANT,
) -> Option<IUIAutomationElement> {
    unsafe {
        let cond = ctx.uia.CreatePropertyCondition(prop, &value).ok()?;
        root.FindFirst(TreeScope_Descendants, &cond).ok()
    }
}

/// Address-bar text of a browser window (Chromium: first Edit; Firefox: `urlbar-input`).
pub fn browser_url(ctx: &UiaCtx, hwnd: isize, firefox: bool) -> Option<String> {
    unsafe {
        let root = ctx.uia.ElementFromHandle(crate::util::hwnd(hwnd)).ok()?;
        let edit = if firefox {
            first_by_property(ctx, &root, UIA_AutomationIdPropertyId, VARIANT::from(BSTR::from("urlbar-input")))?
        } else {
            first_by_property(ctx, &root, UIA_ControlTypePropertyId, VARIANT::from(UIA_EditControlTypeId.0))?
        };
        let vp = edit.GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId).ok()?;
        let url = vp.CurrentValue().ok()?.to_string();
        let url = url.trim().to_string();
        (!url.is_empty() && !url.contains(' ')).then_some(url)
    }
}

/// Ask Chromium/Electron to build its accessibility tree by querying it once.
pub fn wake(ctx: &UiaCtx, hwnd: isize) {
    unsafe {
        if let Ok(root) = ctx.uia.ElementFromHandle(crate::util::hwnd(hwnd)) {
            if let Ok(cond) = ctx.uia.CreateTrueCondition() {
                let _ = root.FindFirst(TreeScope_Children, &cond);
            }
            let _ = root.CurrentName();
        }
    }
}

/// Files selected in a common file dialog: the "File name" box (id 1148) joined with the folder shown in
/// the address bar; falls back to the selected list items.
pub fn picker_selection(ctx: &UiaCtx, dialog: isize) -> Vec<PathBuf> {
    unsafe {
        let Ok(root) = ctx.uia.ElementFromHandle(crate::util::hwnd(dialog)) else { return Vec::new() };
        let folder = dialog_folder(ctx, &root);

        let mut names = Vec::new();
        if let Some(name_box) =
            first_by_property(ctx, &root, UIA_AutomationIdPropertyId, VARIANT::from(BSTR::from("1148")))
        {
            let text = text_of(&name_box).or_else(|| {
                let edit = first_by_property(
                    ctx,
                    &name_box,
                    UIA_ControlTypePropertyId,
                    VARIANT::from(UIA_EditControlTypeId.0),
                )?;
                text_of(&edit)
            });
            if let Some(t) = text {
                names = parse_names(&t);
            }
        }
        if names.is_empty() {
            names = selected_list_items(ctx, &root);
        }
        resolve(&names, folder.as_deref())
    }
}

fn selected_list_items(ctx: &UiaCtx, root: &IUIAutomationElement) -> Vec<String> {
    unsafe {
        let Ok(c1) =
            ctx.uia.CreatePropertyCondition(UIA_ControlTypePropertyId, &VARIANT::from(UIA_ListItemControlTypeId.0))
        else {
            return Vec::new();
        };
        let Ok(c2) = ctx.uia.CreatePropertyCondition(UIA_SelectionItemIsSelectedPropertyId, &VARIANT::from(true))
        else {
            return Vec::new();
        };
        let Ok(cond) = ctx.uia.CreateAndCondition(&c1, &c2) else { return Vec::new() };
        let Ok(found) = root.FindAll(TreeScope_Descendants, &cond) else { return Vec::new() };
        let n = found.Length().unwrap_or(0).min(200);
        (0..n)
            .filter_map(|i| found.GetElement(i).ok())
            .map(|e| bstr(e.CurrentName()))
            .filter(|s| !s.is_empty())
            .collect()
    }
}

/// Folder shown by the dialog's address band ("Address: C:\Users\me\Documents").
fn dialog_folder(ctx: &UiaCtx, root: &IUIAutomationElement) -> Option<PathBuf> {
    unsafe {
        let cond = ctx
            .uia
            .CreatePropertyCondition(UIA_ControlTypePropertyId, &VARIANT::from(UIA_ToolBarControlTypeId.0))
            .ok()?;
        let bars = root.FindAll(TreeScope_Descendants, &cond).ok()?;
        let n = bars.Length().unwrap_or(0).min(50);
        for i in 0..n {
            let Ok(bar) = bars.GetElement(i) else { continue };
            let name = bstr(bar.CurrentName());
            // The label before ": " is localized ("Address", "Adresse", "Dirección"…).
            let Some((_, rest)) = name.split_once(": ") else { continue };
            if let Some(p) = folder_from_label(rest.trim()) {
                return Some(p);
            }
        }
        None
    }
}

fn folder_from_label(label: &str) -> Option<PathBuf> {
    let p = Path::new(label);
    if p.is_absolute() && p.is_dir() {
        return Some(p.to_path_buf());
    }
    // Known folders shown by display name.
    let profile = std::env::var_os("USERPROFILE").map(PathBuf::from)?;
    let last = label.rsplit(['\\', '>', '›']).next().unwrap_or(label).trim();
    let known = ["Desktop", "Documents", "Downloads", "Pictures", "Music", "Videos"];
    for k in known {
        if last.eq_ignore_ascii_case(k) {
            for base in [profile.join(k), profile.join("OneDrive").join(k)] {
                if base.is_dir() {
                    return Some(base);
                }
            }
        }
    }
    None
}

/// `"a.txt" "b.txt"` → [a.txt, b.txt]; a plain name → [name].
pub fn parse_names(text: &str) -> Vec<String> {
    let t = text.trim();
    if t.is_empty() {
        return Vec::new();
    }
    if t.contains('"') {
        return t
            .split('"')
            .enumerate()
            .filter(|(i, s)| i % 2 == 1 && !s.trim().is_empty())
            .map(|(_, s)| s.to_string())
            .collect();
    }
    vec![t.to_string()]
}

fn resolve(names: &[String], folder: Option<&Path>) -> Vec<PathBuf> {
    names
        .iter()
        .filter_map(|n| {
            let p = Path::new(n);
            let full = if p.is_absolute() { p.to_path_buf() } else { folder?.join(p) };
            full.is_file().then_some(full)
        })
        .collect()
}

//! Accessibility (AX) helpers: an owned `AXUIElement` wrapper and the attribute reads the provider needs.

#![allow(dead_code)]

use crate::ffi::{cf_array_items, cf_bool, cf_string, cf_url, cfstr, type_is, OwnedCf};
use accessibility_sys::{
    kAXErrorSuccess, kAXTrustedCheckOptionPrompt, AXIsProcessTrusted, AXIsProcessTrustedWithOptions,
    AXUIElementCopyAttributeValue, AXUIElementCopyElementAtPosition, AXUIElementCreateApplication,
    AXUIElementCreateSystemWide, AXUIElementGetPid, AXUIElementGetTypeID, AXUIElementIsAttributeSettable,
    AXUIElementRef, AXUIElementSetAttributeValue, AXUIElementSetMessagingTimeout,
};
use core_foundation::base::TCFType;
use core_foundation_sys::base::{CFRelease, CFRetain, CFTypeRef};
use core_foundation_sys::dictionary::{
    kCFTypeDictionaryKeyCallBacks, kCFTypeDictionaryValueCallBacks, CFDictionaryCreate,
};
use core_foundation_sys::number::{kCFBooleanFalse, kCFBooleanTrue};
use std::ffi::c_void;

/// Roles of native and web text inputs.
pub const TEXT_ROLES: &[&str] = &["AXTextArea", "AXTextField", "AXComboBox", "AXSearchField"];

/// An owned (+1) accessibility element.
pub struct Element(AXUIElementRef);

// AXUIElement is a CF type; the AX API may be called from any thread.
unsafe impl Send for Element {}
unsafe impl Sync for Element {}

impl Drop for Element {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CFRelease(self.0 as CFTypeRef) }
        }
    }
}

impl Clone for Element {
    fn clone(&self) -> Self {
        unsafe { CFRetain(self.0 as CFTypeRef) };
        Element(self.0)
    }
}

impl Element {
    /// The application element of a process, with a messaging timeout so a hung app never stalls Veil.
    pub fn application(pid: u32, timeout_secs: f32) -> Element {
        let e = Element(unsafe { AXUIElementCreateApplication(pid as i32) });
        e.set_timeout(timeout_secs);
        e
    }

    pub fn system_wide(timeout_secs: f32) -> Element {
        let e = Element(unsafe { AXUIElementCreateSystemWide() });
        e.set_timeout(timeout_secs);
        e
    }

    /// Wrap a reference we were given under the "get" rule (retains it).
    fn from_get(r: CFTypeRef) -> Option<Element> {
        if !type_is(r, unsafe { AXUIElementGetTypeID() }) {
            return None;
        }
        unsafe { CFRetain(r) };
        Some(Element(r as AXUIElementRef))
    }

    pub fn set_timeout(&self, secs: f32) {
        unsafe { AXUIElementSetMessagingTimeout(self.0, secs) };
    }

    /// Copy an attribute value (owned).
    pub fn attr(&self, name: &str) -> Option<OwnedCf> {
        let key = cfstr(name);
        let mut value: CFTypeRef = std::ptr::null();
        let err = unsafe { AXUIElementCopyAttributeValue(self.0, key.as_concrete_TypeRef(), &mut value) };
        if err != kAXErrorSuccess {
            return None;
        }
        OwnedCf::from_create(value)
    }

    pub fn string(&self, name: &str) -> Option<String> {
        let v = self.attr(name)?;
        cf_string(v.as_ptr())
    }

    pub fn boolean(&self, name: &str) -> Option<bool> {
        let v = self.attr(name)?;
        cf_bool(v.as_ptr())
    }

    /// A URL attribute (`AXURL`, `AXDocument`) as a path for file URLs, else as a string.
    pub fn url(&self, name: &str) -> Option<String> {
        let v = self.attr(name)?;
        cf_url(v.as_ptr()).or_else(|| cf_string(v.as_ptr()))
    }

    pub fn element(&self, name: &str) -> Option<Element> {
        let v = self.attr(name)?;
        Element::from_get(v.as_ptr())
    }

    pub fn elements(&self, name: &str) -> Vec<Element> {
        let Some(v) = self.attr(name) else { return Vec::new() };
        cf_array_items(v.as_ptr()).into_iter().filter_map(Element::from_get).collect()
    }

    pub fn children(&self) -> Vec<Element> {
        self.elements("AXChildren")
    }

    pub fn parent(&self) -> Option<Element> {
        self.element("AXParent")
    }

    pub fn role(&self) -> String {
        self.string("AXRole").unwrap_or_default()
    }

    pub fn subrole(&self) -> String {
        self.string("AXSubrole").unwrap_or_default()
    }

    /// Lower-cased title, description, help and identifier (what a button "says").
    pub fn label(&self) -> String {
        ["AXTitle", "AXDescription", "AXHelp", "AXIdentifier"]
            .iter()
            .filter_map(|a| self.string(a))
            .filter(|s| !s.trim().is_empty())
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase()
    }

    /// Title only (lower-cased), as file-picker buttons are matched by their title.
    pub fn title(&self) -> String {
        self.string("AXTitle").unwrap_or_default().to_lowercase()
    }

    pub fn pid(&self) -> Option<u32> {
        let mut pid: i32 = 0;
        let err = unsafe { AXUIElementGetPid(self.0, &mut pid) };
        (err == kAXErrorSuccess && pid > 0).then_some(pid as u32)
    }

    pub fn is_settable(&self, name: &str) -> bool {
        let key = cfstr(name);
        let mut settable: u8 = 0;
        let err = unsafe { AXUIElementIsAttributeSettable(self.0, key.as_concrete_TypeRef(), &mut settable) };
        err == kAXErrorSuccess && settable != 0
    }

    pub fn set_bool(&self, name: &str, value: bool) -> bool {
        let key = cfstr(name);
        let v = unsafe {
            if value {
                kCFBooleanTrue
            } else {
                kCFBooleanFalse
            }
        };
        let err = unsafe { AXUIElementSetAttributeValue(self.0, key.as_concrete_TypeRef(), v as CFTypeRef) };
        err == kAXErrorSuccess
    }

    pub fn set_string(&self, name: &str, value: &str) -> bool {
        let key = cfstr(name);
        let v = cfstr(value);
        let err = unsafe {
            AXUIElementSetAttributeValue(self.0, key.as_concrete_TypeRef(), v.as_concrete_TypeRef() as CFTypeRef)
        };
        err == kAXErrorSuccess
    }

    /// Element at a screen point (Quartz global coordinates). Only valid on the system-wide or an
    /// application element.
    pub fn element_at(&self, x: f64, y: f64) -> Option<Element> {
        let mut out: AXUIElementRef = std::ptr::null_mut();
        let err = unsafe { AXUIElementCopyElementAtPosition(self.0, x as f32, y as f32, &mut out) };
        if err != kAXErrorSuccess || out.is_null() {
            return None;
        }
        Some(Element(out))
    }

    /// Depth-first search with a node budget. Returns the first element for which `f` is true.
    pub fn find(&self, max_nodes: usize, max_depth: usize, f: &mut dyn FnMut(&Element) -> bool) -> Option<Element> {
        let mut budget = max_nodes;
        fn walk(
            e: &Element,
            depth: usize,
            max_depth: usize,
            budget: &mut usize,
            f: &mut dyn FnMut(&Element) -> bool,
        ) -> Option<Element> {
            if *budget == 0 || depth > max_depth {
                return None;
            }
            *budget -= 1;
            if f(e) {
                return Some(e.clone());
            }
            for c in e.children() {
                if let Some(hit) = walk(&c, depth + 1, max_depth, budget, f) {
                    return Some(hit);
                }
            }
            None
        }
        walk(self, 0, max_depth, &mut budget, f)
    }

    /// Concatenate `AXValue` strings of the subtree (contenteditable composers that expose their text
    /// in child nodes), with a node budget.
    pub fn collect_values(&self, max_nodes: usize, max_depth: usize) -> String {
        let mut out = String::new();
        let mut budget = max_nodes;
        fn walk(e: &Element, depth: usize, max_depth: usize, budget: &mut usize, out: &mut String) {
            if *budget == 0 || depth > max_depth {
                return;
            }
            *budget -= 1;
            if let Some(v) = e.string("AXValue") {
                if !v.is_empty() {
                    out.push_str(&v);
                    out.push(' ');
                }
            }
            for c in e.children() {
                walk(&c, depth + 1, max_depth, budget, out);
            }
        }
        walk(self, 0, max_depth, &mut budget, &mut out);
        out
    }
}

/// Is this process trusted for Accessibility?
pub fn is_trusted() -> bool {
    unsafe { AXIsProcessTrusted() }
}

/// Ask macOS to show its Accessibility prompt (adds Veil to the list in System Settings).
pub fn prompt_for_trust() -> bool {
    unsafe {
        let keys = [kAXTrustedCheckOptionPrompt as *const c_void];
        let values = [kCFBooleanTrue as *const c_void];
        let dict = CFDictionaryCreate(
            std::ptr::null(),
            keys.as_ptr(),
            values.as_ptr(),
            1,
            &kCFTypeDictionaryKeyCallBacks,
            &kCFTypeDictionaryValueCallBacks,
        );
        let trusted = AXIsProcessTrustedWithOptions(dict);
        if !dict.is_null() {
            CFRelease(dict as CFTypeRef);
        }
        trusted
    }
}

/// Text of the focused composer of `pid`: `(text, role, exposed)`.
/// `exposed` is false when the app has no editable element in focus (e.g. Chrome without its web tree).
pub fn composer_text(pid: u32) -> (String, String, bool) {
    let app = Element::application(pid, 0.3);
    let Some(mut f) = app.element("AXFocusedUIElement") else { return (String::new(), "none".into(), false) };
    if !TEXT_ROLES.contains(&f.role().as_str()) {
        // Caret inside a contenteditable: the editable root holds the whole message.
        if let Some(root) = f.element("AXEditableAncestor") {
            f = root;
        }
    }
    let role = f.role();
    if !TEXT_ROLES.contains(&role.as_str()) && f.element("AXEditableAncestor").is_none() {
        return (String::new(), role, false);
    }
    if let Some(v) = f.string("AXValue") {
        if !v.is_empty() {
            return (v, role, true);
        }
    }
    (f.collect_values(300, 8), role, true)
}

/// The focused editable element of `pid` (the composer), if any.
pub fn focused_editable(pid: u32) -> Option<Element> {
    let app = Element::application(pid, 0.3);
    let mut f = app.element("AXFocusedUIElement")?;
    if !TEXT_ROLES.contains(&f.role().as_str()) {
        if let Some(root) = f.element("AXEditableAncestor") {
            f = root;
        }
    }
    let role = f.role();
    (TEXT_ROLES.contains(&role.as_str()) || f.element("AXEditableAncestor").is_some()).then_some(f)
}

/// Title of the focused window of `pid`.
pub fn window_title(pid: u32) -> Option<String> {
    let app = Element::application(pid, 0.25);
    app.element("AXFocusedWindow").and_then(|w| w.string("AXTitle")).filter(|t| !t.is_empty())
}

//! Raw CoreGraphics declarations (event taps, synthetic events, window list) and small CoreFoundation
//! helpers. Kept in one place so every unsafe signature can be checked against the SDK headers.

#![allow(non_upper_case_globals, non_camel_case_types, dead_code)]

use core_foundation::base::TCFType;
use core_foundation::string::CFString;
use core_foundation_sys::array::{CFArrayGetCount, CFArrayGetTypeID, CFArrayGetValueAtIndex, CFArrayRef};
use core_foundation_sys::base::{CFGetTypeID, CFRelease, CFRetain, CFTypeRef};
use core_foundation_sys::dictionary::{CFDictionaryGetTypeID, CFDictionaryGetValue, CFDictionaryRef};
use core_foundation_sys::mach_port::CFMachPortRef;
use core_foundation_sys::number::{
    kCFNumberFloat64Type, kCFNumberSInt64Type, CFBooleanGetTypeID, CFBooleanGetValue, CFNumberGetTypeID,
    CFNumberGetValue,
};
use core_foundation_sys::string::{CFStringGetTypeID, CFStringRef};
use core_foundation_sys::url::{
    kCFURLPOSIXPathStyle, CFURLCopyFileSystemPath, CFURLGetString, CFURLGetTypeID, CFURLRef,
};
use std::ffi::{c_ulong, c_void};

// ---------- CoreGraphics types ----------

pub type CGEventRef = *mut c_void;
pub type CGEventSourceRef = *mut c_void;
pub type CGEventTapProxy = *mut c_void;
pub type CGEventMask = u64;
pub type CGEventType = u32;
pub type CGEventFlags = u64;
pub type CGEventField = u32;
pub type CGKeyCode = u16;
pub type CGWindowID = u32;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CGPoint {
    pub x: f64,
    pub y: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CGSize {
    pub width: f64,
    pub height: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CGRect {
    pub origin: CGPoint,
    pub size: CGSize,
}

impl CGRect {
    pub fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.origin.x
            && y >= self.origin.y
            && x < self.origin.x + self.size.width
            && y < self.origin.y + self.size.height
    }
}

// Event types (CGEventTypes.h).
pub const kCGEventLeftMouseDown: CGEventType = 1;
pub const kCGEventLeftMouseUp: CGEventType = 2;
pub const kCGEventLeftMouseDragged: CGEventType = 6;
pub const kCGEventKeyDown: CGEventType = 10;
pub const kCGEventKeyUp: CGEventType = 11;
pub const kCGEventTapDisabledByTimeout: CGEventType = 0xFFFF_FFFE;
pub const kCGEventTapDisabledByUserInput: CGEventType = 0xFFFF_FFFF;

// Tap locations / placement / options.
pub const kCGHIDEventTap: u32 = 0;
pub const kCGSessionEventTap: u32 = 1;
pub const kCGHeadInsertEventTap: u32 = 0;
pub const kCGEventTapOptionDefault: u32 = 0;

// Event source state ids.
pub const kCGEventSourceStateHIDSystemState: i32 = 1;

// Event fields.
pub const kCGMouseEventClickState: CGEventField = 1;
pub const kCGKeyboardEventAutorepeat: CGEventField = 8;
pub const kCGKeyboardEventKeycode: CGEventField = 9;
pub const kCGEventSourceUserData: CGEventField = 42;

// Flags.
pub const kCGEventFlagMaskShift: CGEventFlags = 0x0002_0000;
pub const kCGEventFlagMaskControl: CGEventFlags = 0x0004_0000;
pub const kCGEventFlagMaskAlternate: CGEventFlags = 0x0008_0000;
pub const kCGEventFlagMaskCommand: CGEventFlags = 0x0010_0000;

pub const kCGMouseButtonLeft: u32 = 0;

// Window list.
pub const kCGWindowListOptionOnScreenOnly: u32 = 1 << 0;
pub const kCGWindowListExcludeDesktopElements: u32 = 1 << 4;
pub const kCGNullWindowID: CGWindowID = 0;

pub type CGEventTapCallBack =
    extern "C" fn(proxy: CGEventTapProxy, etype: CGEventType, event: CGEventRef, user_info: *mut c_void) -> CGEventRef;

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    pub fn CGEventTapCreate(
        tap: u32,
        place: u32,
        options: u32,
        events_of_interest: CGEventMask,
        callback: CGEventTapCallBack,
        user_info: *mut c_void,
    ) -> CFMachPortRef;
    pub fn CGEventTapEnable(tap: CFMachPortRef, enable: bool);

    pub fn CGEventGetIntegerValueField(event: CGEventRef, field: CGEventField) -> i64;
    pub fn CGEventSetIntegerValueField(event: CGEventRef, field: CGEventField, value: i64);
    pub fn CGEventGetFlags(event: CGEventRef) -> CGEventFlags;
    pub fn CGEventSetFlags(event: CGEventRef, flags: CGEventFlags);
    pub fn CGEventGetLocation(event: CGEventRef) -> CGPoint;
    pub fn CGEventKeyboardGetUnicodeString(
        event: CGEventRef,
        max_len: c_ulong,
        actual_len: *mut c_ulong,
        buffer: *mut u16,
    );
    pub fn CGEventKeyboardSetUnicodeString(event: CGEventRef, len: c_ulong, buffer: *const u16);

    pub fn CGEventCreateKeyboardEvent(source: CGEventSourceRef, key: CGKeyCode, key_down: bool) -> CGEventRef;
    pub fn CGEventCreateMouseEvent(
        source: CGEventSourceRef,
        mouse_type: CGEventType,
        position: CGPoint,
        button: u32,
    ) -> CGEventRef;
    pub fn CGEventPost(tap: u32, event: CGEventRef);

    pub fn CGEventSourceCreate(state_id: i32) -> CGEventSourceRef;
    pub fn CGEventSourceSetUserData(source: CGEventSourceRef, user_data: i64);

    pub fn CGWindowListCopyWindowInfo(option: u32, relative_to: CGWindowID) -> CFArrayRef;
    pub fn CGRectMakeWithDictionaryRepresentation(dict: CFDictionaryRef, rect: *mut CGRect) -> bool;
}

// ---------- CoreFoundation helpers ----------

/// An owned (+1) CoreFoundation reference, released on drop.
pub struct OwnedCf(pub CFTypeRef);

impl OwnedCf {
    /// Take ownership of a reference obtained under the create/copy rule. `None` if null.
    pub fn from_create(r: CFTypeRef) -> Option<OwnedCf> {
        (!r.is_null()).then_some(OwnedCf(r))
    }

    pub fn as_ptr(&self) -> CFTypeRef {
        self.0
    }
}

impl Drop for OwnedCf {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CFRelease(self.0) }
        }
    }
}

impl Clone for OwnedCf {
    fn clone(&self) -> Self {
        unsafe { CFRetain(self.0) };
        OwnedCf(self.0)
    }
}

// CF objects are immutable here and retain/release are thread-safe.
unsafe impl Send for OwnedCf {}
unsafe impl Sync for OwnedCf {}

pub fn type_is(r: CFTypeRef, type_id: core_foundation_sys::base::CFTypeID) -> bool {
    !r.is_null() && unsafe { CFGetTypeID(r) } == type_id
}

/// Read a CFString (not owned) into a Rust string.
pub fn cf_string(r: CFTypeRef) -> Option<String> {
    if !type_is(r, unsafe { CFStringGetTypeID() }) {
        return None;
    }
    let s = unsafe { CFString::wrap_under_get_rule(r as CFStringRef) };
    Some(s.to_string())
}

pub fn cf_i64(r: CFTypeRef) -> Option<i64> {
    if type_is(r, unsafe { CFBooleanGetTypeID() }) {
        return Some(unsafe { CFBooleanGetValue(r as _) } as i64);
    }
    if !type_is(r, unsafe { CFNumberGetTypeID() }) {
        return None;
    }
    let mut out: i64 = 0;
    let ok = unsafe { CFNumberGetValue(r as _, kCFNumberSInt64Type, &mut out as *mut i64 as *mut c_void) };
    ok.then_some(out)
}

pub fn cf_f64(r: CFTypeRef) -> Option<f64> {
    if !type_is(r, unsafe { CFNumberGetTypeID() }) {
        return None;
    }
    let mut out: f64 = 0.0;
    let ok = unsafe { CFNumberGetValue(r as _, kCFNumberFloat64Type, &mut out as *mut f64 as *mut c_void) };
    ok.then_some(out)
}

pub fn cf_bool(r: CFTypeRef) -> Option<bool> {
    if type_is(r, unsafe { CFBooleanGetTypeID() }) {
        return Some(unsafe { CFBooleanGetValue(r as _) });
    }
    cf_i64(r).map(|v| v != 0)
}

/// A file-system path from a CFURL, or its string form for non-file URLs.
pub fn cf_url(r: CFTypeRef) -> Option<String> {
    if !type_is(r, unsafe { CFURLGetTypeID() }) {
        return None;
    }
    let url = r as CFURLRef;
    let s = unsafe { CFURLGetString(url) };
    let text = cf_string(s as CFTypeRef)?;
    if text.starts_with("file:") {
        let path = OwnedCf::from_create(unsafe { CFURLCopyFileSystemPath(url, kCFURLPOSIXPathStyle) } as CFTypeRef)?;
        return cf_string(path.as_ptr());
    }
    Some(text)
}

/// Iterate a CFArray (not owned).
pub fn cf_array_items(r: CFTypeRef) -> Vec<CFTypeRef> {
    if !type_is(r, unsafe { CFArrayGetTypeID() }) {
        return Vec::new();
    }
    let arr = r as CFArrayRef;
    let n = unsafe { CFArrayGetCount(arr) };
    (0..n).map(|i| unsafe { CFArrayGetValueAtIndex(arr, i) } as CFTypeRef).collect()
}

/// Value for a string key in a CFDictionary (not owned, may be null).
pub fn cf_dict_get(dict: CFTypeRef, key: &str) -> CFTypeRef {
    if !type_is(dict, unsafe { CFDictionaryGetTypeID() }) {
        return std::ptr::null();
    }
    let k = CFString::new(key);
    unsafe { CFDictionaryGetValue(dict as CFDictionaryRef, k.as_concrete_TypeRef() as *const c_void) as CFTypeRef }
}

/// A CFString owned by the caller (create rule), for passing to C APIs.
pub fn cfstr(s: &str) -> CFString {
    CFString::new(s)
}

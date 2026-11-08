//! Prompts (system alerts), the non-activating warning banner, and giving focus back to an app.

use crate::ffi::cfstr;
use core_foundation::base::TCFType;
use core_foundation_sys::base::CFOptionFlags;
use core_foundation_sys::user_notification::{
    kCFUserNotificationAlternateResponse, kCFUserNotificationCautionAlertLevel, kCFUserNotificationDefaultResponse,
    kCFUserNotificationOtherResponse, CFUserNotificationDisplayAlert,
};
use dispatch2::DispatchQueue;
use objc2::rc::{autoreleasepool, Retained};
use objc2::MainThreadMarker;
use objc2_app_kit::{
    NSApplicationActivationOptions, NSBackingStoreType, NSBox, NSBoxType, NSColor, NSFont, NSPanel,
    NSRunningApplication, NSScreen, NSStatusWindowLevel, NSTextAlignment, NSTextField, NSTitlePosition,
    NSWindowCollectionBehavior, NSWindowStyleMask, NSWorkspace,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
use std::cell::RefCell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use veil_core::platform::{Choice, Prompt};

/// Show a system alert (drawn by UserNotificationCenter above every app, works from any thread) and
/// block until the user answers. Up to three buttons: options[0] is the default (Return) button.
pub fn ask(prompt: &Prompt) -> Choice {
    let opts = &prompt.options;
    if opts.is_empty() {
        return prompt.cancel_choice();
    }
    let header = cfstr(&prompt.title);
    let message = cfstr(&prompt.message);
    let labels: Vec<_> = opts.iter().take(3).map(|c| cfstr(prompt.label(*c))).collect();
    let alt = labels.get(1).map(|s| s.as_concrete_TypeRef()).unwrap_or(std::ptr::null());
    let other = labels.get(2).map(|s| s.as_concrete_TypeRef()).unwrap_or(std::ptr::null());
    let mut response: CFOptionFlags = 0;
    let err = unsafe {
        CFUserNotificationDisplayAlert(
            0.0,
            kCFUserNotificationCautionAlertLevel,
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            header.as_concrete_TypeRef(),
            message.as_concrete_TypeRef(),
            labels[0].as_concrete_TypeRef(),
            alt,
            other,
            &mut response,
        )
    };
    if err != 0 {
        tracing::warn!("could not show the alert ({err})");
        return prompt.cancel_choice();
    }
    match response & 0x3 {
        r if r == kCFUserNotificationDefaultResponse => opts[0],
        r if r == kCFUserNotificationAlternateResponse && opts.len() > 1 => opts[1],
        r if r == kCFUserNotificationOtherResponse && opts.len() > 2 => opts[2],
        _ => prompt.cancel_choice(),
    }
}

/// Activate an app again after a prompt took focus, and wait (briefly) until it is frontmost.
pub fn refocus(pid: u32) {
    autoreleasepool(|_| {
        if let Some(app) = NSRunningApplication::runningApplicationWithProcessIdentifier(pid as i32) {
            #[allow(deprecated)]
            app.activateWithOptions(NSApplicationActivationOptions::ActivateIgnoringOtherApps);
        }
    });
    let deadline = Instant::now() + Duration::from_millis(400);
    while Instant::now() < deadline {
        let front = autoreleasepool(|_| {
            NSWorkspace::sharedWorkspace().frontmostApplication().map(|a| a.processIdentifier() as u32)
        });
        if front == Some(pid) {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    // Let the app's window become key before input is replayed (the proof of concept waited 0.2 s).
    std::thread::sleep(Duration::from_millis(150));
}

static BANNER_GENERATION: AtomicU64 = AtomicU64::new(0);

thread_local! {
    /// Main-thread only.
    static BANNER: RefCell<Option<(Retained<NSPanel>, Retained<NSTextField>)>> = const { RefCell::new(None) };
}

const BANNER_WIDTH: f64 = 640.0;
const BANNER_HEIGHT: f64 = 44.0;

/// Small amber banner at the top of the screen for ~4 s. Non-activating: never takes focus.
pub fn banner(text: &str) {
    let text = text.to_string();
    let generation = BANNER_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    DispatchQueue::main().exec_async(move || {
        let Some(mtm) = MainThreadMarker::new() else { return };
        autoreleasepool(|_| show_banner(mtm, &text));
    });
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(4));
        if BANNER_GENERATION.load(Ordering::SeqCst) == generation {
            DispatchQueue::main().exec_async(|| {
                BANNER.with(|b| {
                    if let Some((panel, _)) = b.borrow().as_ref() {
                        panel.orderOut(None);
                    }
                });
            });
        }
    });
}

fn show_banner(mtm: MainThreadMarker, text: &str) {
    BANNER.with(|cell| {
        let mut cell = cell.borrow_mut();
        if cell.is_none() {
            *cell = Some(make_banner(mtm));
        }
        let Some((panel, label)) = cell.as_ref() else { return };
        label.setStringValue(&NSString::from_str(text));
        let screen = NSScreen::mainScreen(mtm).or_else(|| NSScreen::screens(mtm).iter().next());
        if let Some(screen) = screen {
            let f = screen.visibleFrame();
            let x = f.origin.x + (f.size.width - BANNER_WIDTH) / 2.0;
            let y = f.origin.y + f.size.height - BANNER_HEIGHT - 16.0;
            panel.setFrameOrigin(NSPoint::new(x, y));
        }
        panel.orderFrontRegardless();
    });
}

fn make_banner(mtm: MainThreadMarker) -> (Retained<NSPanel>, Retained<NSTextField>) {
    let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(BANNER_WIDTH, BANNER_HEIGHT));
    let style = NSWindowStyleMask::NonactivatingPanel | NSWindowStyleMask::Borderless;
    #[allow(unused_unsafe)]
    let panel = unsafe {
        NSPanel::initWithContentRect_styleMask_backing_defer(
            mtm.alloc::<NSPanel>(),
            frame,
            style,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    #[allow(unused_unsafe)]
    unsafe {
        panel.setReleasedWhenClosed(false);
    }
    panel.setLevel(NSStatusWindowLevel);
    panel.setOpaque(false);
    panel.setBackgroundColor(Some(&NSColor::clearColor()));
    panel.setHasShadow(true);
    panel.setIgnoresMouseEvents(true);
    panel.setCollectionBehavior(
        NSWindowCollectionBehavior::CanJoinAllSpaces | NSWindowCollectionBehavior::FullScreenAuxiliary,
    );

    // A filled, rounded box as the background (no layer code needed).
    let bg = NSBox::new(mtm);
    bg.setFrame(frame);
    bg.setBoxType(NSBoxType::Custom);
    bg.setTitlePosition(NSTitlePosition::NoTitle);
    bg.setBorderWidth(0.0);
    bg.setCornerRadius(10.0);
    bg.setFillColor(&NSColor::colorWithCalibratedRed_green_blue_alpha(0.80, 0.42, 0.04, 0.96));

    let label = NSTextField::labelWithString(&NSString::from_str(""), mtm);
    label.setTextColor(Some(&NSColor::whiteColor()));
    label.setAlignment(NSTextAlignment::Center);
    label.setFont(Some(&NSFont::boldSystemFontOfSize(13.0)));
    label.setFrame(NSRect::new(NSPoint::new(12.0, 12.0), NSSize::new(BANNER_WIDTH - 24.0, 20.0)));

    if let Some(content) = panel.contentView() {
        content.addSubview(&bg);
        content.addSubview(&label);
    }
    (panel, label)
}

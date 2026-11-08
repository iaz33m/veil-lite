//! Native UI: the typing-warning banner (layered, never activates), prompts (TaskDialog with a
//! MessageBox fallback) and focus handling.

use crate::util::{hwnd, hwnd_val, wide};
use parking_lot::Mutex;
use std::cell::{Cell, RefCell};
use std::sync::atomic::{AtomicIsize, Ordering};
use veil_core::{Choice, Prompt, PromptKind};
use windows::core::{s, w, HRESULT, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress, LoadLibraryW};
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows::Win32::UI::Controls::{
    TASKDIALOGCONFIG, TASKDIALOGCONFIG_0, TASKDIALOG_BUTTON, TASKDIALOG_NOTIFICATIONS, TDF_ALLOW_DIALOG_CANCELLATION,
    TDN_CREATED, TD_WARNING_ICON,
};
use windows::Win32::UI::WindowsAndMessaging::*;

const BANNER_W: i32 = 460;
const BANNER_H: i32 = 76;
const BANNER_MS: u32 = 4000;
const WM_SHOW_BANNER: u32 = WM_APP + 1;
const TIMER_HIDE: usize = 1;

static BANNER: AtomicIsize = AtomicIsize::new(0);
static BANNER_TEXT: Mutex<String> = Mutex::new(String::new());
static FONT: AtomicIsize = AtomicIsize::new(0);

/// Thread that owns the banner window (also used as the clipboard owner) and pumps its messages.
pub struct UiThread {
    window: isize,
}

impl UiThread {
    pub fn spawn() -> anyhow::Result<UiThread> {
        let (tx, rx) = crossbeam_channel::bounded::<Result<isize, String>>(1);
        std::thread::Builder::new().name("veil-ui".into()).spawn(move || unsafe {
            match create_banner_window() {
                Ok(h) => {
                    let _ = tx.send(Ok(hwnd_val(h)));
                }
                Err(e) => {
                    let _ = tx.send(Err(e.to_string()));
                    return;
                }
            }
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        })?;
        let window = rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .map_err(|_| anyhow::anyhow!("UI thread did not start"))?
            .map_err(|e| anyhow::anyhow!(e))?;
        BANNER.store(window, Ordering::Release);
        Ok(UiThread { window })
    }

    /// A window of this process, for `OpenClipboard`.
    pub fn owner(&self) -> HWND {
        hwnd(self.window)
    }

    /// Show the banner near the pointer for a few seconds without taking focus.
    pub fn banner(&self, text: &str) {
        *BANNER_TEXT.lock() = text.to_string();
        unsafe {
            let _ = PostMessageW(Some(hwnd(self.window)), WM_SHOW_BANNER, WPARAM(0), LPARAM(0));
        }
    }
}

unsafe fn create_banner_window() -> windows::core::Result<HWND> {
    unsafe {
        let instance = GetModuleHandleW(None)?;
        let class = w!("VeilBanner");
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW,
            lpfnWndProc: Some(banner_proc),
            hInstance: instance.into(),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            lpszClassName: class,
            ..Default::default()
        };
        RegisterClassExW(&wc);
        let h = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_LAYERED | WS_EX_TRANSPARENT,
            class,
            w!("Veil"),
            WS_POPUP,
            0,
            0,
            BANNER_W,
            BANNER_H,
            None,
            None,
            Some(instance.into()),
            None,
        )?;
        let _ = SetLayeredWindowAttributes(h, COLORREF(0), 238, LWA_ALPHA);
        let font = CreateFontW(
            -15,
            0,
            0,
            0,
            FW_SEMIBOLD.0 as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            CLEARTYPE_QUALITY,
            DEFAULT_PITCH.0 as u32,
            w!("Segoe UI"),
        );
        FONT.store(font.0 as isize, Ordering::Release);
        Ok(h)
    }
}

fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    COLORREF(r as u32 | (g as u32) << 8 | (b as u32) << 16)
}

unsafe extern "system" fn banner_proc(h: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_SHOW_BANNER => {
                position_near_cursor(h);
                let _ = InvalidateRect(Some(h), None, true);
                let _ = ShowWindow(h, SW_SHOWNOACTIVATE);
                let _ = SetWindowPos(h, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
                SetTimer(Some(h), TIMER_HIDE, BANNER_MS, None);
                LRESULT(0)
            }
            WM_TIMER if wparam.0 == TIMER_HIDE => {
                let _ = KillTimer(Some(h), TIMER_HIDE);
                let _ = ShowWindow(h, SW_HIDE);
                LRESULT(0)
            }
            WM_NCHITTEST => LRESULT(HTTRANSPARENT as isize),
            WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                let dc = BeginPaint(h, &mut ps);
                let mut rc = RECT { left: 0, top: 0, right: BANNER_W, bottom: BANNER_H };
                let bg = CreateSolidBrush(rgb(30, 30, 34));
                FillRect(dc, &rc, bg);
                let _ = DeleteObject(bg.into());
                let accent = CreateSolidBrush(rgb(214, 69, 65));
                let bar = RECT { left: 0, top: 0, right: 6, bottom: BANNER_H };
                FillRect(dc, &bar, accent);
                let _ = DeleteObject(accent.into());
                let font = FONT.load(Ordering::Acquire);
                let old = if font != 0 { Some(SelectObject(dc, HGDIOBJ(font as *mut _))) } else { None };
                SetBkMode(dc, TRANSPARENT);
                SetTextColor(dc, rgb(245, 245, 247));
                rc.left = 20;
                rc.right -= 14;
                rc.top = 12;
                rc.bottom -= 10;
                let mut text: Vec<u16> = BANNER_TEXT.lock().encode_utf16().collect();
                DrawTextW(dc, &mut text, &mut rc, DT_LEFT | DT_WORDBREAK | DT_NOPREFIX | DT_END_ELLIPSIS);
                if let Some(o) = old {
                    SelectObject(dc, o);
                }
                let _ = EndPaint(h, &ps);
                LRESULT(0)
            }
            _ => DefWindowProcW(h, msg, wparam, lparam),
        }
    }
}

unsafe fn position_near_cursor(h: HWND) {
    unsafe {
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        let mon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        let work = if GetMonitorInfoW(mon, &mut mi).as_bool() {
            mi.rcWork
        } else {
            RECT { left: 0, top: 0, right: 1920, bottom: 1080 }
        };
        let mut x = pt.x - BANNER_W / 2;
        let mut y = pt.y - BANNER_H - 28;
        if y < work.top {
            y = pt.y + 28;
        }
        x = x.clamp(work.left + 8, (work.right - BANNER_W - 8).max(work.left));
        y = y.clamp(work.top + 8, (work.bottom - BANNER_H - 8).max(work.top));
        let _ = SetWindowPos(h, Some(HWND_TOPMOST), x, y, BANNER_W, BANNER_H, SWP_NOACTIVATE);
    }
}

// ----- focus -----

/// Bring a window to the foreground despite the foreground lock: attach to the current foreground
/// thread's input queue for the duration of the call.
pub fn force_foreground(target: HWND) {
    unsafe {
        if IsIconic(target).as_bool() {
            let _ = ShowWindow(target, SW_RESTORE);
        }
        let fg = GetForegroundWindow();
        let fg_tid = GetWindowThreadProcessId(fg, None);
        let me = GetCurrentThreadId();
        let attached = fg_tid != 0 && fg_tid != me && AttachThreadInput(me, fg_tid, true).as_bool();
        let _ = BringWindowToTop(target);
        let _ = SetForegroundWindow(target);
        if attached {
            let _ = AttachThreadInput(me, fg_tid, false);
        }
    }
}

// ----- prompts -----

fn heading(kind: PromptKind) -> &'static str {
    match kind {
        PromptKind::Send => "Send this message?",
        PromptKind::Paste => "Paste this content?",
        PromptKind::Upload => "Upload this file?",
        PromptKind::Drop => "Drop stopped",
        PromptKind::Backstop => "Check your attachment",
        PromptKind::BlockNotice => "Blocked by Veil",
    }
}

/// Show a prompt and wait for the answer.
pub fn ask(prompt: &Prompt) -> Choice {
    if prompt.options.is_empty() {
        return Choice::Ok;
    }
    match task_dialog(prompt) {
        Some(c) => c,
        None => message_box(prompt),
    }
}

type TaskDialogIndirectFn =
    unsafe extern "system" fn(*const TASKDIALOGCONFIG, *mut i32, *mut i32, *mut windows::core::BOOL) -> HRESULT;

/// `TaskDialogIndirect` lives only in comctl32 v6, which is loaded when the executable's manifest
/// activates Common Controls 6. Resolve it at run time so a missing manifest degrades to a MessageBox
/// instead of failing to start.
fn task_dialog_fn() -> Option<TaskDialogIndirectFn> {
    unsafe {
        let module = LoadLibraryW(w!("comctl32.dll")).ok()?;
        let proc = GetProcAddress(module, s!("TaskDialogIndirect"))?;
        Some(std::mem::transmute::<unsafe extern "system" fn() -> isize, TaskDialogIndirectFn>(proc))
    }
}

unsafe extern "system" fn task_dialog_callback(
    h: HWND,
    msg: TASKDIALOG_NOTIFICATIONS,
    _w: WPARAM,
    _l: LPARAM,
    _data: isize,
) -> HRESULT {
    if msg == TDN_CREATED {
        unsafe {
            let _ = SetWindowPos(h, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW);
        }
        force_foreground(h);
    }
    HRESULT(0)
}

const FIRST_BUTTON_ID: i32 = 100;

fn task_dialog(prompt: &Prompt) -> Option<Choice> {
    let f = task_dialog_fn()?;
    let labels: Vec<Vec<u16>> = prompt.options.iter().map(|c| wide(prompt.label(*c))).collect();
    let buttons: Vec<TASKDIALOG_BUTTON> = labels
        .iter()
        .enumerate()
        .map(|(i, l)| TASKDIALOG_BUTTON { nButtonID: FIRST_BUTTON_ID + i as i32, pszButtonText: PCWSTR(l.as_ptr()) })
        .collect();
    let title = wide(&prompt.title);
    let main = wide(heading(prompt.kind));
    let content = wide(&prompt.message);
    let footer = wide(&format!("App: {}", prompt.app.display()));

    let cfg = TASKDIALOGCONFIG {
        cbSize: std::mem::size_of::<TASKDIALOGCONFIG>() as u32,
        hInstance: unsafe { GetModuleHandleW(None) }.map(|m| m.into()).unwrap_or_default(),
        dwFlags: TDF_ALLOW_DIALOG_CANCELLATION,
        pszWindowTitle: PCWSTR(title.as_ptr()),
        Anonymous1: TASKDIALOGCONFIG_0 { pszMainIcon: TD_WARNING_ICON },
        pszMainInstruction: PCWSTR(main.as_ptr()),
        pszContent: PCWSTR(content.as_ptr()),
        cButtons: buttons.len() as u32,
        pButtons: buttons.as_ptr(),
        nDefaultButton: FIRST_BUTTON_ID,
        pszFooter: PCWSTR(footer.as_ptr()),
        pfCallback: Some(task_dialog_callback),
        ..Default::default()
    };

    let mut pressed = 0i32;
    let hr = unsafe { f(&cfg, &mut pressed, std::ptr::null_mut(), std::ptr::null_mut()) };
    if hr.is_err() {
        tracing::warn!("TaskDialogIndirect failed: {hr:?}");
        return None;
    }
    let idx = pressed - FIRST_BUTTON_ID;
    Some(if idx >= 0 && (idx as usize) < prompt.options.len() {
        prompt.options[idx as usize]
    } else {
        prompt.cancel_choice()
    })
}

thread_local! {
    static CBT_HOOK: Cell<isize> = const { Cell::new(0) };
    static CBT_LABELS: RefCell<Vec<(i32, Vec<u16>)>> = const { RefCell::new(Vec::new()) };
}

/// Renames the MessageBox buttons when the box activates, then unhooks itself.
unsafe extern "system" fn cbt_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        if code == HCBT_ACTIVATE as i32 {
            let dlg = HWND(wparam.0 as *mut _);
            CBT_LABELS.with(|labels| {
                for (id, label) in labels.borrow().iter() {
                    let _ = SetDlgItemTextW(dlg, *id, PCWSTR(label.as_ptr()));
                }
            });
            let hook = CBT_HOOK.with(|h| h.replace(0));
            if hook != 0 {
                let _ = UnhookWindowsHookEx(HHOOK(hook as *mut _));
            }
            force_foreground(dlg);
        }
        CallNextHookEx(None, code, wparam, lparam)
    }
}

/// Fallback prompt: a MessageBox whose buttons are relabelled. The cancel slot (Esc) always maps to the
/// prompt's safe cancel choice.
fn message_box(prompt: &Prompt) -> Choice {
    let cancel = prompt.cancel_choice();
    let others: Vec<Choice> = prompt.options.iter().copied().filter(|c| *c != cancel).collect();
    let has_cancel = prompt.options.contains(&cancel);

    // (style, [(button id, choice)])
    let (style, slots): (MESSAGEBOX_STYLE, Vec<(MESSAGEBOX_RESULT, Choice)>) = match (others.len(), has_cancel) {
        (0, _) => (MB_OK, vec![(IDOK, cancel)]),
        (1, true) => (MB_OKCANCEL, vec![(IDOK, others[0]), (IDCANCEL, cancel)]),
        (1, false) => (MB_OK, vec![(IDOK, others[0])]),
        (_, true) => (MB_YESNOCANCEL, vec![(IDYES, others[0]), (IDNO, others[1]), (IDCANCEL, cancel)]),
        (_, false) => (MB_YESNO, vec![(IDYES, others[0]), (IDNO, others[1])]),
    };
    let default_slot = slots.iter().position(|(_, c)| *c == prompt.options[0]).unwrap_or(0);
    let default_flag = match default_slot {
        0 => MB_DEFBUTTON1,
        1 => MB_DEFBUTTON2,
        _ => MB_DEFBUTTON3,
    };

    CBT_LABELS.with(|l| {
        *l.borrow_mut() = slots.iter().map(|(id, c)| (id.0, wide(prompt.label(*c)))).collect();
    });
    unsafe {
        if let Ok(hook) = SetWindowsHookExW(WH_CBT, Some(cbt_proc), None, GetCurrentThreadId()) {
            CBT_HOOK.with(|h| h.set(hook.0 as isize));
        }
    }
    let text = wide(&format!("{}\n\n{}\n\nApp: {}", heading(prompt.kind), prompt.message, prompt.app.display()));
    let caption = wide(&prompt.title);
    let result = unsafe {
        MessageBoxW(
            None,
            PCWSTR(text.as_ptr()),
            PCWSTR(caption.as_ptr()),
            style | default_flag | MB_ICONWARNING | MB_TOPMOST | MB_SETFOREGROUND,
        )
    };
    // If the hook never fired (box failed), make sure it is gone.
    let hook = CBT_HOOK.with(|h| h.replace(0));
    if hook != 0 {
        unsafe {
            let _ = UnhookWindowsHookEx(HHOOK(hook as *mut _));
        }
    }
    slots.iter().find(|(id, _)| *id == result).map(|(_, c)| *c).unwrap_or(cancel)
}

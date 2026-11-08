//! Files selected in Explorer or on the Desktop, read through Shell COM (`IShellWindows`). Used at drag
//! start to learn which files are being dragged. Runs on a COM (MTA) worker; Explorer is out of process so
//! every call is marshalled.

use std::path::PathBuf;
use windows::core::Interface;
use windows::Win32::System::Com::{CoCreateInstance, CoTaskMemFree, IDispatch, IServiceProvider, CLSCTX_ALL};
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Shell::{
    IFolderView, IShellBrowser, IShellItemArray, IShellWindows, IWebBrowserApp, SID_STopLevelBrowser, ShellWindows,
    CSIDL_DESKTOP, SIGDN_FILESYSPATH, SVGIO_SELECTION, SWC_DESKTOP, SWFO_NEEDDISPATCH,
};

const MAX_FILES: u32 = 500;

/// Selection of the Explorer window `root` (a top-level `CabinetWClass` HWND), or of the Desktop.
///
/// Windows 11 Explorer tabs share one top-level window; the first tab with a non-empty selection wins.
pub fn selection(root: isize, desktop: bool) -> Vec<PathBuf> {
    crate::com::ensure_mta();
    let windows: IShellWindows = match unsafe { CoCreateInstance(&ShellWindows, None, CLSCTX_ALL) } {
        Ok(w) => w,
        Err(e) => {
            tracing::debug!("ShellWindows unavailable: {e}");
            return Vec::new();
        }
    };
    if desktop {
        return desktop_dispatch(&windows).map(|d| selected_paths(&d)).unwrap_or_default();
    }
    let count = unsafe { windows.Count() }.unwrap_or(0);
    for i in 0..count {
        let Ok(disp) = (unsafe { windows.Item(&VARIANT::from(i)) }) else { continue };
        let Ok(app) = disp.cast::<IWebBrowserApp>() else { continue };
        let Ok(h) = (unsafe { app.HWND() }) else { continue };
        if h.0 != root {
            continue;
        }
        let paths = selected_paths(&disp);
        if !paths.is_empty() {
            return paths;
        }
    }
    Vec::new()
}

fn desktop_dispatch(windows: &IShellWindows) -> Option<IDispatch> {
    let loc = VARIANT::from(CSIDL_DESKTOP as i32);
    let root = VARIANT::default();
    let mut hwnd = 0i32;
    unsafe { windows.FindWindowSW(&loc, &root, SWC_DESKTOP, &mut hwnd, SWFO_NEEDDISPATCH) }.ok()
}

fn selected_paths(disp: &IDispatch) -> Vec<PathBuf> {
    let run = || -> windows::core::Result<Vec<PathBuf>> {
        let sp: IServiceProvider = disp.cast()?;
        let browser: IShellBrowser = unsafe { sp.QueryService(&SID_STopLevelBrowser)? };
        let view = unsafe { browser.QueryActiveShellView()? };
        let folder_view: IFolderView = view.cast()?;
        let items: IShellItemArray = unsafe { folder_view.Items(SVGIO_SELECTION)? };
        let n = unsafe { items.GetCount()? }.min(MAX_FILES);
        let mut out = Vec::with_capacity(n as usize);
        for i in 0..n {
            let item = unsafe { items.GetItemAt(i)? };
            // Virtual items (e.g. inside zip folders or This PC) have no file-system path.
            let Ok(p) = (unsafe { item.GetDisplayName(SIGDN_FILESYSPATH) }) else { continue };
            let s = unsafe { p.to_string() }.unwrap_or_default();
            unsafe { CoTaskMemFree(Some(p.0 as *const _)) };
            if !s.is_empty() {
                out.push(PathBuf::from(s));
            }
        }
        Ok(out)
    };
    run().unwrap_or_else(|e| {
        tracing::debug!("Explorer selection unavailable: {e}");
        Vec::new()
    })
}

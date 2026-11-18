//! Docked DevTools for jj1-browser.
//!
//! WebView2 can only open DevTools in its own separate window. To show them
//! inside the browser window instead (like Chrome or Edge), the browser starts
//! WebView2 with a local remote-debugging port, asks a tab for its DevTools
//! target id, and loads the DevTools front end for that target into a child
//! WebView docked under the page.
//!
//! The built-in triggers (F12, Ctrl+Shift+I/J/C and the right-click "Inspect"
//! item) are intercepted so they toggle the docked panel instead of popping
//! out a window.

use wry::WebView;

/// Local port WebView2's DevTools server listens on (127.0.0.1 only). It is
/// fixed rather than random because every jj1-browser window shares one
/// WebView2 browser process, and they must all start it with identical
/// arguments.
pub const PORT: u16 = 9338;

/// The WebView2 browser arguments every jj1-browser WebView must use: wry's
/// defaults plus the DevTools server. Only the DevTools front end served from
/// that same local origin may open a debugging connection.
pub fn browser_args() -> String {
    format!(
        "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection \
         --remote-debugging-address=127.0.0.1 --remote-debugging-port={PORT} \
         --remote-allow-origins=http://127.0.0.1:{PORT}"
    )
}

/// The DevTools front end for one page target.
pub fn frontend_url(target_id: &str) -> String {
    format!("http://127.0.0.1:{PORT}/devtools/devtools_app.html?ws=127.0.0.1:{PORT}/devtools/page/{target_id}")
}

/// Is this URL the docked DevTools front end (so it is not mistaken for a page)?
pub fn is_frontend(url: &str) -> bool {
    url.starts_with(&format!("http://127.0.0.1:{PORT}/devtools/"))
}

/// Call `on_toggle` instead of opening the DevTools window when the user
/// presses F12 or Ctrl+Shift+I / J / C in `webview`.
#[cfg(windows)]
pub fn intercept_keys(webview: &WebView, mut on_toggle: impl FnMut() + 'static) -> Result<(), String> {
    use webview2_com::Microsoft::Web::WebView2::Win32::{
        COREWEBVIEW2_KEY_EVENT_KIND, COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN, COREWEBVIEW2_KEY_EVENT_KIND_SYSTEM_KEY_DOWN,
    };
    use webview2_com::AcceleratorKeyPressedEventHandler;
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_CONTROL, VK_F12, VK_SHIFT};
    use wry::WebViewExtWindows;

    let controller = webview.controller();
    // SAFETY: plain COM calls on this webview's controller; out-pointers are
    // valid locals. GetKeyState only reads the calling thread's key state.
    unsafe {
        let handler = AcceleratorKeyPressedEventHandler::create(Box::new(move |_sender, args| {
            let Some(args) = args else { return Ok(()) };
            let mut kind = COREWEBVIEW2_KEY_EVENT_KIND::default();
            args.KeyEventKind(&mut kind)?;
            if kind != COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN && kind != COREWEBVIEW2_KEY_EVENT_KIND_SYSTEM_KEY_DOWN {
                return Ok(());
            }
            let mut key = 0u32;
            args.VirtualKey(&mut key)?;
            let down = |vk: u16| GetKeyState(vk as i32) < 0;
            let ctrl_shift = down(VK_CONTROL.0) && down(VK_SHIFT.0);
            let hit = key == VK_F12.0 as u32 || (ctrl_shift && matches!(key, 0x49 | 0x4A | 0x43)); // I, J, C
            if hit {
                args.SetHandled(true)?;
                // Ignore auto-repeat while the key is held.
                let mut status = Default::default();
                args.PhysicalKeyStatus(&mut status)?;
                if !status.WasKeyDown.as_bool() {
                    on_toggle();
                }
            }
            Ok(())
        }));
        let mut token = 0i64;
        controller.add_AcceleratorKeyPressed(&handler, &mut token).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Replace the right-click "Inspect" item with one that calls `on_inspect`.
#[cfg(windows)]
pub fn replace_inspect_item(webview: &WebView, on_inspect: impl Fn() + Clone + 'static) -> Result<(), String> {
    use webview2_com::Microsoft::Web::WebView2::Win32::{
        ICoreWebView2Environment9, ICoreWebView2_11, COREWEBVIEW2_CONTEXT_MENU_ITEM_KIND_COMMAND,
    };
    use webview2_com::{ContextMenuRequestedEventHandler, CustomItemSelectedEventHandler};
    use windows::core::{w, Interface, PWSTR};
    use windows::Win32::System::Com::IStream;
    use wry::WebViewExtWindows;

    let core: ICoreWebView2_11 = webview.webview().cast().map_err(|e| e.to_string())?;
    let env: ICoreWebView2Environment9 = webview.environment().cast().map_err(|e| e.to_string())?;
    // SAFETY: plain COM calls on interfaces owned by this webview; returned
    // PWSTRs are freed by `take_pwstr`.
    unsafe {
        let handler = ContextMenuRequestedEventHandler::create(Box::new(move |_sender, args| {
            let Some(args) = args else { return Ok(()) };
            let items = args.MenuItems()?;
            let mut count = 0u32;
            items.Count(&mut count)?;
            for i in 0..count {
                let item = items.GetValueAtIndex(i)?;
                let mut name = PWSTR::null();
                item.Name(&mut name)?;
                if webview2_com::take_pwstr(name) != "inspectElement" {
                    continue;
                }
                items.RemoveValueAtIndex(i)?;
                let mine = env.CreateContextMenuItem(w!("Inspect"), None::<&IStream>, COREWEBVIEW2_CONTEXT_MENU_ITEM_KIND_COMMAND)?;
                let cb = on_inspect.clone();
                let selected = CustomItemSelectedEventHandler::create(Box::new(move |_item, _| {
                    cb();
                    Ok(())
                }));
                let mut token = 0i64;
                mine.add_CustomItemSelected(&selected, &mut token)?;
                items.InsertValueAtIndex(i, &mine)?;
                break;
            }
            Ok(())
        }));
        let mut token = 0i64;
        core.add_ContextMenuRequested(&handler, &mut token).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Ask `webview` for its DevTools target id; `done` receives it (or `None`).
#[cfg(windows)]
pub fn target_id(webview: &WebView, done: impl FnOnce(Option<String>) + 'static) -> Result<(), String> {
    use webview2_com::CallDevToolsProtocolMethodCompletedHandler;
    use windows::core::w;
    use wry::WebViewExtWindows;

    let core = webview.webview();
    let handler = CallDevToolsProtocolMethodCompletedHandler::create(Box::new(move |result, json| {
        let id = result
            .ok()
            .and_then(|_| serde_json::from_str::<serde_json::Value>(&json).ok())
            .and_then(|v| v["targetInfo"]["targetId"].as_str().map(str::to_string));
        done(id);
        Ok(())
    }));
    // SAFETY: plain COM call on this webview's interface.
    unsafe { core.CallDevToolsProtocolMethod(w!("Target.getTargetInfo"), w!("{}"), &handler) }.map_err(|e| e.to_string())
}

/// Open WebView2's own separate DevTools window (fallback when docking fails).
#[cfg(windows)]
pub fn open_window(webview: &WebView) {
    use wry::WebViewExtWindows;
    // SAFETY: plain COM call on this webview's interface.
    let _ = unsafe { webview.webview().OpenDevToolsWindow() };
}

#[cfg(not(windows))]
pub fn intercept_keys(_webview: &WebView, _on_toggle: impl FnMut() + 'static) -> Result<(), String> {
    Err("docked DevTools are only implemented for WebView2".into())
}

#[cfg(not(windows))]
pub fn replace_inspect_item(_webview: &WebView, _on_inspect: impl Fn() + Clone + 'static) -> Result<(), String> {
    Err("docked DevTools are only implemented for WebView2".into())
}

#[cfg(not(windows))]
pub fn target_id(_webview: &WebView, _done: impl FnOnce(Option<String>) + 'static) -> Result<(), String> {
    Err("docked DevTools are only implemented for WebView2".into())
}

#[cfg(not(windows))]
pub fn open_window(_webview: &WebView) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frontend_url_points_at_local_target() {
        let u = frontend_url("ABC123");
        assert_eq!(u, "http://127.0.0.1:9338/devtools/devtools_app.html?ws=127.0.0.1:9338/devtools/page/ABC123");
        assert!(is_frontend(&u));
        assert!(!is_frontend("https://example.com/devtools/"));
        assert!(browser_args().contains("--remote-allow-origins=http://127.0.0.1:9338"));
    }
}

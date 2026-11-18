//! Shared pieces of JJ1's WebView layer, used by the agent worker (`jj1-view`)
//! and the human-facing browser (`jj1-browser`).

pub mod browserdata;
pub mod deuri;
pub mod devtools;
pub mod extensions;
pub mod modelfit;
pub mod screenshot;

use wry::WebView;

/// What kind of thing a sub-resource request is for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ResourceKind {
    /// Images, audio/video and fonts.
    Media,
    Other,
}

/// Intercept every request a page makes and answer the ones `block` refuses
/// with an empty 403, so they never reach the network. WebView2 only.
#[cfg(windows)]
pub fn install_request_filter(
    webview: &WebView,
    block: impl Fn(ResourceKind, &str) -> bool + 'static,
) -> Result<(), String> {
    use webview2_com::Microsoft::Web::WebView2::Win32::{
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL, COREWEBVIEW2_WEB_RESOURCE_CONTEXT_FONT,
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_IMAGE, COREWEBVIEW2_WEB_RESOURCE_CONTEXT_MEDIA,
    };
    use webview2_com::WebResourceRequestedEventHandler;
    use windows::core::{w, PWSTR};
    use wry::WebViewExtWindows;

    let core = webview.webview();
    let env = webview.environment();
    // SAFETY: plain COM calls on interfaces owned by this webview; the
    // out-pointers are valid locals and the returned PWSTR is taken over by
    // `take_pwstr`, which frees it.
    unsafe {
        core.AddWebResourceRequestedFilter(w!("*"), COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL)
            .map_err(|e| e.to_string())?;
        let handler = WebResourceRequestedEventHandler::create(Box::new(move |_sender, args| {
            let Some(args) = args else { return Ok(()) };
            let mut context = COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL;
            args.ResourceContext(&mut context)?;
            let request = args.Request()?;
            let mut uri = PWSTR::null();
            request.Uri(&mut uri)?;
            let url = webview2_com::take_pwstr(uri);
            let kind = if context == COREWEBVIEW2_WEB_RESOURCE_CONTEXT_IMAGE
                || context == COREWEBVIEW2_WEB_RESOURCE_CONTEXT_MEDIA
                || context == COREWEBVIEW2_WEB_RESOURCE_CONTEXT_FONT
            {
                ResourceKind::Media
            } else {
                ResourceKind::Other
            };
            if block(kind, &url) {
                let response = env.CreateWebResourceResponse(None, 403, w!("Blocked by JJ1"), w!(""))?;
                args.SetResponse(&response)?;
            }
            Ok(())
        }));
        let mut token = Default::default();
        core.add_WebResourceRequested(&handler, &mut token).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(not(windows))]
pub fn install_request_filter(
    _webview: &WebView,
    _block: impl Fn(ResourceKind, &str) -> bool + 'static,
) -> Result<(), String> {
    Err("request filtering is only implemented for WebView2".into())
}

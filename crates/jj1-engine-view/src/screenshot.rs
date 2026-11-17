//! Capture a region of a window to a PNG (Windows only).
//!
//! Used by the `jj1.captureTab()` extension API. `PrintWindow` with
//! `PW_RENDERFULLCONTENT` renders the WebView2 content (which a plain BitBlt
//! would miss), so this works without touching the WebView2 COM API.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// `%USERPROFILE%\Pictures\JJ1`, created if missing.
pub fn output_dir() -> PathBuf {
    let base = std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .map(|p| p.join("Pictures"))
        .unwrap_or_else(std::env::temp_dir);
    let dir = base.join("JJ1");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

pub fn next_path() -> PathBuf {
    let ts = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    output_dir().join(format!("jj1-shot-{ts}.png"))
}

/// Capture the rectangle `(x, y, w, h)` (physical pixels, relative to the
/// window's client area) of window `hwnd` and write it to `out` as PNG.
#[cfg(windows)]
pub fn capture_client_region(hwnd: isize, x: i32, y: i32, w: i32, h: i32, out: &Path) -> Result<(), String> {
    use windows::Win32::Foundation::{HWND, RECT};
    use windows::Win32::Graphics::Gdi::{
        CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC, GetDIBits, ReleaseDC, SelectObject,
        BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HGDIOBJ,
    };
    use windows::Win32::Storage::Xps::{PrintWindow, PRINT_WINDOW_FLAGS};
    use windows::Win32::UI::WindowsAndMessaging::GetClientRect;

    if w <= 0 || h <= 0 {
        return Err("nothing to capture".into());
    }
    let hwnd = HWND(hwnd as *mut core::ffi::c_void);
    // SAFETY: standard GDI capture sequence. Every created GDI object is freed
    // on every path; `buf` is sized to the full client area that GetDIBits fills.
    unsafe {
        let mut rc = RECT::default();
        GetClientRect(hwnd, &mut rc).map_err(|e| format!("GetClientRect: {e}"))?;
        let (cw, ch) = (rc.right - rc.left, rc.bottom - rc.top);
        if cw <= 0 || ch <= 0 {
            return Err("the window has no client area".into());
        }

        let screen = GetDC(Some(hwnd));
        if screen.is_invalid() {
            return Err("could not get the window device context".into());
        }
        let mem = CreateCompatibleDC(Some(screen));
        let bmp = CreateCompatibleBitmap(screen, cw, ch);
        let old = SelectObject(mem, HGDIOBJ(bmp.0));

        // PW_CLIENTONLY (1) | PW_RENDERFULLCONTENT (2).
        let ok = PrintWindow(hwnd, mem, PRINT_WINDOW_FLAGS(0x3)).as_bool();

        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: cw,
                biHeight: -ch, // negative = top-down rows
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut buf = vec![0u8; (cw as usize) * (ch as usize) * 4];
        let got = GetDIBits(mem, bmp, 0, ch as u32, Some(buf.as_mut_ptr() as *mut _), &mut info, DIB_RGB_COLORS);

        let _ = SelectObject(mem, old);
        let _ = DeleteObject(HGDIOBJ(bmp.0));
        let _ = DeleteDC(mem);
        ReleaseDC(Some(hwnd), screen);

        if !ok || got == 0 {
            return Err("PrintWindow could not capture the tab".into());
        }

        // Clamp the requested crop to the client area.
        let x0 = x.clamp(0, cw);
        let y0 = y.clamp(0, ch);
        let cx = (x0 + w).min(cw) - x0;
        let cy = (y0 + h).min(ch) - y0;
        if cx <= 0 || cy <= 0 {
            return Err("the capture region is outside the window".into());
        }
        // BGRA top-down -> cropped RGBA.
        let mut rgba = Vec::with_capacity((cx as usize) * (cy as usize) * 4);
        for row in 0..cy {
            let src = (((y0 + row) * cw + x0) * 4) as usize;
            for col in 0..cx as usize {
                let p = src + col * 4;
                rgba.push(buf[p + 2]);
                rgba.push(buf[p + 1]);
                rgba.push(buf[p]);
                rgba.push(255);
            }
        }
        write_png(out, cx as u32, cy as u32, &rgba)
    }
}

#[cfg(not(windows))]
pub fn capture_client_region(_hwnd: isize, _x: i32, _y: i32, _w: i32, _h: i32, _out: &Path) -> Result<(), String> {
    Err("screenshot capture is only implemented on Windows".into())
}

fn write_png(out: &Path, w: u32, h: u32, rgba: &[u8]) -> Result<(), String> {
    let file = std::fs::File::create(out).map_err(|e| format!("{}: {e}", out.display()))?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header().map_err(|e| e.to_string())?;
    writer.write_image_data(rgba).map_err(|e| e.to_string())?;
    Ok(())
}

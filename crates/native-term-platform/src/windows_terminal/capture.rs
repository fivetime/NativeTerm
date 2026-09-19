//! A small picture of a Terminal window's selected tab, for the tab list's
//! previews. Terminal renders only the selected tab, so this is all that
//! can be seen of a tab from outside, and only while it is selected.
//! `PrintWindow` with `PW_RENDERFULLCONTENT` draws the window even while
//! other windows cover it (not while it is minimized).

use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC, GetDIBits, ReleaseDC, SelectObject,
    SetBrushOrgEx, SetStretchBltMode, StretchBlt, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HALFTONE,
    HGDIOBJ, SRCCOPY,
};
use windows::Win32::Storage::Xps::{PrintWindow, PRINT_WINDOW_FLAGS};
use windows::Win32::UI::WindowsAndMessaging::{GetWindowRect, IsIconic};

use super::window::hwnd;

/// `PW_RENDERFULLCONTENT`: DirectComposition content (Terminal's) too.
const PW_RENDERFULLCONTENT: u32 = 2;

/// An RGBA picture, rows top to bottom.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// The part of the window to keep, in its own pixels (0, 0 = the window
/// rectangle's corner): the visible frame, below `content_top` (a screen
/// y, the tab strip's bottom) when given.
pub fn crop(window: RECT, frame: RECT, content_top: Option<i32>) -> Option<RECT> {
    let top = content_top.filter(|t| *t > frame.top && *t < frame.bottom).unwrap_or(frame.top);
    let r = RECT {
        left: frame.left - window.left,
        top: top - window.top,
        right: frame.right - window.left,
        bottom: frame.bottom - window.top,
    };
    (r.right > r.left && r.bottom > r.top).then_some(r)
}

/// The window's picture scaled to `width` pixels wide, without the tab
/// strip (above `content_top`); `None` while it is minimized or if it
/// can't be drawn.
pub fn capture(handle: isize, content_top: Option<i32>, width: i32) -> Option<Image> {
    let window = hwnd(handle);
    // SAFETY: `window` is a window handle (a stale one only makes these
    // calls fail); every GDI object created here is selected out and
    // deleted before returning, and `pixels` is exactly the size GetDIBits
    // writes for a `width` × `height` 32-bit top-down bitmap.
    unsafe {
        if IsIconic(window).as_bool() {
            return None;
        }
        let mut rect = RECT::default();
        GetWindowRect(window, &mut rect).ok()?;
        // the visible frame, without the invisible resize borders
        let mut frame = rect;
        let _ = DwmGetWindowAttribute(
            window,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            &mut frame as *mut RECT as *mut _,
            std::mem::size_of::<RECT>() as u32,
        );
        let part = crop(rect, frame, content_top)?;
        let (full_w, full_h) = (rect.right - rect.left, rect.bottom - rect.top);
        let (part_w, part_h) = (part.right - part.left, part.bottom - part.top);
        let width = width.min(part_w).max(1);
        let height = (part_h * width / part_w).max(1);

        let screen = GetDC(None);
        let full_dc = CreateCompatibleDC(Some(screen));
        let full = CreateCompatibleBitmap(screen, full_w, full_h);
        let old_full = SelectObject(full_dc, HGDIOBJ(full.0));
        let printed = PrintWindow(window, full_dc, PRINT_WINDOW_FLAGS(PW_RENDERFULLCONTENT)).as_bool();
        let small_dc = CreateCompatibleDC(Some(screen));
        let small = CreateCompatibleBitmap(screen, width, height);
        let old_small = SelectObject(small_dc, HGDIOBJ(small.0));
        SetStretchBltMode(small_dc, HALFTONE);
        let _ = SetBrushOrgEx(small_dc, 0, 0, None);
        let _ = StretchBlt(small_dc, 0, 0, width, height, Some(full_dc), part.left, part.top, part_w, part_h, SRCCOPY);
        SelectObject(small_dc, old_small);
        SelectObject(full_dc, old_full);

        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height, // top-down
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut pixels = vec![0u8; (width * height * 4) as usize];
        let rows = GetDIBits(
            small_dc,
            small,
            0,
            height as u32,
            Some(pixels.as_mut_ptr() as *mut _),
            &mut info,
            DIB_RGB_COLORS,
        );
        let _ = DeleteDC(small_dc);
        let _ = DeleteDC(full_dc);
        let _ = DeleteObject(HGDIOBJ(small.0));
        let _ = DeleteObject(HGDIOBJ(full.0));
        ReleaseDC(None, screen);
        if !printed || rows != height {
            return None;
        }
        Some(Image { width: width as u32, height: height as u32, rgba: to_rgba(pixels) })
    }
}

/// GDI's BGRX to RGBA (opaque).
fn to_rgba(mut pixels: Vec<u8>) -> Vec<u8> {
    for p in pixels.as_chunks_mut::<4>().0 {
        p.swap(0, 2);
        p[3] = 255;
    }
    pixels
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(left: i32, top: i32, right: i32, bottom: i32) -> RECT {
        RECT { left, top, right, bottom }
    }

    #[test]
    fn the_part_kept() {
        // a window with 7 px invisible borders left, right and bottom
        let window = rect(93, 100, 1107, 807);
        let frame = rect(100, 100, 1100, 800);
        assert_eq!(crop(window, frame, Some(140)), Some(rect(7, 40, 1007, 700)));
        // no tab strip known, or one outside the window
        assert_eq!(crop(window, frame, None), Some(rect(7, 0, 1007, 700)));
        assert_eq!(crop(window, frame, Some(900)), Some(rect(7, 0, 1007, 700)));
        assert_eq!(crop(window, rect(100, 100, 100, 800), None), None);
    }

    #[test]
    fn colors() {
        assert_eq!(to_rgba(vec![1, 2, 3, 0, 10, 20, 30, 0]), vec![3, 2, 1, 255, 30, 20, 10, 255]);
    }
}

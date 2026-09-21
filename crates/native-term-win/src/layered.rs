//! Showing a window's pixels with their own transparency, so a window
//! can be a shape rather than a rectangle — what the floating button
//! needs to be round and slightly see-through.
//!
//! Windows has one way to do this: a layered window (`WS_EX_LAYERED`)
//! whose whole surface is handed over at once with
//! `UpdateLayeredWindow`, from a 32-bit top-down DIB whose colours are
//! premultiplied by their alpha — which is exactly how egui's software
//! renderer already paints (`Color32` is premultiplied), so the frame it
//! rasterized can be shown as it is.
//!
//! Where a pixel is fully transparent the window isn't there at all:
//! clicks land on whatever is behind it, and nothing of the rectangle
//! shows. The frame is not blitted through the window's own device
//! context, so a layered window never receives `WM_PAINT` for it.

use windows::Win32::Foundation::{POINT, RECT, SIZE};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, ReleaseDC, SelectObject, AC_SRC_ALPHA,
    AC_SRC_OVER, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION, DIB_RGB_COLORS, HBITMAP, HDC, HGDIOBJ,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetWindowLongPtrW, GetWindowRect, SetWindowLongPtrW, UpdateLayeredWindow, GWL_EXSTYLE, ULW_ALPHA, WINDOW_EX_STYLE,
    WS_EX_LAYERED,
};

use crate::dock::hwnd;

/// The pixels of one window, kept between frames and resized as needed.
pub struct Layered {
    dc: HDC,
    bitmap: HBITMAP,
    old: HGDIOBJ,
    /// The DIB's pixels: B, G, R, A per pixel, top-down, premultiplied.
    bits: *mut [u8; 4],
    width: i32,
    height: i32,
}

// The pixels belong to this bitmap, which belongs to this struct; the
// pointer is only used between `CreateDIBSection` and `DeleteObject`, on
// the thread that paints the window.
unsafe impl Send for Layered {}

impl Layered {
    /// A window that will show its own transparency. Add this before it
    /// is first shown.
    pub fn take_over(handle: isize) -> Layered {
        ensure(handle);
        Layered {
            dc: HDC(std::ptr::null_mut()),
            bitmap: HBITMAP(std::ptr::null_mut()),
            old: HGDIOBJ::default(),
            bits: std::ptr::null_mut(),
            width: 0,
            height: 0,
        }
    }

    /// Whether `handle` is a layered window (it may have lost the style
    /// if something else changed it).
    #[must_use]
    pub fn is_layered(handle: isize) -> bool {
        // SAFETY: reads a style of a window of this process.
        let style = unsafe { GetWindowLongPtrW(hwnd(handle), GWL_EXSTYLE) };
        WINDOW_EX_STYLE(style as u32).contains(WS_EX_LAYERED)
    }

    /// Hands `draw` the window's pixels (B, G, R, A, premultiplied, rows
    /// top-down, `width` of them a row) and shows what it painted.
    /// Whether it worked.
    pub fn present(&mut self, handle: isize, width: u32, height: u32, draw: impl FnOnce(&mut [[u8; 4]])) -> bool {
        // the window's own library rewrites its styles when it is shown,
        // moved or put on top, and a foreign bit doesn't survive that, so
        // it is put back before every frame (one call, nothing if it is
        // still there)
        ensure(handle);
        let (width, height) = (width.min(i32::MAX as u32) as i32, height.min(i32::MAX as u32) as i32);
        if width <= 0 || height <= 0 || !self.fit(width, height) {
            return false;
        }
        // SAFETY: the bitmap is this struct's, of this size, and nothing
        // else touches it while `draw` has it.
        let pixels = unsafe { std::slice::from_raw_parts_mut(self.bits, (width * height) as usize) };
        pixels.fill([0, 0, 0, 0]);
        draw(pixels);
        // SAFETY: Win32 calls with handles this struct owns; the window
        // keeps its position (no destination point is given).
        unsafe {
            let screen = GetDC(None);
            let mut rect = RECT::default();
            let at = GetWindowRect(hwnd(handle), &mut rect).is_ok().then_some(POINT { x: rect.left, y: rect.top });
            let size = SIZE { cx: width, cy: height };
            let source = POINT { x: 0, y: 0 };
            let blend = BLENDFUNCTION {
                BlendOp: AC_SRC_OVER as u8,
                BlendFlags: 0,
                SourceConstantAlpha: 255,
                AlphaFormat: AC_SRC_ALPHA as u8,
            };
            let done = UpdateLayeredWindow(
                hwnd(handle),
                Some(screen),
                at.as_ref().map(|p| p as *const POINT),
                Some(&size),
                Some(self.dc),
                Some(&source),
                windows::Win32::Foundation::COLORREF(0),
                Some(&blend),
                ULW_ALPHA,
            );
            ReleaseDC(None, screen);
            done.is_ok()
        }
    }

    /// Make (or remake) the bitmap for this size. Whether there is one.
    fn fit(&mut self, width: i32, height: i32) -> bool {
        if self.width == width && self.height == height && !self.bits.is_null() {
            return true;
        }
        self.release();
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                // top-down, as the renderer writes its rows
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        // SAFETY: a memory bitmap of this size and its own device
        // context, both released in `release`.
        unsafe {
            let screen = GetDC(None);
            let dc = CreateCompatibleDC(Some(screen));
            ReleaseDC(None, screen);
            let mut bits = std::ptr::null_mut();
            let Ok(bitmap) = CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0) else {
                let _ = DeleteDC(dc);
                return false;
            };
            self.old = SelectObject(dc, bitmap.into());
            self.dc = dc;
            self.bitmap = bitmap;
            self.bits = bits.cast();
            self.width = width;
            self.height = height;
        }
        true
    }

    fn release(&mut self) {
        if self.dc.is_invalid() {
            return;
        }
        // SAFETY: handles this struct made, not used again.
        unsafe {
            SelectObject(self.dc, self.old);
            let _ = DeleteObject(self.bitmap.into());
            let _ = DeleteDC(self.dc);
        }
        self.dc = HDC(std::ptr::null_mut());
        self.bitmap = HBITMAP(std::ptr::null_mut());
        self.bits = std::ptr::null_mut();
        self.width = 0;
        self.height = 0;
    }
}

/// Make the window a layered one, if it isn't already.
fn ensure(handle: isize) {
    // SAFETY: a window of this process; adding a style bit to it.
    unsafe {
        let window = hwnd(handle);
        let style = GetWindowLongPtrW(window, GWL_EXSTYLE);
        let wanted = style | WS_EX_LAYERED.0 as isize;
        if style != wanted {
            SetWindowLongPtrW(window, GWL_EXSTYLE, wanted);
        }
    }
}

impl Drop for Layered {
    fn drop(&mut self) {
        self.release();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_surface_is_made_once_for_a_size() {
        let mut layered = Layered::take_over(0);
        assert!(layered.fit(8, 4));
        let (dc, bits) = (layered.dc, layered.bits);
        assert!(!bits.is_null());
        assert!(layered.fit(8, 4), "the same size keeps the surface");
        assert_eq!((layered.dc, layered.bits), (dc, bits));
        assert!(layered.fit(16, 4), "another size makes a new one");
        assert_eq!((layered.width, layered.height), (16, 4));
        // every pixel is there and starts transparent
        // SAFETY: the bitmap this struct just made, of this size.
        let pixels = unsafe { std::slice::from_raw_parts_mut(layered.bits, 16 * 4) };
        pixels.fill([1, 2, 3, 4]);
        assert_eq!(pixels[16 * 4 - 1], [1, 2, 3, 4]);
    }

    #[test]
    fn nothing_is_shown_for_an_empty_window() {
        let mut layered = Layered::take_over(0);
        assert!(!layered.present(0, 0, 0, |_| panic!("nothing to paint")));
    }
}

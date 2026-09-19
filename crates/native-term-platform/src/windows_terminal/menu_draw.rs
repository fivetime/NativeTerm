//! Drawing the tab menu with Direct2D and DirectWrite: font fallback (CJK,
//! rare scripts) and color emoji in tab titles, which GDI can't do. A DC
//! render target draws into the popup's paint DC in software, so it needs
//! no GPU (remote desktops included). Everything is in physical pixels
//! (the target runs at 96 DPI).

use windows::core::{Result, HSTRING, PCWSTR};
use windows::Win32::Foundation::{COLORREF, RECT};
use windows::Win32::Globalization::GetUserDefaultLocaleName;
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE, D2D1_ALPHA_MODE_IGNORE, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_PIXEL_FORMAT, D2D_RECT_F,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1CreateFactory, ID2D1DCRenderTarget, ID2D1Factory, ID2D1SolidColorBrush, D2D1_DRAW_TEXT_OPTIONS_CLIP,
    D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT, D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_FEATURE_LEVEL_DEFAULT,
    D2D1_RENDER_TARGET_PROPERTIES, D2D1_RENDER_TARGET_TYPE_DEFAULT, D2D1_RENDER_TARGET_USAGE_NONE, D2D1_ROUNDED_RECT,
    D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE,
};
use windows::Win32::Graphics::DirectWrite::{
    DWriteCreateFactory, IDWriteFactory, IDWriteTextFormat, DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL,
    DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_MEASURING_MODE_NATURAL,
    DWRITE_PARAGRAPH_ALIGNMENT_CENTER, DWRITE_TEXT_METRICS, DWRITE_WORD_WRAPPING_NO_WRAP,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::HDC;

/// The factories and the render target, made once per menu thread.
pub struct Painter {
    dwrite: IDWriteFactory,
    target: ID2D1DCRenderTarget,
    /// With per-pixel alpha, for a layered popup (its own rounded shape).
    alpha_target: ID2D1DCRenderTarget,
    locale: Vec<u16>,
}

fn color(c: COLORREF) -> D2D1_COLOR_F {
    let v = c.0;
    D2D1_COLOR_F {
        r: (v & 0xff) as f32 / 255.0,
        g: ((v >> 8) & 0xff) as f32 / 255.0,
        b: ((v >> 16) & 0xff) as f32 / 255.0,
        a: 1.0,
    }
}

fn rect(left: i32, top: i32, right: i32, bottom: i32) -> D2D_RECT_F {
    D2D_RECT_F { left: left as f32, top: top as f32, right: right as f32, bottom: bottom as f32 }
}

impl Painter {
    pub fn new() -> Result<Painter> {
        unsafe {
            let d2d: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let properties = |alpha: D2D1_ALPHA_MODE| D2D1_RENDER_TARGET_PROPERTIES {
                r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
                pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: alpha },
                // pixels, not DIPs: the menu does its own DPI scaling
                dpiX: 96.0,
                dpiY: 96.0,
                usage: D2D1_RENDER_TARGET_USAGE_NONE,
                minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
            };
            let target = d2d.CreateDCRenderTarget(&properties(D2D1_ALPHA_MODE_IGNORE))?;
            let alpha_target = d2d.CreateDCRenderTarget(&properties(D2D1_ALPHA_MODE_PREMULTIPLIED))?;
            // ClearType needs an opaque background
            alpha_target.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            // the user's locale picks the right CJK glyphs in fallback
            let mut locale = [0u16; 85];
            let n = GetUserDefaultLocaleName(&mut locale);
            let locale = locale[..(n.max(1) as usize - 1)].iter().copied().chain(std::iter::once(0)).collect();
            Ok(Painter { dwrite, target, alpha_target, locale })
        }
    }

    /// A single-line, vertically centered text format.
    pub fn format(&self, family: &str, px: f32) -> Result<IDWriteTextFormat> {
        unsafe {
            let format = self.dwrite.CreateTextFormat(
                &HSTRING::from(family),
                None,
                DWRITE_FONT_WEIGHT_NORMAL,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                px,
                PCWSTR(self.locale.as_ptr()),
            )?;
            format.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
            format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
            Ok(format)
        }
    }

    /// The width of `text` in pixels.
    pub fn width(&self, format: &IDWriteTextFormat, text: &str) -> f32 {
        let wide: Vec<u16> = text.encode_utf16().collect();
        unsafe {
            let Ok(layout) = self.dwrite.CreateTextLayout(&wide, format, f32::MAX, f32::MAX) else { return 0.0 };
            let mut metrics = DWRITE_TEXT_METRICS::default();
            if layout.GetMetrics(&mut metrics).is_err() {
                return 0.0;
            }
            metrics.widthIncludingTrailingWhitespace
        }
    }

    /// Draw into `hdc` (the popup's paint DC), `size` pixels.
    pub fn paint(
        &self,
        hdc: HDC,
        width: i32,
        height: i32,
        background: COLORREF,
        draw: impl FnOnce(&Canvas),
    ) -> Result<()> {
        unsafe {
            self.target.BindDC(hdc, &RECT { left: 0, top: 0, right: width, bottom: height })?;
            self.target.BeginDraw();
            self.target.Clear(Some(&color(background)));
            draw(&Canvas { target: &self.target });
            self.target.EndDraw(None, None)
        }
    }

    /// Draw with transparency into `hdc` (a 32-bit DIB for
    /// `UpdateLayeredWindow`): everything `draw` leaves out stays clear.
    pub fn paint_alpha(&self, hdc: HDC, width: i32, height: i32, draw: impl FnOnce(&Canvas)) -> Result<()> {
        unsafe {
            self.alpha_target.BindDC(hdc, &RECT { left: 0, top: 0, right: width, bottom: height })?;
            self.alpha_target.BeginDraw();
            self.alpha_target.Clear(Some(&D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 0.0 }));
            draw(&Canvas { target: &self.alpha_target });
            self.alpha_target.EndDraw(None, None)
        }
    }
}

pub struct Canvas<'a> {
    target: &'a ID2D1DCRenderTarget,
}

impl Canvas<'_> {
    fn brush(&self, c: COLORREF) -> Option<ID2D1SolidColorBrush> {
        unsafe { self.target.CreateSolidColorBrush(&color(c), None).ok() }
    }

    pub fn fill(&self, left: i32, top: i32, right: i32, bottom: i32, c: COLORREF) {
        if let Some(brush) = self.brush(c) {
            unsafe { self.target.FillRectangle(&rect(left, top, right, bottom), &brush) };
        }
    }

    pub fn fill_rounded(&self, left: i32, top: i32, right: i32, bottom: i32, radius: f32, c: COLORREF) {
        if let Some(brush) = self.brush(c) {
            let rounded = D2D1_ROUNDED_RECT { rect: rect(left, top, right, bottom), radiusX: radius, radiusY: radius };
            unsafe { self.target.FillRoundedRectangle(&rounded, &brush) };
        }
    }

    /// One line in the box, vertically centered, clipped; color emoji
    /// drawn in color.
    #[allow(clippy::too_many_arguments)]
    pub fn text(
        &self,
        format: &IDWriteTextFormat,
        text: &str,
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
        c: COLORREF,
    ) {
        let Some(brush) = self.brush(c) else { return };
        let wide: Vec<u16> = text.encode_utf16().collect();
        unsafe {
            self.target.DrawText(
                &wide,
                format,
                &rect(left, top, right, bottom),
                &brush,
                D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT | D2D1_DRAW_TEXT_OPTIONS_CLIP,
                DWRITE_MEASURING_MODE_NATURAL,
            )
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Measuring follows the font: wider text is wider, and characters
    /// Segoe UI lacks (Han, emoji) still get a width through fallback.
    #[test]
    fn measures_with_fallback() {
        let painter = Painter::new().unwrap();
        let format = painter.format("Segoe UI", 14.0).unwrap();
        let short = painter.width(&format, "web");
        let long = painter.width(&format, "web01.example.com");
        assert!(short > 0.0 && long > short, "{short} {long}");
        let han = painter.width(&format, "控制节点");
        let emoji = painter.width(&format, "🔥");
        assert!(han > 30.0 && emoji > 5.0, "{han} {emoji}");
    }
}

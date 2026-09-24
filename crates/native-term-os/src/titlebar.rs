//! The title bar as the desktop's GTK theme draws it, taken the way
//! Chrome takes it for its own (Chromium's `ui/gtk`): GTK is loaded at
//! run time (`libgtk-3.so.0`, nothing linked, nothing required), a
//! style context is built for the nodes a real GTK title bar has
//! (`window.background.csd > headerbar.titlebar > windowcontrols >
//! button.titlebutton.close > image`), and GTK itself renders each
//! window button — background, frame and symbolic icon, at rest, under
//! the pointer and in an unfocused window — and answers the colours:
//! the header bar's (the tab strip), the window's (the active tab), the
//! title's. So every GTK theme looks like itself with no code of ours
//! for it; only where Chrome goes to Qt (KDE, UKUI, LXQt, a desktop
//! calling itself `Deepin`) is there none of this (see [`toolkit`]).
//!
//! The rendering runs in a child process (`nativeterm --desktop-titlebar
//! DIR dark|light`, [`export`]), as GTK wants its own main thread and
//! global state; [`read`] starts it once per theme and keeps the answer.

use std::path::{Path, PathBuf};

/// A colour, `#rrggbb` in the configuration.
pub type Rgb = (u8, u8, u8);

/// The title bar a GTK theme draws.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Titlebar {
    /// The header bar (Chrome's frame colour: the tab strip), focused and
    /// not.
    pub frame: Rgb,
    pub frame_inactive: Rgb,
    /// The window's background (Chrome's toolbar: the active tab).
    pub window: Rgb,
    /// Text on the window's background, and the title's on the header
    /// bar, focused and not.
    pub text: Rgb,
    pub title: Rgb,
    pub title_inactive: Rgb,
    /// The header bar's padding at its two ends, and GTK's spacing
    /// between its buttons, in pixels at 96 dpi.
    pub padding_left: u32,
    pub padding_right: u32,
    pub spacing: u32,
    /// The header bar's padding above and below its buttons.
    pub padding_top: u32,
    pub padding_bottom: u32,
    /// The buttons: close, minimize, maximize, restore.
    pub buttons: Vec<Button>,
    /// The window's own edge as the theme draws it (shadow, border,
    /// rounded top corners), where it draws one.
    pub edge: Option<Edge>,
}

/// The theme's window decoration (Chromium's WindowFrameProviderGtk): a
/// square picture of a window's border and shadow, to be cut in nine —
/// corners `slice` pixels square kept, edges stretched — and what was
/// measured of it. Pixels at 96 dpi; the pictures drawn at twice that.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Edge {
    /// How far the drawing reaches outside the window on each side (the
    /// shadow and border): top, right, bottom, left.
    pub thickness: [u32; 4],
    /// The top corners' radius.
    pub radius: u32,
    /// The picture's corner size: it is `4 * slice` square, the window
    /// in the middle `2 * slice` of it.
    pub slice: u32,
    /// Focused and not.
    pub focused: PathBuf,
    pub unfocused: PathBuf,
}

/// One title button as GTK drew it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Button {
    /// `close`, `minimize`, `maximize` or `restore`.
    pub name: String,
    /// Its size and margins, in pixels at 96 dpi.
    pub width: u32,
    pub height: u32,
    pub margin_left: u32,
    pub margin_right: u32,
    pub margin_top: u32,
    pub margin_bottom: u32,
    /// The pictures (PNG, drawn at twice the size): at rest, under the
    /// pointer, in a window without the focus.
    pub normal: PathBuf,
    pub hover: PathBuf,
    pub backdrop: PathBuf,
}

/// Which toolkit Chrome takes its looks from on this desktop
/// (Chromium's `GetDefaultSystemTheme`, keyed by `XDG_CURRENT_DESKTOP`
/// as `base::nix::GetDesktopEnvironment` reads it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Toolkit {
    Gtk,
    Qt,
}

pub fn toolkit(xdg_current_desktop: &str, desktop_session: &str) -> Toolkit {
    for value in xdg_current_desktop.split(':').map(str::trim) {
        match value {
            "KDE" | "Deepin" | "UKUI" | "LXQt" => return Toolkit::Qt,
            "Unity" | "GNOME" | "X-Cinnamon" | "Pantheon" | "XFCE" | "COSMIC" => return Toolkit::Gtk,
            _ => {}
        }
    }
    match desktop_session {
        "deepin" | "kde4" | "kde-plasma" | "kde" => Toolkit::Qt,
        // anything else: Chrome's default, which is GTK too
        _ => Toolkit::Gtk,
    }
}

/// The title bar for the GTK theme now in use, `dark` or light, with its
/// pictures under `dir`; `None` where GTK cannot be had. The child
/// process runs once per theme (`key`: the GTK and icon theme names,
/// which change with it); the answer is kept.
#[cfg(all(unix, not(target_os = "macos")))]
pub fn read(dir: &Path, key: &str, dark: bool) -> Option<Titlebar> {
    use std::sync::Mutex;
    static LAST: Mutex<Option<(String, bool, Option<Titlebar>)>> = Mutex::new(None);
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((k, d, answer)) = last.as_ref() {
        if k == key && *d == dark {
            return answer.clone();
        }
    }
    let answer = run_child(dir, dark);
    *last = Some((key.to_string(), dark, answer.clone()));
    answer
}

#[cfg(not(all(unix, not(target_os = "macos"))))]
pub fn read(_dir: &Path, _key: &str, _dark: bool) -> Option<Titlebar> {
    None
}

#[cfg(all(unix, not(target_os = "macos")))]
fn run_child(dir: &Path, dark: bool) -> Option<Titlebar> {
    use std::io::Read;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};
    let exe = std::env::current_exe().ok()?;
    let mut child = Command::new(exe)
        .arg("--desktop-titlebar")
        .arg(dir)
        .arg(if dark { "dark" } else { "light" })
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    // a theme that hangs GTK must not hang NativeTerm
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break,
            Ok(Some(_)) | Err(_) => return None,
            Ok(None) if started.elapsed() > Duration::from_secs(10) => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
        }
    }
    let mut out = String::new();
    child.stdout.take()?.read_to_string(&mut out).ok()?;
    parse(&out)
}

/// The tab strip's colours on a desktop where Chrome goes to Qt (KDE,
/// and Lingmo, which calls itself KDE): KDE's palette, which is where Qt
/// takes its own from there, `~/.config/kdeglobals`. Chrome has the Qt
/// style draw a title bar and averages it; the window manager's title
/// bars on such a desktop are KDE's header colours (`[Colors:Header]`,
/// `[Colors:Header][Inactive]`; before Plasma 5.25 `[WM]`), so the tab
/// strip takes those; the active tab the window's colours, as a KDE tab
/// bar has it (Chrome's QPalette::Button is the header's own colour in
/// Breeze Dark, which would hide it). No buttons: Chrome draws its own there
/// too (Qt prefers the window manager's decorations). `None` without a
/// palette.
pub fn read_qt() -> Option<Titlebar> {
    let home = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    qt_colors(&std::fs::read_to_string(home.join("kdeglobals")).ok()?)
}

/// `kdeglobals` to the tab strip's colours (see [`read_qt`]).
pub fn qt_colors(kdeglobals: &str) -> Option<Titlebar> {
    use crate::appearance::parse::ini_value;
    let color = |section: &str, key: &str| -> Option<Rgb> {
        let v = ini_value(kdeglobals, section, key)?;
        let mut parts = v.split(',').map(|p| p.trim().parse::<u8>());
        Some((parts.next()?.ok()?, parts.next()?.ok()?, parts.next()?.ok()?))
    };
    let window = color("Colors:Window", "BackgroundNormal")?;
    let window_text = color("Colors:Window", "ForegroundNormal");
    let frame =
        color("Colors:Header", "BackgroundNormal").or_else(|| color("WM", "activeBackground")).unwrap_or(window);
    let frame_inactive = color("Colors:Header][Inactive", "BackgroundNormal")
        .or_else(|| color("WM", "inactiveBackground"))
        .unwrap_or(frame);
    let title =
        color("Colors:Header", "ForegroundNormal").or_else(|| color("WM", "activeForeground")).or(window_text)?;
    let title_inactive = color("Colors:Header][Inactive", "ForegroundNormal")
        .or_else(|| color("WM", "inactiveForeground"))
        .unwrap_or(title);
    Some(Titlebar {
        frame,
        frame_inactive,
        window,
        text: window_text.unwrap_or(title),
        title,
        title_inactive,
        ..Titlebar::default()
    })
}

/// The child's answer, tab-separated lines: `color NAME #rrggbb`,
/// `header PADDING_LEFT PADDING_RIGHT SPACING`, `edge TOP RIGHT BOTTOM
/// LEFT RADIUS SLICE FOCUSED UNFOCUSED`, `button NAME W H
/// MARGIN_LEFT MARGIN_RIGHT NORMAL HOVER BACKDROP`.
pub fn parse(text: &str) -> Option<Titlebar> {
    let mut bar = Titlebar::default();
    let mut colors = 0;
    for line in text.lines() {
        let fields: Vec<&str> = line.split('\t').collect();
        match fields.as_slice() {
            ["color", name, value] => {
                let rgb = hex(value)?;
                match *name {
                    "frame" => bar.frame = rgb,
                    "frame_inactive" => bar.frame_inactive = rgb,
                    "window" => bar.window = rgb,
                    "text" => bar.text = rgb,
                    "title" => bar.title = rgb,
                    "title_inactive" => bar.title_inactive = rgb,
                    _ => continue,
                }
                colors += 1;
            }
            ["header", left, right, spacing] => {
                bar.padding_left = left.parse().ok()?;
                bar.padding_right = right.parse().ok()?;
                bar.spacing = spacing.parse().ok()?;
            }
            ["header", left, right, spacing, top, bottom] => {
                bar.padding_left = left.parse().ok()?;
                bar.padding_right = right.parse().ok()?;
                bar.spacing = spacing.parse().ok()?;
                bar.padding_top = top.parse().ok()?;
                bar.padding_bottom = bottom.parse().ok()?;
            }
            ["edge", top, right, bottom, left, radius, slice, focused, unfocused] => {
                bar.edge = Some(Edge {
                    thickness: [top.parse().ok()?, right.parse().ok()?, bottom.parse().ok()?, left.parse().ok()?],
                    radius: radius.parse().ok()?,
                    slice: slice.parse().ok()?,
                    focused: PathBuf::from(focused),
                    unfocused: PathBuf::from(unfocused),
                })
            }
            ["button", name, w, h, ml, mr, normal, hover, backdrop] => bar.buttons.push(Button {
                name: name.to_string(),
                width: w.parse().ok()?,
                height: h.parse().ok()?,
                margin_left: ml.parse().ok()?,
                margin_right: mr.parse().ok()?,
                normal: PathBuf::from(normal),
                hover: PathBuf::from(hover),
                backdrop: PathBuf::from(backdrop),
                ..Button::default()
            }),
            ["button", name, w, h, ml, mr, mt, mb, normal, hover, backdrop] => bar.buttons.push(Button {
                name: name.to_string(),
                width: w.parse().ok()?,
                height: h.parse().ok()?,
                margin_left: ml.parse().ok()?,
                margin_right: mr.parse().ok()?,
                margin_top: mt.parse().ok()?,
                margin_bottom: mb.parse().ok()?,
                normal: PathBuf::from(normal),
                hover: PathBuf::from(hover),
                backdrop: PathBuf::from(backdrop),
            }),
            _ => {}
        }
    }
    (colors == 6 && !bar.buttons.is_empty()).then_some(bar)
}

fn hex(value: &str) -> Option<Rgb> {
    let v = value.strip_prefix('#').filter(|v| v.len() == 6)?;
    let byte = |i: usize| u8::from_str_radix(&v[i..i + 2], 16).ok();
    Some((byte(0)?, byte(2)?, byte(4)?))
}

/// The child process's work: render the title bar of the GTK theme in
/// use under `dir` (a folder per theme) and answer it as [`parse`] reads.
#[cfg(all(unix, not(target_os = "macos")))]
pub fn export(dir: &Path, dark: bool) -> Result<String, String> {
    gtk::export(dir, dark)
}

#[cfg(not(all(unix, not(target_os = "macos"))))]
pub fn export(_dir: &Path, _dark: bool) -> Result<String, String> {
    Err("GTK title bars are for Linux".into())
}

#[cfg(all(unix, not(target_os = "macos")))]
mod gtk {
    //! GTK 3 through `dlopen`, the calls Chromium's `ui/gtk` makes on
    //! its GTK 3 path (gtk_util.cc, nav_button_provider_gtk.cc,
    //! gtk_color_mixers.cc).

    use std::ffi::{c_char, c_double, c_int, c_uint, c_void, CStr, CString};
    use std::fmt::Write as _;
    use std::path::Path;

    type Ptr = *mut c_void;

    #[repr(C)]
    #[derive(Default)]
    struct Border {
        left: i16,
        right: i16,
        top: i16,
        bottom: i16,
    }

    #[repr(C)]
    #[derive(Default)]
    struct Rgba {
        red: c_double,
        green: c_double,
        blue: c_double,
        alpha: c_double,
    }

    const STATE_NORMAL: c_uint = 0;
    const STATE_ACTIVE: c_uint = 1 << 0;
    const STATE_PRELIGHT: c_uint = 1 << 1;
    const STATE_SELECTED: c_uint = 1 << 2;
    const STATE_INSENSITIVE: c_uint = 1 << 3;
    const STATE_FOCUSED: c_uint = 1 << 5;
    const STATE_BACKDROP: c_uint = 1 << 6;
    const STATE_CHECKED: c_uint = 1 << 11;
    /// G_TYPE_NONE: G_TYPE_MAKE_FUNDAMENTAL(1)
    const TYPE_NONE: usize = 1 << 2;
    const LOOKUP_USE_BUILTIN: c_uint = 1 << 2;
    const LOOKUP_GENERIC_FALLBACK: c_uint = 1 << 3;
    const FORMAT_ARGB32: c_int = 0;
    /// Pictures at twice the size, for sharpness at any scale up to 2.
    const SCALE: c_int = 2;
    /// gtkheaderbar.c: GTK_ICON_SIZE_MENU.
    const ICON_SIZE: c_int = 16;
    /// The GtkHeaderBar spacing between its children.
    const HEADER_SPACING: u32 = 6;

    /// Declares the functions and loads them all, or fails.
    macro_rules! api {
        (plain { $($name:ident: fn($($arg:ty),*) $(-> $ret:ty)?;)* } variadic { $($vname:ident: fn($($varg:ty),*);)* }) => {
            #[allow(non_snake_case)]
            struct Api {
                $($name: unsafe extern "C" fn($($arg),*) $(-> $ret)?,)*
                $($vname: unsafe extern "C" fn($($varg),*, ...),)*
            }
            impl Api {
                fn load(lib: Ptr) -> Result<Api, String> {
                    Ok(Api {
                        $($name: {
                            let f = sym(lib, stringify!($name))?;
                            // SAFETY: the symbol is GTK 3's (or a library it
                            // links) of this name, whose C signature is the
                            // one declared here
                            unsafe { std::mem::transmute::<Ptr, unsafe extern "C" fn($($arg),*) $(-> $ret)?>(f) }
                        },)*
                        $($vname: {
                            let f = sym(lib, stringify!($vname))?;
                            // SAFETY: as above, a C variadic function
                            unsafe { std::mem::transmute::<Ptr, unsafe extern "C" fn($($varg),*, ...)>(f) }
                        },)*
                    })
                }
            }
        };
    }

    fn sym(lib: Ptr, name: &str) -> Result<Ptr, String> {
        let c = CString::new(name).map_err(|e| e.to_string())?;
        // SAFETY: `lib` is a live dlopen handle; dlsym also searches the
        // libraries it depends on (GDK, GObject, cairo)
        let f = unsafe { libc::dlsym(lib, c.as_ptr()) };
        if f.is_null() {
            Err(format!("no {name} in GTK"))
        } else {
            Ok(f)
        }
    }

    api! {
        plain {
        gtk_init_check: fn(*mut c_int, *mut *mut *mut c_char) -> c_int;
        gtk_settings_get_default: fn() -> Ptr;
        gtk_widget_path_new: fn() -> Ptr;
        gtk_widget_path_copy: fn(Ptr) -> Ptr;
        gtk_widget_path_append_type: fn(Ptr, usize) -> c_int;
        gtk_widget_path_iter_set_object_name: fn(Ptr, c_int, *const c_char);
        gtk_widget_path_iter_add_class: fn(Ptr, c_int, *const c_char);
        gtk_widget_path_iter_set_state: fn(Ptr, c_int, c_uint);
        gtk_widget_path_unref: fn(Ptr);
        gtk_style_context_new: fn() -> Ptr;
        gtk_style_context_get_path: fn(Ptr) -> Ptr;
        gtk_style_context_set_path: fn(Ptr, Ptr);
        gtk_style_context_set_state: fn(Ptr, c_uint);
        gtk_style_context_get_state: fn(Ptr) -> c_uint;
        gtk_style_context_set_scale: fn(Ptr, c_int);
        gtk_style_context_set_parent: fn(Ptr, Ptr);
        gtk_style_context_add_class: fn(Ptr, *const c_char);
        gtk_style_context_add_provider: fn(Ptr, Ptr, c_uint);
        gtk_style_context_get_padding: fn(Ptr, c_uint, *mut Border);
        gtk_style_context_get_border: fn(Ptr, c_uint, *mut Border);
        gtk_style_context_get_margin: fn(Ptr, c_uint, *mut Border);
        gtk_style_context_get_color: fn(Ptr, c_uint, *mut Rgba);
        gtk_css_provider_new: fn() -> Ptr;
        gtk_css_provider_load_from_data: fn(Ptr, *const c_char, isize, *mut Ptr) -> c_int;
        gtk_render_background: fn(Ptr, Ptr, c_double, c_double, c_double, c_double);
        gtk_render_frame: fn(Ptr, Ptr, c_double, c_double, c_double, c_double);
        gtk_render_icon: fn(Ptr, Ptr, Ptr, c_double, c_double);
        gtk_icon_theme_get_default: fn() -> Ptr;
        gtk_icon_theme_lookup_icon_for_scale: fn(Ptr, *const c_char, c_int, c_int, c_uint) -> Ptr;
        gtk_icon_info_load_symbolic_for_context: fn(Ptr, Ptr, *mut c_int, *mut Ptr) -> Ptr;
        gdk_pixbuf_get_width: fn(Ptr) -> c_int;
        gdk_pixbuf_get_height: fn(Ptr) -> c_int;
        cairo_image_surface_create: fn(c_int, c_int, c_int) -> Ptr;
        cairo_image_surface_get_data: fn(Ptr) -> *mut u8;
        cairo_image_surface_get_stride: fn(Ptr) -> c_int;
        cairo_surface_set_device_scale: fn(Ptr, c_double, c_double);
        cairo_surface_flush: fn(Ptr);
        cairo_surface_write_to_png: fn(Ptr, *const c_char) -> c_int;
        cairo_surface_destroy: fn(Ptr);
        cairo_create: fn(Ptr) -> Ptr;
        cairo_destroy: fn(Ptr);
        cairo_save: fn(Ptr);
        cairo_restore: fn(Ptr);
        cairo_scale: fn(Ptr, c_double, c_double);
        g_object_unref: fn(Ptr);
        }
        variadic {
            g_object_set: fn(Ptr, *const c_char);
            g_object_get: fn(Ptr, *const c_char);
            gtk_style_context_get: fn(Ptr, c_uint);
        }
    }

    fn cstr(s: &str) -> CString {
        CString::new(s).unwrap_or_default()
    }

    /// A style context and the ones above it (root first), as Chromium's
    /// GtkCssContext keeps its parent.
    #[derive(Clone)]
    struct Context(Vec<Ptr>);

    impl Context {
        fn leaf(&self) -> Ptr {
            *self.0.last().expect("a context has a node")
        }
    }

    struct Gtk {
        api: Api,
    }

    impl Gtk {
        /// `AppendCssNodeToStyleContext`: a node like
        /// `button.titlebutton.close:hover` under `parent`.
        fn append(&self, parent: Option<&Context>, node: &str) -> Context {
            let a = &self.api;
            let (object_name, classes, state) = parse_node(node);
            // SAFETY: GTK 3 calls on objects GTK made in this process,
            // on the thread that initialised it
            unsafe {
                let path = match parent {
                    Some(p) => (a.gtk_widget_path_copy)((a.gtk_style_context_get_path)(p.leaf())),
                    None => (a.gtk_widget_path_new)(),
                };
                (a.gtk_widget_path_append_type)(path, TYPE_NONE);
                if !object_name.is_empty() {
                    (a.gtk_widget_path_iter_set_object_name)(path, -1, cstr(&object_name).as_ptr());
                }
                for class in &classes {
                    (a.gtk_widget_path_iter_add_class)(path, -1, cstr(class).as_ptr());
                }
                (a.gtk_widget_path_iter_set_state)(path, -1, state);
                let context = (a.gtk_style_context_new)();
                (a.gtk_style_context_set_path)(context, path);
                (a.gtk_style_context_set_state)(context, state);
                (a.gtk_style_context_set_scale)(context, 1);
                if let Some(p) = parent {
                    (a.gtk_style_context_set_parent)(context, p.leaf());
                }
                (a.gtk_widget_path_unref)(path);
                let mut chain = parent.map(|p| p.0.clone()).unwrap_or_default();
                chain.push(context);
                Context(chain)
            }
        }

        /// `GetStyleContextFromCss`: under `window.background`.
        fn css_context(&self, selector: &str) -> Context {
            let mut context = self.append(None, "window.background");
            for node in selector.split_whitespace() {
                context = self.append(Some(&context), node);
            }
            context
        }

        fn state(&self, c: Ptr) -> c_uint {
            // SAFETY: a live style context
            unsafe { (self.api.gtk_style_context_get_state)(c) }
        }

        fn padding(&self, c: Ptr) -> Border {
            let mut b = Border::default();
            // SAFETY: a live style context, a GtkBorder to fill
            unsafe { (self.api.gtk_style_context_get_padding)(c, self.state(c), &mut b) };
            b
        }

        fn border(&self, c: Ptr) -> Border {
            let mut b = Border::default();
            // SAFETY: as padding
            unsafe { (self.api.gtk_style_context_get_border)(c, self.state(c), &mut b) };
            b
        }

        fn margin(&self, c: Ptr) -> Border {
            let mut b = Border::default();
            // SAFETY: as padding
            unsafe { (self.api.gtk_style_context_get_margin)(c, self.state(c), &mut b) };
            b
        }

        /// `GetMinimumContentSize`: CSS min-width and min-height.
        fn min_size(&self, c: Ptr) -> (i32, i32) {
            let (mut w, mut h): (c_int, c_int) = (0, 0);
            // SAFETY: gtk_style_context_get(context, state, name, &value,
            // ..., NULL) with int properties
            unsafe {
                (self.api.gtk_style_context_get)(
                    c,
                    self.state(c),
                    c"min-width".as_ptr(),
                    &mut w as *mut c_int,
                    c"min-height".as_ptr(),
                    &mut h as *mut c_int,
                    std::ptr::null::<c_char>(),
                )
            };
            (w, h)
        }

        /// `GetMinimumWidgetSize`: content (inflated by its own margin),
        /// at least the widget's minimum, plus its padding and border.
        fn widget_size(&self, content: (i32, i32), content_ctx: Option<Ptr>, widget: Ptr) -> (i32, i32) {
            let (mut w, mut h) = content;
            if let Some(c) = content_ctx {
                let m = self.margin(c);
                w += i32::from(m.left + m.right);
                h += i32::from(m.top + m.bottom);
            }
            let (mw, mh) = self.min_size(widget);
            let (p, b) = (self.padding(widget), self.border(widget));
            (
                w.max(mw) + i32::from(p.left + p.right + b.left + b.right),
                h.max(mh) + i32::from(p.top + p.bottom + b.top + b.bottom),
            )
        }

        /// `ApplyCssToContext`: a provider over the context and its parents.
        fn apply_css(&self, context: &Context, css: &str) {
            let a = &self.api;
            // SAFETY: a new provider loaded with CSS text, added to live
            // contexts at the highest priority
            unsafe {
                let provider = (a.gtk_css_provider_new)();
                let css = cstr(css);
                (a.gtk_css_provider_load_from_data)(provider, css.as_ptr(), -1, std::ptr::null_mut());
                for c in &context.0 {
                    (a.gtk_style_context_add_provider)(*c, provider, u32::MAX);
                }
            }
        }

        /// `GetBgColorFromStyleContext`: the backgrounds of the context
        /// and its parents, borders stripped, rendered into 24x24 and
        /// averaged (a theme may leave background-color unused under an
        /// image).
        fn bg_color(&self, context: &Context) -> (u8, u8, u8, u8) {
            self.apply_css(context, "* { border-radius: 0px; border-style: none; box-shadow: none; }");
            self.average(24, 24, |cr| {
                for c in &context.0 {
                    // SAFETY: a live context and cairo context
                    unsafe { (self.api.gtk_render_background)(*c, cr, 0., 0., 24., 24.) };
                }
            })
        }

        /// `GetFgColor`: the color property, over the background where it
        /// is translucent.
        fn fg_color(&self, context: &Context) -> (u8, u8, u8) {
            let mut c = Rgba::default();
            // SAFETY: a live context and a GdkRGBA to fill
            unsafe { (self.api.gtk_style_context_get_color)(context.leaf(), self.state(context.leaf()), &mut c) };
            let fg = [c.red, c.green, c.blue].map(|v| v.clamp(0., 1.));
            if c.alpha >= 1. {
                return rgb8(fg);
            }
            let (r, g, b, _) = self.bg_color(context);
            let bg = [r, g, b].map(|v| f64::from(v) / 255.);
            let a = c.alpha.clamp(0., 1.);
            rgb8([0, 1, 2].map(|i| fg[i] * a + bg[i] * (1. - a)))
        }

        /// Paints into a transparent ARGB surface and averages it as
        /// `CairoSurface::GetAveragePixelValue(false)`.
        fn average(&self, w: c_int, h: c_int, paint: impl FnOnce(Ptr)) -> (u8, u8, u8, u8) {
            let a = &self.api;
            // SAFETY: a new cairo image surface, read after painting and
            // freed here
            unsafe {
                let surface = (a.cairo_image_surface_create)(FORMAT_ARGB32, w, h);
                let cr = (a.cairo_create)(surface);
                paint(cr);
                (a.cairo_destroy)(cr);
                (a.cairo_surface_flush)(surface);
                let data = (a.cairo_image_surface_get_data)(surface);
                let stride = (a.cairo_image_surface_get_stride)(surface) as usize;
                let (mut sa, mut sr, mut sg, mut sb) = (0u64, 0u64, 0u64, 0u64);
                for y in 0..h as usize {
                    let row = std::slice::from_raw_parts(data.add(y * stride), w as usize * 4);
                    // premultiplied ARGB32 in native (little-endian) order: B G R A
                    for px in row.as_chunks::<4>().0 {
                        sb += u64::from(px[0]);
                        sg += u64::from(px[1]);
                        sr += u64::from(px[2]);
                        sa += u64::from(px[3]);
                    }
                }
                (a.cairo_surface_destroy)(surface);
                if sa == 0 {
                    return (0, 0, 0, 0);
                }
                let n = (w * h) as u64;
                ((sr * 255 / sa) as u8, (sg * 255 / sa) as u8, (sb * 255 / sa) as u8, (sa / n) as u8)
            }
        }

        /// Paints `w` by `h` (at `scale`) into a transparent surface and
        /// returns each pixel's alpha, row by row.
        fn paint_alpha(&self, w: c_int, h: c_int, scale: c_int, paint: impl FnOnce(Ptr)) -> Result<Vec<u8>, String> {
            self.paint_impl(w, h, scale, None, paint)
        }

        /// As `paint_alpha`, and writes the picture to `file` (PNG).
        fn paint(
            &self,
            w: c_int,
            h: c_int,
            scale: c_int,
            file: &Path,
            paint: impl FnOnce(Ptr),
        ) -> Result<Vec<u8>, String> {
            self.paint_impl(w, h, scale, Some(file), paint)
        }

        fn paint_impl(
            &self,
            w: c_int,
            h: c_int,
            scale: c_int,
            file: Option<&Path>,
            paint: impl FnOnce(Ptr),
        ) -> Result<Vec<u8>, String> {
            let a = &self.api;
            // SAFETY: a new cairo image surface, painted, read and freed here
            unsafe {
                let (pw, ph) = (scale * w, scale * h);
                let surface = (a.cairo_image_surface_create)(FORMAT_ARGB32, pw, ph);
                (a.cairo_surface_set_device_scale)(surface, f64::from(scale), f64::from(scale));
                let cr = (a.cairo_create)(surface);
                paint(cr);
                (a.cairo_destroy)(cr);
                (a.cairo_surface_flush)(surface);
                let data = (a.cairo_image_surface_get_data)(surface);
                let stride = (a.cairo_image_surface_get_stride)(surface) as usize;
                let mut alpha = Vec::with_capacity((pw * ph) as usize);
                for y in 0..ph as usize {
                    let row = std::slice::from_raw_parts(data.add(y * stride), pw as usize * 4);
                    // ARGB32 in native (little-endian) order: B G R A
                    alpha.extend(row.as_chunks::<4>().0.iter().map(|px| px[3]));
                }
                let status = match file {
                    Some(file) => (a.cairo_surface_write_to_png)(surface, cstr(&file.to_string_lossy()).as_ptr()),
                    None => 0,
                };
                (a.cairo_surface_destroy)(surface);
                if status != 0 {
                    return Err(format!(
                        "could not write {}",
                        file.map(|f| f.display().to_string()).unwrap_or_default()
                    ));
                }
                Ok(alpha)
            }
        }

        fn icon(&self, name: &str, context: Ptr, scale: c_int) -> Ptr {
            let a = &self.api;
            // SAFETY: the default icon theme, an icon looked up in it and
            // loaded as a symbolic icon coloured for `context`
            unsafe {
                let info = (a.gtk_icon_theme_lookup_icon_for_scale)(
                    (a.gtk_icon_theme_get_default)(),
                    cstr(name).as_ptr(),
                    ICON_SIZE,
                    scale,
                    LOOKUP_USE_BUILTIN | LOOKUP_GENERIC_FALLBACK,
                );
                if info.is_null() {
                    return std::ptr::null_mut();
                }
                let pixbuf = (a.gtk_icon_info_load_symbolic_for_context)(
                    info,
                    context,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                );
                (a.g_object_unref)(info);
                pixbuf
            }
        }

        fn string_setting(&self, name: &CStr) -> String {
            let mut value: *mut c_char = std::ptr::null_mut();
            // SAFETY: g_object_get(settings, name, &char*, NULL) of a
            // string property; the copy is GTK's to leak in this short
            // child process
            unsafe {
                (self.api.g_object_get)(
                    (self.api.gtk_settings_get_default)(),
                    name.as_ptr(),
                    &mut value as *mut *mut c_char,
                    std::ptr::null::<c_char>(),
                );
                if value.is_null() {
                    String::new()
                } else {
                    CStr::from_ptr(value).to_string_lossy().into_owned()
                }
            }
        }
    }

    fn rgb8(v: [f64; 3]) -> (u8, u8, u8) {
        let b = |x: f64| (x * 255.).round() as u8;
        (b(v[0]), b(v[1]), b(v[2]))
    }

    /// A node `object.class.class:pseudo`: its object name, classes and
    /// state flags (the pseudo-classes GTK 3 knows).
    pub(super) fn parse_node(node: &str) -> (String, Vec<String>, c_uint) {
        let mut object_name = String::new();
        let mut classes = Vec::new();
        let mut state = STATE_NORMAL;
        let mut kind = 'o';
        let mut token = String::new();
        let mut flush = |kind: char, token: &mut String| match kind {
            'o' => object_name = std::mem::take(token),
            '.' => classes.push(std::mem::take(token)),
            ':' => {
                state |= match token.as_str() {
                    "active" => STATE_ACTIVE,
                    "hover" => STATE_PRELIGHT,
                    "selected" => STATE_SELECTED,
                    "disabled" => STATE_INSENSITIVE,
                    "focus" => STATE_FOCUSED,
                    "backdrop" => STATE_BACKDROP,
                    "checked" => STATE_CHECKED,
                    _ => 0,
                };
                token.clear();
            }
            _ => token.clear(),
        };
        for ch in node.chars() {
            if ch == '.' || ch == ':' {
                flush(kind, &mut token);
                kind = ch;
            } else {
                token.push(ch);
            }
        }
        flush(kind, &mut token);
        (object_name, classes, state)
    }

    fn hex((r, g, b): (u8, u8, u8)) -> String {
        format!("#{r:02x}{g:02x}{b:02x}")
    }

    /// A theme name as a folder name.
    fn folder(name: &str) -> String {
        name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect()
    }

    pub(super) fn export(dir: &Path, dark: bool) -> Result<String, String> {
        // SAFETY: loading GTK 3 by its soname; RTLD_GLOBAL so its own
        // modules (theme engines) find it
        let lib = unsafe { libc::dlopen(c"libgtk-3.so.0".as_ptr(), libc::RTLD_NOW | libc::RTLD_GLOBAL) };
        if lib.is_null() {
            return Err("no GTK 3 (libgtk-3.so.0)".into());
        }
        let gtk = Gtk { api: Api::load(lib)? };
        let a = &gtk.api;
        // SAFETY: GTK's own initialisation, without arguments, on this
        // (the child process's main) thread
        if unsafe { (a.gtk_init_check)(std::ptr::null_mut(), std::ptr::null_mut()) } == 0 {
            return Err("GTK could not start (no display)".into());
        }
        // Chrome's SetDarkTheme: GTK 3 follows no colour-scheme itself
        // SAFETY: g_object_set(settings, name, gboolean, NULL)
        unsafe {
            (a.g_object_set)(
                (a.gtk_settings_get_default)(),
                c"gtk-application-prefer-dark-theme".as_ptr(),
                c_int::from(dark),
                std::ptr::null::<c_char>(),
            )
        };
        let theme = gtk.string_setting(c"gtk-theme-name");
        let icons = gtk.string_setting(c"gtk-icon-theme-name");
        let out_dir =
            dir.join(format!("{}-{}-{}", folder(&theme), folder(&icons), if dark { "dark" } else { "light" }));
        std::fs::create_dir_all(&out_dir).map_err(|e| e.to_string())?;

        let mut out = String::new();
        let header_sel = "headerbar.header-bar.titlebar";
        let color = |out: &mut String, name: &str, rgb: (u8, u8, u8)| {
            let _ = writeln!(out, "color\t{name}\t{}", hex(rgb));
        };
        let opaque = |(r, g, b, _): (u8, u8, u8, u8)| (r, g, b);
        color(&mut out, "frame", opaque(gtk.bg_color(&gtk.css_context(header_sel))));
        color(&mut out, "frame_inactive", opaque(gtk.bg_color(&gtk.css_context(&format!("{header_sel}:backdrop")))));
        color(&mut out, "window", opaque(gtk.bg_color(&gtk.css_context(""))));
        color(&mut out, "text", gtk.fg_color(&gtk.css_context("label")));
        color(&mut out, "title", gtk.fg_color(&gtk.css_context(&format!("{header_sel} label.title"))));
        // Chrome: :backdrop on every node, so a theme cascading the
        // title's colour from .background:backdrop applies
        let window = gtk.append(None, "window.background:backdrop");
        let header = gtk.append(Some(&window), &format!("{header_sel}:backdrop"));
        let label = gtk.append(Some(&header), "label.title:backdrop");
        color(&mut out, "title_inactive", gtk.fg_color(&label));

        // the title bar's nodes, as nav_button_provider_gtk.cc builds them
        let header = gtk.append(Some(&gtk.append(None, "window.background.csd")), header_sel);
        let padding = gtk.padding(header.leaf());
        let _ = writeln!(
            out,
            "header\t{}\t{}\t{}\t{}\t{}",
            padding.left.max(0),
            padding.right.max(0),
            HEADER_SPACING,
            padding.top.max(0),
            padding.bottom.max(0)
        );
        let controls = gtk.append(Some(&header), "windowcontrols");
        for (name, class, icon) in [
            ("close", "close", "window-close-symbolic"),
            ("minimize", "minimize", "window-minimize-symbolic"),
            ("maximize", "maximize", "window-maximize-symbolic"),
            ("restore", "maximize", "window-restore-symbolic"),
        ] {
            // the size of a button at rest (views::ImageButton wants one
            // size for every state)
            let button = gtk.append(Some(&controls), &format!("button.titlebutton.{class}"));
            let pixbuf = gtk.icon(icon, button.leaf(), 1);
            if pixbuf.is_null() {
                continue;
            }
            // SAFETY: a pixbuf GTK just made
            let icon_size = unsafe { ((a.gdk_pixbuf_get_width)(pixbuf), (a.gdk_pixbuf_get_height)(pixbuf)) };
            let image = gtk.append(Some(&button), "image");
            let image_size = gtk.widget_size(icon_size, None, image.leaf());
            let (w, h) = gtk.widget_size(image_size, Some(image.leaf()), button.leaf());
            let margin = gtk.margin(button.leaf());
            let mut files = Vec::new();
            for (state_name, flags) in
                [("normal", STATE_NORMAL), ("hover", STATE_PRELIGHT), ("backdrop", STATE_BACKDROP)]
            {
                let file = out_dir.join(format!("{name}-{state_name}.png"));
                render_button(&gtk, &controls, class, icon, flags, (w, h), &file)?;
                files.push(file);
            }
            let _ = writeln!(
                out,
                "button\t{name}\t{w}\t{h}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                margin.left.max(0),
                margin.right.max(0),
                margin.top.max(0),
                margin.bottom.max(0),
                files[0].display(),
                files[1].display(),
                files[2].display()
            );
        }
        if let Some(line) = edge(&gtk, &out_dir)? {
            out.push_str(&line);
        }
        Ok(out)
    }

    /// Chromium's `kMaxFrameSizeDip`: the most a theme's decoration may
    /// reach outside the window.
    const MAX_FRAME: i32 = 64;
    /// Chromium's `kMaxCornerRadiusDip`.
    const MAX_RADIUS: i32 = 32;

    /// `DecorationContext` (GTK 3): the window's `decoration` node, its
    /// bottom corners square (the content covers them).
    fn decoration(gtk: &Gtk, focused: bool) -> Context {
        let window = gtk.append(None, if focused { "window.background.csd" } else { "window.background.csd:backdrop" });
        let decoration = gtk.append(Some(&window), if focused { "decoration" } else { "decoration:backdrop" });
        gtk.apply_css(&decoration, "* { border-bottom-left-radius: 0; border-bottom-right-radius: 0; }");
        decoration
    }

    /// `WindowFrameProviderGtk::GetOrCreateAsset` at scale 2, then
    /// `GetFrameThicknessDip` and `ComputeTopCornerRadius`: the edge line,
    /// or none where the theme draws nothing around a window.
    fn edge(gtk: &Gtk, out_dir: &Path) -> Result<Option<String>, String> {
        let side = 4 * MAX_FRAME;
        let mut files = Vec::new();
        let mut thickness = [0u32; 4];
        for focused in [true, false] {
            let context = decoration(gtk, focused);
            let (p, b) = (gtk.padding(context.leaf()), gtk.border(context.leaf()));
            // the window in the middle, grown by the decoration's own
            // padding and border
            let x = f64::from(MAX_FRAME) - f64::from(p.left + b.left);
            let y = f64::from(MAX_FRAME) - f64::from(p.top + b.top);
            let w = f64::from(2 * MAX_FRAME) + f64::from(p.left + p.right + b.left + b.right);
            let h = f64::from(2 * MAX_FRAME) + f64::from(p.top + p.bottom + b.top + b.bottom);
            let file = out_dir.join(if focused { "edge-focused.png" } else { "edge-unfocused.png" });
            let alpha = gtk.paint(side, side, SCALE, &file, |cr| {
                // SAFETY: a live context and cairo context
                unsafe {
                    (gtk.api.gtk_render_background)(context.leaf(), cr, x, y, w, h);
                    (gtk.api.gtk_render_frame)(context.leaf(), cr, x, y, w, h);
                }
            })?;
            if focused {
                // how far the drawing reaches out from the window, along
                // the middle of each side
                let px = (SCALE * side) as usize;
                let frame = (SCALE * MAX_FRAME) as usize;
                let at = |x: usize, y: usize| alpha[y * px + x];
                let inset = |pixel: &dyn Fn(usize) -> u8| {
                    (0..frame).find(|&i| pixel(i) != 0).map_or(0, |i| (frame - i) as u32).div_ceil(SCALE as u32)
                };
                let mid = 2 * frame;
                thickness = [
                    inset(&|i| at(mid, i)),
                    inset(&|i| at(px - 1 - i, mid)),
                    inset(&|i| at(mid, px - 1 - i)),
                    inset(&|i| at(i, mid)),
                ];
            }
            files.push(file);
        }
        if thickness == [0; 4] {
            return Ok(None);
        }
        Ok(Some(format!(
            "edge\t{}\t{}\t{}\t{}\t{}\t{MAX_FRAME}\t{}\t{}\n",
            thickness[0],
            thickness[1],
            thickness[2],
            thickness[3],
            top_corner_radius(gtk),
            files[0].display(),
            files[1].display()
        )))
    }

    /// `ComputeTopCornerRadius` (GTK 3): the header bar painted black
    /// with only its top-left corner rounded, and the first pixel along
    /// its edges that is fully covered.
    fn top_corner_radius(gtk: &Gtk) -> u32 {
        let window = gtk.append(None, "window.background.csd:backdrop");
        let header = gtk.append(Some(&window), "headerbar.header-bar.titlebar:backdrop");
        gtk.apply_css(
            &header,
            "window, headerbar { background-image: none; background-color: black; box-shadow: none; border: none; \
             border-bottom-left-radius: 0; border-bottom-right-radius: 0; border-top-right-radius: 0; }",
        );
        let size = MAX_RADIUS;
        let Ok(alpha) = gtk.paint_alpha(size, size, 1, |cr| {
            // SAFETY: a live context and cairo context
            unsafe { (gtk.api.gtk_render_background)(header.leaf(), cr, 0., 0., f64::from(size), f64::from(size)) };
        }) else {
            return 0;
        };
        let n = size as usize;
        (0..n).find(|&i| alpha[i * n] == 255 && alpha[i] == 255).unwrap_or(n) as u32
    }

    /// `NavButtonImageSource::GetImageForScale` at scale 2: the button's
    /// background and frame, then its icon centred, as GTK draws them.
    fn render_button(
        gtk: &Gtk,
        controls: &Context,
        class: &str,
        icon: &str,
        state: c_uint,
        (w, h): (i32, i32),
        file: &Path,
    ) -> Result<(), String> {
        let a = &gtk.api;
        let button = gtk.append(Some(controls), "button.titlebutton");
        let leaf = button.leaf();
        // SAFETY: GTK and cairo calls on objects made here, freed here
        unsafe {
            (a.gtk_style_context_set_scale)(leaf, SCALE);
            (a.gtk_style_context_add_class)(leaf, cstr(class).as_ptr());
            (a.gtk_style_context_set_state)(leaf, state);
            let image = gtk.append(Some(&button), "image");
            (a.gtk_style_context_set_scale)(image.leaf(), SCALE);
            let pixbuf = gtk.icon(icon, image.leaf(), SCALE);

            let surface = (a.cairo_image_surface_create)(FORMAT_ARGB32, SCALE * w, SCALE * h);
            let cr = (a.cairo_create)(surface);
            (a.cairo_surface_set_device_scale)(surface, f64::from(SCALE), f64::from(SCALE));
            (a.cairo_save)(cr);
            (a.gtk_render_background)(leaf, cr, 0., 0., f64::from(w), f64::from(h));
            (a.gtk_render_frame)(leaf, cr, 0., 0., f64::from(w), f64::from(h));
            (a.cairo_restore)(cr);
            if !pixbuf.is_null() {
                (a.cairo_save)(cr);
                // the pixbuf is at the full resolution already
                (a.cairo_scale)(cr, 1. / f64::from(SCALE), 1. / f64::from(SCALE));
                let (pw, ph) = ((a.gdk_pixbuf_get_width)(pixbuf), (a.gdk_pixbuf_get_height)(pixbuf));
                (a.gtk_render_icon)(leaf, cr, pixbuf, f64::from((SCALE * w - pw) / 2), f64::from((SCALE * h - ph) / 2));
                (a.cairo_restore)(cr);
            }
            (a.cairo_destroy)(cr);
            let path = cstr(&file.to_string_lossy());
            let status = (a.cairo_surface_write_to_png)(surface, path.as_ptr());
            (a.cairo_surface_destroy)(surface);
            if status != 0 {
                return Err(format!("could not write {}", file.display()));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toolkits_as_chrome_picks_them() {
        assert_eq!(toolkit("Pantheon", ""), Toolkit::Gtk);
        assert_eq!(toolkit("ubuntu:GNOME", ""), Toolkit::Gtk);
        assert_eq!(toolkit("zorin:GNOME", ""), Toolkit::Gtk);
        assert_eq!(toolkit("X-Cinnamon", ""), Toolkit::Gtk);
        assert_eq!(toolkit("KDE", ""), Toolkit::Qt);
        assert_eq!(toolkit("Deepin", ""), Toolkit::Qt);
        assert_eq!(toolkit("DDE", ""), Toolkit::Gtk, "deepin 23 says DDE, which Chrome does not know");
        assert_eq!(toolkit("", "deepin"), Toolkit::Qt);
        assert_eq!(toolkit("", ""), Toolkit::Gtk, "Chrome's default");
    }

    /// What this desktop gives: run in its session's environment with
    /// `cargo test -p native-term-os -- --ignored --nocapture this_desktop`.
    #[test]
    #[ignore]
    fn this_desktop_titlebar() {
        let var = |name: &str| std::env::var(name).unwrap_or_default();
        let kit = toolkit(&var("XDG_CURRENT_DESKTOP"), &var("DESKTOP_SESSION"));
        println!("toolkit: {kit:?}");
        if kit == Toolkit::Qt {
            println!("qt: {:?}", read_qt());
        }
    }

    #[test]
    fn kdes_palette() {
        // EndeavourOS, Breeze Dark (Plasma 6): header colours
        let breeze_dark = "[Colors:Button]\nBackgroundNormal=41,44,48\nForegroundNormal=252,252,252\n\
            [Colors:Header]\nBackgroundNormal=41,44,48\nForegroundNormal=252,252,252\n\
            [Colors:Header][Inactive]\nBackgroundNormal=32,35,38\nForegroundNormal=161,169,177\n\
            [Colors:Window]\nBackgroundNormal=32,35,38\nForegroundNormal=252,252,252\n\
            [WM]\nactiveBackground=39,44,49\n";
        let bar = qt_colors(breeze_dark).unwrap();
        assert_eq!((bar.frame, bar.frame_inactive), ((41, 44, 48), (32, 35, 38)), "the header, not [WM]");
        assert_eq!((bar.window, bar.text), ((32, 35, 38), (252, 252, 252)), "the active tab: the window's");
        assert_eq!((bar.title, bar.title_inactive), ((252, 252, 252), (161, 169, 177)));
        assert!(bar.buttons.is_empty() && bar.edge.is_none(), "Qt: colours only");
        // Lingmo: no header colours, [WM] with an alpha
        let lingmo = "[Colors:Button]\nBackgroundNormal=250,250,250\n[Colors:Window]\nBackgroundNormal=240,240,240\n\
            ForegroundNormal=48,48,48\n[WM]\nactiveBackground=240,240,240,204\nactiveForeground=48,48,48\n\
            inactiveBackground=240,240,240,204\ninactiveForeground=96,96,96\n";
        let bar = qt_colors(lingmo).unwrap();
        assert_eq!((bar.frame, bar.title_inactive), ((240, 240, 240), (96, 96, 96)));
        assert_eq!(bar.window, (240, 240, 240));
        assert_eq!(qt_colors("[General]\nName=x\n"), None, "no palette");
    }

    #[test]
    fn the_childs_answer() {
        let text = "color\tframe\t#2e2e2e\ncolor\tframe_inactive\t#303030\ncolor\twindow\t#242424\n\
            color\ttext\t#ffffff\ncolor\ttitle\t#fafafa\ncolor\ttitle_inactive\t#909090\n\
            header\t6\t6\t6\n\
            button\tclose\t24\t24\t0\t0\t/t/close-normal.png\t/t/close hover.png\t/t/close-backdrop.png\n";
        let bar = parse(text).unwrap();
        assert_eq!(bar.frame, (0x2e, 0x2e, 0x2e));
        assert_eq!(bar.title_inactive, (0x90, 0x90, 0x90));
        assert_eq!((bar.padding_left, bar.spacing), (6, 6));
        assert_eq!(bar.buttons[0].name, "close");
        assert_eq!((bar.padding_top, bar.buttons[0].margin_top), (0, 0), "an older answer: nothing vertical");
        let vertical = text
            .replace("header\t6\t6\t6\n", "header\t6\t6\t6\t3\t4\n")
            .replace("\t24\t24\t0\t0\t", "\t24\t24\t0\t0\t1\t2\t");
        let bar = parse(&vertical).unwrap();
        assert_eq!((bar.padding_top, bar.padding_bottom), (3, 4));
        assert_eq!((bar.buttons[0].margin_top, bar.buttons[0].margin_bottom), (1, 2));
        assert_eq!(bar.buttons[0].hover, PathBuf::from("/t/close hover.png"), "spaces kept");
        assert_eq!(bar.edge, None, "a theme that draws no edge");
        let with_edge = format!("{text}edge\t2\t30\t40\t30\t8\t64\t/t/edge-focused.png\t/t/edge-unfocused.png\n");
        let edge = parse(&with_edge).unwrap().edge.unwrap();
        assert_eq!((edge.thickness, edge.radius, edge.slice), ([2, 30, 40, 30], 8, 64));
        assert_eq!(edge.unfocused, PathBuf::from("/t/edge-unfocused.png"));
        assert_eq!(parse("header\t6\t6\t6\n"), None, "no colours, no title bar");
        assert_eq!(parse(&text.replace("#2e2e2e", "#2e2e")), None, "a bad colour");
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn css_nodes() {
        let (object, classes, state) = gtk::parse_node("button.titlebutton.close:hover");
        assert_eq!(object, "button");
        assert_eq!(classes, ["titlebutton", "close"]);
        assert_eq!(state, 1 << 1);
        let (object, classes, state) = gtk::parse_node("headerbar.header-bar.titlebar:backdrop");
        assert_eq!((object.as_str(), classes.len(), state), ("headerbar", 2, 1 << 6));
        assert_eq!(gtk::parse_node("").0, "");
    }
}

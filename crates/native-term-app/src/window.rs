//! NativeTerm's windows: egui on winit, painted on the CPU.
//!
//! A GPU renderer cost 74–165 MB private memory for this small, mostly
//! idle window (the driver's share, measured with an empty window). The
//! CPU renderer (`egui_software_backend`) paints a frame in a few
//! milliseconds and presents it with GDI (`softbuffer`), with no GPU
//! memory at all. A window is repainted only on input and on
//! `request_repaint`.
//!
//! The main window (dockable, see `dock.rs`), the floating button (shown
//! while the docked main window is hidden), and windows opened while
//! running (`open`, e.g. a host's files). Each has its own egui context,
//! so they paint independently.
//!
//! The floating button's window is shown a second way: its frame carries
//! its own transparency (`native_term_os::layered`), so the button can
//! be a round, slightly see-through shape instead of a rectangle. The
//! renderer is the same; only the way the pixels reach the screen
//! differs.

use std::cell::RefCell;
use std::num::NonZeroU32;
use std::rc::Rc;
use std::time::{Duration, Instant};

use egui::ViewportId;
use egui_software_backend::{BufferMutRef, ColorFieldOrder, EguiSoftwareRender};
use egui_winit::accesskit_winit;
use native_term_os::dock as win;
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::window::{Window, WindowId};

use crate::dock::{self, Edge, Slide};
use crate::shell;

/// What a window shows.
pub trait Ui {
    fn ui(&mut self, ui: &mut egui::Ui);

    /// The main window is closing and the program ends.
    fn on_exit(&mut self) {}

    /// A window from `open`: its close button was clicked. False keeps it
    /// open (to ask first, e.g. while files are being transferred).
    fn close_requested(&mut self) -> bool {
        true
    }

    /// A window from `open` wants to close (checked after each frame).
    fn wants_close(&self) -> bool {
        false
    }
}

/// A window to open (see `open`).
struct Request {
    key: String,
    viewport: egui::ViewportBuilder,
    factory: Factory,
}

thread_local! {
    static REQUESTS: RefCell<Vec<Request>> = const { RefCell::new(Vec::new()) };
}

/// Opens another window, or brings the one with the same `key` to the
/// front. Called from a window's UI (the event loop's thread); the window
/// appears once the current frame is done. It ends when closed (its `Ui`
/// is dropped then) or with the main window.
pub fn open(
    key: impl Into<String>,
    viewport: egui::ViewportBuilder,
    factory: impl FnOnce(&egui::Context) -> Box<dyn Ui> + 'static,
) {
    let request = Request { key: key.into(), viewport, factory: Box::new(factory) };
    REQUESTS.with(|r| r.borrow_mut().push(request));
}

type Factory = Box<dyn FnOnce(&egui::Context) -> Box<dyn Ui>>;

/// Where the window was: position of its rectangle and inner size in
/// physical pixels, and the edge it was docked at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Placement {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub edge: Option<Edge>,
}

impl Placement {
    pub fn to_setting(self) -> String {
        format!("{},{},{},{},{}", self.x, self.y, self.width, self.height, self.edge.map_or("", |e| e.name()))
    }

    pub fn from_setting(text: &str) -> Option<Placement> {
        let mut parts = text.split(',');
        let mut num = || parts.next()?.trim().parse::<i64>().ok();
        let (x, y, width, height) = (num()?, num()?, num()?, num()?);
        let edge = text.rsplit(',').next().and_then(Edge::from_name);
        Some(Placement {
            x: i32::try_from(x).ok()?,
            y: i32::try_from(y).ok()?,
            width: u32::try_from(width).ok().filter(|w| *w >= 200)?,
            height: u32::try_from(height).ok().filter(|h| *h >= 200)?,
            edge,
        })
    }
}

pub type SavePlacement = Box<dyn Fn(Placement)>;

/// The floating button: what it shows, where it was, and how to remember
/// where it is.
pub struct FloatingButton {
    pub viewport: egui::ViewportBuilder,
    pub position: Option<(i32, i32)>,
    pub save: Box<dyn Fn(i32, i32)>,
    pub factory: Factory,
}

/// QQ-style docking state (see `dock.rs`).
#[derive(Default)]
struct Docking {
    edge: Option<Edge>,
    hidden: bool,
    /// The slide in progress, and whether it hides the window.
    slide: Option<(Slide, bool)>,
    /// When to check whether the pointer has left.
    leave_check: Option<Instant>,
    /// When to check where a move by the user ended.
    settle_check: Option<Instant>,
    /// Moves until then are NativeTerm's own.
    own_move_until: Option<Instant>,
    /// The pointer is over the client area (winit reports leaving it).
    cursor_in_client: bool,
    /// Brought out on request (the floating button): stays out until the
    /// pointer has been over it, or it loses the focus.
    until_visited: bool,
    /// Where the window was when it was hidden outright (see
    /// `Runner::set_hidden`), to put it back there.
    #[cfg(not(windows))]
    shown_at: Option<(i32, i32)>,
    /// Dragged against the top edge and maximized by the window manager
    /// for it (KWin does): unmaximized, then docked at the top.
    top_after_restore: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Which {
    Main,
    Button,
    /// A window from `open`, by its number.
    Extra(u64),
}

enum UserEvent {
    Repaint { which: Which, when: Instant, pass: u64 },
    AccessKit(accesskit_winit::Event),
}

impl From<accesskit_winit::Event> for UserEvent {
    fn from(event: accesskit_winit::Event) -> Self {
        UserEvent::AccessKit(event)
    }
}

/// One window and everything that paints it.
struct Pane {
    ctx: egui::Context,
    window: Rc<Window>,
    _context: softbuffer::Context<Rc<Window>>,
    surface: softbuffer::Surface<Rc<Window>, Rc<Window>>,
    renderer: EguiSoftwareRender,
    state: egui_winit::State,
    info: egui::ViewportInfo,
    ui: Box<dyn Ui>,
    /// Shown with its own transparency (the floating button).
    layered: Option<native_term_os::layered::Layered>,
    shown: bool,
    last_paint: Option<Instant>,
    repaint_at: Option<Instant>,
    hwnd: isize,
}

impl Pane {
    fn create(
        event_loop: &ActiveEventLoop,
        proxy: &EventLoopProxy<UserEvent>,
        which: Which,
        viewport: &egui::ViewportBuilder,
        factory: Factory,
        layered: bool,
        before_show: impl FnOnce(&Window),
    ) -> Result<Pane, String> {
        let ctx = egui::Context::default();
        let repaint_proxy = proxy.clone();
        ctx.set_request_repaint_callback(move |info| {
            let _ = repaint_proxy.send_event(UserEvent::Repaint {
                which,
                when: Instant::now() + info.delay,
                pass: info.current_cumulative_pass_nr,
            });
        });
        crate::install_fonts(&ctx);
        let window = Rc::new(egui_winit::create_window(&ctx, event_loop, viewport).map_err(|e| e.to_string())?);
        let hwnd = window_handle(&window).ok_or("no window handle")?;
        before_show(&window);
        let context = softbuffer::Context::new(Rc::clone(&window)).map_err(|e| e.to_string())?;
        let surface = softbuffer::Surface::new(&context, Rc::clone(&window)).map_err(|e| e.to_string())?;
        let mut state = egui_winit::State::new(
            ctx.clone(),
            ViewportId::ROOT,
            event_loop,
            Some(window.scale_factor() as f32),
            event_loop.system_theme(),
            None,
        );
        state.init_accesskit(event_loop, &window, proxy.clone());
        let mut info = egui::ViewportInfo::default();
        egui_winit::update_viewport_info(&mut info, &ctx, &window, true);
        let ui = factory(&ctx);
        let layered = layered.then(|| native_term_os::layered::Layered::take_over(hwnd));
        Ok(Pane {
            ctx,
            window,
            _context: context,
            surface,
            renderer: EguiSoftwareRender::new(ColorFieldOrder::Bgra),
            state,
            info,
            ui,
            layered,
            shown: false,
            last_paint: None,
            repaint_at: None,
            hwnd,
        })
    }

    /// Paint a frame. Returns true the first time (the window was just
    /// shown).
    fn paint(&mut self, frame_log: Option<&std::fs::File>, show: bool) -> Result<bool, String> {
        let size = self.window.inner_size();
        let (Some(width), Some(height)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) else {
            // minimized: nothing to show; texture updates must not be
            // produced without being painted
            return Ok(false);
        };
        let started = Instant::now();
        egui_winit::update_viewport_info(&mut self.info, &self.ctx, &self.window, false);
        let mut input = self.state.take_egui_input(&self.window);
        input.viewports = std::iter::once((ViewportId::ROOT, self.info.clone())).collect();
        let ui = &mut self.ui;
        let mut output = self.ctx.run_ui(input, |root| ui.ui(root));
        if std::env::var_os("NATIVETERM_REPAINT_LOG").is_some() {
            // what asked for the next frame (diagnostics)
            eprintln!("repaint causes: {:?}", self.ctx.repaint_causes());
        }
        self.info.events.clear();
        self.state.handle_platform_output(&self.window, std::mem::take(&mut output.platform_output));
        if let Some(viewport) = output.viewport_output.remove(&ViewportId::ROOT) {
            let mut actions = Vec::new();
            egui_winit::process_viewport_commands(
                &self.ctx,
                &mut self.info,
                viewport.commands,
                &self.window,
                &mut actions,
            );
            for action in actions {
                let event = match action {
                    egui_winit::ActionRequested::Cut => Some(egui::Event::Cut),
                    egui_winit::ActionRequested::Copy => Some(egui::Event::Copy),
                    egui_winit::ActionRequested::Paste => {
                        self.state.clipboard_text().map(|t| egui::Event::Paste(t.replace("\r\n", "\n")))
                    }
                    egui_winit::ActionRequested::Screenshot(_) => None,
                };
                self.state.egui_input_mut().events.extend(event);
            }
        }

        let ran = started.elapsed();
        let primitives = self.ctx.tessellate(output.shapes, output.pixels_per_point);
        let tessellated = started.elapsed();
        // the size may have changed through a viewport command
        let size = self.window.inner_size();
        let (Some(width), Some(height)) =
            (NonZeroU32::new(size.width).or(Some(width)), NonZeroU32::new(size.height).or(Some(height)))
        else {
            return Ok(false);
        };
        let (renderer, hwnd) = (&mut self.renderer, self.hwnd);
        let mut paint = |bytes: &mut [[u8; 4]]| {
            let mut target = BufferMutRef::new(bytes, width.get() as usize, height.get() as usize);
            renderer.render(&mut target, &primitives, &output.textures_delta, output.pixels_per_point);
        };
        // its own transparency: the whole surface at once, premultiplied
        // (which is how egui paints), so the shape it drew is the window
        let shown = match self.layered.as_mut() {
            Some(layered) => layered.present(hwnd, width.get(), height.get(), &mut paint),
            None => false,
        };
        if !shown {
            if self.layered.take().is_some() {
                // it could not be handed over with its transparency: from
                // here on this window is shown the ordinary way, square
                #[cfg(windows)]
                eprintln!("the floating button is shown without transparency: {}", std::io::Error::last_os_error());
            }
            self.surface.resize(width, height).map_err(|e| e.to_string())?;
            let mut buffer = self.surface.buffer_mut().map_err(|e| e.to_string())?;
            buffer.fill(0);
            let pixels: &mut [u32] = &mut buffer;
            // softbuffer's 0RGB u32 in little-endian bytes is B, G, R, 0.
            // SAFETY: same size, [u8; 4] has no alignment requirement, and
            // the slice borrows `buffer` for its whole life.
            let bytes = unsafe { std::slice::from_raw_parts_mut(pixels.as_mut_ptr().cast::<[u8; 4]>(), pixels.len()) };
            paint(bytes);
            // on Wayland the next redraw then waits for the compositor's
            // frame callback; without it frames go out faster than the
            // compositor hands buffers back, and `buffer_mut` blocks until
            // it does (KWin in a VM: the window stopped responding)
            self.window.pre_present_notify();
            buffer.present().map_err(|e| e.to_string())?;
        }
        let rendered = started.elapsed();
        if let Some(log) = frame_log {
            use std::io::Write;
            let _ = writeln!(
                &*log,
                "{}x{} ui {:?} tessellate {:?} raster {:?} present {:?}",
                width,
                height,
                ran,
                tessellated - ran,
                rendered - tessellated,
                started.elapsed() - rendered
            );
        }
        self.last_paint = Some(started);
        if !self.shown {
            self.shown = true;
            if show {
                self.window.set_visible(true);
            }
            return Ok(true);
        }
        Ok(false)
    }

    /// No vsync here: animations (smooth scrolling) would repaint as fast
    /// as the CPU allows, so frames are spaced by the display's rate.
    fn too_soon(&mut self, now: Instant) -> bool {
        let next = self.last_paint.map(|last| last + frame_interval(&self.window));
        match next.filter(|next| *next > now) {
            Some(next) => {
                self.repaint_at = Some(self.repaint_at.map_or(next, |at| at.min(next)));
                true
            }
            None => false,
        }
    }

    fn on_repaint(&mut self, when: Instant, pass: u64) {
        let current = self.ctx.cumulative_pass_nr_for(ViewportId::ROOT);
        // a request from a pass that has been painted since is stale
        if current == pass || current == pass + 1 {
            if when <= Instant::now() {
                self.window.request_redraw();
            } else {
                self.repaint_at = Some(self.repaint_at.map_or(when, |at| at.min(when)));
            }
        }
    }

    fn on_accesskit(&mut self, event: accesskit_winit::WindowEvent) {
        match event {
            accesskit_winit::WindowEvent::InitialTreeRequested => {
                self.ctx.enable_accesskit();
                self.window.request_redraw();
            }
            accesskit_winit::WindowEvent::ActionRequested(request) => {
                self.state.on_accesskit_action_request(request);
                self.window.request_redraw();
            }
            accesskit_winit::WindowEvent::AccessibilityDeactivated => self.ctx.disable_accesskit(),
        }
    }

    /// Pending repaint: request it if due; returns when it is due otherwise.
    fn tick(&mut self, now: Instant) -> Option<Instant> {
        if self.repaint_at.is_some_and(|at| at <= now) {
            self.repaint_at = None;
            self.window.request_redraw();
        }
        self.repaint_at
    }
}

struct Runner {
    proxy: EventLoopProxy<UserEvent>,
    viewport: egui::ViewportBuilder,
    factory: Option<Factory>,
    main: Option<Pane>,
    button_spec: Option<FloatingButton>,
    button: Option<Pane>,
    /// Save the button's position after it was dragged.
    button_saver: Option<Box<dyn Fn(i32, i32)>>,
    button_settle: Option<Instant>,
    error: Option<String>,
    /// `NATIVETERM_FRAME_LOG=<file>`: per-frame timings (diagnostics).
    frame_log: Option<std::fs::File>,
    placement: Option<Placement>,
    save: SavePlacement,
    docking: Docking,
    /// Windows from `open`: number, key, window.
    extras: Vec<(u64, String, Pane)>,
    next_extra: u64,
    /// The strip along the edge that stands for the docked window while
    /// it is hidden outright (see `set_hidden`): plain, painted by the
    /// server, brings the window back when the pointer touches it.
    #[cfg(not(windows))]
    strip: Option<Window>,
    /// Docking done by KWin (KDE Plasma under Wayland), while this lives.
    #[cfg(all(unix, not(target_os = "macos")))]
    kwin_dock: Option<native_term_os::kwin::KwinDock>,
    /// Where the floating button was last shown (see `sync_button`).
    #[cfg(not(windows))]
    button_at: Option<(i32, i32)>,
}

/// Run the windows until the main one is closed.
pub fn run(
    viewport: egui::ViewportBuilder,
    placement: Option<Placement>,
    save: SavePlacement,
    button: Option<FloatingButton>,
    factory: impl FnOnce(&egui::Context) -> Box<dyn Ui> + 'static,
) -> Result<(), String> {
    let event_loop = EventLoop::<UserEvent>::with_user_event().build().map_err(|e| e.to_string())?;
    let proxy = event_loop.create_proxy();
    let mut runner = Runner {
        proxy,
        // shown after the first frame: no white flash, and AccessKit
        // must be set up before the window is visible
        viewport: viewport.with_visible(false),
        factory: Some(Box::new(factory)),
        main: None,
        button_spec: button,
        button: None,
        button_saver: None,
        button_settle: None,
        error: None,
        frame_log: std::env::var_os("NATIVETERM_FRAME_LOG").and_then(|path| std::fs::File::create(path).ok()),
        placement,
        save,
        docking: Docking::default(),
        extras: Vec::new(),
        next_extra: 1,
        #[cfg(not(windows))]
        strip: None,
        #[cfg(not(windows))]
        button_at: None,
        #[cfg(all(unix, not(target_os = "macos")))]
        kwin_dock: None,
    };
    event_loop.run_app(&mut runner).map_err(|e| e.to_string())?;
    match runner.error {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

impl Runner {
    fn start(&mut self, event_loop: &ActiveEventLoop) -> Result<(), String> {
        let factory = self.factory.take().ok_or("the window was already started")?;
        let placement = self.placement;
        let main = Pane::create(event_loop, &self.proxy, Which::Main, &self.viewport, factory, false, |window| {
            if let Some(p) = placement {
                // only if it is still on a monitor
                if win::on_a_monitor(p.x + p.width as i32 / 2, p.y + 16) {
                    let _ = window.request_inner_size(winit::dpi::PhysicalSize::new(p.width, p.height));
                    window.set_outer_position(winit::dpi::PhysicalPosition::new(p.x, p.y));
                }
            }
        })?;
        self.main = Some(main);
        // Wayland lets no client place or find windows: nothing docks, so
        // the strip and the floating button are never needed there, and a
        // surface the compositor never shows would never get its buffers
        // back either
        if self.main.as_ref().is_some_and(|m| on_wayland(&m.window)) {
            self.button_spec = None;
            // except on KDE Plasma, where KWin docks the window itself for
            // us: a script it runs (see native_term_os::kwin)
            #[cfg(all(unix, not(target_os = "macos")))]
            if native_term_os::kwin::available() {
                let log = std::env::var_os("NATIVETERM_DOCK_LOG").is_some();
                match native_term_os::kwin::KwinDock::start(log) {
                    Ok(dock) => self.kwin_dock = Some(dock),
                    Err(e) => dock_log(|| format!("KWin docking script: {e}")),
                }
            }
        }
        #[cfg(not(windows))]
        if !self.main.as_ref().is_some_and(|m| on_wayland(&m.window)) {
            let attributes = Window::default_attributes()
                .with_title("NativeTerm")
                .with_decorations(false)
                .with_resizable(false)
                .with_visible(false)
                .with_window_level(winit::window::WindowLevel::AlwaysOnTop)
                .with_inner_size(winit::dpi::PhysicalSize::new(200u32, dock::STRIP as u32));
            if let Ok(strip) = event_loop.create_window(attributes) {
                let colour = native_term_os::desktop::accent().unwrap_or((0x80, 0x80, 0x80));
                if let Some(handle) = window_handle(&strip) {
                    win::fill(handle, colour);
                }
                self.strip = Some(strip);
            }
        }
        if let Some(spec) = self.button_spec.take() {
            let viewport = spec.viewport.with_visible(false);
            let position = spec.position;
            let button =
                Pane::create(event_loop, &self.proxy, Which::Button, &viewport, spec.factory, true, |window| {
                    match position.filter(|(x, y)| win::on_a_monitor(*x, *y)) {
                        Some((x, y)) => window.set_outer_position(winit::dpi::PhysicalPosition::new(x, y)),
                        None => {
                            // bottom right of the main window's monitor
                            let hwnd = window_handle(window).unwrap_or_default();
                            if let Some(work) = win::work_area(hwnd) {
                                let size = window.outer_size();
                                let margin = (32.0 * window.scale_factor()) as i32;
                                window.set_outer_position(winit::dpi::PhysicalPosition::new(
                                    work.right - size.width as i32 - margin,
                                    work.bottom - size.height as i32 - margin,
                                ));
                            }
                        }
                    }
                });
            match button {
                Ok(mut pane) => {
                    // painted once, hidden, so it can appear without a flash
                    pane.paint(None, false)?;
                    self.button = Some(pane);
                    self.button_saver = Some(spec.save);
                }
                Err(e) => eprintln!("floating button: {e}"),
            }
        }
        Ok(())
    }

    fn pane(&mut self, id: WindowId) -> Option<(Which, &mut Pane)> {
        if self.main.as_ref().is_some_and(|p| p.window.id() == id) {
            return self.main.as_mut().map(|p| (Which::Main, p));
        }
        if self.button.as_ref().is_some_and(|p| p.window.id() == id) {
            return self.button.as_mut().map(|p| (Which::Button, p));
        }
        self.extras.iter_mut().find(|(_, _, p)| p.window.id() == id).map(|(n, _, p)| (Which::Extra(*n), p))
    }

    fn extra(&mut self, n: u64) -> Option<&mut Pane> {
        self.extras.iter_mut().find(|(m, _, _)| *m == n).map(|(_, _, p)| p)
    }

    /// Opens the windows asked for since the last time (or shows the open
    /// one with the same key).
    fn open_requested(&mut self, event_loop: &ActiveEventLoop) {
        let requests = REQUESTS.with(|r| std::mem::take(&mut *r.borrow_mut()));
        for request in requests {
            if let Some((_, _, pane)) = self.extras.iter().find(|(_, k, _)| *k == request.key) {
                if pane.window.is_minimized() == Some(true) {
                    pane.window.set_minimized(false);
                }
                pane.window.focus_window();
                continue;
            }
            let n = self.next_extra;
            self.next_extra += 1;
            let viewport = request.viewport.with_visible(false);
            match Pane::create(event_loop, &self.proxy, Which::Extra(n), &viewport, request.factory, false, |_| {}) {
                Ok(mut pane) => {
                    // painted at once: a hidden window gets no redraw
                    if let Err(e) = pane.paint(self.frame_log.as_ref(), true) {
                        eprintln!("window {}: {e}", request.key);
                        continue;
                    }
                    pane.window.focus_window();
                    self.extras.push((n, request.key, pane));
                }
                Err(e) => eprintln!("window {}: {e}", request.key),
            }
        }
    }

    fn close_extra(&mut self, n: u64) {
        // dropping the pane drops its Ui (which ends its connections)
        self.extras.retain(|(m, _, _)| *m != n);
    }

    fn hwnd(&self) -> Option<isize> {
        self.main.as_ref().map(|r| r.hwnd)
    }

    /// Where the window is now (as it would be shown, if docked and hidden).
    fn placement(&self) -> Option<Placement> {
        let r = self.main.as_ref()?;
        let size = r.window.inner_size();
        let (mut x, mut y) = win::window_bounds(r.hwnd).map(|b| (b.left, b.top))?;
        if let (Some(edge), true) = (self.docking.edge, self.docking.hidden) {
            if let Some((sx, sy)) = self.docked_target(edge, false) {
                (x, y) = (sx, sy);
            }
        }
        Some(Placement { x, y, width: size.width, height: size.height, edge: self.docking.edge })
    }

    fn save_placement(&self) {
        if let Some(p) = self.placement() {
            (self.save)(p);
        }
    }

    fn docked_target(&self, edge: Edge, hidden: bool) -> Option<(i32, i32)> {
        let hwnd = self.hwnd()?;
        let window = win::window_bounds(hwnd)?;
        let frame = win::frame_bounds(hwnd)?;
        let work = win::work_area(hwnd)?;
        Some(dock::docked_position(edge, window, frame, work, hidden))
    }

    fn move_to(&mut self, (x, y): (i32, i32)) {
        if let Some(hwnd) = self.hwnd() {
            self.docking.own_move_until = Some(Instant::now() + Duration::from_millis(80));
            win::move_window(hwnd, x, y);
        }
    }

    fn start_slide(&mut self, hide: bool) {
        if cfg!(windows) {
            self.slide_window(hide);
        } else {
            self.set_hidden(hide);
        }
    }

    /// Where the window manager keeps windows on the screen (X11), a
    /// docked window can't slide away: it is hidden outright, and a
    /// strip along the edge stands for it until the pointer touches it.
    fn set_hidden(&mut self, hide: bool) {
        let Some(edge) = self.docking.edge else { return };
        dock_log(|| format!("hidden {hide} at {edge:?}, frame {:?}", self.hwnd().and_then(win::frame_bounds)));
        #[cfg(windows)]
        let _ = edge;
        self.docking.slide = None;
        self.docking.leave_check = None;
        if hide {
            let at = self.hwnd().and_then(win::frame_bounds);
            #[cfg(not(windows))]
            {
                self.docking.shown_at = at.map(|b| (b.left, b.top));
                if let (Some(strip), Some(frame)) = (&self.strip, at) {
                    let (x, y, w, h) = match edge {
                        Edge::Top => (frame.left, frame.top, frame.width(), dock::STRIP),
                        Edge::Left => (frame.left, frame.top, dock::STRIP, frame.height()),
                        Edge::Right => (frame.right - dock::STRIP, frame.top, dock::STRIP, frame.height()),
                    };
                    let _ = strip.request_inner_size(winit::dpi::PhysicalSize::new(w.max(1) as u32, h.max(1) as u32));
                    strip.set_outer_position(winit::dpi::PhysicalPosition::new(x, y));
                    strip.set_visible(true);
                    // the window manager places a newly mapped window as it
                    // likes; a move once it is mapped is honoured
                    if let Some(handle) = window_handle(strip) {
                        win::move_window(handle, x, y);
                    }
                }
            }
            let _ = at;
            if let Some(main) = &self.main {
                main.window.set_visible(false);
            }
            self.docking.hidden = true;
        } else {
            #[cfg(not(windows))]
            if let Some(strip) = &self.strip {
                strip.set_visible(false);
            }
            if let Some(main) = &self.main {
                main.window.set_visible(true);
            }
            self.docking.hidden = false;
            // mapped again, the window manager may have placed it anew
            #[cfg(not(windows))]
            if let Some(at) = self.docking.shown_at.take() {
                self.move_to(at);
            }
            if !self.docking.until_visited {
                self.docking.leave_check = Some(Instant::now() + dock::LEAVE_DELAY);
            }
        }
    }

    fn slide_window(&mut self, hide: bool) {
        let Some(edge) = self.docking.edge else { return };
        let (Some(hwnd), Some(to)) = (self.hwnd(), self.docked_target(edge, hide)) else { return };
        let Some(from) = win::window_bounds(hwnd).map(|b| (b.left, b.top)) else { return };
        if !hide {
            self.docking.hidden = false;
        }
        // someone who turned Windows' animations off gets none of ours:
        // the window arrives at once (the same path, already over)
        let started = match native_term_os::desktop::animations() {
            true => Instant::now(),
            false => Instant::now().checked_sub(dock::SLIDE).unwrap_or_else(Instant::now),
        };
        self.docking.slide = Some((Slide { from, to, started }, hide));
        self.docking.leave_check = None;
    }

    fn dock_at(&mut self, edge: Option<Edge>) {
        if self.main.is_none() {
            return;
        }
        dock_log(|| format!("dock at {edge:?}, frame {:?}", self.hwnd().and_then(win::frame_bounds)));
        let was = self.docking.edge;
        self.docking.edge = edge;
        if self.docking.hidden {
            // undocked while hidden outright (X11): back on the screen
            self.set_hidden(false);
        }
        self.docking.hidden = false;
        dock::publish_edge(edge);
        if let Some(r) = &self.main {
            // through winit, which otherwise resets the level on its own
            let level = if edge.is_some() {
                winit::window::WindowLevel::AlwaysOnTop
            } else {
                winit::window::WindowLevel::Normal
            };
            r.window.set_window_level(level);
            // the top bar shows "Pin" only while docked
            r.window.request_redraw();
        }
        if let Some(edge) = edge {
            if let Some(target) = self.docked_target(edge, false) {
                self.move_to(target);
            }
            self.docking.leave_check = Some(Instant::now() + dock::LEAVE_DELAY);
        }
        if was != edge || edge.is_none() {
            self.save_placement();
        }
    }

    /// Bring the main window out: slide it back if docked, and activate it.
    fn show_main(&mut self) {
        if self.docking.edge.is_some() && (self.docking.hidden || self.docking.slide.is_some_and(|(_, hide)| hide)) {
            self.start_slide(false);
            self.docking.until_visited = true;
        }
        if let Some(main) = &self.main {
            if main.window.is_minimized() == Some(true) {
                main.window.set_minimized(false);
            }
            main.window.focus_window();
            main.window.request_redraw();
        }
    }

    /// Timers and slides; returns when to come back.
    fn tick_docking(&mut self, now: Instant) -> Option<Instant> {
        if let Some((slide, hide)) = self.docking.slide {
            let (pos, done) = slide.at(now);
            self.move_to(pos);
            if done {
                self.docking.slide = None;
                self.docking.hidden = hide;
                if !hide && !self.docking.until_visited {
                    self.docking.leave_check = Some(now + dock::LEAVE_DELAY);
                }
            } else {
                return Some(now + Duration::from_millis(10));
            }
        }
        if self.docking.edge.is_some() && self.docking.hidden && dock::pinned() {
            self.start_slide(false);
            return Some(now);
        }
        if self.docking.settle_check.is_some_and(|at| at <= now) {
            self.docking.settle_check = None;
            if win::mouse_button_down() {
                self.docking.settle_check = Some(now + dock::DRAG_POLL);
            } else if std::mem::take(&mut self.docking.top_after_restore) {
                self.dock_at(Some(Edge::Top));
            } else if let Some(hwnd) = self.hwnd() {
                let maximized = self.main.as_ref().is_some_and(|r| r.window.is_maximized());
                // a window manager that maximizes a window dragged to the
                // top edge (KWin): its own size back, then docked there
                let at_top = maximized
                    && win::work_area(hwnd)
                        .zip(win::cursor())
                        .is_some_and(|(work, (_, y))| (0..=dock::SNAP).contains(&(y - work.top)));
                if at_top {
                    dock_log(|| "maximized by a drag to the top: restoring".into());
                    if let Some(r) = &self.main {
                        r.window.set_maximized(false);
                    }
                    self.docking.top_after_restore = true;
                    self.docking.settle_check = Some(now + Duration::from_millis(300));
                    return self.docking.settle_check;
                }
                let edge = match (win::frame_bounds(hwnd), win::work_area(hwnd)) {
                    (Some(frame), Some(work)) if !maximized => {
                        let monitor = win::monitor_bounds(hwnd).unwrap_or(work);
                        // the pointer says which edge was meant: a window
                        // manager that tiles a window dragged to a side
                        // (KWin: half the screen, top to bottom, and it
                        // ignores a program's own moves while tiled) leaves
                        // it touching the top edge too; a side panel that
                        // tall is what docking there is for anyway
                        // (Windows' Aero Snap tiles and maximizes the same way)
                        let at_pointer =
                            win::cursor().and_then(|c| dock::edge_at_pointer(c, work, monitor, win::on_a_monitor));
                        at_pointer.or_else(|| dock::snap_edge(frame, work, monitor, win::on_a_monitor))
                    }
                    _ => None,
                };
                dock_log(|| {
                    let frame = win::frame_bounds(hwnd);
                    format!("settled: frame {frame:?} cursor {:?} edge {edge:?}", win::cursor())
                });
                if edge.is_some() || self.docking.edge.is_some() {
                    self.dock_at(edge);
                } else {
                    self.save_placement();
                }
            }
        }
        if self.docking.leave_check.is_some_and(|at| at <= now) {
            self.docking.leave_check = None;
            if let (Some(_), false, Some(hwnd)) = (self.docking.edge, self.docking.hidden, self.hwnd()) {
                let inside = match (win::cursor(), win::window_bounds(hwnd)) {
                    (Some((x, y)), Some(b)) => b.contains(x, y),
                    _ => true,
                };
                let typing =
                    self.main.as_ref().is_some_and(|r| r.ctx.egui_wants_keyboard_input() && r.window.has_focus());
                // a move by the user is still being settled: it may undock
                let moving = self.docking.settle_check.is_some();
                if (self.docking.cursor_in_client && !moving) || dock::pinned() {
                    // no polling: leaving the client area is reported, and
                    // unpinning takes a click inside
                } else if inside || moving || win::mouse_button_down() || typing {
                    // keep watching while the pointer is over the frame or a
                    // button is held
                    self.docking.leave_check = Some(now + dock::LEAVE_DELAY);
                } else {
                    dock_log(|| {
                        format!(
                            "leaving: cursor {:?} window {:?} in client {}",
                            win::cursor(),
                            win::window_bounds(hwnd),
                            self.docking.cursor_in_client
                        )
                    });
                    self.start_slide(true);
                    return Some(now);
                }
            }
        }
        // hidden behind the strip (X11): the pointer reaching it brings
        // the window back. Its enter event alone is not enough: a window
        // manager's own screen-edge windows (KWin's, a pixel wide) can sit
        // over it and take the pointer instead
        #[cfg(not(windows))]
        let strip_poll = if self.docking.hidden && self.docking.edge.is_some() {
            let over = self.strip.as_ref().and_then(window_handle).and_then(win::window_bounds).zip(win::cursor());
            if over.is_some_and(|(b, (x, y))| {
                // the edge pixel itself counts too
                dock::Bounds { left: b.left - 1, top: b.top, right: b.right + 1, bottom: b.bottom }.contains(x, y)
            }) {
                dock_log(|| "pointer over the strip".into());
                self.start_slide(false);
                return Some(now);
            }
            Some(now + STRIP_POLL)
        } else {
            None
        };
        #[cfg(windows)]
        let strip_poll = None;
        [self.docking.settle_check, self.docking.leave_check, strip_poll].into_iter().flatten().min()
    }

    fn docking_event(&mut self, event: &WindowEvent) {
        let now = Instant::now();
        if let WindowEvent::CursorEntered { .. } = event {
            self.docking.cursor_in_client = true;
            self.docking.until_visited = false;
        }
        if let WindowEvent::Focused(false) = event {
            if self.docking.until_visited && self.docking.edge.is_some() && !self.docking.hidden {
                self.docking.until_visited = false;
                self.docking.leave_check = Some(now + dock::LEAVE_DELAY);
            }
        }
        match event {
            WindowEvent::Moved(_) => {
                let ours = self.docking.slide.is_some() || self.docking.own_move_until.is_some_and(|t| t > now);
                if !ours {
                    self.docking.settle_check = Some(now + dock::DRAG_POLL);
                }
            }
            WindowEvent::CursorLeft { .. } => {
                self.docking.cursor_in_client = false;
                if self.docking.edge.is_some() && !self.docking.hidden {
                    self.docking.leave_check = Some(now + dock::LEAVE_DELAY);
                }
            }
            WindowEvent::CursorEntered { .. } | WindowEvent::CursorMoved { .. } | WindowEvent::Focused(true)
                if self.docking.edge.is_some() && self.docking.hidden && self.docking.slide.is_none() =>
            {
                self.start_slide(false);
            }
            _ => {}
        }
    }

    /// The floating button is there while the docked main window is away.
    fn sync_button(&mut self) {
        let wanted = self.docking.edge.is_some() && self.docking.hidden && self.docking.slide.is_none();
        let keep_open = crate::fab::expanded();
        if let Some(button) = &self.button {
            let visible = button.window.is_visible().unwrap_or(false);
            if (wanted || (visible && keep_open)) != visible {
                // where it is while shown: a window unmapped on X11 forgets
                // its place, and the window manager places a newly mapped
                // one as it likes; a move once it is mapped is honoured
                #[cfg(not(windows))]
                let at = match visible {
                    true => win::window_bounds(button.hwnd).map(|b| (b.left, b.top)),
                    // before the first map, where it was put at creation
                    false => self.button_at.or_else(|| button.window.outer_position().ok().map(|p| (p.x, p.y))),
                };
                button.window.set_visible(!visible);
                if !visible {
                    #[cfg(not(windows))]
                    if let Some((x, y)) = at {
                        win::move_window(button.hwnd, x, y);
                    }
                    button.window.request_redraw();
                }
                #[cfg(not(windows))]
                {
                    self.button_at = at;
                }
            }
        }
    }

    fn paint(&mut self, which: Which) -> Result<(), String> {
        let log = self.frame_log.as_ref();
        match which {
            Which::Main => {
                let Some(main) = self.main.as_mut() else { return Ok(()) };
                let first = main.paint(log, true)?;
                // docked when NativeTerm was closed: dock again
                if first {
                    if let Some(edge) = self.placement.and_then(|p| p.edge) {
                        self.dock_at(Some(edge));
                    }
                }
            }
            Which::Button => {
                if let Some(button) = self.button.as_mut() {
                    button.paint(log, false)?;
                }
            }
            Which::Extra(n) => {
                let Some((_, _, pane)) = self.extras.iter_mut().find(|(m, _, _)| *m == n) else { return Ok(()) };
                pane.paint(log, true)?;
                if pane.ui.wants_close() {
                    self.close_extra(n);
                }
            }
        }
        Ok(())
    }

    fn fail(&mut self, event_loop: &ActiveEventLoop, error: String) {
        self.error = Some(error);
        event_loop.exit();
    }
}

impl ApplicationHandler<UserEvent> for Runner {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.main.is_none() {
            // paint at once: a hidden window gets no redraw
            if let Err(e) = self.start(event_loop).and_then(|()| self.paint(Which::Main)) {
                self.fail(event_loop, e);
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        let now = Instant::now();
        #[cfg(not(windows))]
        if self.strip.as_ref().is_some_and(|s| s.id() == id) {
            if matches!(event, WindowEvent::CursorEntered { .. }) && self.docking.hidden {
                self.start_slide(false);
            }
            return;
        }
        let Some((which, pane)) = self.pane(id) else { return };
        match event {
            WindowEvent::RedrawRequested => {
                if pane.too_soon(now) {
                    return;
                }
                if let Err(e) = self.paint(which) {
                    self.fail(event_loop, e);
                }
            }
            WindowEvent::CloseRequested if which == Which::Main => {
                self.save_placement();
                if let Some(main) = self.main.as_mut() {
                    main.ui.on_exit();
                }
                self.extras.clear();
                event_loop.exit();
            }
            WindowEvent::CloseRequested => {
                if let Which::Extra(n) = which {
                    if pane.ui.close_requested() {
                        self.close_extra(n);
                    } else {
                        pane.window.request_redraw();
                    }
                }
            }
            event => {
                if pane.state.on_window_event(&pane.window, &event).repaint {
                    pane.window.request_redraw();
                }
                match which {
                    Which::Main => self.docking_event(&event),
                    Which::Button => {
                        if let WindowEvent::Moved(_) = event {
                            self.button_settle = Some(now + dock::DRAG_POLL);
                        }
                    }
                    Which::Extra(_) => {}
                }
            }
        }
    }

    fn user_event(&mut self, _: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::Repaint { which, when, pass } => {
                let pane = match which {
                    Which::Main => self.main.as_mut(),
                    Which::Button => self.button.as_mut(),
                    Which::Extra(n) => self.extra(n),
                };
                if let Some(pane) = pane {
                    pane.on_repaint(when, pass);
                }
            }
            UserEvent::AccessKit(event) => {
                if let Some((_, pane)) = self.pane(event.window_id) {
                    pane.on_accesskit(event.window_event);
                }
            }
        }
    }

    /// The windows go while the event loop still has its display: on
    /// Wayland each window's clipboard worker (smithay-clipboard) destroys
    /// its objects on that connection when dropped, and after the loop
    /// the connection is gone (a crash on every close).
    fn exiting(&mut self, _: &ActiveEventLoop) {
        self.extras.clear();
        self.button = None;
        #[cfg(not(windows))]
        {
            self.strip = None;
        }
        self.main = None;
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.open_requested(event_loop);
        let now = Instant::now();
        if shell::take_show_main() {
            self.show_main();
        }
        let docking = self.tick_docking(now);
        self.sync_button();
        if self.button_settle.is_some_and(|at| at <= now) {
            self.button_settle = None;
            if win::mouse_button_down() {
                self.button_settle = Some(now + dock::DRAG_POLL);
            } else if let (Some(button), Some(save)) = (&self.button, &self.button_saver) {
                // only the collapsed button's place is remembered
                if !crate::fab::expanded() {
                    if let Some(b) = win::window_bounds(button.hwnd) {
                        save(b.left, b.top);
                    }
                }
            }
        }
        let main = self.main.as_mut().and_then(|p| p.tick(now));
        let button = self.button.as_mut().and_then(|p| p.tick(now));
        let extras = self.extras.iter_mut().filter_map(|(_, _, p)| p.tick(now)).min();
        match [main, button, docking, self.button_settle, extras].into_iter().flatten().min() {
            Some(at) => event_loop.set_control_flow(ControlFlow::WaitUntil(at.max(now))),
            None => event_loop.set_control_flow(ControlFlow::Wait),
        }
    }
}

/// The window's native handle: the `HWND`, which docking, the layered
/// button and the Terminal windows are all about, or the X window id,
/// which docking asks the X server about. Elsewhere there is nothing of
/// that, and `0` stands for a handle nothing asks after.
fn window_handle(window: &Window) -> Option<isize> {
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match window.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(h) => Some(h.hwnd.get()),
        RawWindowHandle::Xlib(h) => Some(h.window as isize),
        RawWindowHandle::Xcb(h) => Some(h.window.get() as isize),
        _ => Some(0),
    }
}

/// How often the pointer is looked for over the strip while the docked
/// window is hidden outright (X11).
#[cfg(not(windows))]
const STRIP_POLL: Duration = Duration::from_millis(100);

/// `NATIVETERM_DOCK_LOG=1`: what docking decides, on stderr (diagnostics).
fn dock_log(what: impl FnOnce() -> String) {
    if std::env::var_os("NATIVETERM_DOCK_LOG").is_some() {
        eprintln!("dock: {}", what());
    }
}

/// Whether the window is a Wayland surface (not X11, not Win32).
fn on_wayland(window: &Window) -> bool {
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    matches!(window.window_handle().map(|h| h.as_raw()), Ok(RawWindowHandle::Wayland(_)))
}

/// One frame at the window's monitor refresh rate (60 Hz if unknown).
fn frame_interval(window: &Window) -> Duration {
    let millihertz = window.current_monitor().and_then(|m| m.refresh_rate_millihertz()).unwrap_or(60_000);
    Duration::from_micros(1_000_000_000 / u64::from(millihertz.max(1_000)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placement_setting() {
        let p = Placement { x: -7, y: 0, width: 960, height: 640, edge: Some(Edge::Top) };
        assert_eq!(Placement::from_setting(&p.to_setting()), Some(p));
        let free = Placement { edge: None, ..p };
        assert_eq!(free.to_setting(), "-7,0,960,640,");
        assert_eq!(Placement::from_setting(&free.to_setting()), Some(free));
        assert_eq!(Placement::from_setting("1,2,3"), None);
        assert_eq!(Placement::from_setting("1,2,30,40,"), None, "too small");
    }
}

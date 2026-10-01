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

use crate::dialog_window::{dialog_of, owned_by};
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
    place: Option<Place>,
    left: Option<LeftAt>,
    owner: Option<Owner>,
    modal: bool,
}

/// The window a dialog belongs to: kept above it and with it by the
/// window system (Windows' owner window, X11's `WM_TRANSIENT_FOR`, a
/// child window on macOS), and, for a modal one, kept from input while
/// the dialog is open.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Owner {
    Main,
    /// Another window from `open`, by its key.
    Window(String),
}

/// Told where a window was when it closed, its top left corner in the
/// screen's pixels, when the person had moved it (see `open_at`).
pub type LeftAt = Box<dyn Fn(i32, i32)>;

/// A window whose place is kept: since when it is shown, and whether
/// the person has moved it.
struct Kept {
    left: LeftAt,
    shown: Instant,
    moved: bool,
}

/// A window is put in its place when it is shown, by the window manager
/// too (its frame around it, a place of its own choosing): it moves a few
/// times then. A move after this long is the person's.
const SETTLED: Duration = Duration::from_millis(1500);

/// Where a window opens, in the screen's pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    /// Its middle at the point.
    Around(i32, i32),
    /// Its top left corner, title bar and all, at the point.
    At(i32, i32),
}

thread_local! {
    static REQUESTS: RefCell<Vec<Request>> = const { RefCell::new(Vec::new()) };
    /// The keys of the windows from `open` that are open (see `is_open`).
    static OPEN_KEYS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    /// Windows asked to close (see `close`).
    static CLOSING: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
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
    let request = Request {
        key: key.into(),
        viewport,
        factory: Box::new(factory),
        place: None,
        left: None,
        owner: None,
        modal: false,
    };
    REQUESTS.with(|r| r.borrow_mut().push(request));
}

/// Opens a dialog of `owner`'s, in the middle of it: a window of its own
/// that the window system keeps above its owner. A `modal` one keeps the
/// owner from input until it is closed (a click on the owner brings the
/// dialog forward), as Windows' `DialogBox` and the desktops' modal
/// dialogs do; it gives the owner the keyboard back when it closes.
pub fn open_dialog(
    key: impl Into<String>,
    viewport: egui::ViewportBuilder,
    owner: Owner,
    modal: bool,
    factory: impl FnOnce(&egui::Context) -> Box<dyn Ui> + 'static,
) {
    let request = Request {
        key: key.into(),
        viewport,
        factory: Box::new(factory),
        place: None,
        left: None,
        owner: Some(owner),
        modal,
    };
    REQUESTS.with(|r| r.borrow_mut().push(request));
}

/// Whether a window from `open` with this key is open (its `Ui` may say it
/// wants to close: it is gone once the frame is done).
pub fn is_open(key: &str) -> bool {
    OPEN_KEYS.with(|k| k.borrow().iter().any(|open| open == key))
}

/// Closes the window from `open` with this key, once the current frame is
/// done (a dialog its owner is done with).
pub fn close(key: &str) {
    CLOSING.with(|c| c.borrow_mut().push(key.to_string()));
}

/// The same, the window at `place` (`None`: where the window system puts
/// it), moved into the work area of the screen that is on where it would
/// reach beyond it; `left` is told where the window was when it closed,
/// when the person had moved it: `Place::At` that, the next time. Where
/// the window system places windows itself and says nothing of where
/// they are (Wayland), it does, and `left` hears nothing.
pub fn open_at(
    key: impl Into<String>,
    viewport: egui::ViewportBuilder,
    place: Option<Place>,
    left: impl Fn(i32, i32) + 'static,
    factory: impl FnOnce(&egui::Context) -> Box<dyn Ui> + 'static,
) {
    let request = Request {
        key: key.into(),
        viewport,
        factory: Box::new(factory),
        place,
        left: Some(Box::new(left)),
        owner: None,
        modal: false,
    };
    REQUESTS.with(|r| r.borrow_mut().push(request));
}

/// The pointer's place, for a window to open around.
pub fn pointer() -> Option<Place> {
    win::cursor().map(|(x, y)| Place::Around(x, y))
}

/// The same, the window's middle where the pointer is now (within the
/// screen's work area): for what another program's window asked for, the
/// pointer being there. Where the window system places windows itself
/// (Wayland), it does.
pub fn open_at_pointer(
    key: impl Into<String>,
    viewport: egui::ViewportBuilder,
    factory: impl FnOnce(&egui::Context) -> Box<dyn Ui> + 'static,
) {
    let request = Request {
        key: key.into(),
        viewport,
        factory: Box::new(factory),
        place: pointer(),
        left: None,
        owner: None,
        modal: false,
    };
    REQUESTS.with(|r| r.borrow_mut().push(request));
}

/// Where a window of `size` goes for `place`, within the work area of
/// the screen the place is on (from the area's left and top where the
/// window is larger).
fn placed(place: Place, size: (i32, i32)) -> (i32, i32) {
    match place {
        Place::Around(x, y) => around((x, y), size, win::work_area_at(x, y)),
        Place::At(x, y) => {
            let middle = (x + size.0 / 2, y + size.1 / 2);
            around(middle, size, win::work_area_at(middle.0, middle.1))
        }
    }
}

/// Where a window of `size` goes for its middle to be at `middle`, within
/// `area` (from its left and top where it is larger).
fn around(middle: (i32, i32), size: (i32, i32), area: Option<win::Bounds>) -> (i32, i32) {
    let (x, y) = (middle.0 - size.0 / 2, middle.1 - size.1 / 2);
    match area {
        Some(area) => (x.min(area.right - size.0).max(area.left), y.min(area.bottom - size.1).max(area.top)),
        None => (x, y),
    }
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

/// How a window is made: an ordinary one, one shown with its own
/// transparency (the floating button), or a dialog of another window's
/// (modal or not).
enum Made<'a> {
    Plain,
    Layered,
    Owned(&'a Window, bool),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Which {
    Main,
    Button,
    /// A window from `open`, by its number.
    Extra(u64),
}

enum UserEvent {
    Repaint {
        which: Which,
        when: Instant,
        pass: u64,
    },
    AccessKit(accesskit_winit::Event),
    /// An activation token for another program's window (Wayland), asked
    /// for from another thread: the answer goes back on this.
    #[cfg(all(unix, not(target_os = "macos")))]
    ActivationToken(std::sync::mpsc::Sender<Option<String>>),
}

impl From<accesskit_winit::Event> for UserEvent {
    fn from(event: accesskit_winit::Event) -> Self {
        UserEvent::AccessKit(event)
    }
}

/// The main window's handle (`window_handle`), for what the app asks the
/// system about it; 0 before there is one.
static MAIN_HANDLE: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

/// The main window's handle, once it is made.
pub fn main_handle() -> Option<isize> {
    Some(MAIN_HANDLE.load(std::sync::atomic::Ordering::Relaxed)).filter(|handle| *handle != 0)
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
    /// The look it has (every window follows the one chosen).
    look: Option<crate::looks::Preset>,
    /// Its own button that closes it was clicked (the main window's
    /// header has one): as if the system's had been.
    close_asked: bool,
}

impl Pane {
    fn create(
        event_loop: &ActiveEventLoop,
        proxy: &EventLoopProxy<UserEvent>,
        which: Which,
        viewport: &egui::ViewportBuilder,
        factory: Factory,
        made: Made<'_>,
        before_show: impl FnOnce(&Window),
    ) -> Result<Pane, String> {
        let (layered, owner) = match made {
            Made::Plain => (false, None),
            Made::Layered => (true, None),
            Made::Owned(owner, modal) => (false, Some((owner, modal))),
        };
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
        let attributes = owned_by(egui_winit::create_winit_window_attributes(&ctx, viewport.clone()), owner);
        let window = event_loop.create_window(attributes).map_err(|e| e.to_string())?;
        egui_winit::apply_viewport_builder_to_window(&ctx, &window, viewport);
        if let Some((owner, modal)) = owner {
            dialog_of(&window, owner, modal);
        }
        let window = Rc::new(window);
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
            look: None,
            close_asked: false,
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
        // the look chosen in the main window is every window's
        if let Some(look) = crate::looks::chosen().filter(|look| self.look != Some(*look)) {
            look.apply(&self.ctx);
            self.look = Some(look);
        }
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
            // the window is taken by its header or an edge of its own: the
            // system moves it from here on, and the button's release is the
            // system's too (X11's window manager grabs the pointer, AppKit
            // runs the drag itself); winit sends one after the drag on
            // Windows only. Without it the next press would be no press.
            let taken = viewport.commands.iter().any(|command| {
                matches!(command, egui::ViewportCommand::StartDrag | egui::ViewportCommand::BeginResize(_))
            });
            if taken && !cfg!(windows) {
                if let Some(pos) = self.ctx.pointer_latest_pos() {
                    let modifiers = self.ctx.input(|i| i.modifiers);
                    let button = egui::PointerButton::Primary;
                    let released = egui::Event::PointerButton { pos, button, pressed: false, modifiers };
                    self.state.egui_input_mut().events.push(released);
                    self.ctx.request_repaint();
                }
            }
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
            self.close_asked |= self.info.events.contains(&egui::ViewportEvent::Close);
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
    /// Activation tokens asked for and not given yet: the request's serial,
    /// where the answer goes.
    #[cfg(all(unix, not(target_os = "macos")))]
    token_asked: Vec<(winit::event_loop::AsyncRequestSerial, std::sync::mpsc::Sender<Option<String>>)>,
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
    /// Those of them whose place is kept, by number.
    kept: Vec<(u64, Kept)>,
    /// Dialogs and the window they belong to, by number: whether modal.
    owned: Vec<(u64, Which, bool)>,
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
    let mut builder = EventLoop::<UserEvent>::with_user_event();
    // an accessory (no Dock icon) until its windows are made: only a
    // window made then stays in sight beside another application's
    // full-screen window (see native_term_os::dock::over_fullscreen)
    #[cfg(target_os = "macos")]
    {
        use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
        builder.with_activation_policy(ActivationPolicy::Accessory);
    }
    let event_loop = builder.build().map_err(|e| e.to_string())?;
    let proxy = event_loop.create_proxy();
    activation_source(&event_loop, proxy.clone());
    let mut runner = Runner {
        #[cfg(all(unix, not(target_os = "macos")))]
        token_asked: Vec::new(),
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
        kept: Vec::new(),
        owned: Vec::new(),
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
        let main =
            Pane::create(event_loop, &self.proxy, Which::Main, &self.viewport, factory, Made::Plain, |window| {
                // without the system's title bar (`main.rs`) the window
                // would be without its shadow too
                #[cfg(windows)]
                {
                    use winit::platform::windows::WindowExtWindows;
                    window.set_undecorated_shadow(true);
                }
                if let Some(p) = placement {
                    // only if it is still on a monitor
                    if win::on_a_monitor(p.x + p.width as i32 / 2, p.y + 16) {
                        let _ = window.request_inner_size(winit::dpi::PhysicalSize::new(p.width, p.height));
                        window.set_outer_position(winit::dpi::PhysicalPosition::new(p.x, p.y));
                    }
                }
            })?;
        MAIN_HANDLE.store(main.hwnd, std::sync::atomic::Ordering::Relaxed);
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
                    // in sight beside a full-screen Terminal window (macOS)
                    win::over_fullscreen(handle, true);
                }
                self.strip = Some(strip);
            }
        }
        if let Some(spec) = self.button_spec.take() {
            let viewport = spec.viewport.with_visible(false);
            let position = spec.position;
            let button = Pane::create(
                event_loop,
                &self.proxy,
                Which::Button,
                &viewport,
                spec.factory,
                Made::Layered,
                |window| {
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
                },
            );
            match button {
                Ok(mut pane) => {
                    // painted once, hidden, so it can appear without a flash
                    pane.paint(None, false)?;
                    // in sight beside a full-screen Terminal window (macOS)
                    if let Some(handle) = window_handle(&pane.window) {
                        win::over_fullscreen(handle, true);
                    }
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
                activate_x11(&pane.window);
                continue;
            }
            let n = self.next_extra;
            self.next_extra += 1;
            let above = request.viewport.window_level == Some(egui::WindowLevel::AlwaysOnTop);
            let mut viewport = request.viewport.with_visible(false);
            if let Some(place) = request.place {
                // the window is made where it is to be: a window moved
                // before it is shown says nothing of it to the window
                // manager, which then puts it where it puts new windows
                // (KWin: the screen's middle); one made at a place does
                let scale = self.main.as_ref().map_or(1.0, |main| main.window.scale_factor());
                let pixels = |points: f32| (f64::from(points) * scale) as i32;
                let size = viewport.inner_size.map_or((0, 0), |size| (pixels(size.x), pixels(size.y)));
                let (x, y) = placed(place, size);
                let points = |pixels: i32| (f64::from(pixels) / scale) as f32;
                viewport = viewport.with_position([points(x), points(y)]);
            }
            // a dialog: over its owner's middle, the window system told
            // whose it is
            let owner = request.owner.as_ref().and_then(|owner| self.owner_of(owner));
            if let Some(owner) = owner.and_then(|which| self.pane_of(which)) {
                let scale = owner.window.scale_factor();
                let size = viewport.inner_size.unwrap_or(egui::vec2(480.0, 320.0));
                let pixels = ((f64::from(size.x) * scale) as u32, (f64::from(size.y) * scale) as u32);
                if let Ok(at) = owner.window.outer_position() {
                    let (x, y) = crate::dialog_window::centered((at.x, at.y), owner.window.outer_size().into(), pixels);
                    viewport = viewport.with_position([(f64::from(x) / scale) as f32, (f64::from(y) / scale) as f32]);
                }
            }
            let owner_window = owner.and_then(|which| self.pane_of(which)).map(|pane| (&*pane.window, request.modal));
            let made = Pane::create(
                event_loop,
                &self.proxy,
                Which::Extra(n),
                &viewport,
                request.factory,
                owner_window.map_or(Made::Plain, |(owner, modal)| Made::Owned(owner, modal)),
                |_| {},
            );
            match made {
                Ok(mut pane) => {
                    // painted at once: a hidden window gets no redraw. A
                    // dialog is painted twice first, hidden: it takes the
                    // size of what it has (see the skin's dialogs), then is
                    // put over its owner's middle and shown
                    let log = self.frame_log.as_ref();
                    let painted = match owner {
                        None => pane.paint(log, true),
                        Some(_) => pane.paint(log, false).and_then(|_| pane.paint(log, false)),
                    };
                    if let Err(e) = painted {
                        eprintln!("window {}: {e}", request.key);
                        continue;
                    }
                    if let Some(owner) = owner {
                        if let Some(owner_pane) = self.pane_of(owner) {
                            if let Ok(at) = owner_pane.window.outer_position() {
                                let size = pane.window.outer_size();
                                let (x, y) = crate::dialog_window::centered(
                                    (at.x, at.y),
                                    owner_pane.window.outer_size().into(),
                                    (size.width, size.height),
                                );
                                pane.window.set_outer_position(winit::dpi::PhysicalPosition::new(x, y));
                            }
                            // a modal one: the owner takes no input until it closes
                            if request.modal {
                                win::enable_window(owner_pane.hwnd, false);
                            }
                        }
                        pane.window.set_visible(true);
                        self.owned.push((n, owner, request.modal));
                    }
                    // (above the others once it is shown: X11's window
                    // managers take no such state from a window not shown yet)
                    if above {
                        pane.window.set_window_level(winit::window::WindowLevel::AlwaysOnTop);
                    }
                    pane.window.focus_window();
                    activate_x11(&pane.window);
                    OPEN_KEYS.with(|k| k.borrow_mut().push(request.key.clone()));
                    self.extras.push((n, request.key, pane));
                    if let Some(left) = request.left {
                        self.kept.push((n, Kept { left, shown: Instant::now(), moved: false }));
                    }
                }
                Err(e) => eprintln!("window {}: {e}", request.key),
            }
        }
    }

    fn close_extra(&mut self, n: u64) {
        // where the person left it, for the next time
        if let Some(at) = self.kept.iter().position(|(m, _)| *m == n) {
            let (_, kept) = self.kept.remove(at);
            let frame = self.extra(n).and_then(|pane| win::window_bounds(pane.hwnd));
            if let (true, Some(frame)) = (kept.moved, frame) {
                (kept.left)(frame.left, frame.top);
            }
        }
        // a dialog's owner takes input again, and the keyboard, before the
        // dialog goes (else the window system gives the keyboard to
        // another program's window)
        if let Some(at) = self.owned.iter().position(|(m, _, _)| *m == n) {
            let (_, owner, modal) = self.owned.remove(at);
            let still = self.owned.iter().any(|(_, o, m)| *o == owner && *m);
            if let Some(pane) = self.pane_of(owner) {
                if modal && !still {
                    win::enable_window(pane.hwnd, true);
                }
                pane.window.focus_window();
            }
        }
        if let Some((_, key, _)) = self.extras.iter().find(|(m, _, _)| *m == n) {
            let key = key.clone();
            OPEN_KEYS.with(|k| k.borrow_mut().retain(|open| *open != key));
        }
        // dropping the pane drops its Ui (which ends its connections)
        self.extras.retain(|(m, _, _)| *m != n);
    }

    /// The window `owner` names, if it is open.
    fn owner_of(&self, owner: &Owner) -> Option<Which> {
        match owner {
            Owner::Main => self.main.is_some().then_some(Which::Main),
            Owner::Window(key) => self.extras.iter().find(|(_, k, _)| k == key).map(|(n, _, _)| Which::Extra(*n)),
        }
    }

    fn pane_of(&self, which: Which) -> Option<&Pane> {
        match which {
            Which::Main => self.main.as_ref(),
            Which::Button => self.button.as_ref(),
            Which::Extra(n) => self.extras.iter().find(|(m, _, _)| *m == n).map(|(_, _, pane)| pane),
        }
    }

    /// The modal dialog over `which`, if one is open (the latest).
    fn modal_over(&self, which: Which) -> Option<u64> {
        self.owned.iter().rev().find(|(_, owner, modal)| *owner == which && *modal).map(|(n, _, _)| *n)
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
            // docked, in sight beside a full-screen Terminal window
            // (macOS); undocked, a window of one Space again
            if let Some(handle) = window_handle(&r.window) {
                win::over_fullscreen(handle, edge.is_some());
            }
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
                } else if inside
                    || moving
                    || win::mouse_button_down()
                    || typing
                    || self.owned.iter().any(|(_, owner, _)| *owner == Which::Main)
                {
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
                // hidden, the window is unmapped (or slid away by us): a
                // move reported then is no drag of the person's — gala
                // (elementary) reports one for the unmapped window, which
                // undocked it and left the strip dead
                let ours = self.docking.hidden
                    || self.docking.slide.is_some()
                    || self.docking.own_move_until.is_some_and(|t| t > now);
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
                // its own close button (the skin's title bar) is the
                // system's: asked first, as `CloseRequested` is
                let asked = std::mem::take(&mut pane.close_asked) && pane.ui.close_requested();
                if asked || pane.ui.wants_close() {
                    self.close_extra(n);
                }
            }
        }
        Ok(())
    }

    /// The main window closes, and the program ends.
    fn close_main(&mut self, event_loop: &ActiveEventLoop) {
        self.save_placement();
        if let Some(main) = self.main.as_mut() {
            main.ui.on_exit();
        }
        self.extras.clear();
        event_loop.exit();
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
            // the windows made: a regular application from here on (macOS)
            win::regular_application();
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
        #[cfg(all(unix, not(target_os = "macos")))]
        if let WindowEvent::ActivationTokenDone { serial, token } = &event {
            if let Some(at) = self.token_asked.iter().position(|(s, _)| s == serial) {
                let (_, answer) = self.token_asked.remove(at);
                let _ = answer.send(Some(token.clone().into_raw()));
            }
            return;
        }
        if let Some(which) = self.pane(id).map(|(which, _)| which) {
            if let Some(dialog) = self.modal_over(which) {
                if crate::dialog_window::is_input(&event) {
                    if crate::dialog_window::is_press(&event) {
                        if let Some(pane) = self.pane_of(Which::Extra(dialog)) {
                            pane.window.focus_window();
                            activate_x11(&pane.window);
                        }
                    }
                    return;
                }
            }
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
            WindowEvent::CloseRequested if which == Which::Main => self.close_main(event_loop),
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
                    Which::Extra(n) => {
                        if let WindowEvent::Moved(_) = event {
                            if let Some((_, kept)) = self.kept.iter_mut().find(|(m, _)| *m == n) {
                                kept.moved |= kept.shown.elapsed() > SETTLED;
                            }
                        }
                    }
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
            #[cfg(all(unix, not(target_os = "macos")))]
            UserEvent::ActivationToken(answer) => self.ask_activation_token(answer),
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
        if self.main.as_ref().is_some_and(|main| main.close_asked) {
            self.close_main(event_loop);
            return;
        }
        self.open_requested(event_loop);
        for key in CLOSING.with(|c| std::mem::take(&mut *c.borrow_mut())) {
            if let Some(n) = self.extras.iter().find(|(_, k, _)| *k == key).map(|(n, _, _)| *n) {
                self.close_extra(n);
            }
        }
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
/// A window's handle (Windows' `HWND`): a dialog's owner.
#[cfg(windows)]
pub(crate) fn handle_of(window: &Window) -> Option<isize> {
    window_handle(window)
}

fn window_handle(window: &Window) -> Option<isize> {
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match window.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(h) => Some(h.hwnd.get()),
        RawWindowHandle::Xlib(h) => Some(h.window as isize),
        RawWindowHandle::Xcb(h) => Some(h.window.get() as isize),
        // the window's view; `native_term_os::dock` finds its NSWindow
        RawWindowHandle::AppKit(h) => Some(h.ns_view.as_ptr() as isize),
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

#[cfg(all(unix, not(target_os = "macos")))]
impl Runner {
    /// An activation token from the compositor, asked with the main
    /// window's last input (Wayland); none elsewhere, or without a window.
    fn ask_activation_token(&mut self, answer: std::sync::mpsc::Sender<Option<String>>) {
        use winit::platform::startup_notify::WindowExtStartupNotify;
        match self.main.as_ref().and_then(|main| main.window.request_activation_token().ok()) {
            Some(serial) => self.token_asked.push((serial, answer)),
            None => {
                let _ = answer.send(None);
            }
        }
    }
}

/// Where activation tokens come from (Wayland only: X11 and the others
/// bring a window forward without one): the main window, on the event
/// loop's thread, asked from another and waited for there. Never asked
/// on the event loop's own thread, which would wait for itself.
fn activation_source<T>(event_loop: &winit::event_loop::EventLoop<T>, proxy: EventLoopProxy<UserEvent>) {
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        use winit::platform::wayland::EventLoopExtWayland;
        if !event_loop.is_wayland() {
            return;
        }
        let gui = std::thread::current().id();
        native_term_os::activation::set_source(move || {
            if std::thread::current().id() == gui {
                return None;
            }
            let (tx, rx) = std::sync::mpsc::channel();
            proxy.send_event(UserEvent::ActivationToken(tx)).ok()?;
            // (the compositor answers at once; one that doesn't is let go)
            rx.recv_timeout(std::time::Duration::from_secs(1)).ok().flatten()
        });
    }
    #[cfg(not(all(unix, not(target_os = "macos"))))]
    {
        let _ = (event_loop, proxy);
    }
}

/// On X11, activated as Chromium activates its windows
/// (`native_term_os::x11_activate`: with the X server's time): KWin's focus
/// stealing prevention put a window asked for a moment after a click
/// below the window clicked (NativeTerm's docked main window, which stays
/// above the others), where winit's request alone was not honoured.
fn activate_x11(window: &winit::window::Window) {
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
        let id = match window.window_handle().map(|h| h.as_raw()) {
            Ok(RawWindowHandle::Xlib(h)) => h.window as u32,
            Ok(RawWindowHandle::Xcb(h)) => h.window.get(),
            _ => return,
        };
        // (round trips to the X server: not on the event loop's thread)
        std::thread::spawn(move || native_term_os::x11_activate::activate(id));
    }
    #[cfg(not(all(unix, not(target_os = "macos"))))]
    {
        let _ = window;
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn a_window_around_a_point() {
        let area = Some(super::win::Bounds { left: 0, top: 30, right: 1920, bottom: 1160 });
        assert_eq!(super::around((960, 600), (460, 220), area), (730, 490));
        // near the screen's edges: within the work area
        assert_eq!(super::around((10, 40), (460, 220), area), (0, 30));
        assert_eq!(super::around((1910, 1150), (460, 220), area), (1460, 940));
        // no work area known: where asked
        assert_eq!(super::around((10, 40), (460, 220), None), (-220, -70));
        // larger than the area: from its left and top
        assert_eq!(super::around((100, 100), (3000, 2000), area), (0, 30));
    }

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

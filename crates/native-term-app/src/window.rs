//! NativeTerm's window: egui on winit, painted on the CPU.
//!
//! A GPU renderer cost 74–165 MB private memory for this small, mostly
//! idle window (the driver's share, measured with an empty window). The
//! CPU renderer (`egui_software_backend`) paints a frame in a few
//! milliseconds and presents it with GDI (`softbuffer`), with no GPU
//! memory at all. The window is repainted only on input and on
//! `request_repaint`.

use std::num::NonZeroU32;
use std::rc::Rc;
use std::time::{Duration, Instant};

use egui::ViewportId;
use egui_software_backend::{BufferMutRef, ColorFieldOrder, EguiSoftwareRender};
use egui_winit::accesskit_winit;
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::window::{Window, WindowId};
use native_term_win::dock as win;

use crate::dock::{self, Edge, Slide};

/// What the window shows.
pub trait Ui {
    fn ui(&mut self, ui: &mut egui::Ui);
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
}

enum UserEvent {
    Repaint { when: Instant, pass: u64 },
    AccessKit(accesskit_winit::Event),
}

impl From<accesskit_winit::Event> for UserEvent {
    fn from(event: accesskit_winit::Event) -> Self {
        UserEvent::AccessKit(event)
    }
}

struct Running {
    window: Rc<Window>,
    _context: softbuffer::Context<Rc<Window>>,
    surface: softbuffer::Surface<Rc<Window>, Rc<Window>>,
    state: egui_winit::State,
    info: egui::ViewportInfo,
    ui: Box<dyn Ui>,
    shown: bool,
    last_paint: Option<Instant>,
    hwnd: isize,
}

struct Runner {
    ctx: egui::Context,
    proxy: EventLoopProxy<UserEvent>,
    viewport: egui::ViewportBuilder,
    factory: Option<Factory>,
    renderer: EguiSoftwareRender,
    running: Option<Running>,
    repaint_at: Option<Instant>,
    error: Option<String>,
    /// `NATIVETERM_FRAME_LOG=<file>`: per-frame timings (diagnostics).
    frame_log: Option<std::fs::File>,
    placement: Option<Placement>,
    save: SavePlacement,
    docking: Docking,
}

/// Run the window until it is closed.
pub fn run(
    viewport: egui::ViewportBuilder,
    placement: Option<Placement>,
    save: SavePlacement,
    factory: impl FnOnce(&egui::Context) -> Box<dyn Ui> + 'static,
) -> Result<(), String> {
    let event_loop = EventLoop::<UserEvent>::with_user_event().build().map_err(|e| e.to_string())?;
    let ctx = egui::Context::default();
    let proxy = event_loop.create_proxy();
    let repaint_proxy = proxy.clone();
    ctx.set_request_repaint_callback(move |info| {
        let _ = repaint_proxy.send_event(UserEvent::Repaint {
            when: Instant::now() + info.delay,
            pass: info.current_cumulative_pass_nr,
        });
    });
    let mut runner = Runner {
        ctx,
        proxy,
        // shown after the first frame: no white flash, and AccessKit
        // must be set up before the window is visible
        viewport: viewport.with_visible(false),
        factory: Some(Box::new(factory)),
        renderer: EguiSoftwareRender::new(ColorFieldOrder::Bgra),
        running: None,
        repaint_at: None,
        error: None,
        frame_log: std::env::var_os("NATIVETERM_FRAME_LOG").and_then(|path| std::fs::File::create(path).ok()),
        placement,
        save,
        docking: Docking::default(),
    };
    event_loop.run_app(&mut runner).map_err(|e| e.to_string())?;
    match runner.error {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

impl Runner {
    fn start(&mut self, event_loop: &ActiveEventLoop) -> Result<(), String> {
        let window = Rc::new(egui_winit::create_window(&self.ctx, event_loop, &self.viewport).map_err(|e| e.to_string())?);
        let hwnd = window_handle(&window).ok_or("no window handle")?;
        if let Some(p) = self.placement {
            // only if it is still on a monitor
            if win::on_a_monitor(p.x + p.width as i32 / 2, p.y + 16) {
                let _ = window.request_inner_size(winit::dpi::PhysicalSize::new(p.width, p.height));
                window.set_outer_position(winit::dpi::PhysicalPosition::new(p.x, p.y));
            }
        }
        let context = softbuffer::Context::new(Rc::clone(&window)).map_err(|e| e.to_string())?;
        let surface = softbuffer::Surface::new(&context, Rc::clone(&window)).map_err(|e| e.to_string())?;
        let mut state = egui_winit::State::new(
            self.ctx.clone(),
            ViewportId::ROOT,
            event_loop,
            Some(window.scale_factor() as f32),
            event_loop.system_theme(),
            None,
        );
        state.init_accesskit(event_loop, &window, self.proxy.clone());
        let mut info = egui::ViewportInfo::default();
        egui_winit::update_viewport_info(&mut info, &self.ctx, &window, true);
        let factory = self.factory.take().ok_or("the window was already started")?;
        let ui = factory(&self.ctx);
        self.running = Some(Running { window, _context: context, surface, state, info, ui, shown: false, last_paint: None, hwnd });
        Ok(())
    }

    fn hwnd(&self) -> Option<isize> {
        self.running.as_ref().map(|r| r.hwnd)
    }

    /// Where the window is now (as it would be shown, if docked and hidden).
    fn placement(&self) -> Option<Placement> {
        let r = self.running.as_ref()?;
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
        let Some(edge) = self.docking.edge else { return };
        let (Some(hwnd), Some(to)) = (self.hwnd(), self.docked_target(edge, hide)) else { return };
        let Some(from) = win::window_bounds(hwnd).map(|b| (b.left, b.top)) else { return };
        if !hide {
            self.docking.hidden = false;
        }
        self.docking.slide = Some((Slide { from, to, started: Instant::now() }, hide));
        self.docking.leave_check = None;
    }

    fn dock_at(&mut self, edge: Option<Edge>) {
        if self.running.is_none() {
            return;
        }
        let was = self.docking.edge;
        self.docking.edge = edge;
        self.docking.hidden = false;
        dock::publish_edge(edge);
        if let Some(r) = &self.running {
            // through winit, which otherwise resets the level on its own
            let level = if edge.is_some() { winit::window::WindowLevel::AlwaysOnTop } else { winit::window::WindowLevel::Normal };
            r.window.set_window_level(level);
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

    /// Timers and slides; returns when to come back.
    fn tick_docking(&mut self, now: Instant) -> Option<Instant> {
        if let Some((slide, hide)) = self.docking.slide {
            let (pos, done) = slide.at(now);
            self.move_to(pos);
            if done {
                self.docking.slide = None;
                self.docking.hidden = hide;
                if !hide {
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
            } else if let Some(hwnd) = self.hwnd() {
                let maximized = self.running.as_ref().is_some_and(|r| r.window.is_maximized());
                let edge = match (win::frame_bounds(hwnd), win::work_area(hwnd)) {
                    (Some(frame), Some(work)) if !maximized => dock::snap_edge(frame, work, win::on_a_monitor),
                    _ => None,
                };
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
                let typing = self.ctx.egui_wants_keyboard_input()
                    && self.running.as_ref().is_some_and(|r| r.window.has_focus());
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
                    self.start_slide(true);
                    return Some(now);
                }
            }
        }
        [self.docking.settle_check, self.docking.leave_check].into_iter().flatten().min()
    }

    fn docking_event(&mut self, event: &WindowEvent) {
        let now = Instant::now();
        if let WindowEvent::CursorEntered { .. } = event {
            self.docking.cursor_in_client = true;
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

    fn paint(&mut self) -> Result<(), String> {
        let Some(r) = self.running.as_mut() else { return Ok(()) };
        let size = r.window.inner_size();
        let (Some(width), Some(height)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) else {
            // minimized: nothing to show; texture updates must not be
            // produced without being painted
            return Ok(());
        };
        let started = Instant::now();
        egui_winit::update_viewport_info(&mut r.info, &self.ctx, &r.window, false);
        let mut input = r.state.take_egui_input(&r.window);
        input.viewports = std::iter::once((ViewportId::ROOT, r.info.clone())).collect();
        let ui = &mut r.ui;
        let mut output = self.ctx.run_ui(input, |root| ui.ui(root));
        r.info.events.clear();
        r.state.handle_platform_output(&r.window, std::mem::take(&mut output.platform_output));
        if let Some(viewport) = output.viewport_output.remove(&ViewportId::ROOT) {
            let mut actions = Vec::new();
            egui_winit::process_viewport_commands(&self.ctx, &mut r.info, viewport.commands, &r.window, &mut actions);
            for action in actions {
                let event = match action {
                    egui_winit::ActionRequested::Cut => Some(egui::Event::Cut),
                    egui_winit::ActionRequested::Copy => Some(egui::Event::Copy),
                    egui_winit::ActionRequested::Paste => {
                        r.state.clipboard_text().map(|t| egui::Event::Paste(t.replace("\r\n", "\n")))
                    }
                    egui_winit::ActionRequested::Screenshot(_) => None,
                };
                r.state.egui_input_mut().events.extend(event);
            }
        }

        let ran = started.elapsed();
        let primitives = self.ctx.tessellate(output.shapes, output.pixels_per_point);
        let tessellated = started.elapsed();
        r.surface.resize(width, height).map_err(|e| e.to_string())?;
        let mut buffer = r.surface.buffer_mut().map_err(|e| e.to_string())?;
        buffer.fill(0);
        let pixels: &mut [u32] = &mut buffer;
        // softbuffer's 0RGB u32 in little-endian bytes is B, G, R, 0.
        // SAFETY: same size, [u8; 4] has no alignment requirement, and the
        // slice borrows `buffer` for its whole life.
        let bytes = unsafe { std::slice::from_raw_parts_mut(pixels.as_mut_ptr().cast::<[u8; 4]>(), pixels.len()) };
        let mut target = BufferMutRef::new(bytes, width.get() as usize, height.get() as usize);
        self.renderer.render(&mut target, &primitives, &output.textures_delta, output.pixels_per_point);
        let rendered = started.elapsed();
        buffer.present().map_err(|e| e.to_string())?;
        if let Some(log) = &self.frame_log {
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
        r.last_paint = Some(started);
        if !r.shown {
            r.shown = true;
            r.window.set_visible(true);
            // docked when NativeTerm was closed: dock again
            if let Some(edge) = self.placement.and_then(|p| p.edge) {
                self.dock_at(Some(edge));
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
        if self.running.is_none() {
            // paint at once: a hidden window gets no redraw
            if let Err(e) = self.start(event_loop).and_then(|()| self.paint()) {
                self.fail(event_loop, e);
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::RedrawRequested => {
                // no vsync here: animations (smooth scrolling) would repaint
                // as fast as the CPU allows, so cap at the display's rate
                if let Some(r) = &self.running {
                    let next = r.last_paint.map(|last| last + frame_interval(&r.window));
                    if let Some(next) = next.filter(|next| *next > Instant::now()) {
                        self.repaint_at = Some(self.repaint_at.map_or(next, |at| at.min(next)));
                        return;
                    }
                }
                if let Err(e) = self.paint() {
                    self.fail(event_loop, e);
                }
            }
            WindowEvent::CloseRequested => {
                self.save_placement();
                event_loop.exit();
            }
            event => {
                self.docking_event(&event);
                if let Some(r) = self.running.as_mut() {
                    if r.state.on_window_event(&r.window, &event).repaint {
                        r.window.request_redraw();
                    }
                }
            }
        }
    }

    fn user_event(&mut self, _: &ActiveEventLoop, event: UserEvent) {
        let Some(r) = self.running.as_mut() else { return };
        match event {
            UserEvent::Repaint { when, pass } => {
                let current = self.ctx.cumulative_pass_nr_for(ViewportId::ROOT);
                // a request from a pass that has been painted since is stale
                if current == pass || current == pass + 1 {
                    if when <= Instant::now() {
                        r.window.request_redraw();
                    } else {
                        self.repaint_at = Some(self.repaint_at.map_or(when, |at| at.min(when)));
                    }
                }
            }
            UserEvent::AccessKit(event) => match event.window_event {
                accesskit_winit::WindowEvent::InitialTreeRequested => {
                    self.ctx.enable_accesskit();
                    r.window.request_redraw();
                }
                accesskit_winit::WindowEvent::ActionRequested(request) => {
                    r.state.on_accesskit_action_request(request);
                    r.window.request_redraw();
                }
                accesskit_winit::WindowEvent::AccessibilityDeactivated => self.ctx.disable_accesskit(),
            },
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let now = Instant::now();
        let docking = self.tick_docking(now);
        if self.repaint_at.is_some_and(|at| at <= now) {
            self.repaint_at = None;
            if let Some(r) = &self.running {
                r.window.request_redraw();
            }
        }
        match [self.repaint_at, docking].into_iter().flatten().min() {
            Some(at) => event_loop.set_control_flow(ControlFlow::WaitUntil(at.max(now))),
            None => event_loop.set_control_flow(ControlFlow::Wait),
        }
    }
}

fn window_handle(window: &Window) -> Option<isize> {
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match window.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(h) => Some(h.hwnd.get()),
        _ => None,
    }
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

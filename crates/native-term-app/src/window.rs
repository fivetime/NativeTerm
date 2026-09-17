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

/// What the window shows.
pub trait Ui {
    fn ui(&mut self, ui: &mut egui::Ui);
}

type Factory = Box<dyn FnOnce(&egui::Context) -> Box<dyn Ui>>;

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
}

/// Run the window until it is closed.
pub fn run(
    viewport: egui::ViewportBuilder,
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
        self.running = Some(Running { window, _context: context, surface, state, info, ui, shown: false, last_paint: None });
        Ok(())
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
            WindowEvent::CloseRequested => event_loop.exit(),
            event => {
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
        match self.repaint_at {
            Some(at) if at <= Instant::now() => {
                self.repaint_at = None;
                if let Some(r) = &self.running {
                    r.window.request_redraw();
                }
                event_loop.set_control_flow(ControlFlow::Wait);
            }
            Some(at) => event_loop.set_control_flow(ControlFlow::WaitUntil(at)),
            None => event_loop.set_control_flow(ControlFlow::Wait),
        }
    }
}

/// One frame at the window's monitor refresh rate (60 Hz if unknown).
fn frame_interval(window: &Window) -> Duration {
    let millihertz = window.current_monitor().and_then(|m| m.refresh_rate_millihertz()).unwrap_or(60_000);
    Duration::from_micros(1_000_000_000 / u64::from(millihertz.max(1_000)))
}

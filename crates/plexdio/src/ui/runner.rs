//! Minimal winit + egui runner with a CPU (software) renderer.
//!
//! Why not eframe/OpenGL: many real Windows machines (VMs, RDP sessions, boxes without a GPU
//! driver) only expose the GDI OpenGL 1.1 implementation, and egui's GL backend needs 2.0+.
//! Rasterising egui's meshes on the CPU and blitting with GDI (softbuffer) works everywhere,
//! needs no GPU driver, and is cheap for a mostly-static music player UI.
//!
//! Frames are painted directly from `about_to_wait` instead of going through
//! `Window::request_redraw`. Under Wine, `RedrawWindow(RDW_INTERNALPAINT)` (what winit uses for
//! `request_redraw`) can leave the thread's `QS_PAINT` bit stuck, which turns an idle event loop
//! into a busy loop.

use egui::ViewportId;
use egui_software_backend::{BufferMutRef, ColorFieldOrder, EguiSoftwareRender};
use std::num::NonZeroU32;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::window::{Window, WindowId};

/// Window changes the app asks for after a frame: title, mini-player geometry, tray show/hide.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WindowCmd {
    Title(String),
    AlwaysOnTop(bool),
    Resizable(bool),
    /// Logical client size in points.
    Size(u32, u32),
    Hide,
    Show,
}

/// Implemented by the application.
pub trait UiApp {
    /// Build the UI for one frame.
    fn ui(&mut self, ui: &mut egui::Ui);
    /// Called once the native window exists.
    fn on_window_created(&mut self, _ctx: &egui::Context, _window: &Window) {}
    /// The user clicked the close button. Return `true` to exit, or `false` to keep running
    /// (for example after hiding the window to the tray).
    fn on_close_requested(&mut self, _ctx: &egui::Context) -> bool {
        true
    }
    /// The app wants to quit (polled after each frame).
    fn wants_exit(&self) -> bool {
        false
    }
    /// Window changes to apply after this frame.
    fn take_window_cmds(&mut self) -> Vec<WindowCmd> {
        Vec::new()
    }
    /// Hide the window to the tray instead of keeping it in the taskbar when minimized.
    fn minimizes_to_tray(&self) -> bool {
        false
    }
    /// Called once on shutdown.
    fn on_exit(&mut self) {}
}

#[derive(Debug, Clone, Copy)]
pub enum RunnerEvent {
    /// Re-evaluate repaint deadlines (sent by egui's repaint callback from any thread).
    Wake,
}

static WAKE: std::sync::OnceLock<EventLoopProxy<RunnerEvent>> = std::sync::OnceLock::new();

/// Ask for a frame from another thread. Used by the tray thread, which has no egui context.
pub fn wake() {
    if let Some(proxy) = WAKE.get() {
        let _ = proxy.send_event(RunnerEvent::Wake);
    }
}

pub struct RunnerOptions {
    pub viewport: egui::ViewportBuilder,
    pub background: egui::Color32,
}

struct Gfx {
    window: Rc<Window>,
    surface: softbuffer::Surface<Rc<Window>, Rc<Window>>,
    state: egui_winit::State,
    renderer: EguiSoftwareRender,
    viewport_info: egui::ViewportInfo,
}

struct Runner<A: UiApp> {
    app: A,
    ctx: egui::Context,
    options: RunnerOptions,
    gfx: Option<Gfx>,
    next_repaint: Arc<Mutex<Option<Instant>>>,
    needs_paint: bool,
    exiting: bool,
}

pub fn run<A: UiApp + 'static>(
    options: RunnerOptions,
    ctx: egui::Context,
    make_app: impl FnOnce(&egui::Context, EventLoopProxy<RunnerEvent>) -> A,
) -> Result<(), String> {
    let event_loop = EventLoop::<RunnerEvent>::with_user_event()
        .build()
        .map_err(|e| format!("cannot create event loop: {e}"))?;
    let proxy = event_loop.create_proxy();
    let _ = WAKE.set(proxy.clone());

    let next_repaint: Arc<Mutex<Option<Instant>>> = Arc::new(Mutex::new(None));
    {
        let next_repaint = next_repaint.clone();
        let proxy = proxy.clone();
        ctx.set_request_repaint_callback(move |info| {
            if info.viewport_id != ViewportId::ROOT {
                return;
            }
            let when = Instant::now() + info.delay.min(Duration::from_secs(3600));
            if let Ok(mut nr) = next_repaint.lock() {
                *nr = Some(nr.map_or(when, |t| t.min(when)));
            }
            // Only interrupt the event loop when the repaint is due now. Waking for a delayed
            // request would paint immediately, and that paint requests the next one: a loop that
            // burns a core while playing (the seek bar asks for 4 repaints a second). Delayed
            // requests are picked up by the `WaitUntil` deadline in `about_to_wait`.
            if info.delay.is_zero() {
                let _ = proxy.send_event(RunnerEvent::Wake);
            }
        });
    }

    let app = make_app(&ctx, proxy);
    let mut runner = Runner {
        app,
        ctx,
        options,
        gfx: None,
        next_repaint,
        needs_paint: true,
        exiting: false,
    };
    event_loop
        .run_app(&mut runner)
        .map_err(|e| format!("event loop error: {e}"))?;
    Ok(())
}

impl<A: UiApp> Runner<A> {
    fn create_gfx(&mut self, event_loop: &ActiveEventLoop) -> Result<Gfx, String> {
        let window = egui_winit::create_window(&self.ctx, event_loop, &self.options.viewport)
            .map_err(|e| format!("cannot create window: {e}"))?;
        let window = Rc::new(window);
        let context = softbuffer::Context::new(window.clone()).map_err(|e| format!("softbuffer context: {e}"))?;
        let surface =
            softbuffer::Surface::new(&context, window.clone()).map_err(|e| format!("softbuffer surface: {e}"))?;
        let state = egui_winit::State::new(
            self.ctx.clone(),
            ViewportId::ROOT,
            event_loop,
            Some(window.scale_factor() as f32),
            None,
            None,
        );
        let mut viewport_info = egui::ViewportInfo::default();
        egui_winit::update_viewport_info(&mut viewport_info, &self.ctx, &window, true);
        Ok(Gfx {
            window,
            surface,
            state,
            renderer: EguiSoftwareRender::new(ColorFieldOrder::Bgra),
            viewport_info,
        })
    }

    fn paint(&mut self, event_loop: &ActiveEventLoop) {
        let Some(gfx) = self.gfx.as_mut() else { return };
        self.needs_paint = false;
        if let Ok(mut nr) = self.next_repaint.lock() {
            *nr = None;
        }

        egui_winit::update_viewport_info(&mut gfx.viewport_info, &self.ctx, &gfx.window, false);
        let mut raw_input = gfx.state.take_egui_input(&gfx.window);
        raw_input.viewports.insert(ViewportId::ROOT, gfx.viewport_info.clone());

        let app = &mut self.app;
        let mut full_output = self.ctx.run_ui(raw_input, |ui| app.ui(ui));

        gfx.state
            .handle_platform_output_with_event_loop(&gfx.window, event_loop, full_output.platform_output);

        for cmd in self.app.take_window_cmds() {
            match cmd {
                WindowCmd::Title(t) => gfx.window.set_title(&t),
                WindowCmd::AlwaysOnTop(on) => {
                    let level = if on { winit::window::WindowLevel::AlwaysOnTop } else { winit::window::WindowLevel::Normal };
                    gfx.window.set_window_level(level);
                }
                WindowCmd::Resizable(on) => gfx.window.set_resizable(on),
                WindowCmd::Size(w, h) => {
                    let _ = gfx.window.request_inner_size(winit::dpi::LogicalSize::new(w as f64, h as f64));
                }
                WindowCmd::Hide => {
                    gfx.window.set_visible(false);
                }
                WindowCmd::Show => {
                    gfx.window.set_minimized(false);
                    gfx.window.set_visible(true);
                    gfx.window.focus_window();
                }
            }
        }

        let mut commands = Vec::new();
        let mut repaint_delay = Duration::MAX;
        for (id, out) in full_output.viewport_output {
            if id == ViewportId::ROOT {
                commands.extend(out.commands);
                repaint_delay = out.repaint_delay;
            }
        }
        let mut actions = Vec::new();
        egui_winit::process_viewport_commands(&self.ctx, &mut gfx.viewport_info, commands, &gfx.window, &mut actions);

        if repaint_delay < Duration::from_secs(3600) {
            let when = Instant::now() + repaint_delay;
            if let Ok(mut nr) = self.next_repaint.lock() {
                *nr = Some(nr.map_or(when, |t| t.min(when)));
            }
        }

        let size = gfx.window.inner_size();
        let visible = gfx.window.is_visible().unwrap_or(true) && gfx.window.is_minimized() != Some(true);
        let (Some(w), Some(h)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) else {
            full_output.textures_delta.clear();
            return;
        };
        if !visible {
            full_output.textures_delta.clear();
            return;
        }
        let primitives = self.ctx.tessellate(full_output.shapes, full_output.pixels_per_point);
        if let Err(e) = gfx.surface.resize(w, h) {
            log::error!("surface resize failed: {e}");
            full_output.textures_delta.clear();
            return;
        }
        let mut buffer = match gfx.surface.buffer_mut() {
            Ok(b) => b,
            Err(e) => {
                log::error!("surface buffer unavailable: {e}");
                full_output.textures_delta.clear();
                return;
            }
        };
        let bg = self.options.background;
        let bg_px = u32::from_be_bytes([0, bg.r(), bg.g(), bg.b()]);
        buffer.fill(bg_px);
        // softbuffer pixels are 0x00RRGGBB u32s, i.e. bytes B,G,R,0 in memory (little endian).
        let px: &mut [u32] = &mut buffer;
        // SAFETY: [u32] and [[u8; 4]] have the same size and alignment requirements are relaxed.
        let bytes: &mut [[u8; 4]] =
            unsafe { std::slice::from_raw_parts_mut(px.as_mut_ptr().cast::<[u8; 4]>(), px.len()) };
        let mut target = BufferMutRef::new(bytes, w.get() as usize, h.get() as usize);
        gfx.renderer.render(&mut target, &primitives, &full_output.textures_delta, full_output.pixels_per_point);
        // The renderer applies the deltas by reference; epaint panics if they are dropped unapplied.
        full_output.textures_delta.clear();
        if let Err(e) = buffer.present() {
            log::error!("present failed: {e}");
        }
    }
}

impl<A: UiApp> ApplicationHandler<RunnerEvent> for Runner<A> {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gfx.is_some() {
            return;
        }
        match self.create_gfx(event_loop) {
            Ok(gfx) => {
                self.app.on_window_created(&self.ctx, &gfx.window);
                self.gfx = Some(gfx);
                self.needs_paint = true;
            }
            Err(e) => {
                log::error!("{e}");
                event_loop.exit();
            }
        }
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, _event: RunnerEvent) {
        // A wake may carry new work (tray clicks, media keys) as well as a repaint deadline, and
        // repainting is cheap; the event loop goes back to waiting right after.
        self.needs_paint = true;
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(gfx) = self.gfx.as_mut() else { return };
        match &event {
            WindowEvent::CloseRequested => {
                if self.app.on_close_requested(&self.ctx) {
                    self.exiting = true;
                    event_loop.exit();
                } else {
                    self.needs_paint = true;
                }
                return;
            }
            WindowEvent::RedrawRequested => {
                // OS-initiated repaint (window uncovered, restored, ...).
                self.needs_paint = true;
                return;
            }
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => {
                self.needs_paint = true;
            }
            _ => {}
        }
        let response = gfx.state.on_window_event(&gfx.window, &event);
        if response.repaint {
            self.needs_paint = true;
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.exiting {
            return;
        }
        if self.app.minimizes_to_tray() {
            if let Some(gfx) = &self.gfx {
                if gfx.window.is_minimized() == Some(true) && gfx.window.is_visible().unwrap_or(false) {
                    gfx.window.set_visible(false);
                    log::info!("window minimized to the tray");
                }
            }
        }
        let deadline = self.next_repaint.lock().ok().and_then(|g| *g);
        let now = Instant::now();
        if self.needs_paint || deadline.is_some_and(|d| d <= now) {
            self.paint(event_loop);
            if self.app.wants_exit() {
                self.exiting = true;
                event_loop.exit();
                return;
            }
        }
        let deadline = self.next_repaint.lock().ok().and_then(|g| *g);
        event_loop.set_control_flow(match deadline {
            Some(d) if d <= Instant::now() => ControlFlow::Poll,
            Some(d) => ControlFlow::WaitUntil(d),
            None => ControlFlow::Wait,
        });
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        self.app.on_exit();
    }
}

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
    /// Smallest logical client size the user (and the OS) may resize to.
    MinSize(u32, u32),
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
    /// Texture (font atlas) updates not yet handed to the renderer. Frames run while the window
    /// is minimized or hidden still add glyphs to the atlas; dropping those deltas leaves holes
    /// in the text after restore, so they are kept until the next frame that actually renders.
    pending_textures: egui::TexturesDelta,
}

struct Runner<A: UiApp> {
    app: A,
    ctx: egui::Context,
    options: RunnerOptions,
    gfx: Option<Gfx>,
    next_repaint: Arc<Mutex<Option<Instant>>>,
    needs_paint: bool,
    exiting: bool,
    /// When the window was last checked to still be on a monitor.
    last_screen_check: Instant,
}

/// How often the window is checked to still be reachable on a monitor. Windows does not
/// always send an event when a display changes resolution, is unplugged, or wakes from sleep.
const SCREEN_CHECK: Duration = Duration::from_secs(3);

/// A screen rectangle in physical pixels: x, y, width, height.
pub type ScreenRect = (i32, i32, i32, i32);

/// New window position (x, y) and, when it had to shrink, its new size.
pub type Rescue = (i32, i32, Option<(i32, i32)>);

/// Where to put a window so its title bar strip (the top `grab_h` pixels, what the user drags
/// it by) is reachable again. `None` when it already is: at least a 120×16 px piece of the
/// strip lies on one of the `monitors`. Otherwise the window moves onto monitor `nearest`
/// (clamped inside it); `resizable` windows larger than that monitor are also shrunk to fit,
/// returned as the new size.
pub fn rescue_position(win: ScreenRect, grab_h: i32, monitors: &[ScreenRect], nearest: usize, resizable: bool) -> Option<Rescue> {
    let (x, y, w, h) = win;
    if monitors.is_empty() || w <= 0 || h <= 0 {
        return None;
    }
    let band = (x, y, w, grab_h.min(h).max(1));
    let need_w = w.min(120);
    let need_h = band.3.min(16);
    let reachable = monitors.iter().any(|m| {
        let iw = (band.0 + band.2).min(m.0 + m.2) - band.0.max(m.0);
        let ih = (band.1 + band.3).min(m.1 + m.3) - band.1.max(m.1);
        iw >= need_w && ih >= need_h
    });
    if reachable {
        return None;
    }
    let (mx, my, mw, mh) = monitors[nearest.min(monitors.len() - 1)];
    let (mut nw, mut nh) = (w, h);
    let mut size = None;
    if resizable && (w > mw || h > mh) {
        // Leave a margin so the edges can be grabbed too.
        nw = w.min((mw - 40).max(200));
        nh = h.min((mh - 80).max(150));
        size = Some((nw, nh));
    }
    let nx = if nw >= mw { mx } else { x.clamp(mx, mx + mw - nw) };
    let ny = if nh >= mh { my } else { y.clamp(my, my + mh - nh) };
    Some((nx, ny, size))
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
        last_screen_check: Instant::now(),
    };
    event_loop
        .run_app(&mut runner)
        .map_err(|e| format!("event loop error: {e}"))?;
    Ok(())
}

/// Bring the window back onto a monitor if a display change (resolution, unplugged screen,
/// sleep) left its title bar out of reach. Minimized, hidden and maximized windows are left to
/// Windows.
fn ensure_on_screen(window: &Window) {
    if window.is_minimized() == Some(true) || !window.is_visible().unwrap_or(true) || window.is_maximized() {
        return;
    }
    let Ok(pos) = window.outer_position() else { return };
    let size = window.outer_size();
    let monitors: Vec<winit::monitor::MonitorHandle> = window.available_monitors().collect();
    let rects: Vec<ScreenRect> = monitors
        .iter()
        .map(|m| (m.position().x, m.position().y, m.size().width as i32, m.size().height as i32))
        .collect();
    // The monitor Windows considers nearest, else the primary one, else the first.
    let pick = window.current_monitor().or_else(|| window.primary_monitor());
    let nearest = pick.and_then(|p| monitors.iter().position(|m| *m == p)).unwrap_or(0);
    let grab_h = (crate::ui::titlebar::HEIGHT as f64 * window.scale_factor()).round() as i32;
    let win = (pos.x, pos.y, size.width as i32, size.height as i32);
    if let Some((x, y, new_size)) = rescue_position(win, grab_h, &rects, nearest, window.is_resizable()) {
        log::info!("window was off screen at {win:?}; moving it to {x},{y} (monitors {rects:?})");
        if let Some((w, h)) = new_size {
            // Outer size minus the (frameless) window's borders is close enough to the inner size.
            let _ = window.request_inner_size(winit::dpi::PhysicalSize::new(w as u32, h as u32));
        }
        window.set_outer_position(winit::dpi::PhysicalPosition::new(x, y));
    }
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
            pending_textures: egui::TexturesDelta::default(),
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
        gfx.pending_textures.append(std::mem::take(&mut full_output.textures_delta));

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
                WindowCmd::MinSize(w, h) => {
                    gfx.window.set_min_inner_size(Some(winit::dpi::LogicalSize::new(w as f64, h as f64)));
                }
                WindowCmd::Size(w, h) => {
                    let _ = gfx.window.request_inner_size(winit::dpi::LogicalSize::new(w as f64, h as f64));
                }
                WindowCmd::Hide => {
                    gfx.window.set_visible(false);
                }
                WindowCmd::Show => {
                    gfx.window.set_minimized(false);
                    gfx.window.set_visible(true);
                    ensure_on_screen(&gfx.window);
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
        // Skipped frames keep their texture deltas in `pending_textures` for the next render.
        let (Some(w), Some(h)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) else {
            return;
        };
        if !visible {
            return;
        }
        let primitives = self.ctx.tessellate(full_output.shapes, full_output.pixels_per_point);
        if let Err(e) = gfx.surface.resize(w, h) {
            log::error!("surface resize failed: {e}");
            return;
        }
        let mut buffer = match gfx.surface.buffer_mut() {
            Ok(b) => b,
            Err(e) => {
                log::error!("surface buffer unavailable: {e}");
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
        gfx.renderer.render(&mut target, &primitives, &gfx.pending_textures, full_output.pixels_per_point);
        // The renderer applies the deltas by reference; epaint panics if they are dropped unapplied.
        gfx.pending_textures.clear();
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
            WindowEvent::Resized(_) => {
                self.needs_paint = true;
            }
            WindowEvent::ScaleFactorChanged { .. } => {
                // Usually a display change: check the window is still reachable.
                self.needs_paint = true;
                self.last_screen_check = Instant::now() - SCREEN_CHECK;
            }
            _ => {}
        }
        let response = gfx.state.on_window_event(&gfx.window, &event);
        if response.repaint {
            self.needs_paint = true;
        }
        // While the user drags a window edge, Windows runs its own modal sizing loop and
        // `about_to_wait` does not fire until the button is released, so the content would stay
        // at the old size. Paint right here instead, once egui has seen the new size.
        if matches!(event, WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. }) && !self.exiting {
            self.paint(event_loop);
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
        if self.last_screen_check.elapsed() >= SCREEN_CHECK {
            self.last_screen_check = Instant::now();
            if let Some(gfx) = &self.gfx {
                ensure_on_screen(&gfx.window);
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
        // Wake for the next screen check too (no frame is painted for it).
        let check = self.last_screen_check + SCREEN_CHECK;
        let deadline = self.next_repaint.lock().ok().and_then(|g| *g).map_or(check, |d| d.min(check));
        event_loop.set_control_flow(if deadline <= Instant::now() { ControlFlow::Poll } else { ControlFlow::WaitUntil(deadline) });
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(gfx) = self.gfx.as_mut() {
            // Quitting while minimized can leave deltas queued; epaint asserts on dropping them.
            gfx.pending_textures.clear();
        }
        self.app.on_exit();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FHD: ScreenRect = (0, 0, 1920, 1080);

    #[test]
    fn a_window_on_screen_stays_put() {
        assert_eq!(rescue_position((100, 100, 1180, 760), 32, &[FHD], 0, true), None);
        // Mostly off the right edge, but enough title bar to grab.
        assert_eq!(rescue_position((1700, 100, 1180, 760), 32, &[FHD], 0, true), None);
        // Partly on a second monitor.
        assert_eq!(rescue_position((1850, 100, 800, 600), 32, &[FHD, (1920, 0, 1920, 1080)], 0, true), None);
    }

    #[test]
    fn a_window_left_on_a_removed_monitor_comes_back() {
        // It was on a second screen at x = 2500 that is gone.
        assert_eq!(rescue_position((2500, 200, 1180, 760), 32, &[FHD], 0, true), Some((740, 200, None)));
        // Title bar above the top edge (only the bottom of the window is visible).
        assert_eq!(rescue_position((300, -700, 1180, 760), 32, &[FHD], 0, true), Some((300, 0, None)));
    }

    #[test]
    fn a_too_big_window_shrinks_after_a_resolution_drop() {
        // 1180×760 on a 1024×768 screen, title bar off to the left.
        let r = rescue_position((-1100, 10, 1180, 760), 32, &[(0, 0, 1024, 768)], 0, true);
        assert_eq!(r, Some((0, 10, Some((984, 688)))));
        // The mini-player is not resizable: only moved.
        assert_eq!(rescue_position((5000, 5000, 520, 84), 32, &[FHD], 0, false), Some((1400, 996, None)));
    }

    #[test]
    fn monitors_left_of_and_above_the_primary() {
        let left = (-1280, 0, 1280, 1024);
        assert_eq!(rescue_position((-1200, 50, 800, 600), 32, &[FHD, left], 0, true), None);
        // Unplugged left screen: back onto the primary.
        assert_eq!(rescue_position((-1200, 50, 800, 600), 32, &[FHD], 0, true), Some((0, 50, None)));
    }
}

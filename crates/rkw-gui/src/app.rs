//! The window: a `winit` event loop, a `pixels` surface, and the blit.
//!
//! There is very little policy here, which is the point. The machine paces
//! itself against the speaker ([`crate::pacing`]), the keyboard translation is
//! a table ([`crate::keys`]), and what is left is: take the newest frame if
//! there is one, turn palette indices into pixels, and hand them to `pixels`.
//!
//! # The window does not drive the machine
//!
//! A frontend built the other way round — run a frame, draw it, sleep until
//! the next 50 Hz tick — has two clocks in it, the timer's and the audio
//! device's, and they disagree by a few parts per million forever. Here the
//! emulation thread is already running at the speaker's rate, so the window's
//! only job is to notice that a frame has appeared. It polls at a few times
//! the frame rate, which costs a `Mutex` load per look, and draws when there
//! is something new.
//!
//! # Scaling
//!
//! `pixels` scales by whole numbers only — its scaling renderer takes the
//! floor of the ratio — so the picture is always crisp and always aspect
//! correct, with a letterbox where the window is not an exact multiple.
//! `--scale` picks the multiple the window opens at, which is the case where
//! there is no letterbox at all.

use std::sync::Arc;
use std::time::{Duration, Instant};

use pixels::{Pixels, SurfaceTexture};
use rkw_debug::emu::RunState;
use rkw_spectrum::{HEIGHT, PALETTE, WIDTH};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow};
use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;
use winit::window::{Fullscreen, Window, WindowId};

use crate::keys::{Action, Hotkey, translate};
use crate::session::Session;

/// How often the window looks for a new frame.
///
/// Four times the frame rate: often enough that a frame is never held back by
/// a whole poll interval, rarely enough that the look costs nothing. This is
/// not a frame clock — nothing here decides when a frame happens.
const POLL: Duration = Duration::from_millis(4);

/// The window, once there is one. `winit` creates windows on `resumed`, so
/// everything here is `Option` until the event loop has started.
struct Screen {
    window: Arc<Window>,
    pixels: Pixels<'static>,
}

/// The frontend.
pub struct App {
    session: Session,
    screen: Option<Screen>,
    scale: u32,
    fullscreen: bool,
    /// What the title said last, so it is only set when it changes: on some
    /// platforms setting a title is a round trip to the compositor.
    title: String,
}

impl App {
    pub fn new(session: Session, scale: u32, fullscreen: bool) -> App {
        App {
            session,
            screen: None,
            scale: scale.max(1),
            fullscreen,
            title: String::new(),
        }
    }

    /// Take the newest frame and paint it, or repaint the last one.
    fn redraw(&mut self) {
        let Some(screen) = &mut self.screen else {
            return;
        };
        let frame = self.session.latest_frame();
        blit(frame.pixels(), screen.pixels.frame_mut());
        if let Err(e) = screen.pixels.render() {
            eprintln!("rkw: render: {e}");
        }
    }

    /// What the title bar says: the state the user cannot see in the picture.
    fn retitle(&mut self) {
        let state = match self.session.state() {
            RunState::Running => "",
            _ => " — paused",
        };
        let speed = match self.session.speed().label() {
            "1x" => String::new(),
            label => format!(" — {label}"),
        };
        let mute = if self.session.is_muted() {
            " — muted"
        } else {
            ""
        };
        let title = format!("rkw{state}{speed}{mute}");
        if title != self.title
            && let Some(screen) = &self.screen
        {
            screen.window.set_title(&title);
            self.title = title;
        }
    }

    fn toggle_fullscreen(&mut self) {
        self.fullscreen = !self.fullscreen;
        if let Some(screen) = &self.screen {
            screen.window.set_fullscreen(
                self.fullscreen
                    .then(|| Fullscreen::Borderless(screen.window.current_monitor())),
            );
        }
    }

    fn hotkey(&mut self, hotkey: Hotkey, event_loop: &ActiveEventLoop) {
        match hotkey {
            Hotkey::Pause => {
                self.session.toggle_pause();
            }
            Hotkey::Reset => self.session.reset(),
            Hotkey::Fullscreen => self.toggle_fullscreen(),
            Hotkey::Speed => {
                self.session.cycle_speed();
            }
            Hotkey::Mute => {
                self.session.toggle_mute();
            }
            Hotkey::Quit => event_loop.exit(),
        }
        self.session.apply_mute_policy();
        self.retitle();
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.screen.is_some() {
            return;
        }
        let size = LogicalSize::new(
            (WIDTH as u32 * self.scale) as f64,
            (HEIGHT as u32 * self.scale) as f64,
        );
        let attributes = Window::default_attributes()
            .with_title("rkw")
            .with_inner_size(size)
            // Below one machine pixel per screen pixel there is nothing left
            // to look at, and `pixels` would be scaling down.
            .with_min_inner_size(LogicalSize::new(WIDTH as f64, HEIGHT as f64));
        let window = match event_loop.create_window(attributes) {
            Ok(window) => Arc::new(window),
            Err(e) => {
                eprintln!("rkw: cannot open a window: {e}");
                event_loop.exit();
                return;
            }
        };
        if self.fullscreen {
            window.set_fullscreen(Some(Fullscreen::Borderless(window.current_monitor())));
        }

        let physical = window.inner_size();
        let surface = SurfaceTexture::new(physical.width, physical.height, Arc::clone(&window));
        let pixels = match Pixels::new(WIDTH as u32, HEIGHT as u32, surface) {
            Ok(pixels) => pixels,
            Err(e) => {
                eprintln!("rkw: cannot make a surface: {e}");
                event_loop.exit();
                return;
            }
        };
        self.screen = Some(Screen { window, pixels });
        self.retitle();
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(screen) = &mut self.screen
                    && size.width > 0
                    && size.height > 0
                    && let Err(e) = screen.pixels.resize_surface(size.width, size.height)
                {
                    eprintln!("rkw: resize: {e}");
                }
            }
            WindowEvent::RedrawRequested => self.redraw(),
            // Key releases stop arriving once the window is not focused, so
            // anything held would stay held. Letting it all up is the only
            // state that is right whatever the user does next.
            WindowEvent::Focused(false) => self.session.release_all(),
            WindowEvent::KeyboardInput { event, .. } => {
                let pressed = event.state == ElementState::Pressed;
                match translate(&event.key_without_modifiers()) {
                    Action::Machine(key) => self.session.key(key, pressed),
                    // On the press, not the release: a hotkey that acted on
                    // both would toggle twice.
                    Action::Frontend(hotkey) if pressed && !event.repeat => {
                        self.hotkey(hotkey, event_loop)
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    /// Nothing to react to, so: has a frame arrived?
    ///
    /// This is where the window's rate is decided, and it is decided by
    /// asking rather than by counting. The machine publishes at the rate the
    /// speaker consumes; a poll that finds nothing new costs an atomic load.
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.session.apply_mute_policy();
        if self.session.take_frame().is_some()
            && let Some(screen) = &self.screen
        {
            screen.window.request_redraw();
        }
        self.retitle();
        event_loop.set_control_flow(ControlFlow::WaitUntil(Instant::now() + POLL));
    }

    fn exiting(&mut self, _: &ActiveEventLoop) {
        // The picture stops here; the thread is joined when the session is
        // dropped by `run`.
        self.screen = None;
    }
}

/// Palette indices to the RGBA `pixels` wants.
///
/// A hundred thousand pixels a frame, fifty times a second: five megapixels a
/// second, which is nothing for a lookup and a copy, and is why the frame
/// travels between threads as one byte per pixel rather than four.
pub fn blit(indices: &[u8], out: &mut [u8]) {
    for (index, pixel) in indices.iter().zip(out.chunks_exact_mut(4)) {
        let rgb = PALETTE[usize::from(*index) & 0x0F];
        pixel[0] = rgb[0];
        pixel[1] = rgb[1];
        pixel[2] = rgb[2];
        pixel[3] = 0xFF;
    }
}

/// Run until the window closes, and hand the machine back.
pub fn run(
    session: Session,
    scale: u32,
    fullscreen: bool,
) -> Result<(), winit::error::EventLoopError> {
    let event_loop = winit::event_loop::EventLoop::new()?;
    let mut app = App::new(session, scale, fullscreen);
    event_loop.run_app(&mut app)?;
    app.session.quit();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_palette_index_becomes_an_opaque_pixel() {
        let mut out = [0u8; 8];
        blit(&[0, 7], &mut out);
        assert_eq!(out, [0, 0, 0, 0xFF, 0xD7, 0xD7, 0xD7, 0xFF]);
    }

    /// The framebuffer is palette indices, and a byte that is not one would be
    /// a panic in the middle of a blit rather than a wrong colour.
    #[test]
    fn an_index_outside_the_palette_is_masked_rather_than_a_panic() {
        let mut out = [0u8; 4];
        blit(&[0xFF], &mut out);
        assert_eq!(out, [0xFF, 0xFF, 0xFF, 0xFF]);
    }

    #[test]
    fn a_surface_larger_than_the_frame_is_filled_as_far_as_the_frame_goes() {
        let mut out = [1u8; 12];
        blit(&[4], &mut out);
        assert_eq!(out[..4], [0x00, 0xD7, 0x00, 0xFF]);
        assert_eq!(out[4..], [1; 8]);
    }
}

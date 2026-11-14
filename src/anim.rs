//! Short, event-driven animations.
//!
//! Each animated value is a `Tween` that eases from one number to another over
//! a few milliseconds. A single window timer ticks only while at least one
//! tween is moving and is killed as soon as everything settles, so an idle
//! Snipt still uses no CPU at all.

use std::time::Instant;
use windows_sys::Win32::UI::WindowsAndMessaging::{SPI_GETCLIENTAREAANIMATION, SystemParametersInfoW};

pub const OPEN_MS: f32 = 110.0;
pub const CLOSE_MS: f32 = 90.0;
pub const SELECT_MS: f32 = 90.0;
pub const HEIGHT_MS: f32 = 130.0;
pub const HOVER_MS: f32 = 90.0;
pub const CONFIRM_MS: f32 = 110.0;
/// How far (96-DPI px) the popup slides up while fading in.
pub const SLIDE: i32 = 6;
/// How far it drops while fading out.
pub const CLOSE_SLIDE: i32 = 4;

#[derive(Clone, Copy)]
pub struct Tween {
    from: f32,
    to: f32,
    start: Instant,
    ms: f32,
}

impl Tween {
    /// A value at rest.
    pub fn at(v: f32) -> Tween {
        Tween { from: v, to: v, start: Instant::now(), ms: 0.0 }
    }

    pub fn from_to(from: f32, to: f32, ms: f32) -> Tween {
        Tween { from, to, start: Instant::now(), ms }
    }

    /// Re-targets smoothly from wherever the value is right now.
    pub fn go(&mut self, to: f32, ms: f32) {
        *self = Tween::from_to(self.value(), to, ms);
    }

    fn progress(&self) -> f32 {
        if self.ms <= 0.0 {
            return 1.0;
        }
        (self.start.elapsed().as_secs_f32() * 1000.0 / self.ms).min(1.0)
    }

    /// Current value with an ease-out cubic curve (fast start, soft landing).
    pub fn value(&self) -> f32 {
        let t = self.progress();
        let eased = 1.0 - (1.0 - t).powi(3);
        self.from + (self.to - self.from) * eased
    }

    pub fn target(&self) -> f32 {
        self.to
    }

    pub fn done(&self) -> bool {
        self.progress() >= 1.0
    }
}

pub struct Anim {
    /// Follows the Windows "Animation effects" setting; off = everything instant.
    pub enabled: bool,
    /// The animation timer is running.
    pub running: bool,
    /// Popup appearing: 0 → 1 (opacity and slide).
    pub open: Tween,
    /// The window is temporarily layered (translucent) for the fade-in.
    pub layered: bool,
    /// Popup disappearing: opacity 1 → 0 (focus is already back in your app).
    pub close: Tween,
    pub closing: bool,
    /// Final top edge of the popup, in screen pixels.
    pub base_y: i32,
    /// Window height in 96-DPI units, and the last height applied in pixels.
    pub height: Tween,
    pub applied_h: i32,
    /// Vertical offset of the selection highlight inside the list.
    pub select_y: Tween,
    /// Jump instead of glide on the next paint (new search, popup reopened).
    pub snap_select: bool,
    /// Hovered control: 0 → 1.
    pub hover: Tween,
    /// Delete confirmation row: 0 → 1, and which snippet it belongs to.
    pub confirm: Tween,
    pub confirm_for: Option<usize>,
    /// One more paint is needed to draw the final frame.
    pub repaint: bool,
}

impl Anim {
    pub fn new() -> Anim {
        Anim {
            enabled: true,
            running: false,
            open: Tween::at(1.0),
            layered: false,
            close: Tween::at(0.0),
            closing: false,
            base_y: 0,
            height: Tween::at(0.0),
            applied_h: 0,
            select_y: Tween::at(0.0),
            snap_select: true,
            hover: Tween::at(1.0),
            confirm: Tween::at(1.0),
            confirm_for: None,
            repaint: false,
        }
    }

    /// Duration to use, or 0 when animations are turned off in Windows.
    pub fn ms(&self, ms: f32) -> f32 {
        if self.enabled { ms } else { 0.0 }
    }

    /// Something drawn inside the window is still moving.
    pub fn painting(&self) -> bool {
        !self.select_y.done() || !self.hover.done() || !self.confirm.done()
    }
}

/// Windows Settings > Accessibility > Visual effects > Animation effects.
pub fn system_enabled() -> bool {
    let mut on: i32 = 1;
    unsafe { SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION, 0, &mut on as *mut i32 as _, 0) };
    on != 0
}

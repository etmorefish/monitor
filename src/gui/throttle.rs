//! Input-driven repaint throttle for the dashboard.
//!
//! egui repaints on every input event by design, and eframe on Windows turns
//! each pointer move into a full paint. For a dashboard that only redraws once
//! per sample, that is pure waste: the mouse alone would drive 60 fps and burn
//! a core (the Intel Arc GL ICD busy-waits in vsync'd `SwapBuffers`).
//!
//! The fix has three parts:
//!
//! 1. The vendored `egui-winit` no longer repaints per `CursorMoved` and no
//!    longer forwards raw `MouseMotion`; it only schedules the *next* pass at
//!    most [`egui_winit::MOVE_REPAINT_DELAY_MS`] after a move. Moving the
//!    mouse therefore never starts a paint by itself.
//! 2. [`MouseThrottle::hook`] (wired in via `App::raw_input_hook`) drops the
//!    queued `PointerMoved` events and re-injects at most one per interval,
//!    carrying the freshest position. This both caps the hover update rate and
//!    breaks egui's own feedback loop, where any processed move schedules an
//!    immediate second frame (`wants_repaint_after`), which would otherwise
//!    chain into event-rate rendering.
//! 3. `vsync: false` + [`wait_for_vblank`] in `Dashboard::ui`: the swap no
//!    longer busy-waits in the driver, and `DwmFlush` re-adds pacing as a
//!    proper kernel wait, so frames land on a vblank without tearing.

use std::time::{Duration, Instant};

use eframe::egui::{self, Event, Pos2, RawInput};

/// Throttle state for `Dashboard::raw_input_hook`.
pub struct MouseThrottle {
    /// When the last `PointerMoved` was delivered to egui.
    last_allowed: Instant,
    /// Freshest pointer position seen but not yet delivered.
    pending: Option<Pos2>,
}

impl MouseThrottle {
    pub fn new() -> Self {
        Self {
            // The first move after startup or after leaving the window is
            // delivered immediately.
            last_allowed: Instant::now() - Duration::from_millis(egui_winit::MOVE_REPAINT_DELAY_MS),
            pending: None,
        }
    }

    /// Called from `App::raw_input_hook` before every pass.
    pub fn hook(&mut self, ctx: &egui::Context, raw_input: &mut RawInput) {
        let now = Instant::now();
        let slot = Duration::from_millis(egui_winit::MOVE_REPAINT_DELAY_MS);

        let mut freshest: Option<Pos2> = None;
        let mut pointer_gone = false;
        raw_input.events.retain(|event| match event {
            Event::PointerMoved(position) => {
                freshest = Some(*position);
                false
            }
            Event::PointerGone => {
                pointer_gone = true;
                false
            }
            _ => true,
        });

        let elapsed = now.duration_since(self.last_allowed);

        if pointer_gone {
            // The pointer left the window. A leave is a discrete event, but it
            // must share the slot budget with moves: both are delivered at
            // most once per interval, otherwise a flood of PointerGone (for
            // instance synthetic input to an unfocused window) would chain
            // full frames again.
            self.pending = None;
            if elapsed >= slot {
                raw_input.events.push(Event::PointerGone);
                self.last_allowed = now;
            } else {
                ctx.request_repaint_after(slot.saturating_sub(elapsed));
            }
            return;
        }

        if let Some(position) = freshest {
            self.pending = Some(position);
        }
        if let Some(position) = self.pending {
            if elapsed >= slot {
                raw_input.events.push(Event::PointerMoved(position));
                self.pending = None;
                self.last_allowed = now;
            } else {
                // A move is waiting but this pass may not show it yet: wake up
                // exactly when the slot opens. The scheduled pass then runs the
                // same hook and delivers the freshest position.
                ctx.request_repaint_after(slot.saturating_sub(elapsed));
            }
        }
    }
}

/// Wait for the next DWM vblank.
///
/// With `vsync: false` the GL swap returns immediately (no driver busy-wait);
/// this call re-adds pacing as a proper blocking wait so presents land on a
/// vblank and the window does not tear. Cost is a kernel wait, not CPU spin.
#[cfg(target_os = "windows")]
pub fn wait_for_vblank() {
    #[link(name = "dwmapi")]
    extern "system" {
        fn DwmFlush() -> i32;
    }

    unsafe {
        DwmFlush();
    }
}

#[cfg(not(target_os = "windows"))]
pub fn wait_for_vblank() {}

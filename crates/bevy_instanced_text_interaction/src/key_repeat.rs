//! Generic key-repeat plumbing for action enums.
//!
//! Lives here (not in `bevy_instanced_text`) because key-repeat is an input-
//! timing concern, and the engine doc is explicit: "knows nothing about
//! input." Both editor and terminal want this — both have a leafwing
//! `Action` enum dispatched to typed editing/terminal events, and both
//! want held-key auto-repeat without redoing the timing logic.

use bevy::platform::time::Instant;
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

/// Held-action timing knobs. Both delays in milliseconds.
#[derive(Clone, Debug, Reflect, Serialize, Deserialize)]
#[reflect(Default, Debug)]
pub struct KeyRepeatSettings {
    /// Delay between initial press and first auto-repeat.
    pub initial_delay_ms: u64,
    /// Delay between subsequent auto-repeats.
    pub repeat_delay_ms: u64,
}

impl Default for KeyRepeatSettings {
    fn default() -> Self {
        Self {
            initial_delay_ms: 500,
            repeat_delay_ms: 50,
        }
    }
}

/// Per-input-manager key-repeat state. `Instant`s aren't `Reflect`; only
/// the action enum is observable through reflection.
///
/// Generic over the action enum so editor + terminal can each instantiate
/// `KeyRepeatState<EditorAction>` / `KeyRepeatState<TerminalAction>`.
#[derive(Component, Reflect)]
#[reflect(Component)]
pub struct KeyRepeatState<A: Send + Sync + Clone + Copy + Reflect + 'static> {
    pub current_action: Option<A>,
    #[reflect(ignore)]
    pub press_start: Option<Instant>,
    #[reflect(ignore)]
    pub last_repeat: Option<Instant>,
}

impl<A: Send + Sync + Clone + Copy + Reflect + 'static> Default for KeyRepeatState<A> {
    fn default() -> Self {
        Self {
            current_action: None,
            press_start: None,
            last_repeat: None,
        }
    }
}

impl<A: Send + Sync + Clone + Copy + Reflect + 'static> KeyRepeatState<A> {
    /// Mark `action` as freshly pressed and start its repeat clock.
    pub fn arm(&mut self, action: A, now: Instant) {
        self.current_action = Some(action);
        self.press_start = Some(now);
        self.last_repeat = None;
    }

    /// If the held action's repeat clock says fire now, return the action
    /// and update internal state. Returns `None` if no repeat is due.
    pub fn tick(&mut self, now: Instant, settings: &KeyRepeatSettings) -> Option<A> {
        let action = self.current_action?;
        let press_start = self.press_start?;
        let initial = settings.initial_delay_ms as f64 / 1000.0;
        let interval = settings.repeat_delay_ms as f64 / 1000.0;
        let elapsed = now.duration_since(press_start).as_secs_f64();
        if elapsed < initial {
            return None;
        }
        let should_repeat = match self.last_repeat {
            Some(last) => now.duration_since(last).as_secs_f64() >= interval,
            None => true,
        };
        if should_repeat {
            self.last_repeat = Some(now);
            Some(action)
        } else {
            None
        }
    }

    /// Action no longer held — stop tracking.
    pub fn release(&mut self) {
        self.current_action = None;
        self.press_start = None;
        self.last_repeat = None;
    }
}

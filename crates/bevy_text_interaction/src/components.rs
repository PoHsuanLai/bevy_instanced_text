//! Per-view interaction state components.
//!
//! These attach to entities carrying [`bevy_text_engine::TextView`] when
//! mouse + keyboard interactivity is desired. Editors typically attach
//! them automatically via `#[require]`; plain text views (chat, logs)
//! omit them when they want to be display-only.

use bevy::prelude::*;

/// Selection state for a text view. Tracks character indices into the rope.
#[derive(Component, Default, Debug, Clone)]
pub struct TextViewSelectionState {
    pub selection_start: Option<usize>,
    pub selection_end: Option<usize>,
}

/// Per-view mouse drag tracking for text selection.
#[derive(Component, Default)]
pub struct TextViewDragState {
    pub is_dragging: bool,
    pub drag_start_pos: Option<usize>,
    pub drag_start_scroll_offset: f32,
    pub last_screen_pos: Option<Vec2>,
}

/// Per-view scrolling behaviour.
///
/// Each entity carries its own `ScrollConfig` so two text views can scroll
/// at different speeds or with smooth-vs-instant independently. The scroll
/// system falls back to `ScrollConfig::default()` for entities that don't
/// have one attached.
#[derive(Component, Clone, Debug)]
pub struct ScrollConfig {
    /// Scroll speed multiplier (lines per wheel notch).
    pub speed: f32,
    /// Smooth-scroll easing toward `target_scroll_offset`.
    pub smooth: bool,
}

impl Default for ScrollConfig {
    fn default() -> Self {
        Self {
            speed: 3.0,
            smooth: true,
        }
    }
}

impl ScrollConfig {
    /// Build a `ScrollConfig` with the given scroll speed.
    pub const fn from_speed(speed: f32) -> Self {
        Self {
            speed,
            smooth: true,
        }
    }

    /// Override smooth-scroll on/off.
    pub const fn with_smooth(mut self, smooth: bool) -> Self {
        self.smooth = smooth;
        self
    }
}

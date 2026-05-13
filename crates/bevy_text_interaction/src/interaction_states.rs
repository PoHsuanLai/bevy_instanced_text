//! Per-view interaction state components.
//!
//! These attach to entities carrying [`bevy_instanced_text::TextView`] when
//! mouse + keyboard interactivity is desired. Editors typically attach
//! them automatically via `#[require]`; plain text views (chat, logs)
//! omit them when they want to be display-only.

use bevy::prelude::*;

/// Per-view mouse drag tracking for text selection.
#[derive(Component, Default, Reflect)]
#[reflect(Component, Default)]
pub struct TextViewDragState {
    pub is_dragging: bool,
    pub drag_start_pos: Option<usize>,
    pub drag_start_scroll_offset: f32,
    pub last_screen_pos: Option<Vec2>,
    /// Selection mode chosen for the active drag. Set on press (from
    /// click count + Alt modifier); read by the drag observer to expand
    /// the selection accordingly. Reset to `Simple` after release.
    pub mode: crate::selection::SelectionMode,
    /// Last press time + position for click-count detection (single /
    /// double / triple click → Simple / Semantic / Line).
    pub last_press_time: f64,
    pub last_press_pos: Option<Vec2>,
    pub click_count: u8,
}

/// Per-view scrolling behaviour.
///
/// Each entity carries its own `ScrollConfig` so two text views can scroll
/// at different speeds or with smooth-vs-instant independently. The scroll
/// system falls back to `ScrollConfig::default()` for entities that don't
/// have one attached.
#[derive(Component, Clone, Debug, Reflect)]
#[reflect(Component, Debug)]
pub struct ScrollConfig {
    /// Scroll speed multiplier (lines per wheel notch).
    pub speed: f32,
    /// Smooth-scroll easing toward `target_scroll_offset`.
    pub smooth: bool,
    pub smooth_scroll_duration: f32,
}

impl Default for ScrollConfig {
    fn default() -> Self {
        Self {
            speed: 3.0,
            smooth: true,
            smooth_scroll_duration: 0.125,
        }
    }
}

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

/// Per-view scrolling behaviour. Field names mirror Monaco
/// `IEditorOptions` scroll knobs.
#[derive(Component, Clone, Debug, Reflect)]
#[reflect(Component, Debug)]
pub struct ScrollConfig {
    /// Mouse-wheel scroll multiplier (lines per notch).
    pub mouse_wheel_scroll_sensitivity: f32,
    pub smooth_scrolling: bool,
    pub smooth_scroll_duration: f32,
    pub scroll_beyond_last_line: bool,
    pub scroll_beyond_last_column: u32,
    pub mouse_wheel_zoom: bool,
    pub fast_scroll_sensitivity: f32,
    pub scroll_predominant_axis: bool,
    pub reveal_horizontal_right_padding: f32,
    pub scrollbar: ScrollbarConfig,
}

#[derive(Clone, Debug, Reflect)]
#[reflect(Debug)]
pub struct ScrollbarConfig {
    pub vertical: ScrollbarVisibility,
    pub horizontal: ScrollbarVisibility,
    pub vertical_scrollbar_size: f32,
    pub horizontal_scrollbar_size: f32,
    pub scroll_by_page: bool,
    pub always_consume_mouse_wheel: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Reflect)]
#[reflect(Debug, PartialEq)]
pub enum ScrollbarVisibility {
    #[default]
    Auto,
    Visible,
    Hidden,
}

impl Default for ScrollbarConfig {
    fn default() -> Self {
        Self {
            vertical: ScrollbarVisibility::Auto,
            horizontal: ScrollbarVisibility::Auto,
            vertical_scrollbar_size: 14.0,
            horizontal_scrollbar_size: 12.0,
            scroll_by_page: false,
            always_consume_mouse_wheel: true,
        }
    }
}

impl Default for ScrollConfig {
    fn default() -> Self {
        Self {
            mouse_wheel_scroll_sensitivity: 3.0,
            smooth_scrolling: true,
            smooth_scroll_duration: 0.125,
            scroll_beyond_last_line: true,
            scroll_beyond_last_column: 5,
            mouse_wheel_zoom: false,
            fast_scroll_sensitivity: 5.0,
            scroll_predominant_axis: true,
            reveal_horizontal_right_padding: 30.0,
            scrollbar: ScrollbarConfig::default(),
        }
    }
}

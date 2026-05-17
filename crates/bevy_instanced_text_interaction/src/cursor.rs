//! Per-app cursor appearance + caret/blink helpers.
//!
//! The shape primitives + blink math live here so any text widget — editor,
//! terminal, REPL — can render its own caret without re-deriving the timing
//! curve. The cursor's display position (rope offset → glyph pixel) is the
//! caller's responsibility; this module just turns "I want to draw a caret
//! at row R, x X" into a `RectOverlay` and answers "should the caret be
//! visible right now?".

use bevy::prelude::*;
use bevy_instanced_text::{CornerRadii, RectOverlay, RowVertical};
use serde::{Deserialize, Serialize};

use crate::key_repeat::KeyRepeatSettings;

/// Per-entity cursor appearance + key-repeat timing. Cascaded onto every
/// `TextEditor` (and `BevyTerminal`) by `#[require]`, so the simple case
/// — one editor, default look — needs no extra spawn boilerplate. Hosts
/// with multiple editors that want distinct cursor styles override the
/// component on the affected entity at spawn time:
///
/// ```rust,ignore
/// commands.spawn((
///     TextEditor,
///     CursorSettings { blink_rate: 0.0, ..default() },
/// ));
/// ```
#[derive(Clone, Debug, Component, Serialize, Deserialize, Reflect)]
#[reflect(Component, Default, Debug)]
pub struct CursorSettings {
    pub style: CursorStyle,
    pub blinking: CursorBlinkingMode,
    pub smooth_caret_animation: SmoothCaretAnimation,
    /// In pixels; for `Line` and `Underline` styles.
    pub width: f32,
    /// Fraction of line height.
    pub height_multiplier: f32,
    /// Seconds per blink cycle; 0 = no blink.
    pub blink_rate: f32,
    pub animation_speed: f32,
    /// Seconds the caret stays solid after the cursor moves before
    /// resuming the blink animation. macOS uses ~0.5s, Windows ~0.53s,
    /// GNOME ~1.2s — set to match the host platform's convention.
    pub blink_pause_secs: f64,
    pub surrounding_lines: u32,
    pub surrounding_lines_style: SurroundingLinesStyle,
    pub overtype_style: CursorStyle,
    pub overtype_on_paste: bool,
    pub key_repeat: KeyRepeatSettings,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, Reflect)]
#[reflect(Debug, PartialEq)]
pub enum CursorStyle {
    #[default]
    Line,
    Block,
    Underline,
    LineThin,
    BlockOutline,
    UnderlineThin,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, Reflect)]
#[reflect(Debug, PartialEq)]
pub enum CursorBlinkingMode {
    #[default]
    Blink,
    Smooth,
    Phase,
    Expand,
    Solid,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, Reflect)]
#[reflect(Debug, PartialEq)]
pub enum SmoothCaretAnimation {
    Off,
    Explicit,
    #[default]
    On,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, Reflect)]
#[reflect(Debug, PartialEq)]
pub enum SurroundingLinesStyle {
    #[default]
    Default,
    All,
}

impl Default for CursorSettings {
    fn default() -> Self {
        Self {
            style: CursorStyle::Line,
            blinking: CursorBlinkingMode::Blink,
            smooth_caret_animation: SmoothCaretAnimation::On,
            width: 2.0,
            height_multiplier: 1.0,
            blink_rate: 0.5,
            animation_speed: 10.0,
            blink_pause_secs: 0.5,
            surrounding_lines: 0,
            surrounding_lines_style: SurroundingLinesStyle::Default,
            overtype_style: CursorStyle::Block,
            overtype_on_paste: true,
            key_repeat: KeyRepeatSettings::default(),
        }
    }
}

/// Per-entity blink-phase timestamp.
///
/// The "did the cursor just move" detector is domain-specific (an editor
/// watches a char offset; a terminal watches a grid cell), so each consumer
/// runs its own detector and writes `last_change_secs = now_secs` whenever
/// it sees a move. [`cursor_blink_visible`] reads this value to decide
/// whether the caret is in the solid post-move window or in the regular
/// blink phase.
#[derive(Component, Default, Reflect)]
#[reflect(Component, Default)]
pub struct BlinkPhase {
    pub last_change_secs: f64,
}

/// Returns whether the caret should be drawn this frame.
///
/// `now_secs` is the current time (e.g., `time.elapsed_secs_f64()`).
/// `last_move_secs` is when the cursor last moved (in the same clock) —
/// typically [`BlinkPhase::last_change_secs`].
/// `blink_rate` of `0.0` disables blinking — the caret stays visible.
/// `pause_secs` is the post-move solid window before blinking resumes.
pub fn cursor_blink_visible(
    blink_rate: f32,
    pause_secs: f64,
    now_secs: f64,
    last_move_secs: f64,
) -> bool {
    if blink_rate == 0.0 {
        return true;
    }
    let time_since_move = now_secs - last_move_secs;
    if time_since_move < pause_secs {
        return true;
    }
    let blink_time = (time_since_move - pause_secs) as f32;
    let phase = (blink_time * blink_rate) % 1.0;
    phase < 0.5
}

/// Build a caret `RectOverlay` for the given display row + horizontal pixel.
///
/// Uses `z = 1` to draw above text. The caller is expected to drain previous-
/// frame caret rects (those with `z == 1`) before pushing a new one.
pub fn caret_overlay(
    display_row: u32,
    x_left: f32,
    settings: &CursorSettings,
    color: Color,
) -> RectOverlay {
    let x_right = x_left + caret_width(settings);
    RectOverlay {
        display_row,
        x_range: x_left..x_right,
        vertical: caret_vertical(settings),
        color,
        z: 1,
        corners: CornerRadii::ZERO,
    }
}

fn caret_width(settings: &CursorSettings) -> f32 {
    match settings.style {
        CursorStyle::Line | CursorStyle::LineThin => settings.width,
        CursorStyle::Block
        | CursorStyle::BlockOutline
        | CursorStyle::Underline
        | CursorStyle::UnderlineThin => settings.width.max(1.0),
    }
}

fn caret_vertical(settings: &CursorSettings) -> RowVertical {
    match settings.style {
        CursorStyle::Line
        | CursorStyle::LineThin
        | CursorStyle::Block
        | CursorStyle::BlockOutline => RowVertical::Caret {
            height_fraction: settings.height_multiplier,
        },
        CursorStyle::Underline | CursorStyle::UnderlineThin => RowVertical::BottomBand {
            thickness: settings.height_multiplier.max(1.0),
        },
    }
}

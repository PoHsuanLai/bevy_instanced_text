//! Rope buffer + scroll state + content metrics for a scrollable text-rendering entity.

use bevy::prelude::*;
use ropey::Rope;

/// Source-of-truth rope and cache-invalidation key. Mutators bump
/// `content_version` so the display-map fingerprint can rebuild the layout.
#[derive(Component, Reflect)]
#[reflect(Component)]
pub struct TextBuffer {
    #[reflect(ignore)]
    pub rope: Rope,

    /// Bumped on every rope mutation; the display-map fingerprint reads this
    /// to decide whether to rebuild the layout.
    pub content_version: u64,
}

impl Default for TextBuffer {
    fn default() -> Self {
        Self {
            rope: Rope::from_str(""),
            content_version: 0,
        }
    }
}

impl TextBuffer {
    pub fn with_text(text: &str) -> Self {
        Self {
            rope: Rope::from_str(text),
            content_version: 1,
        }
    }

    pub fn line_count(&self) -> usize {
        self.rope.len_lines()
    }

    pub fn text(&self) -> String {
        self.rope.to_string()
    }

    pub fn set_text(&mut self, text: &str) {
        self.rope = Rope::from_str(text);
        self.content_version += 1;
    }

    /// Bump `content_version` to force a layout rebuild on the next frame.
    /// Use after any rope mutation that didn't go through `set_text`.
    pub fn bump_version(&mut self) {
        self.content_version += 1;
    }
}

/// Vertical and horizontal scroll offsets and their smooth-scroll targets.
#[derive(Component, Reflect)]
#[reflect(Component)]
pub struct ScrollState {
    pub scroll_offset: f32,
    pub target_scroll_offset: f32,
    pub horizontal_scroll_offset: f32,
    pub target_horizontal_scroll_offset: f32,
    /// Animation duration in seconds. Synced from `ScrollConfig::smooth_scroll_duration`.
    pub smooth_scroll_duration: f32,
    #[reflect(ignore)]
    pub vertical_anim: Option<ScrollAnimation>,
    #[reflect(ignore)]
    pub horizontal_anim: Option<ScrollAnimation>,
}

impl Default for ScrollState {
    fn default() -> Self {
        Self {
            scroll_offset: 0.0,
            target_scroll_offset: 0.0,
            horizontal_scroll_offset: 0.0,
            target_horizontal_scroll_offset: 0.0,
            smooth_scroll_duration: 0.125,
            vertical_anim: None,
            horizontal_anim: None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ScrollAnimation {
    pub from: f32,
    pub to: f32,
    pub elapsed: f32,
    pub duration: f32,
    pub composite: Option<CompositeStops>,
}

/// Two-stage composite curve for jumps > 2.5× viewport; avoids the floaty
/// tail that a single easeOutCubic produces over large distances.
#[derive(Clone, Debug)]
pub struct CompositeStops {
    pub stop1: f32,
    pub stop2: f32,
    pub split: f32,
}

/// Recomputable layout cache — widest shaped line, used by external scroll UI to size horizontal extent.
#[derive(Component, Default, Reflect)]
#[reflect(Component, Default)]
pub struct ContentMetrics {
    pub max_content_width: f32,
}

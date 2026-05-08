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
#[derive(Component, Default, Reflect)]
#[reflect(Component, Default)]
pub struct ScrollState {
    pub scroll_offset: f32,
    pub target_scroll_offset: f32,
    pub horizontal_scroll_offset: f32,
    pub target_horizontal_scroll_offset: f32,
}

/// Recomputable layout cache — widest shaped line, used by external scroll UI to size horizontal extent.
#[derive(Component, Default, Reflect)]
#[reflect(Component, Default)]
pub struct ContentMetrics {
    pub max_content_width: f32,
}

//! Plain-data Components that plug into `produce_layouts`.
//!
//! The engine's layout system queries each `TextView` entity for these
//! components. They're optional: an entity without [`HiddenLines`] shows every
//! line; one without [`LineStyles`] renders with `DisplayLayout::default_fg`.

use bevy::prelude::*;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use super::glyph::TextFormat;

/// Optional Component on a `TextView` entity selecting which buffer lines
/// the engine renders. Absent ⇒ every line is visible.
///
/// `Arc<HashSet>` so cloning during change-detection is cheap. Producers
/// write a fresh `Arc::new(set)` on each refresh.
#[derive(Component, Default, Clone)]
pub struct HiddenLines(pub Arc<HashSet<usize>>);

impl HiddenLines {
    pub fn new(lines: HashSet<usize>) -> Self {
        Self(Arc::new(lines))
    }

    pub fn is_visible(&self, buffer_line: usize) -> bool {
        !self.0.contains(&buffer_line)
    }
}

/// Optional Component on a `TextView` entity carrying styled runs per
/// buffer line. Absent ⇒ every line renders with `default_fg`.
///
/// Producers (e.g. the editor's syntax-styling system) compute styled runs
/// for the visible buffer-line window via the shared
/// [`super::layout_builder::visible_buffer_range`] helper, build a fresh
/// `HashMap`, and write a new `LineStyles` Component.
///
/// **Single-writer rule**: at most one system per entity should write
/// `LineStyles` per frame. Two producers writing to the same entity will
/// silently overwrite each other.
#[derive(Component, Default, Clone)]
pub struct LineStyles {
    /// Maps `buffer_line → styled runs`. Sparse: only the visible window is
    /// populated. Lines absent from the map render plain.
    pub by_line: Arc<HashMap<u32, Vec<FormattedSpan>>>,
}

impl LineStyles {
    pub fn new(by_line: HashMap<u32, Vec<FormattedSpan>>) -> Self {
        Self {
            by_line: Arc::new(by_line),
        }
    }

    /// Returns the runs for `buffer_line`, or `None` if it isn't styled.
    pub fn get(&self, buffer_line: u32) -> Option<&Vec<FormattedSpan>> {
        self.by_line.get(&buffer_line)
    }
}

/// Soft-wrap configuration Component. Mirrors Bevy's `TextBounds` name.
///
/// `width = None` disables wrap (one display row per visible buffer line).
/// When set, lines wider than `width` split into multiple continuation rows,
/// each inset by `indent_px`.
#[derive(Component, Clone, Copy, Debug, Reflect)]
#[reflect(Component, Default)]
pub struct TextBounds {
    /// Pixel width budget for a row. `None` ⇒ no wrap.
    pub width: Option<f32>,
    /// Continuation-row left inset in pixels.
    pub indent_px: f32,
}

impl Default for TextBounds {
    fn default() -> Self {
        Self {
            width: None,
            indent_px: 0.0,
        }
    }
}

/// One styled span: text payload plus its format. The element type of
/// [`LineStyles`].
///
/// Producers concatenate `text` payloads to form the line that gets shaped;
/// the engine then rebases each span's `format.byte_range` to match its
/// position in the concatenation. `format.byte_range` on input is ignored —
/// set it to `0..0` (or anything) when constructing.
///
/// `is_virtual` marks the span as inline decoration text — it participates
/// in shaping (subsequent glyphs are pushed right) but is invisible to
/// byte-addressed APIs like cursor movement, selection, and
/// `DisplayLayout::x_at_byte` / `byte_at_x`. Use for inlay hints, ghost-
/// text autosuggest, inline diff annotations — any text that should render
/// inline but isn't part of the source buffer.
#[derive(Clone, Debug)]
pub struct FormattedSpan {
    pub text: String,
    pub format: TextFormat,
    pub is_virtual: bool,
}

//! Plain-data Components that plug into `produce_layouts`
//! and `produce_block_layout`.
//!
//! The engine's layout systems query each `TextView` entity for these
//! components. They're optional: an entity without [`HiddenLines`] shows every
//! line; one without [`LineStyles`] renders with `DisplayLayout::default_fg`.

use bevy::prelude::*;
use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::sync::Arc;

use super::snapshot::{Block, StyleRun};

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
/// `HashMap`, and write a new `LineStyles` Component. Lines outside
/// `covered` (or with no map entry) fall back to plain text.
#[derive(Component, Default, Clone)]
pub struct LineStyles {
    /// Maps `buffer_line → styled runs`. Sparse: only the producer's
    /// covered window is populated. Lines absent from the map render plain.
    pub by_line: Arc<HashMap<u32, Vec<RunWithText>>>,
    /// Buffer-line range the producer styled this frame. Engine reads this
    /// to detect "we scrolled past what was styled" and falls back to plain
    /// for those rows until the producer catches up next frame.
    pub covered: Range<u32>,
}

impl LineStyles {
    pub fn new(by_line: HashMap<u32, Vec<RunWithText>>, covered: Range<u32>) -> Self {
        Self {
            by_line: Arc::new(by_line),
            covered,
        }
    }

    /// Returns the runs for `buffer_line`, or `None` if it isn't styled.
    pub fn get(&self, buffer_line: u32) -> Option<&Vec<RunWithText>> {
        self.by_line.get(&buffer_line)
    }
}

/// Optional Component on a `TextView` entity that drives the static-content
/// path. When present, `produce_block_layout` reads
/// the blocks each frame (gated by `Changed<BlockList>`) and writes the
/// entity's `DisplayLayout`. Mutually exclusive with the rope-driven
/// [`LineStyles`] flow — an entity uses one or the other.
///
/// `Arc<Vec<Block>>` so updates can swap the whole list without copying;
/// cloning is cheap (refcount bump).
#[derive(Component, Default, Clone)]
pub struct BlockList(pub Arc<Vec<Block>>);

impl BlockList {
    pub fn new(blocks: Vec<Block>) -> Self {
        Self(Arc::new(blocks))
    }
}

/// Soft-wrap configuration Component.
///
/// `budget_px = None` disables wrap (one display row per visible buffer
/// line). When set, lines wider than `budget_px` split into multiple
/// continuation rows, each inset by `indent_px`.
#[derive(Component, Clone, Copy, Debug, Reflect)]
#[reflect(Component, Default)]
pub struct LayoutWrap {
    /// Pixel width budget for a row. `None` ⇒ no wrap.
    pub budget_px: Option<f32>,
    /// Continuation-row left inset in pixels.
    pub indent_px: f32,
}

impl Default for LayoutWrap {
    fn default() -> Self {
        Self {
            budget_px: None,
            indent_px: 0.0,
        }
    }
}

/// One styled run plus its text payload, the element type of [`LineStyles`].
/// Producers concatenate `text` payloads to form the line that gets shaped;
/// the engine then rebases each run's `byte_range` to match.
///
/// `run.byte_range` on input is ignored — the engine overwrites it with the
/// correct range based on the position of `text` in the concatenation. Set
/// it to `0..0` (or anything) when constructing.
#[derive(Clone, Debug)]
pub struct RunWithText {
    pub text: String,
    pub run: StyleRun,
}

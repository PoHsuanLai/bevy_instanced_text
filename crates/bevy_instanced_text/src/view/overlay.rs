//! Paint-time overlays: cursor, selection, line highlights, bracket matches.
//!
//! Overlays are decoration the editor (or any consumer) writes *alongside* the
//! display layout. The renderer reads them during the same pass and emits quads
//! into the same instance buffer as glyphs (sharing the atlas's `solid_uv`).
//!
//! Single-writer rule: each system that produces overlays must `clear()` first
//! and append, so the rect list rebuilds each frame. Bumping `version` skips
//! the GPU upload when nothing changed.

use bevy::prelude::*;
use std::ops::Range;

use super::pipeline::DisplayLayout;

/// Per-entity list of decoration rectangles painted over the text (cursors,
/// selections, bracket highlights, line bands). Cleared and rebuilt each frame
/// by the systems that own each overlay type.
#[derive(Component, Default, Clone, Reflect)]
#[reflect(Component, Default)]
pub struct TextViewOverlays {
    pub rects: Vec<RectOverlay>,
}

impl TextViewOverlays {
    /// Reset for a fresh frame. Call once at the start of `OverlaySet`.
    pub fn clear(&mut self) {
        self.rects.clear();
    }
}

/// A rectangle drawn anchored to a display row.
///
/// `display_row` indexes into `DisplayLayout.lines`; `x_range` is in pixels
/// relative to the row's text origin. `0.0..f32::MAX` covers the full line.
///
/// `vertical` declares *what kind* of decoration this is — `Full` for selection
/// backgrounds, `Caret` for cursors, `TopBand`/`BottomBand` for cursor-line
/// borders, `UnderBaseline` for underlines/squiggles. The renderer translates
/// these into pixels using the row's geometry. Producers never compute Y.
///
/// `corners` carries per-corner radii so multi-row block backgrounds
/// (the first row rounds top-left/top-right, the last row rounds
/// bottom-left/bottom-right, middle rows are sharp) read as a single
/// continuous panel. Use [`CornerRadii::uniform`] for the common case.
///
/// **Prefer `RectOverlay` over spawning Sprites** for any decoration
/// that's row-aligned (highlights, selections, bracket boxes, indent
/// guides, gutter bars). Overlays go through the engine's instanced
/// batch in the same draw call as glyphs, share its atlas, and use the
/// engine's row-anchor convention by definition — they can't drift.
/// Reach for [`super::bounds::RowMetrics`] only when the decoration
/// genuinely can't be a rect (custom mesh, popup, Bevy UI node).
#[derive(Clone, Debug, Reflect)]
#[reflect(Debug)]
pub struct RectOverlay {
    pub display_row: u32,
    pub x_range: Range<f32>,
    pub vertical: RowVertical,
    pub color: Color,
    /// Z order: -1 = below text (selection bg, line highlight), +1 = above text (carets).
    pub z: i8,
    pub corners: CornerRadii,
}

/// Per-corner radii in pixels. `0.0` = sharp corner. The renderer's SDF
/// uses the matching radius for each quadrant of the quad, so a rect
/// with `tl = tr = R, bl = br = 0` rounds only its top corners — the
/// pattern needed for the first row of a multi-row code-block panel.
#[derive(Clone, Copy, Debug, Default, PartialEq, Reflect)]
#[reflect(Default, Debug, PartialEq)]
pub struct CornerRadii {
    pub tl: f32,
    pub tr: f32,
    pub bl: f32,
    pub br: f32,
}

impl CornerRadii {
    pub const ZERO: Self = Self {
        tl: 0.0,
        tr: 0.0,
        bl: 0.0,
        br: 0.0,
    };

    /// All four corners share the same radius. Cursor / caret / single-row
    /// backgrounds use this; multi-row panels use the per-corner ctors below.
    pub const fn uniform(r: f32) -> Self {
        Self {
            tl: r,
            tr: r,
            bl: r,
            br: r,
        }
    }

    /// Round only the top corners (used on the first row of a multi-row
    /// block background).
    pub const fn top(r: f32) -> Self {
        Self {
            tl: r,
            tr: r,
            bl: 0.0,
            br: 0.0,
        }
    }

    /// Round only the bottom corners (used on the last row of a multi-row
    /// block background).
    pub const fn bottom(r: f32) -> Self {
        Self {
            tl: 0.0,
            tr: 0.0,
            bl: r,
            br: r,
        }
    }

    pub fn max(&self) -> f32 {
        self.tl.max(self.tr).max(self.bl).max(self.br)
    }
}

/// Semantic vertical placement within a row. Resolved to pixels by the renderer.
#[derive(Clone, Copy, Debug, Reflect)]
#[reflect(Debug)]
pub enum RowVertical {
    /// Span the row's typographic text band (cap-to-descender). Used
    /// for selection backgrounds and line-highlight bands so the rect
    /// hugs the visible text rather than straddling line-leading
    /// whitespace. Adjacent rows leave a small gap between bands —
    /// preferred when rects shouldn't visually merge across rows.
    Full,
    /// Span the row's full leaded height (`y_top .. y_top + line_height`).
    /// Used for multi-row block backgrounds (fenced code blocks,
    /// blockquotes) where adjacent rows should paint a continuous
    /// panel with no line-spacing gap between them.
    FullLeaded,
    /// Vertically centered on the row, at `height_fraction * line_height` tall.
    /// `1.0` = full row, `0.9` ≈ vscode-ish caret.
    Caret { height_fraction: f32 },
    /// Thin band along the row's top edge.
    TopBand { thickness: f32 },
    /// Thin band along the row's bottom edge.
    BottomBand { thickness: f32 },
    /// Underline below the typographic baseline (squiggle / error indicator).
    UnderBaseline { thickness: f32, gap: f32 },
    /// Strikethrough at mid-cap height (between baseline and cap-top).
    Strikethrough { thickness: f32 },
    /// Underline just below the baseline.
    Underline { thickness: f32, gap: f32 },
}

/// Where a display row sits within a multi-row span. Drives per-corner
/// rounding so a multi-row panel rounds only its outer corners — the first
/// row rounds the top, the last row rounds the bottom, middle rows are
/// sharp, and a single-row span rounds all four.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowPosition {
    Only,
    First,
    Middle,
    Last,
}

impl RowPosition {
    /// Map a position to corner radii for the common "round only the
    /// outer corners of a multi-row panel" pattern.
    pub fn corners(self, r: f32) -> CornerRadii {
        match self {
            RowPosition::Only => CornerRadii::uniform(r),
            RowPosition::First => CornerRadii::top(r),
            RowPosition::Last => CornerRadii::bottom(r),
            RowPosition::Middle => CornerRadii::ZERO,
        }
    }
}

/// Walk every display row in `layout` whose `buffer_row` falls in
/// `[first_buffer_row, last_buffer_row]` (inclusive), invoking `builder`
/// once per display row with that row's [`RowPosition`] within the span.
///
/// A buffer line can wrap into multiple display rows; this iterator collects
/// the full set first so first/middle/last classification has the full count
/// to work with. Used by markdown for code-block / blockquote / rule
/// backgrounds and by editors for fold-region / multi-row range highlights.
pub fn for_each_row_in_buffer_span(
    layout: &DisplayLayout,
    first_buffer_row: u32,
    last_buffer_row: u32,
    mut builder: impl FnMut(u32, RowPosition),
) {
    let rows: Vec<u32> = layout
        .lines
        .iter()
        .filter(|l| l.buffer_row >= first_buffer_row && l.buffer_row <= last_buffer_row)
        .map(|l| l.display_row)
        .collect();
    let len = rows.len();
    if len == 0 {
        return;
    }
    let last_idx = len - 1;
    for (i, display_row) in rows.into_iter().enumerate() {
        let pos = if len == 1 {
            RowPosition::Only
        } else if i == 0 {
            RowPosition::First
        } else if i == last_idx {
            RowPosition::Last
        } else {
            RowPosition::Middle
        };
        builder(display_row, pos);
    }
}

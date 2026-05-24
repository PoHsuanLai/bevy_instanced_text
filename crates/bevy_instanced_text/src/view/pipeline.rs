//! `DisplayLayout` — the immutable, paint-ready snapshot the renderer consumes.

use bevy::prelude::*;
use std::ops::Range;
use std::sync::Arc;

use super::glyph::ShapedLine;

/// Per-entity rendering snapshot. Written by `display_map::build_display_layout`
/// (or `trivial_layout`); read-only for the renderer.
// Not Reflect: contains Arc<Vec<ShapedLine>> with cosmic_text::CacheKey leaves
// that don't impl Reflect, and would be stale anyway (rebuilt every frame).
#[derive(Component, Clone)]
pub struct DisplayLayout {
    /// Visible-window slice of shaped lines. Shared (Arc) so scroll-only and
    /// content-only paths can swap one without rebuilding the other.
    pub lines: Arc<Vec<ShapedLine>>,
    /// Display row range covered by `lines` (absolute, into the full document).
    pub visible_rows: Range<u32>,
    /// Total display row count for the entire document (for sizing external scroll UI).
    pub total_display_rows: u32,
    pub line_height: f32,
    /// Width of one column in pixels. Monospace assumption — for proportional fonts
    /// this is a hint only and per-glyph advance from shaping wins.
    pub char_width: f32,
    /// Vertical baseline offset within a line, in pixels.
    pub baseline_offset: f32,
    /// Default foreground color when a `ShapedLine.runs` is empty.
    pub default_fg: Color,
    /// Bumps when content / wrap / fold / styling changes (anything that invalidates `lines`).
    pub version: u64,
    /// Bumps independently when only scroll changed — same `lines`, different viewport slice.
    pub scroll_version: u64,
}

impl Default for DisplayLayout {
    fn default() -> Self {
        Self {
            lines: Arc::new(Vec::new()),
            visible_rows: 0..0,
            total_display_rows: 0,
            line_height: 16.0,
            char_width: 8.0,
            baseline_offset: 0.0,
            default_fg: Color::WHITE,
            version: 0,
            scroll_version: 0,
        }
    }
}

impl DisplayLayout {
    /// Cheap nothing-changed check via pointer equality.
    pub fn lines_unchanged(&self, other: &DisplayLayout) -> bool {
        Arc::ptr_eq(&self.lines, &other.lines)
    }

    /// Total height of the document in logical pixels — equivalent to
    /// `total_display_rows * line_height`. Use this to size an external
    /// scroll UI, or as the upper clamp for [`scroll_to_bottom_target`].
    pub fn total_content_height(&self) -> f32 {
        self.total_display_rows as f32 * self.line_height
    }

    /// Maximum sensible `ScrollPosition.y` for this layout against a node of
    /// `viewport_height` logical pixels. Pins the last row to the bottom of
    /// the viewport; returns `0.0` when content fits entirely.
    ///
    /// Pair with `scroll_pos.y = layout.scroll_to_bottom_target(h)` to
    /// "scroll to end" without the `f32::MAX` hack.
    pub fn scroll_to_bottom_target(&self, viewport_height: f32) -> f32 {
        (self.total_content_height() - viewport_height).max(0.0)
    }

    /// Pixel x where source `byte` begins within `display_row`, line-local
    /// (does not include `ShapedLine.x_offset`). Uses shaped advances when
    /// present; falls back to a `char_width` walk over `text` otherwise.
    ///
    /// `byte` is a **source** byte — a byte that exists in the source
    /// buffer. Virtual spans inserted by producers don't count toward
    /// this offset; the returned x falls *after* any preceding virtual
    /// runs on the row (so a click after the inlay hint lands past it).
    ///
    /// Returns `None` if `display_row` is not in this layout's visible window.
    pub fn x_at_byte(&self, display_row: u32, byte: usize) -> Option<f32> {
        let line = self.lines.iter().find(|l| l.display_row == display_row)?;
        let concat = line.concat_byte_for_source_byte(byte);
        Some(line_x_at_byte(line, concat, self.char_width))
    }

    /// Pixel x at the right edge of a source-byte range's rendered glyphs in
    /// `display_row`, line-local. Differs from `x_at_byte(end_byte)` when a
    /// virtual span (e.g. an inlay hint) is anchored at `end_byte`:
    /// `x_at_byte(end_byte)` would jump past the virtual run, whereas this
    /// returns the source glyphs' own trailing edge — so a highlight box
    /// over `[start_byte, end_byte)` sits flush against the source text and
    /// does not engulf the adjacent inlay.
    ///
    /// Use this when sizing an overlay or background to a specific source
    /// span: bracket-match highlights, find highlights, single-character
    /// selection rendering. `x_at_byte(start)..x_after_source_range(start, end)`
    /// gives the correct rendered span for the source bytes themselves,
    /// excluding any adjacent virtual decoration.
    ///
    /// Source bytes are contiguous in concat space (virtuals can only sit
    /// between source bytes, not inside them), so the concat range is just
    /// `[concat_for(start), concat_for(start) + (end - start))`.
    ///
    /// Returns `None` if `display_row` is not in this layout's visible window.
    pub fn x_after_source_range(
        &self,
        display_row: u32,
        start_byte: usize,
        end_byte: usize,
    ) -> Option<f32> {
        let line = self.lines.iter().find(|l| l.display_row == display_row)?;
        let concat_start = line.concat_byte_for_source_byte(start_byte);
        let concat_end = concat_start + end_byte.saturating_sub(start_byte);
        Some(line_x_at_byte(line, concat_end, self.char_width))
    }

    /// Source byte offset within `display_row` at pixel x (line-local).
    /// Inverse of `x_at_byte`. Snaps to the nearest cluster boundary using
    /// shaped advances when present.
    ///
    /// If the click lands inside a virtual span (an inlay hint, etc.) it
    /// snaps to the source byte at the virtual range's left edge if the
    /// click is in the range's left half, otherwise to the source byte at
    /// the right edge. Returned bytes always refer to source positions.
    ///
    /// Returns `None` if `display_row` is not in this layout's visible window.
    pub fn byte_at_x(&self, display_row: u32, x: f32) -> Option<usize> {
        let line = self.lines.iter().find(|l| l.display_row == display_row)?;
        let concat = line_byte_at_x(line, x, self.char_width);
        Some(concat_to_source_with_snap(line, concat, x))
    }

    /// Layout-local pixel position `(x, y_top)` of the given byte within
    /// `display_row`. `x` includes the row's `x_offset` (indent), `y_top`
    /// is the row's pre-computed top edge in layout-local coords.
    ///
    /// Hosts wanting world-space coordinates add the `ComputedNode`-derived
    /// origin themselves — this helper stays viewport-agnostic so it works
    /// for hosts that compose multiple viewports / RenderLayers.
    ///
    /// Use case: anchoring inline decorations (images, buttons, gauges)
    /// inside a markdown / chat / log view. Producers attach their own
    /// Components carrying `(buffer_row, byte_offset, …)` and a system
    /// reads `pos_at_byte(buffer_to_display(...))` to position child
    /// `Sprite` / `Node` entities.
    ///
    /// Returns `None` if `display_row` is not in this layout's visible window.
    pub fn pos_at_byte(&self, display_row: u32, byte: usize) -> Option<Vec2> {
        let line = self.lines.iter().find(|l| l.display_row == display_row)?;
        let concat = line.concat_byte_for_source_byte(byte);
        let x = line.x_offset + line_x_at_byte(line, concat, self.char_width);
        Some(Vec2::new(x, line.y_top))
    }

    /// Map `(buffer_row, byte_in_line)` to `(display_row, byte_in_display_row)`.
    /// Returns `None` if no row in the visible window matches.
    pub fn buffer_to_display(&self, buffer_row: u32, byte_in_line: usize) -> Option<(u32, usize)> {
        // Rows sharing a buffer_row are in ascending buffer_byte_offset.
        // Pick the largest offset that's <= byte_in_line.
        let mut best: Option<&ShapedLine> = None;
        for line in self.lines.iter() {
            if line.buffer_row != buffer_row {
                continue;
            }
            if line.buffer_byte_offset <= byte_in_line {
                best = Some(line);
            } else {
                break;
            }
        }
        let line = best?;
        let local = byte_in_line.saturating_sub(line.buffer_byte_offset);
        Some((line.display_row, local.min(line.text.len())))
    }
}

/// Translate a concat-byte hit-test result into a source-byte position,
/// snapping out of any virtual range the concat byte falls inside.
///
/// Snap direction: if `x_for_snap` is in the left half of the virtual
/// run, the click is closer to its left edge — return the source byte at
/// that edge. Otherwise return the source byte at the right edge.
fn concat_to_source_with_snap(line: &ShapedLine, concat_byte: usize, x_for_snap: f32) -> usize {
    let Some(range) = line.virtual_range_at_concat_byte(concat_byte) else {
        return line.source_byte_for_concat_byte(concat_byte, false);
    };
    // Find the x extent of the virtual range from shaped glyphs.
    let (range_left_x, range_right_x) = if let Some(shape) = &line.shape {
        let left = shape
            .glyphs
            .iter()
            .find(|g| g.byte_index >= range.start)
            .map(|g| g.x)
            .unwrap_or(0.0);
        let right = shape
            .glyphs
            .iter()
            .find(|g| g.byte_index >= range.end)
            .map(|g| g.x)
            .unwrap_or(shape.width);
        (left, right)
    } else {
        // Fallback: treat each byte as char_width-wide (rough but only used
        // when no shape is attached, which is itself an approximation).
        (range.start as f32, range.end as f32)
    };
    let mid = (range_left_x + range_right_x) * 0.5;
    let snap_right = x_for_snap >= mid;
    let edge_concat = if snap_right { range.end } else { range.start };
    line.source_byte_for_concat_byte(edge_concat, snap_right)
}

/// Line-local pixel x for a byte offset. Reused by `render.rs` for run start
/// positions and background widths.
pub(crate) fn line_x_at_byte(line: &ShapedLine, byte: usize, char_width_fallback: f32) -> f32 {
    if let Some(shape) = &line.shape {
        // Linear scan is fine — visible lines are short. Cluster starts are
        // monotonic for LTR; BiDi isn't rendered yet so scanning is correct.
        for g in &shape.glyphs {
            if g.byte_index >= byte {
                return g.x;
            }
        }
        return shape.width;
    }
    let prefix = line.text.get(..byte).unwrap_or("");
    let mut x = 0.0;
    for ch in prefix.chars() {
        if ch == '\t' {
            x += char_width_fallback * 4.0;
        } else if ch != '\n' && ch != '\r' {
            x += char_width_fallback;
        }
    }
    x
}

/// Inverse of [`line_x_at_byte`]: snap a line-local pixel x to the nearest cluster boundary.
pub(crate) fn line_byte_at_x(line: &ShapedLine, x: f32, char_width_fallback: f32) -> usize {
    if let Some(shape) = &line.shape {
        if x <= 0.0 {
            return shape.glyphs.first().map(|g| g.byte_index).unwrap_or(0);
        }
        for window in shape.glyphs.windows(2) {
            let cur = &window[0];
            let next = &window[1];
            if x < next.x {
                let mid = (cur.x + next.x) * 0.5;
                return if x < mid {
                    cur.byte_index
                } else {
                    next.byte_index
                };
            }
        }
        return line.text.len();
    }
    if char_width_fallback <= 0.0 {
        return 0;
    }
    let col = (x / char_width_fallback).max(0.0) as usize;
    let mut byte = 0;
    let mut current_col = 0;
    for ch in line.text.chars() {
        if current_col >= col || ch == '\n' || ch == '\r' {
            break;
        }
        if ch == '\t' {
            current_col += 4;
        } else {
            current_col += 1;
        }
        byte += ch.len_utf8();
    }
    byte
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout_with(total_rows: u32, line_height: f32) -> DisplayLayout {
        DisplayLayout {
            total_display_rows: total_rows,
            line_height,
            ..Default::default()
        }
    }

    /// `total_content_height` is `rows * line_height` — no rounding, no off-by-one.
    #[test]
    fn total_content_height_multiplies_rows_by_line_height() {
        let layout = layout_with(10, 21.0);
        assert!((layout.total_content_height() - 210.0).abs() < 1e-4);
    }

    /// When content fits entirely, scroll target is `0` — no negative drift.
    #[test]
    fn scroll_to_bottom_zero_when_content_fits() {
        let layout = layout_with(5, 20.0); // 100 px of content
        assert_eq!(layout.scroll_to_bottom_target(200.0), 0.0);
        assert_eq!(layout.scroll_to_bottom_target(100.0), 0.0);
    }

    /// Overflowing content pins the last row to the viewport bottom.
    #[test]
    fn scroll_to_bottom_pins_last_row() {
        let layout = layout_with(20, 20.0); // 400 px content
                                            // Viewport 100 px — bottom 100 px shows last 5 rows, so scroll target = 300.
        assert!((layout.scroll_to_bottom_target(100.0) - 300.0).abs() < 1e-4);
    }
}

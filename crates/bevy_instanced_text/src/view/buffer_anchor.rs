//! Buffer-position → node-local logical px anchoring.
//!
//! [`BufferAnchorParam`] is the higher-level companion to
//! [`crate::RowMetricsParam`]: it folds in the editor's `TextBuffer`
//! and `DisplayLayout` so LSP-flavored `(line, character)` and
//! rope-flavored `char_index` lookups land on the same screen pixel.
//!
//! Buffer→display row honors soft-wrap and folding via
//! [`DisplayLayout::buffer_to_display`] when a layout is present, else
//! a 1:1 mapping. Pixel-x uses [`DisplayLayout::x_at_byte`] with a
//! monospace fallback (`character * char_width`).
//!
//! # Example — UI-node popup
//!
//! ```ignore
//! fn render_completion(
//!     mut commands: Commands,
//!     anchors: BufferAnchorParam,
//!     popups: Query<(Entity, &CompletionPopupData)>,
//! ) {
//!     for (editor, popup) in popups.iter() {
//!         let anchor = anchors.at_buffer_pos(editor, popup.line, popup.character);
//!         commands.entity(popup_entity).insert(Node {
//!             position_type: PositionType::Absolute,
//!             left: Val::Px(anchor.below_left.x),
//!             top: Val::Px(anchor.below_left.y),
//!             ..default()
//!         });
//!     }
//! }
//! ```

use bevy::ecs::entity::Entity;
use bevy::ecs::system::{Query, SystemParam};
use bevy::math::Vec2;

use bevy::ui::ComputedNode;

use std::marker::PhantomData;

use super::anchor::{row_metrics_with_baseline, RowMetrics, DEFAULT_BASELINE_OFFSET_RATIO};
use super::font::MonoCellWidth;
use super::layout::DisplayLayout;
use super::state::{SmoothScroll, TextBuffer, TextContent};
use bevy::ui::ScrollPosition;
use bevy::text::TextFont;
use bevy::ecs::component::Component;

/// Resolved coordinates for a buffer position, in node-local logical
/// pixels (top-left origin, +Y down) — the same space `Node::top` /
/// `Node::left` consume and the same space `render_layout` emits glyph
/// positions in.
///
/// Every flavor a consumer plausibly needs is materialized once at
/// query time so callers don't pick the wrong combination of
/// `RowMetrics` / horizontal-scroll. Numeric cost: half a dozen
/// multiplies and adds — far cheaper than the cache miss from
/// re-fetching components per call site.
#[derive(Clone, Copy, Debug)]
pub struct AnchorPoint {
    /// Display row, post soft-wrap and folding when a layout is present.
    pub display_row: u32,
    /// Line-local pen-x (no `text_area_left`, no horizontal scroll).
    pub pixel_x: f32,
    /// Cell's leaded-box top-left.
    pub top_left: Vec2,
    /// `top_left + (0, line_height)`. Popups flipping below the cursor
    /// anchor here.
    pub below_left: Vec2,
    /// Glyph-band midpoint — for decorations that should sit *with* the
    /// text rather than straddling the leaded box.
    pub band_center: Vec2,
    /// Glyph baseline at this column.
    pub baseline: Vec2,
    pub line_height: f32,
    /// `top_left` lies within the visible viewport.
    pub on_screen: bool,
}

/// `SystemParam` resolving `(line, character)` or char-index to an
/// [`AnchorPoint`]. Generic over the buffer's [`TextContent`] so
/// terminals, labels, and rope-backed editors share one anchor lookup.
/// Falls back to monospace pixel-x when no layout is attached.
type BufferAnchorQuery<'w, 's, T> = Query<
    'w,
    's,
    (
        Entity,
        &'static ComputedNode,
        &'static ScrollPosition,
        &'static SmoothScroll,
        &'static TextFont,
        &'static bevy::text::LineHeight,
        &'static MonoCellWidth,
        &'static TextBuffer<T>,
        Option<&'static DisplayLayout>,
    ),
>;

#[derive(SystemParam)]
pub struct BufferAnchorParam<'w, 's, T: TextContent + Component = super::state::TextSpan> {
    query: BufferAnchorQuery<'w, 's, T>,
    _phantom: PhantomData<&'w T>,
}

impl<'w, 's, T: TextContent + Component> BufferAnchorParam<'w, 's, T> {
    /// Anchor a buffer `(line, character)` on `entity`.
    ///
    /// `character` is a byte offset within the line when a layout is
    /// attached, or a monospace cell index otherwise. LSP servers
    /// report UTF-16 code units; convert before calling for non-ASCII.
    pub fn at_buffer_pos(&self, entity: Entity, line: u32, character: u32) -> Option<AnchorPoint> {
        let (_, computed, scroll_pos, smooth, font, lh, mono, _buffer, layout) = self.query.get(entity).ok()?;
        let line_height = crate::view::font::resolve_line_height(*lh, font.font_size);
        let metrics = build_metrics(computed, scroll_pos.y, smooth.horizontal, font, line_height, mono, layout);

        let (display_row, pixel_x) =
            resolve_display_row_and_x(line, character as usize, mono, layout);

        Some(self.build_anchor(computed, line_height, &metrics, display_row, pixel_x))
    }

    /// Anchor a content `char_index` (char offset within the buffer's
    /// [`TextContent`]). Convenient for popups whose state stores the
    /// trigger position as a char offset rather than `(line, character)`.
    pub fn at_char_index(&self, entity: Entity, char_index: usize) -> Option<AnchorPoint> {
        let (_, computed, scroll_pos, smooth, font, lh, mono, buffer, layout) = self.query.get(entity).ok()?;
        let line_height = crate::view::font::resolve_line_height(*lh, font.font_size);
        let metrics = build_metrics(computed, scroll_pos.y, smooth.horizontal, font, line_height, mono, layout);

        let char_index = char_index.min(buffer.char_count());
        let line_index = buffer.char_to_line(char_index);
        let line_start = buffer.line_to_char(line_index);
        let col_chars = char_index - line_start;
        let byte_in_line = col_chars;

        let (display_row, pixel_x) =
            resolve_display_row_and_x(line_index as u32, byte_in_line, mono, layout);

        Some(self.build_anchor(computed, line_height, &metrics, display_row, pixel_x))
    }

    fn build_anchor(
        &self,
        computed: &ComputedNode,
        line_height: f32,
        metrics: &RowMetrics,
        display_row: u32,
        pixel_x: f32,
    ) -> AnchorPoint {
        let inv = computed.inverse_scale_factor();
        let logical = computed.size() * inv;

        let top_left = metrics.cell_top_left_at_x(display_row, pixel_x);
        let below_left = Vec2::new(top_left.x, top_left.y + line_height);
        let band = metrics.row_glyph_band(display_row);
        let band_center = Vec2::new(top_left.x, (band.min.y + band.max.y) * 0.5);
        let baseline = Vec2::new(top_left.x, metrics.glyph_baseline_y(display_row));

        let on_screen = top_left.x >= 0.0
            && top_left.y >= 0.0
            && top_left.x < logical.x
            && top_left.y + line_height <= logical.y;

        // `computed` is read indirectly via `metrics`.
        let _ = computed;

        AnchorPoint {
            display_row,
            pixel_x,
            top_left,
            below_left,
            band_center,
            baseline,
            line_height,
            on_screen,
        }
    }
}

fn build_metrics(
    computed: &ComputedNode,
    scroll_y: f32,
    horizontal_scroll: f32,
    font: &TextFont,
    line_height: f32,
    mono: &MonoCellWidth,
    layout: Option<&DisplayLayout>,
) -> RowMetrics {
    let baseline = layout
        .map(|l| l.baseline_offset)
        .unwrap_or(font.font_size * DEFAULT_BASELINE_OFFSET_RATIO);
    row_metrics_with_baseline(computed, scroll_y, horizontal_scroll, line_height, mono, baseline)
}

fn resolve_display_row_and_x(
    buffer_line: u32,
    byte_in_line: usize,
    mono: &MonoCellWidth,
    layout: Option<&DisplayLayout>,
) -> (u32, f32) {
    if let Some(layout) = layout {
        if let Some((display_row, byte_in_row)) =
            layout.buffer_to_display(buffer_line, byte_in_line)
        {
            let pixel_x = layout
                .x_at_byte(display_row, byte_in_row)
                .unwrap_or(byte_in_row as f32 * mono.px);
            return (display_row, pixel_x);
        }
    }
    // Fallback: 1:1 buffer-line → display-row, monospace columns.
    (buffer_line, byte_in_line as f32 * mono.px)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::font::MonoCellWidth;
    use crate::view::layout::DisplayLayout;
    use crate::view::state::SmoothScroll;
    use bevy::asset::Handle;
    use bevy::ecs::system::RunSystemOnce;
    use bevy::math::Vec2;
    use bevy::prelude::*;

    fn make_computed() -> bevy::ui::ComputedNode {
        let mut c = bevy::ui::ComputedNode::default();
        c.size = Vec2::new(800.0, 600.0);
        c.inverse_scale_factor = 1.0;
        c.padding.min_inset = Vec2::new(50.0, 8.0);
        c
    }

    fn make_editor_world() -> (World, Entity) {
        let computed = make_computed();
        // scroll_pos.y=100.0 is equivalent to the old scroll_offset=-100.0.
        let scroll_pos = bevy::ui::ScrollPosition(Vec2::new(0.0, 100.0));
        let smooth = SmoothScroll { target_y: 100.0, horizontal: 0.0, ..Default::default() };
        let font = bevy::text::TextFont::from_font_size(14.0);
        let line_height = bevy::text::LineHeight::Px(21.0);
        let mono = MonoCellWidth { px: 8.4 };
        let buffer = TextBuffer::new(super::super::state::TextSpan::new(
            "hello world\nsecond line\nthird",
        ));
        let mut layout = DisplayLayout::default();
        layout.baseline_offset = 14.0 * 0.32;

        let mut world = World::new();
        let entity = world.spawn((computed, scroll_pos, smooth, font, line_height, mono, buffer, layout)).id();
        (world, entity)
    }

    /// `at_buffer_pos` and `at_char_index` should agree when given
    /// equivalent inputs — same row, same column, same screen coords.
    /// If they drift the two LSP entry points (`(line, character)`
    /// from LSP semantic data, `char_index` from popup state) will
    /// produce visually different popups for the same logical position.
    #[test]
    fn buffer_pos_and_char_index_agree() {
        let (mut world, entity) = make_editor_world();
        let (a, b) = world
            .run_system_once(move |anchors: BufferAnchorParam<super::super::state::TextSpan>| {
                let a = anchors.at_buffer_pos(entity, 1, 3).unwrap();
                // "hello world\n" = 12 chars, "sec" = 3 → char_index 15.
                let b = anchors.at_char_index(entity, 15).unwrap();
                (a, b)
            })
            .unwrap();
        assert_eq!(a.display_row, b.display_row);
        assert!((a.pixel_x - b.pixel_x).abs() < 1e-3);
        assert!((a.top_left - b.top_left).length() < 1e-3);
    }

    /// Without a `DisplayLayout` (or with one whose
    /// `buffer_to_display` returns `None`), the helper must still
    /// produce a sensible fallback so consumers don't blow up before
    /// the first layout pass. Matches the rope→cell math the example's
    /// `cursor_screen_pos` used pre-API.
    #[test]
    fn fallback_uses_monospace_columns() {
        let computed = make_computed();
        let scroll_pos = bevy::ui::ScrollPosition::default();
        let smooth = SmoothScroll::default();
        let font = bevy::text::TextFont::from_font_size(14.0);
        let line_height = bevy::text::LineHeight::Px(21.0);
        let mono = MonoCellWidth { px: 8.4 };
        let buffer = TextBuffer::new(super::super::state::TextSpan::new("plain text"));

        let mut world = World::new();
        let entity = world.spawn((computed, scroll_pos, smooth, font, line_height, mono, buffer)).id();

        let anchor = world
            .run_system_once(move |anchors: BufferAnchorParam<super::super::state::TextSpan>| {
                anchors.at_buffer_pos(entity, 0, 5).unwrap()
            })
            .unwrap();
        // No layout → 1:1 buffer-row, monospace columns.
        assert_eq!(anchor.display_row, 0);
        assert!((anchor.pixel_x - 5.0 * 8.4).abs() < 1e-3);
    }

    /// The anchor the helper produces must match the math
    /// `cursor_screen_pos` did manually before this API existed —
    /// otherwise migrating callers shifts their popups.
    #[test]
    fn top_left_matches_legacy_cursor_screen_pos() {
        let (mut world, entity) = make_editor_world();
        let anchor = world
            .run_system_once(move |anchors: BufferAnchorParam<super::super::state::TextSpan>| {
                anchors.at_buffer_pos(entity, 1, 3).unwrap()
            })
            .unwrap();

        // Legacy formula from examples/editor_lsp.rs::cursor_screen_pos
        // — now lives entirely in `RowMetrics::cell_top_left_at_x`.
        let computed = make_computed();
        let mono = MonoCellWidth { px: 8.4 };
        let metrics = super::row_metrics_with_baseline(&computed, 100.0, 0.0, 21.0, &mono, 14.0 * 0.32);
        let expected = metrics.cell_top_left_at_x(1, 3.0 * mono.px);

        assert!((anchor.top_left.x - expected.x).abs() < 1e-3);
        assert!((anchor.top_left.y - expected.y).abs() < 1e-3);
    }

    /// The anchor the helper produces must match
    /// `RowMetrics::cell_top_left_at_x` — the helper is a convenience
    /// wrapper, not a parallel code path.
    #[test]
    fn top_left_matches_row_metrics() {
        let (mut world, entity) = make_editor_world();
        let anchor = world
            .run_system_once(move |anchors: BufferAnchorParam<super::super::state::TextSpan>| {
                anchors.at_buffer_pos(entity, 2, 4).unwrap()
            })
            .unwrap();

        let computed = make_computed();
        let mono = MonoCellWidth { px: 8.4 };
        let metrics = super::row_metrics_with_baseline(&computed, 100.0, 0.0, 21.0, &mono, 14.0 * 0.32);
        let expected = metrics.cell_top_left_at_x(2, 4.0 * mono.px);

        assert!((anchor.top_left - expected).length() < 1e-3);
    }
}

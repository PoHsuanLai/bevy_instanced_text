//! Row-anchor positioning helpers in node-local logical pixels (top-left
//! origin, +Y down) — the same space `Node::top`/`Node::left` consume.
//!
//! Prefer pushing a [`RectOverlay`] into [`TextViewOverlays`] for
//! row-aligned decorations (selections, highlight bars, indent guides);
//! reach for these helpers only when you need to position a sibling UI
//! [`Node`](bevy::ui::Node) child relative to a glyph cell.
//!
//! [`RectOverlay`]: super::overlay::RectOverlay
//! [`TextViewOverlays`]: super::overlay::TextViewOverlays
//!
//! # Example
//! ```no_run
//! # use bevy::prelude::*;
//! # use bevy_instanced_text::prelude::*;
//! # use bevy_instanced_text::{MonoCellWidth, resolve_line_height, SmoothScroll};
//! # use bevy_instanced_text::view::pipeline::DisplayLayout;
//! # use bevy_instanced_text::view::bounds::row_metrics;
//! fn position_my_popup(
//!     editor: Query<(
//!         &ComputedNode,
//!         &bevy::ui::ScrollPosition,
//!         &SmoothScroll,
//!         &TextFont,
//!         &bevy::text::LineHeight,
//!         &MonoCellWidth,
//!         &DisplayLayout,
//!     )>,
//! ) {
//!     let (computed, scroll_pos, smooth, font, lh, mono, _layout) = editor.single().unwrap();
//!     let line_height = resolve_line_height(*lh, font.font_size);
//!     let metrics = row_metrics(computed, scroll_pos, smooth, font, line_height, mono);
//!     let band = metrics.row_glyph_band(12);
//!     let popup_top_left = band.min - bevy::math::Vec2::new(0.0, 100.0);
//!     // commands.spawn(Node { left: Val::Px(popup_top_left.x), top: Val::Px(popup_top_left.y), .. });
//! }
//! ```

use bevy::math::{Rect, Vec2};
use bevy::ui::ComputedNode;

use super::font::MonoCellWidth;
use super::text::SmoothScroll;
use bevy::text::TextFont;
use bevy::ui::ScrollPosition;

/// Default baseline-offset ratio (~32% of font size), matching
/// `TextFont::from_size` and `layout_builder` defaults. Fed into
/// [`row_metrics_with_baseline`] by [`row_metrics`].
pub const DEFAULT_BASELINE_OFFSET_RATIO: f32 = 0.32;

/// Snapshot of row-anchor constants for one text view. All output
/// coords are in node-local logical px (top-left origin, +Y down).
///
/// Construct via [`row_metrics`] (canonical baseline ratio) or
/// [`row_metrics_with_baseline`] (pass `DisplayLayout::baseline_offset`
/// for byte-identical agreement with the renderer).
#[derive(Clone, Copy, Debug)]
pub struct RowMetrics {
    text_area_top_with_scroll: f32,
    text_area_left: f32,
    viewport_width: f32,
    char_width: f32,
    line_height: f32,
    baseline_offset: f32,
    horizontal_scroll: f32,
}

/// Build a [`RowMetrics`] using the canonical baseline ratio.
/// Prefer [`row_metrics_with_baseline`] passing `DisplayLayout::baseline_offset`
/// when a layout is available.
pub fn row_metrics(
    computed: &ComputedNode,
    scroll_pos: &ScrollPosition,
    smooth: &SmoothScroll,
    font: &TextFont,
    line_height: f32,
    mono: &MonoCellWidth,
) -> RowMetrics {
    row_metrics_with_baseline(
        computed,
        scroll_pos.y,
        smooth.horizontal,
        line_height,
        mono,
        font.font_size * DEFAULT_BASELINE_OFFSET_RATIO,
    )
}

/// As [`row_metrics`] but lets the caller pass an explicit
/// `baseline_offset` (e.g. read from `DisplayLayout::baseline_offset`).
pub fn row_metrics_with_baseline(
    computed: &ComputedNode,
    scroll_y: f32,
    horizontal_scroll: f32,
    line_height: f32,
    mono: &MonoCellWidth,
    baseline_offset: f32,
) -> RowMetrics {
    let inv = computed.inverse_scale_factor();
    let logical = computed.size() * inv;
    let inset = computed.content_inset();
    let text_area_left = inset.min_inset.x * inv;
    let text_area_top = inset.min_inset.y * inv;
    RowMetrics {
        text_area_top_with_scroll: text_area_top - scroll_y,
        text_area_left,
        viewport_width: logical.x,
        char_width: mono.px,
        line_height,
        baseline_offset,
        horizontal_scroll,
    }
}

impl RowMetrics {
    /// Y of `display_row`'s leaded-box top. Mirrors `layout_builder::y_top_for`.
    pub fn row_y_top(&self, display_row: u32) -> f32 {
        self.text_area_top_with_scroll + display_row as f32 * self.line_height
    }

    /// Rect covering the row's full leaded box, spanning the text area.
    pub fn row_full_box(&self, display_row: u32) -> Rect {
        self.row_full_box_with_height(display_row, self.line_height)
    }

    /// [`row_full_box`](Self::row_full_box) with an explicit per-row
    /// `line_height` override (e.g. for a shaped heading).
    pub fn row_full_box_with_height(&self, display_row: u32, line_height: f32) -> Rect {
        let y_top = self.text_area_top_with_scroll + display_row as f32 * line_height;
        let content_width = self.row_content_width();
        Rect {
            min: Vec2::new(self.text_area_left, y_top),
            max: Vec2::new(self.text_area_left + content_width, y_top + line_height),
        }
    }

    /// Rect covering the row's visible glyph band (cap-to-descender).
    /// Matches `RectOverlay { vertical: Full }` — selection backgrounds,
    /// bracket boxes, highlight bars should align here, not the full
    /// leaded box (which looks too tall).
    pub fn row_glyph_band(&self, display_row: u32) -> Rect {
        self.row_glyph_band_with_height(display_row, self.line_height)
    }

    /// [`row_glyph_band`](Self::row_glyph_band) with an explicit per-row
    /// `line_height` override.
    pub fn row_glyph_band_with_height(&self, display_row: u32, line_height: f32) -> Rect {
        // Mirrors `render::push_overlay_quad`'s `RowVertical::Full` math.
        let baseline_y_off = line_height * 0.5 + self.baseline_offset;
        let cap = baseline_y_off + self.baseline_offset * 0.6;
        let band_top_y_off = baseline_y_off - cap * 0.25;
        let y_top = self.text_area_top_with_scroll + display_row as f32 * line_height;
        let band_top = y_top + band_top_y_off;
        let content_width = self.row_content_width();
        Rect {
            min: Vec2::new(self.text_area_left, band_top),
            max: Vec2::new(self.text_area_left + content_width, band_top + cap),
        }
    }

    /// Top-left of the cell at `(display_row, column)` on a monospace
    /// grid. For shaped text, pass a layout-derived pen-x to
    /// [`cell_top_left_at_x`](Self::cell_top_left_at_x) instead.
    pub fn cell_top_left(&self, display_row: u32, column: u32) -> Vec2 {
        let pixel_x = column as f32 * self.char_width;
        self.cell_top_left_at_x(display_row, pixel_x)
    }

    /// Top-left of a cell at the given line-local pen-x (before horizontal scroll).
    pub fn cell_top_left_at_x(&self, display_row: u32, pixel_x: f32) -> Vec2 {
        Vec2::new(
            self.text_area_left + pixel_x - self.horizontal_scroll,
            self.row_y_top(display_row),
        )
    }

    /// Top-left of `(display_row, column)` snapped to the glyph band
    /// instead of the leaded box — for markers that should sit with the
    /// text rather than the row's spacing.
    pub fn cell_glyph_band_top_left(&self, display_row: u32, column: u32) -> Vec2 {
        let band = self.row_glyph_band(display_row);
        Vec2::new(
            self.text_area_left + column as f32 * self.char_width - self.horizontal_scroll,
            band.min.y,
        )
    }

    /// Y of `display_row`'s glyph baseline.
    pub fn glyph_baseline_y(&self, display_row: u32) -> f32 {
        self.row_y_top(display_row) + self.line_height * 0.5 + self.baseline_offset
    }

    pub fn cell_width(&self) -> f32 {
        self.char_width
    }

    pub fn row_height(&self) -> f32 {
        self.line_height
    }

    pub fn text_area_left(&self) -> f32 {
        self.text_area_left
    }

    fn row_content_width(&self) -> f32 {
        (self.viewport_width - self.text_area_left).max(0.0)
    }
}

/// `SystemParam` shorthand for the most common consumer pattern: take
/// any number of editor entities and look up `RowMetrics` for one of
/// them by entity. Saves the boilerplate of declaring a 4-tuple query
/// (`viewport, scroll, font, layout`) and unwrapping it on every
/// chrome-positioning system.
///
/// ```ignore
/// fn render_my_chrome(metrics: RowMetricsParam, editors: Query<Entity, With<MyEditor>>) {
///     let editor = editors.single().unwrap();
///     let m = metrics.get(editor).expect("editor has TextView components");
///     let band = m.row_glyph_band(7);
///     // ...
/// }
/// ```
///
/// `DisplayLayout` is optional — without it, falls back to the canonical
/// baseline ratio.
#[derive(bevy::ecs::system::SystemParam)]
pub struct RowMetricsParam<'w, 's> {
    query: bevy::ecs::system::Query<
        'w,
        's,
        (
            bevy::ecs::entity::Entity,
            &'static bevy::ui::ComputedNode,
            &'static bevy::ui::ScrollPosition,
            &'static SmoothScroll,
            &'static TextFont,
            &'static bevy::text::LineHeight,
            &'static MonoCellWidth,
            Option<&'static super::pipeline::DisplayLayout>,
        ),
    >,
}

impl<'w, 's> RowMetricsParam<'w, 's> {
    /// `RowMetrics` for `entity`, or `None` if a required component is missing.
    pub fn get(&self, entity: bevy::ecs::entity::Entity) -> Option<RowMetrics> {
        let (_, computed, scroll_pos, smooth, font, lh, mono, layout) = self.query.get(entity).ok()?;
        let line_height = crate::view::font::resolve_line_height(*lh, font.font_size);
        let baseline = layout
            .map(|l| l.baseline_offset)
            .unwrap_or(font.font_size * DEFAULT_BASELINE_OFFSET_RATIO);
        Some(row_metrics_with_baseline(computed, scroll_pos.y, smooth.horizontal, line_height, mono, baseline))
    }

    /// [`get`](Self::get) that panics on missing components.
    pub fn get_or_panic(&self, entity: bevy::ecs::entity::Entity) -> RowMetrics {
        self.get(entity).unwrap_or_else(|| {
            panic!(
                "RowMetricsParam: entity {:?} is missing one of \
                 (ComputedNode, ScrollPosition, SmoothScroll, TextFont, LineHeight, MonoCellWidth)",
                entity
            )
        })
    }

    /// `(entity, RowMetrics)` for every text view in the world.
    pub fn iter(&self) -> impl Iterator<Item = (bevy::ecs::entity::Entity, RowMetrics)> + '_ {
        self.query
            .iter()
            .map(|(entity, computed, scroll_pos, smooth, font, lh, mono, layout)| {
                let line_height = crate::view::font::resolve_line_height(*lh, font.font_size);
                let baseline = layout
                    .map(|l| l.baseline_offset)
                    .unwrap_or(font.font_size * DEFAULT_BASELINE_OFFSET_RATIO);
                (
                    entity,
                    row_metrics_with_baseline(computed, scroll_pos.y, smooth.horizontal, line_height, mono, baseline),
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::overlay::{CornerRadii, RectOverlay, RowVertical};

    fn make_metrics() -> RowMetrics {
        // 800x600 logical at 1x DPI; padding.left=50, padding.top=8.
        // scroll_y=100.0 is equivalent to the old scroll_offset=-100.0.
        let mut computed = bevy::ui::ComputedNode::default();
        computed.size = bevy::math::Vec2::new(800.0, 600.0);
        computed.inverse_scale_factor = 1.0;
        computed.padding.min_inset = bevy::math::Vec2::new(50.0, 8.0);
        let mono = MonoCellWidth { px: 8.4 };
        row_metrics_with_baseline(&computed, 100.0, 0.0, 21.0, &mono, 14.0 * 0.32)
    }

    /// `row_glyph_band`'s Y bounds must agree with what
    /// `render::push_overlay_quad` computes for `RowVertical::Full`.
    /// If this drifts, every consumer's row-aligned decoration drifts
    /// off the engine's actual glyph band.
    #[test]
    fn glyph_band_matches_full_overlay_math() {
        let metrics = make_metrics();
        let line_height = metrics.line_height;
        let baseline_offset = metrics.baseline_offset;

        // Engine-side derivation copied verbatim from render.rs (top-left convention).
        let baseline_y_off = line_height * 0.5 + baseline_offset;
        let cap_to_descender = baseline_y_off + baseline_offset * 0.6;
        let text_band_above_baseline = cap_to_descender * 0.25;
        let y_off = baseline_y_off - text_band_above_baseline;
        let height = cap_to_descender;

        for display_row in [0u32, 1, 5, 12, 50] {
            let row_y_top = metrics.row_y_top(display_row);
            let engine_band_top = row_y_top + y_off;
            let engine_band_bot = engine_band_top + height;

            let band = metrics.row_glyph_band(display_row);
            assert!(
                (band.min.y - engine_band_top).abs() < 1e-3,
                "row {display_row}: band.min.y {} != engine {}",
                band.min.y,
                engine_band_top,
            );
            assert!(
                (band.max.y - engine_band_bot).abs() < 1e-3,
                "row {display_row}: band.max.y {} != engine {}",
                band.max.y,
                engine_band_bot,
            );
        }
    }

    /// `row_full_box` covers `[y_top, y_top + line_height]` in
    /// screen-space, matching the user-intuitive "row's leaded box".
    #[test]
    fn full_box_spans_y_top_plus_line_height() {
        let metrics = make_metrics();
        let line_height = metrics.line_height;

        for display_row in [0u32, 1, 5, 12, 50] {
            let row_y_top = metrics.row_y_top(display_row);
            let r = metrics.row_full_box(display_row);
            assert!((r.min.y - row_y_top).abs() < 1e-3);
            assert!((r.max.y - (row_y_top + line_height)).abs() < 1e-3);
        }
    }

    /// Cell positioning composes scroll + horizontal scroll + text-area
    /// padding the same way the renderer composes them for glyph quads.
    #[test]
    fn cell_top_left_composes_offsets() {
        let metrics = make_metrics();
        let pos = metrics.cell_top_left(3, 10);
        let expected_x = metrics.text_area_left + 10.0 * metrics.char_width;
        let expected_y = metrics.row_y_top(3);
        assert!((pos.x - expected_x).abs() < 1e-3);
        assert!((pos.y - expected_y).abs() < 1e-3);
    }

    /// Confirm the overlay model is still accessible — the helpers
    /// don't replace `RectOverlay`, they complement it. This test
    /// anchors the stylistic guidance in the module docs: row-aligned
    /// decorations should prefer overlays.
    #[test]
    fn overlay_path_remains_idiomatic() {
        let _: RectOverlay = RectOverlay {
            display_row: 0,
            x_range: 0.0..100.0,
            color: Default::default(),
            z: -1,
            corners: CornerRadii::ZERO,
            vertical: RowVertical::Full,
        };
    }

    /// `RowMetricsParam` produces output identical to calling
    /// `row_metrics_with_baseline` directly on the same component
    /// values. The system param is a convenience wrapper, not a
    /// separate code path; this test ensures it stays that way.
    #[test]
    fn system_param_matches_direct_call() {
        use crate::view::font::MonoCellWidth;
        use crate::view::pipeline::DisplayLayout;
        use bevy::ecs::system::RunSystemOnce;
        use bevy::prelude::*;

        let mut world = World::new();
        let mut computed = bevy::ui::ComputedNode::default();
        computed.size = bevy::math::Vec2::new(800.0, 600.0);
        computed.inverse_scale_factor = 1.0;
        computed.padding.min_inset = bevy::math::Vec2::new(50.0, 8.0);
        // scroll_y=100.0 is equivalent to the old scroll_offset=-100.0.
        let scroll_pos = bevy::ui::ScrollPosition(bevy::math::Vec2::new(0.0, 100.0));
        let smooth = SmoothScroll { target_y: 100.0, horizontal: 0.0, ..Default::default() };
        let font = bevy::text::TextFont::from_font_size(14.0);
        let line_height_comp = bevy::text::LineHeight::Px(21.0);
        let mono = MonoCellWidth { px: 8.4 };
        let mut layout = DisplayLayout::default();
        layout.baseline_offset = 14.0 * 0.32;

        let direct = row_metrics_with_baseline(&computed, scroll_pos.y, smooth.horizontal, 21.0, &mono, layout.baseline_offset);

        let entity = world
            .spawn((computed, scroll_pos, smooth, font.clone(), line_height_comp, mono, layout.clone()))
            .id();

        let result = world
            .run_system_once(move |metrics: RowMetricsParam| metrics.get_or_panic(entity))
            .unwrap();

        // Sample a few rows; if any cell of the snapshot differs the
        // SystemParam wrapper has drifted from the direct call.
        for row in [0u32, 1, 7, 42] {
            assert!((direct.row_y_top(row) - result.row_y_top(row)).abs() < 1e-6);
            let a = direct.row_glyph_band(row);
            let b = result.row_glyph_band(row);
            assert!((a.min - b.min).length() < 1e-6);
            assert!((a.max - b.max).length() < 1e-6);
        }
    }
}

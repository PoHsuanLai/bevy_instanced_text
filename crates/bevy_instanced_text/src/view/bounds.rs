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
//! # use bevy_instanced_text::{MonoCellWidth, resolve_line_height};
//! # use bevy::ui::ScrollPosition;
//! # use bevy_instanced_text::view::pipeline::DisplayLayout;
//! # use bevy_instanced_text::view::bounds::row_metrics;
//! fn position_my_popup(
//!     editor: Query<(
//!         &ComputedNode,
//!         &ScrollPosition,
//!         &TextFont,
//!         &bevy::text::LineHeight,
//!         &MonoCellWidth,
//!         &DisplayLayout,
//!     )>,
//! ) {
//!     let (computed, scroll, font, lh, mono, _layout) = editor.single().unwrap();
//!     let line_height = resolve_line_height(*lh, font.font_size);
//!     let metrics = row_metrics(computed, scroll, font, line_height, mono);
//!     let band = metrics.row_glyph_band(12);
//!     let popup_top_left = band.min - bevy::math::Vec2::new(0.0, 100.0);
//!     // commands.spawn(Node { left: Val::Px(popup_top_left.x), top: Val::Px(popup_top_left.y), .. });
//! }
//! ```

use bevy::math::{Rect, Vec2};
use bevy::ui::{ComputedNode, ScrollPosition};

use super::font::MonoCellWidth;
use bevy::text::TextFont;

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
    viewport_height: f32,
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
    scroll: &ScrollPosition,
    font: &TextFont,
    line_height: f32,
    mono: &MonoCellWidth,
) -> RowMetrics {
    row_metrics_with_baseline(
        computed,
        scroll.y,
        scroll.x,
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
        viewport_height: logical.y,
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

    /// `Node::top` value for a `bevy_ui` [`Text`](bevy::ui::widget::Text)
    /// child of the editor that should sit with its baseline on
    /// `display_row`'s baseline.
    ///
    /// Use for sibling UI text — inlay hints, inline diagnostic labels,
    /// CodeLens annotations — that has to align with the editor's
    /// instanced glyphs without sharing a flex container. The child must
    /// use `position_type: Absolute` and the returned value as `top`;
    /// pair with [`cell_top_left_at_x`](Self::cell_top_left_at_x) `.x`
    /// for `left`.
    ///
    /// `font_size` is the child's own font size (may differ from the
    /// editor's — e.g. inlay hints render smaller). `line_height` is the
    /// child's resolved `LineHeight` in px; pass
    /// [`resolve_line_height`](super::font::resolve_line_height) of the
    /// child's `LineHeight` to get this. The child's baseline-offset is
    /// taken as [`DEFAULT_BASELINE_OFFSET_RATIO`] `* font_size`, which
    /// matches Bevy's text pipeline for any font without per-font
    /// overrides.
    pub fn ui_text_top_at_row_baseline(
        &self,
        display_row: u32,
        font_size: f32,
        line_height: f32,
    ) -> f32 {
        let child_baseline_from_top =
            line_height * 0.5 + font_size * DEFAULT_BASELINE_OFFSET_RATIO;
        self.glyph_baseline_y(display_row) - child_baseline_from_top
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

    /// Display row containing node-local Y coordinate `local_y` (top-left
    /// origin, +Y down — the same space `Node::top`/`Node::left` consume).
    /// Returns `None` when `local_y` falls outside the text area or above
    /// the first row.
    ///
    /// Inverse of [`row_y_top`](Self::row_y_top). Use this for click-to-row
    /// hit-testing on any UI panel — sidebar, list, log, gutter.
    pub fn pick_row(&self, local_y: f32) -> Option<u32> {
        if self.line_height <= 0.0 {
            return None;
        }
        let rel = local_y - self.text_area_top_with_scroll;
        if rel < 0.0 {
            return None;
        }
        Some((rel / self.line_height) as u32)
    }

    /// Display row containing a Bevy picking [`HitData`] position.
    ///
    /// Bevy UI's picking backend reports `hit.position` as a normalized
    /// `(-0.5, -0.5)..(0.5, 0.5)` Vec3 relative to the node's top-left and
    /// bottom-right corners. This converts that to node-local pixels and
    /// calls [`pick_row`](Self::pick_row).
    ///
    /// Use from `On<Pointer<Press>>` / `On<Pointer<Click>>` observers
    /// to get the row that was clicked with one call.
    ///
    /// [`HitData`]: bevy::picking::backend::HitData
    pub fn pick_row_from_hit(&self, hit: &bevy::picking::backend::HitData) -> Option<u32> {
        let norm = hit.position?;
        let local_y = (norm.y + 0.5) * self.viewport_height;
        self.pick_row(local_y)
    }

    /// Monospace column containing a Bevy picking [`HitData`] position.
    /// See [`pick_row_from_hit`](Self::pick_row_from_hit) for the position
    /// convention.
    ///
    /// [`HitData`]: bevy::picking::backend::HitData
    pub fn pick_column_from_hit(&self, hit: &bevy::picking::backend::HitData) -> Option<u32> {
        let norm = hit.position?;
        let local_x = (norm.x + 0.5) * self.viewport_width;
        self.pick_column(local_x)
    }

    /// Monospace column containing node-local X coordinate `local_x`.
    /// Returns `None` when `local_x` falls left of the text area.
    ///
    /// Pairs with [`pick_row`](Self::pick_row) for click-to-cell hit-testing
    /// in monospace contexts (terminals, gutters, fixed-pitch tables).
    /// For shaped text, walk `DisplayLayout` runs instead.
    pub fn pick_column(&self, local_x: f32) -> Option<u32> {
        if self.char_width <= 0.0 {
            return None;
        }
        let rel = local_x - self.text_area_left + self.horizontal_scroll;
        if rel < 0.0 {
            return None;
        }
        Some((rel / self.char_width) as u32)
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
    #[allow(clippy::type_complexity)]
    query: bevy::ecs::system::Query<
        'w,
        's,
        (
            bevy::ecs::entity::Entity,
            &'static bevy::ui::ComputedNode,
            &'static ScrollPosition,
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
        let (_, computed, scroll, font, lh, mono, layout) = self.query.get(entity).ok()?;
        let line_height = crate::view::font::resolve_line_height(*lh, font.font_size);
        let baseline = layout
            .map(|l| l.baseline_offset)
            .unwrap_or(font.font_size * DEFAULT_BASELINE_OFFSET_RATIO);
        Some(row_metrics_with_baseline(
            computed,
            scroll.y,
            scroll.x,
            line_height,
            mono,
            baseline,
        ))
    }

    /// [`get`](Self::get) that panics on missing components.
    pub fn get_or_panic(&self, entity: bevy::ecs::entity::Entity) -> RowMetrics {
        self.get(entity).unwrap_or_else(|| {
            panic!(
                "RowMetricsParam: entity {:?} is missing one of \
                 (ComputedNode, ScrollPosition, TextFont, LineHeight, MonoCellWidth)",
                entity
            )
        })
    }

    /// `(entity, RowMetrics)` for every text view in the world.
    pub fn iter(&self) -> impl Iterator<Item = (bevy::ecs::entity::Entity, RowMetrics)> + '_ {
        self.query
            .iter()
            .map(|(entity, computed, scroll, font, lh, mono, layout)| {
                let line_height = crate::view::font::resolve_line_height(*lh, font.font_size);
                let baseline = layout
                    .map(|l| l.baseline_offset)
                    .unwrap_or(font.font_size * DEFAULT_BASELINE_OFFSET_RATIO);
                (
                    entity,
                    row_metrics_with_baseline(
                        computed,
                        scroll.y,
                        scroll.x,
                        line_height,
                        mono,
                        baseline,
                    ),
                )
            })
    }
}

#[cfg(test)]
#[allow(clippy::field_reassign_with_default)]
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

    /// `pick_row` is the inverse of `row_y_top`: the Y at any point inside
    /// row N must round-trip back to N, and the boundary at `y_top(N)`
    /// belongs to row N (not N-1).
    #[test]
    fn pick_row_inverts_row_y_top() {
        let metrics = make_metrics();
        for row in [0u32, 1, 5, 12, 50] {
            let y_top = metrics.row_y_top(row);
            let y_mid = y_top + metrics.line_height * 0.5;
            let y_just_below_top = y_top + 0.001;
            assert_eq!(metrics.pick_row(y_top), Some(row), "row {row} top boundary");
            assert_eq!(
                metrics.pick_row(y_just_below_top),
                Some(row),
                "row {row} just inside"
            );
            assert_eq!(metrics.pick_row(y_mid), Some(row), "row {row} midpoint");
        }
    }

    /// Y above the text area returns `None` rather than wrapping to a high
    /// row index — callers can distinguish "missed" from "row 0".
    #[test]
    fn pick_row_rejects_above_text_area() {
        let metrics = make_metrics();
        // text_area_top_with_scroll = 8.0 - 100.0 = -92.0, so anything below
        // that is in row 0 (scrolled). Pick a y that's clearly above.
        let y_above = metrics.text_area_top_with_scroll - 10.0;
        assert!(metrics.pick_row(y_above).is_none());
    }

    /// `pick_row_from_hit` decodes Bevy's normalized hit position and
    /// matches what `pick_row` would give for the equivalent local Y.
    /// This is the path picking observers take, so any drift between the
    /// two would surface as off-by-one rows under clicks.
    #[test]
    fn pick_row_from_hit_matches_pick_row() {
        use bevy::math::Vec3;
        use bevy::picking::backend::HitData;
        use bevy::prelude::Entity;
        let metrics = make_metrics();
        // viewport_height = 600 (from make_metrics).
        // norm.y = -0.5 → local_y = 0; norm.y = 0.0 → local_y = 300; etc.
        for norm_y in [-0.4f32, -0.1, 0.0, 0.25, 0.49] {
            let hit = HitData::new(
                Entity::PLACEHOLDER,
                0.0,
                Some(Vec3::new(0.0, norm_y, 0.0)),
                None,
            );
            let local_y = (norm_y + 0.5) * 600.0;
            assert_eq!(
                metrics.pick_row_from_hit(&hit),
                metrics.pick_row(local_y),
                "norm_y={norm_y} local_y={local_y}",
            );
        }
    }

    /// `pick_column` inverts the column-to-x mapping for monospace cells,
    /// accounting for horizontal scroll and text area inset.
    #[test]
    fn pick_column_inverts_cell_top_left() {
        let metrics = make_metrics();
        for col in [0u32, 1, 7, 25] {
            let pos = metrics.cell_top_left(0, col);
            // pos.x is in node-local space; pick_column should recover col.
            assert_eq!(metrics.pick_column(pos.x + 0.001), Some(col));
            assert_eq!(
                metrics.pick_column(pos.x + metrics.char_width * 0.5),
                Some(col)
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
        let scroll = ScrollPosition(bevy::math::Vec2::new(0.0, 100.0));
        let font = bevy::text::TextFont::from_font_size(14.0);
        let line_height_comp = bevy::text::LineHeight::Px(21.0);
        let mono = MonoCellWidth { px: 8.4 };
        let mut layout = DisplayLayout::default();
        layout.baseline_offset = 14.0 * 0.32;

        let direct = row_metrics_with_baseline(
            &computed,
            scroll.y,
            scroll.x,
            21.0,
            &mono,
            layout.baseline_offset,
        );

        let entity = world
            .spawn((
                computed,
                scroll,
                font.clone(),
                line_height_comp,
                mono,
                layout.clone(),
            ))
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

//! World-space positioning helpers for text-view consumers.
//!
//! `bevy_instanced_text` emits glyph quads using a specific row-anchor
//! convention (top-of-leaded-box at `text_area_top + scroll_offset +
//! display_row * line_height`, glyph baseline derived from row top, etc.).
//! Any consumer that wants to position a non-overlay entity (a popup, a
//! sprite, a custom mesh) over a particular row or character cell needs
//! to apply that same convention.
//!
//! Rather than have each consumer reproduce the math (and silently
//! drift when the engine's convention shifts), they should call into
//! these helpers. The helpers are derived from the same constants the
//! renderer uses — by construction they can't get out of sync with what
//! `render_layout` paints.
//!
//! For row-aligned visual decorations (selection backgrounds, highlight
//! bars, indent guides, bracket boxes), prefer pushing a [`RectOverlay`]
//! into [`TextViewOverlays`] — the engine paints those itself and you
//! don't need this module at all. Reach for these helpers only for
//! consumers that can't fit into the overlay model: Bevy UI nodes,
//! freestanding sprites, popups, or anything outside the engine's draw.
//!
//! All helpers return values in the **centered-ortho world space** the
//! engine renders into: viewport top-left at
//! `(-width/2, +height/2)`, +Y up. A `Camera2d` placed at the panel
//! center sees them at the right place.
//!
//! [`RectOverlay`]: super::overlay::RectOverlay
//! [`TextViewOverlays`]: super::overlay::TextViewOverlays
//!
//! # Example
//! ```no_run
//! # use bevy::prelude::*;
//! # use bevy_instanced_text::prelude::*;
//! # use bevy_instanced_text::view::anchor::row_metrics;
//! fn position_my_popup(
//!     editor: Query<(&TextViewViewport, &ScrollState, &FontConfig, &DisplayLayout)>,
//! ) {
//!     let (viewport, scroll, font, layout) = editor.single().unwrap();
//!     let metrics = row_metrics(viewport, scroll, font);
//!     // World-space rect of the visible glyph band on display row 12.
//!     let band = metrics.row_glyph_band(12);
//!     let popup_pos = band.min - bevy::math::Vec2::new(0.0, 100.0);
//!     // commands.spawn(...);
//! }
//! ```
//!
//! Mirrors of the renderer's internal anchor math live here, all
//! exercised by tests in this module to keep them in lockstep.

use bevy::math::{Rect, Vec2};

use super::font::FontConfig;
use super::state::ScrollState;
use super::viewport::TextViewViewport;

/// Default baseline-offset ratio matching `FontConfig::from_size` /
/// `layout_builder` defaults: ~32% of font size. Consumers that don't
/// have a `DisplayLayout` on hand can pass this into
/// [`row_metrics_with_baseline`] (or just call [`row_metrics`] which
/// uses it implicitly).
pub const DEFAULT_BASELINE_OFFSET_RATIO: f32 = 0.32;

/// Row-anchor metrics for a single text view, snapshotted from
/// `(viewport, scroll, font, baseline_offset)` so consumers can query
/// many rows without re-deriving the constants.
///
/// Construct via [`row_metrics`] (uses the canonical baseline ratio) or
/// [`row_metrics_with_baseline`] (when you have a `DisplayLayout` and
/// want byte-identical output to the renderer).
#[derive(Clone, Copy, Debug)]
pub struct RowMetrics {
    /// World-space top of the viewport rectangle. Y inversion baseline.
    pub world_top: f32,
    /// World-space left of the viewport rectangle.
    pub world_left: f32,
    /// Engine-side `text_area_top + scroll_offset` — screen-space Y where
    /// `display_row = 0` begins.
    text_area_top_with_scroll: f32,
    /// Screen-space pixel offset for `text_area_left` (gutter + margin).
    text_area_left: f32,
    /// Viewport content width in pixels. Used by `row_*_box` helpers for
    /// viewport-spanning rects.
    viewport_width: f32,
    /// Pixel width of one monospace cell (`FontConfig.char_width`).
    char_width: f32,
    /// Pixel height of a row's leaded box. Per-row overrides
    /// (`ShapedLine.line_height`) are accepted via the `_with_height`
    /// variants; this is the layout default.
    line_height: f32,
    /// Engine-side `baseline_offset` — pixel distance from leaded-box
    /// midline to the glyph baseline. Read from `DisplayLayout` when
    /// possible; otherwise defaulted via [`DEFAULT_BASELINE_OFFSET_RATIO`].
    baseline_offset: f32,
    /// Horizontal scroll, subtracted from `text_area_left` cells.
    horizontal_scroll: f32,
}

/// Snapshot the engine's row-anchor math for a text view, using the
/// canonical baseline ratio (0.32). Cheap (a handful of float ops +
/// struct copy).
///
/// If you have a `DisplayLayout` (you almost always do — it's a
/// required component of `TextView`), prefer
/// [`row_metrics_with_baseline`] passing `layout.baseline_offset` — that
/// stays byte-identical with the renderer even when the layout
/// customizes the baseline.
pub fn row_metrics(
    viewport: &TextViewViewport,
    scroll: &ScrollState,
    font: &FontConfig,
) -> RowMetrics {
    row_metrics_with_baseline(
        viewport,
        scroll,
        font,
        font.font_size * DEFAULT_BASELINE_OFFSET_RATIO,
    )
}

/// As [`row_metrics`] but lets the caller pass an explicit
/// `baseline_offset` (e.g. read from `DisplayLayout::baseline_offset`).
pub fn row_metrics_with_baseline(
    viewport: &TextViewViewport,
    scroll: &ScrollState,
    font: &FontConfig,
    baseline_offset: f32,
) -> RowMetrics {
    RowMetrics {
        world_top: viewport.world_top(),
        world_left: viewport.world_left(),
        text_area_top_with_scroll: viewport.text_area_top + scroll.scroll_offset,
        text_area_left: viewport.text_area_left,
        viewport_width: viewport.width as f32,
        char_width: font.char_width,
        line_height: font.line_height,
        baseline_offset,
        horizontal_scroll: scroll.horizontal_scroll_offset,
    }
}

impl RowMetrics {
    /// Screen-space Y of `display_row`'s leaded-box top. Mirrors
    /// `layout_builder::y_top_for`.
    pub fn row_y_top(&self, display_row: u32) -> f32 {
        self.text_area_top_with_scroll + display_row as f32 * self.line_height
    }

    /// World-space Y of the row's leaded-box top edge (highest point of
    /// the row in screen-space; centered-ortho `+Y` is up, so this is a
    /// larger value than the row's bottom).
    pub fn row_world_y_top(&self, display_row: u32) -> f32 {
        self.world_top - self.row_y_top(display_row)
    }

    /// World-space rectangle covering the row's full leaded box (the
    /// entire `[y_top, y_top + line_height]` strip in screen-Y).
    ///
    /// The horizontal extent spans the viewport's content area
    /// (`text_area_left` to `viewport.width`); narrow it yourself if you
    /// only want a column subrange.
    pub fn row_full_box(&self, display_row: u32) -> Rect {
        self.row_full_box_with_height(display_row, self.line_height)
    }

    /// As [`row_full_box`](Self::row_full_box) but with a per-row
    /// `line_height` override (e.g. when a `ShapedLine` overrides the
    /// layout default for headings).
    pub fn row_full_box_with_height(&self, display_row: u32, line_height: f32) -> Rect {
        let row_y_top = self.text_area_top_with_scroll + display_row as f32 * line_height;
        let world_y_top = self.world_top - row_y_top;
        let content_width = self.row_content_width();
        Rect {
            min: Vec2::new(
                self.world_left + self.text_area_left,
                world_y_top - line_height,
            ),
            max: Vec2::new(
                self.world_left + self.text_area_left + content_width,
                world_y_top,
            ),
        }
    }

    /// World-space rectangle covering only the visible glyph band of
    /// `display_row` (cap-to-descender, not the full leaded box). This
    /// is the band selection backgrounds, bracket boxes, and highlight
    /// bars should align with — straddling the leaded box looks too
    /// tall.
    ///
    /// Matches `RectOverlay { vertical: Full }`.
    pub fn row_glyph_band(&self, display_row: u32) -> Rect {
        self.row_glyph_band_with_height(display_row, self.line_height)
    }

    /// Same as [`row_glyph_band`](Self::row_glyph_band) but lets the
    /// caller pass a per-row `line_height` override.
    pub fn row_glyph_band_with_height(&self, display_row: u32, line_height: f32) -> Rect {
        // Mirror render::push_overlay_quad's RowVertical::Full math.
        let baseline_y_off = line_height * 0.5 + self.baseline_offset;
        let cap = baseline_y_off + self.baseline_offset * 0.6;
        let band_top_y_off = baseline_y_off - cap * 0.25;
        let row_y_top = self.text_area_top_with_scroll + display_row as f32 * line_height;
        let world_y_top = self.world_top - row_y_top - band_top_y_off;
        let world_y_bot = world_y_top - cap;
        let content_width = self.row_content_width();
        Rect {
            min: Vec2::new(self.world_left + self.text_area_left, world_y_bot),
            max: Vec2::new(
                self.world_left + self.text_area_left + content_width,
                world_y_top,
            ),
        }
    }

    /// World-space top-left of the cell at `(display_row, column)`,
    /// assuming a monospace grid (`char_width`-sized cells). For
    /// proportional or shaped text use
    /// [`cell_world_pos_at_x`](Self::cell_world_pos_at_x) with a pixel
    /// x derived from the layout instead.
    ///
    /// "Top-left" here means the corner with the largest Y and smallest
    /// X — i.e. the visual top-left as you'd expect from a screen-space
    /// origin, mapped into centered-ortho world space. Anchored to the
    /// row's leaded-box top; for glyph-band-aligned positioning use
    /// [`cell_glyph_band_top_left`](Self::cell_glyph_band_top_left).
    pub fn cell_world_pos(&self, display_row: u32, column: u32) -> Vec2 {
        let pixel_x = column as f32 * self.char_width;
        self.cell_world_pos_at_x(display_row, pixel_x)
    }

    /// World-space top-left of a cell whose horizontal pen-x (relative
    /// to `text_area_left`, before horizontal-scroll subtraction) is
    /// `pixel_x`. Use this for cells positioned by a `DisplayLayout`'s
    /// shaped advances rather than a monospace grid.
    pub fn cell_world_pos_at_x(&self, display_row: u32, pixel_x: f32) -> Vec2 {
        Vec2::new(
            self.world_left + self.text_area_left + pixel_x - self.horizontal_scroll,
            self.row_world_y_top(display_row),
        )
    }

    /// Top-left of the cell at `(display_row, column)` snapped to the
    /// **glyph band** (not the leaded box). Useful when you want to
    /// place a marker or rect that visually sits with the text rather
    /// than the row's spacing.
    pub fn cell_glyph_band_top_left(&self, display_row: u32, column: u32) -> Vec2 {
        let band = self.row_glyph_band(display_row);
        Vec2::new(
            self.world_left + self.text_area_left + column as f32 * self.char_width
                - self.horizontal_scroll,
            band.max.y,
        )
    }

    /// Screen-space (top-down) Y of `display_row`'s glyph baseline.
    /// Mirrors the renderer's
    /// `line.y_top + line_height/2 + baseline_offset`. Useful for
    /// consumers that emit glyph instances directly (e.g. the editor's
    /// gutter line numbers, which paint into their own batch); pass the
    /// returned value into the same world-Y conversion the renderer
    /// uses (`world_top - screen_y - glyph.size.y`).
    pub fn glyph_baseline_screen_y(&self, display_row: u32) -> f32 {
        self.row_y_top(display_row) + self.line_height * 0.5 + self.baseline_offset
    }

    /// World-space cell width in pixels (`char_width` in monospace
    /// fonts). For proportional fonts the layout's per-glyph advances
    /// should be used instead.
    pub fn cell_width(&self) -> f32 {
        self.char_width
    }

    /// World-space row leaded-box height. Equals `font.line_height`
    /// when no per-row override is in play.
    pub fn row_height(&self) -> f32 {
        self.line_height
    }

    /// Width of the viewport's content area in pixels (`viewport.width
    /// - text_area_left`). Used by `row_*_box` helpers that span the
    /// viewport; consumers wanting a tighter fit should override
    /// `rect.max.x` directly.
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
/// Reads `Option<&DisplayLayout>` so consumers that haven't laid out
/// yet still get sensible metrics (using the canonical baseline ratio).
/// When the layout is present, its `baseline_offset` is used so the
/// helper output stays byte-identical with the renderer even if the
/// layout customizes the baseline.
#[derive(bevy::ecs::system::SystemParam)]
pub struct RowMetricsParam<'w, 's> {
    query: bevy::ecs::system::Query<
        'w,
        's,
        (
            bevy::ecs::entity::Entity,
            &'static TextViewViewport,
            &'static ScrollState,
            &'static FontConfig,
            Option<&'static super::layout::DisplayLayout>,
        ),
    >,
}

impl<'w, 's> RowMetricsParam<'w, 's> {
    /// Build a `RowMetrics` snapshot for the given editor entity.
    /// Returns `None` when the entity is missing a required component
    /// (`TextViewViewport`, `ScrollState`, or `FontConfig`).
    pub fn get(&self, entity: bevy::ecs::entity::Entity) -> Option<RowMetrics> {
        let (_, viewport, scroll, font, layout) = self.query.get(entity).ok()?;
        let baseline = layout
            .map(|l| l.baseline_offset)
            .unwrap_or(font.font_size * DEFAULT_BASELINE_OFFSET_RATIO);
        Some(row_metrics_with_baseline(viewport, scroll, font, baseline))
    }

    /// As [`get`](Self::get) but `panic`s when the entity is missing
    /// the required components. Useful for systems that have already
    /// proven the entity is a valid editor (e.g. via a separate query
    /// in the same system).
    pub fn get_or_panic(&self, entity: bevy::ecs::entity::Entity) -> RowMetrics {
        self.get(entity).unwrap_or_else(|| {
            panic!(
                "RowMetricsParam: entity {:?} is missing one of \
                 (TextViewViewport, ScrollState, FontConfig)",
                entity
            )
        })
    }

    /// Iterate over every text view in the world, yielding
    /// `(entity, RowMetrics)` pairs. Useful for systems that operate
    /// across multiple editors (e.g. a global indent-guide pass).
    pub fn iter(&self) -> impl Iterator<Item = (bevy::ecs::entity::Entity, RowMetrics)> + '_ {
        self.query
            .iter()
            .map(|(entity, viewport, scroll, font, layout)| {
                let baseline = layout
                    .map(|l| l.baseline_offset)
                    .unwrap_or(font.font_size * DEFAULT_BASELINE_OFFSET_RATIO);
                (
                    entity,
                    row_metrics_with_baseline(viewport, scroll, font, baseline),
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::overlay::{CornerRadii, RectOverlay, RowVertical};
    use bevy::asset::Handle;

    fn make_metrics() -> RowMetrics {
        let viewport = TextViewViewport {
            width: 800,
            height: 600,
            hit_test_position: Vec2::ZERO,
            text_area_left: 50.0,
            text_area_top: 8.0,
            gutter_width: 40.0,
        };
        let scroll = ScrollState {
            scroll_offset: -100.0,
            target_scroll_offset: -100.0,
            horizontal_scroll_offset: 0.0,
            target_horizontal_scroll_offset: 0.0,
            ..Default::default()
        };
        let font = FontConfig {
            font: Handle::default(),
            font_size: 14.0,
            line_height: 21.0,
            char_width: 8.4,
            font_bold: None,
            font_italic: None,
            font_bold_italic: None,
            font_synthesis: Default::default(),
        };
        // baseline_offset matching FontConfig::from_size / layout default.
        row_metrics_with_baseline(&viewport, &scroll, &font, 14.0 * 0.32)
    }

    /// `row_glyph_band`'s world-Y must agree with what
    /// `render::push_overlay_quad` computes for `RowVertical::Full`.
    /// If this drifts, every consumer's row-aligned decoration drifts
    /// off the engine's actual glyph band.
    #[test]
    fn glyph_band_matches_full_overlay_math() {
        let metrics = make_metrics();
        let line_height = metrics.line_height;
        let baseline_offset = metrics.baseline_offset;

        // Engine-side derivation copied verbatim from render.rs.
        let baseline_y_off = line_height * 0.5 + baseline_offset;
        let cap_to_descender = baseline_y_off + baseline_offset * 0.6;
        let text_band_above_baseline = cap_to_descender * 0.25;
        let y_off = baseline_y_off - text_band_above_baseline;
        let height = cap_to_descender;

        for display_row in [0u32, 1, 5, 12, 50] {
            let row_y_top = metrics.row_y_top(display_row);
            let engine_world_y_center = metrics.world_top - row_y_top - y_off - height * 0.5;
            let engine_world_y_top = engine_world_y_center + height * 0.5;
            let engine_world_y_bot = engine_world_y_center - height * 0.5;

            let band = metrics.row_glyph_band(display_row);
            assert!(
                (band.max.y - engine_world_y_top).abs() < 1e-3,
                "row {display_row}: band.max.y {} != engine {}",
                band.max.y,
                engine_world_y_top,
            );
            assert!(
                (band.min.y - engine_world_y_bot).abs() < 1e-3,
                "row {display_row}: band.min.y {} != engine {}",
                band.min.y,
                engine_world_y_bot,
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
            let expected_world_y_top = metrics.world_top - row_y_top;
            let expected_world_y_bot = expected_world_y_top - line_height;

            let r = metrics.row_full_box(display_row);
            assert!((r.max.y - expected_world_y_top).abs() < 1e-3);
            assert!((r.min.y - expected_world_y_bot).abs() < 1e-3);
        }
    }

    /// Cell positioning composes scroll + horizontal scroll + text-area
    /// padding the same way the renderer composes them for glyph quads.
    #[test]
    fn cell_world_pos_composes_offsets() {
        let metrics = make_metrics();
        let pos = metrics.cell_world_pos(3, 10);
        let expected_x = metrics.world_left + metrics.text_area_left + 10.0 * metrics.char_width;
        let expected_y = metrics.world_top - metrics.row_y_top(3);
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
        use crate::view::layout::DisplayLayout;
        use bevy::ecs::system::RunSystemOnce;
        use bevy::prelude::*;

        let mut world = World::new();
        let viewport = TextViewViewport {
            width: 800,
            height: 600,
            hit_test_position: Vec2::ZERO,
            text_area_left: 50.0,
            text_area_top: 8.0,
            gutter_width: 40.0,
        };
        let scroll = ScrollState {
            scroll_offset: -100.0,
            target_scroll_offset: -100.0,
            horizontal_scroll_offset: 0.0,
            target_horizontal_scroll_offset: 0.0,
            ..Default::default()
        };
        let font = FontConfig {
            font: Handle::default(),
            font_size: 14.0,
            line_height: 21.0,
            char_width: 8.4,
            font_bold: None,
            font_italic: None,
            font_bold_italic: None,
            font_synthesis: Default::default(),
        };
        let mut layout = DisplayLayout::default();
        layout.baseline_offset = 14.0 * 0.32;

        // Compute the expected snapshot before moving the components
        // into the entity (`ScrollState` isn't `Clone`).
        let direct = row_metrics_with_baseline(&viewport, &scroll, &font, layout.baseline_offset);

        let entity = world
            .spawn((viewport, scroll, font.clone(), layout.clone()))
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

//! Buffer-position → screen / world anchoring.
//!
//! [`BufferAnchorParam`] is the higher-level companion to
//! [`crate::RowMetricsParam`]: where `RowMetrics` answers
//! *"given a display row + pixel x, where on screen is that?"*,
//! `BufferAnchor` answers *"given a buffer line + character (or rope
//! char-offset), where on screen is that?"* It folds in the editor's
//! `TextBuffer` and `DisplayLayout` so consumers don't have to repeat
//! the buffer→display→pixel resolution chain — and so the LSP-flavored
//! `(line, character)` and the rope-flavored `char_index` paths produce
//! consistent results.
//!
//! Two conversions live in here:
//!
//! 1. **Buffer → display row.** When a layout is present, this honors
//!    soft-wrap and folding via [`DisplayLayout::buffer_to_display`];
//!    otherwise it falls back to a 1:1 buffer-line → display-row
//!    mapping (correct for unwrapped, unfolded views).
//! 2. **Pixel x.** Layout-aware via [`DisplayLayout::x_at_byte`] (uses
//!    cosmic-text's actual advances) or monospace fallback
//!    (`character * char_width`). Monospace is a fine default for
//!    code-editor workloads where every glyph is one cell wide.
//!
//! The output [`AnchorPoint`] carries every flavor of coordinate a
//! consumer plausibly needs:
//!
//! - `screen_top_left` — top-down pixel coords relative to the editor
//!   panel's origin. egui popups want this. Hosts add their panel's own
//!   offset.
//! - `world_top_left`, `world_band_center`, `world_baseline` —
//!   centered-ortho world space, for sprite-anchored UI on the same
//!   camera as the engine.
//! - `display_row`, `pixel_x_screen` — the underlying engine
//!   coordinates, in case a consumer wants to push their own
//!   `RectOverlay` or compose with `RowMetrics`.
//! - `on_screen` — whether the anchor falls inside the editor's
//!   viewport rectangle (popups can use this to hide rather than
//!   render off-edge).
//!
//! # Example — egui popup
//!
//! ```ignore
//! fn render_completion(
//!     mut contexts: EguiContexts,
//!     anchors: BufferAnchorParam,
//!     popups: Query<(Entity, &CompletionPopupData)>,
//! ) {
//!     for (editor, popup) in popups.iter() {
//!         let anchor = anchors.at_buffer_pos(editor, popup.line, popup.character);
//!         let pos = egui::pos2(anchor.screen_top_left.x, anchor.screen_below.y);
//!         // ...
//!     }
//! }
//! ```
//!
//! # Example — sprite-anchored UI
//!
//! ```ignore
//! fn render_my_widget(anchors: BufferAnchorParam, editor: Single<Entity, With<MyEditor>>) {
//!     let anchor = anchors.at_buffer_pos(*editor, line, character);
//!     // commands.spawn(Sprite { ... }, Transform::from_translation(anchor.world_band_center.extend(5.0)));
//! }
//! ```

use bevy::ecs::entity::Entity;
use bevy::ecs::system::{Query, SystemParam};
use bevy::math::Vec2;

use super::anchor::{row_metrics_with_baseline, RowMetrics, DEFAULT_BASELINE_OFFSET_RATIO};
use super::font::FontConfig;
use super::layout::DisplayLayout;
use super::state::{ScrollState, TextBuffer};
use super::viewport::TextViewViewport;

/// Resolved screen / world coordinates for a buffer position.
///
/// Every flavor a consumer plausibly needs is materialized once at
/// query time so callers don't pick the wrong combination of
/// `RowMetrics` / `viewport_offset` / horizontal-scroll. Numeric cost:
/// half a dozen multiplies and adds — far cheaper than the cache miss
/// from re-fetching components per call site.
#[derive(Clone, Copy, Debug)]
pub struct AnchorPoint {
    /// 0-indexed display row the buffer position resolves to (post
    /// soft-wrap and folding when a `DisplayLayout` is attached).
    pub display_row: u32,
    /// Pen-x within the row in pixels, line-local — does not include
    /// `text_area_left` or horizontal scroll.
    pub pixel_x: f32,
    /// Top-down screen-space top-left corner of the cell, relative to
    /// the editor entity's viewport origin (top-left of the editor
    /// panel = `(0, 0)`). The host adds its own panel offset.
    pub screen_top_left: Vec2,
    /// Top-down screen-space bottom-left of the cell — i.e.
    /// `screen_top_left + (0, line_height)`. egui popups that want to
    /// flip below the cursor anchor here.
    pub screen_below_left: Vec2,
    /// Centered-ortho world-space top-left of the cell. Sprite-anchored
    /// UI placed on the engine's `Camera2d` uses this directly.
    pub world_top_left: Vec2,
    /// Centered-ortho world-space midpoint of the row's glyph band
    /// (cap-to-descender). Sprites anchored here visually sit *with*
    /// the text rather than straddling the leaded box.
    pub world_band_center: Vec2,
    /// Centered-ortho world-space glyph-baseline point at this column
    /// (X = `world_top_left.x`, Y = baseline). Useful for placing
    /// underline-like decorations or text rendered through some other
    /// pipeline that already aligns to baseline.
    pub world_baseline: Vec2,
    /// Row leaded-box height in pixels (matches `font.line_height`
    /// unless a per-row override is in play).
    pub line_height: f32,
    /// `true` when the anchor's `(screen_top_left)` falls inside the
    /// editor's viewport rectangle. Consumers rendering into a
    /// non-clipped layer (egui overlay) can use this to hide popups
    /// scrolled out of view rather than draw off-edge.
    pub on_screen: bool,
}

/// `SystemParam` shorthand: take any number of editor entities and
/// resolve `(line, character)` or rope `char_index` to an
/// [`AnchorPoint`] for a given editor.
///
/// Composes a `RowMetrics` snapshot with the entity's `TextBuffer` and
/// optional `DisplayLayout`, so layout-aware (soft-wrapped, folded)
/// editors resolve correctly while trivial/un-laid-out editors still
/// get a sensible monospace fallback.
#[derive(SystemParam)]
pub struct BufferAnchorParam<'w, 's> {
    query: Query<
        'w,
        's,
        (
            Entity,
            &'static TextViewViewport,
            &'static ScrollState,
            &'static FontConfig,
            &'static TextBuffer,
            Option<&'static DisplayLayout>,
        ),
    >,
}

impl<'w, 's> BufferAnchorParam<'w, 's> {
    /// Anchor a buffer `(line, character)` (LSP-flavored) on the given
    /// editor entity.
    ///
    /// `character` is treated as a **byte offset within the line** when
    /// a `DisplayLayout` is attached (so cosmic-text's shaped advances
    /// resolve the pen-x), and as a monospace cell index otherwise.
    /// LSP servers report UTF-16 code units; for ASCII code (the common
    /// case) the two coincide. Non-ASCII positions need a separate
    /// UTF-16→byte conversion before calling here.
    ///
    /// Returns `None` only when the entity is missing a required
    /// component (`TextViewViewport`, `ScrollState`, `FontConfig`,
    /// `TextBuffer`).
    pub fn at_buffer_pos(
        &self,
        entity: Entity,
        line: u32,
        character: u32,
    ) -> Option<AnchorPoint> {
        let (_, viewport, scroll, font, _buffer, layout) = self.query.get(entity).ok()?;
        let metrics = build_metrics(viewport, scroll, font, layout);

        let (display_row, pixel_x) = resolve_display_row_and_x(
            line,
            character as usize,
            font,
            layout,
        );

        Some(self.build_anchor(viewport, scroll, font, &metrics, display_row, pixel_x))
    }

    /// Anchor a rope-flavored `char_index` (offset within
    /// `TextBuffer.rope`). Convenient for LSP popups whose state stores
    /// the trigger position as a rope offset rather than `(line,
    /// character)`.
    pub fn at_rope_char_index(
        &self,
        entity: Entity,
        char_index: usize,
    ) -> Option<AnchorPoint> {
        let (_, viewport, scroll, font, buffer, layout) = self.query.get(entity).ok()?;
        let metrics = build_metrics(viewport, scroll, font, layout);

        let char_index = char_index.min(buffer.rope.len_chars());
        let line_index = buffer.rope.char_to_line(char_index);
        let line_start = buffer.rope.line_to_char(line_index);
        let col_chars = char_index - line_start;
        // For ASCII-only buffers, char count == byte count. For
        // non-ASCII, this is approximate — but the rope-char-index
        // entry point is a convenience for LSP popups that already
        // store positions as char-offsets, not a precise multibyte API.
        let byte_in_line = col_chars;

        let (display_row, pixel_x) = resolve_display_row_and_x(
            line_index as u32,
            byte_in_line,
            font,
            layout,
        );

        Some(self.build_anchor(viewport, scroll, font, &metrics, display_row, pixel_x))
    }

    fn build_anchor(
        &self,
        viewport: &TextViewViewport,
        scroll: &ScrollState,
        font: &FontConfig,
        metrics: &RowMetrics,
        display_row: u32,
        pixel_x: f32,
    ) -> AnchorPoint {
        let line_height = font.line_height;

        // Top-down screen coords relative to the editor panel.
        let screen_x =
            viewport.text_area_left + pixel_x - scroll.horizontal_scroll_offset;
        let screen_y = metrics.row_y_top(display_row);

        let screen_top_left = Vec2::new(screen_x, screen_y);
        let screen_below_left = Vec2::new(screen_x, screen_y + line_height);

        // World-space — engine's centered-ortho convention.
        let world_top_left = metrics.cell_world_pos_at_x(display_row, pixel_x);
        let band = metrics.row_glyph_band(display_row);
        let world_band_center = Vec2::new(world_top_left.x, (band.min.y + band.max.y) * 0.5);
        let world_baseline = Vec2::new(
            world_top_left.x,
            metrics.world_top - metrics.glyph_baseline_screen_y(display_row),
        );

        let on_screen = screen_x >= 0.0
            && screen_y >= 0.0
            && screen_x < viewport.width as f32
            && screen_y + line_height <= viewport.height as f32;

        AnchorPoint {
            display_row,
            pixel_x,
            screen_top_left,
            screen_below_left,
            world_top_left,
            world_band_center,
            world_baseline,
            line_height,
            on_screen,
        }
    }
}

fn build_metrics(
    viewport: &TextViewViewport,
    scroll: &ScrollState,
    font: &FontConfig,
    layout: Option<&DisplayLayout>,
) -> RowMetrics {
    let baseline = layout
        .map(|l| l.baseline_offset)
        .unwrap_or(font.font_size * DEFAULT_BASELINE_OFFSET_RATIO);
    row_metrics_with_baseline(viewport, scroll, font, baseline)
}

fn resolve_display_row_and_x(
    buffer_line: u32,
    byte_in_line: usize,
    font: &FontConfig,
    layout: Option<&DisplayLayout>,
) -> (u32, f32) {
    if let Some(layout) = layout {
        if let Some((display_row, byte_in_row)) = layout.buffer_to_display(buffer_line, byte_in_line) {
            let pixel_x = layout
                .x_at_byte(display_row, byte_in_row)
                .unwrap_or_else(|| byte_in_row as f32 * font.char_width);
            return (display_row, pixel_x);
        }
    }
    // Fallback: 1:1 buffer-line → display-row, monospace columns.
    (buffer_line, byte_in_line as f32 * font.char_width)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::layout::DisplayLayout;
    use bevy::asset::Handle;
    use bevy::ecs::system::RunSystemOnce;
    use bevy::math::Vec2;
    use bevy::prelude::*;

    fn make_editor_world() -> (World, Entity) {
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
            inline_bg_hpad_em: 0.25,
        };
        let buffer = TextBuffer::with_text("hello world\nsecond line\nthird");
        let mut layout = DisplayLayout::default();
        layout.baseline_offset = 14.0 * 0.32;

        let mut world = World::new();
        let entity = world.spawn((viewport, scroll, font, buffer, layout)).id();
        (world, entity)
    }

    /// `at_buffer_pos` and `at_rope_char_index` should agree when given
    /// equivalent inputs — same row, same column, same screen coords.
    /// If they drift the two LSP entry points (`(line, character)`
    /// from LSP semantic data, `char_index` from popup state) will
    /// produce visually different popups for the same logical position.
    #[test]
    fn buffer_pos_and_rope_index_agree() {
        let (mut world, entity) = make_editor_world();
        let (a, b) = world
            .run_system_once(move |anchors: BufferAnchorParam| {
                let a = anchors.at_buffer_pos(entity, 1, 3).unwrap();
                // "hello world\n" = 12 chars, "sec" = 3 → char_index 15.
                let b = anchors.at_rope_char_index(entity, 15).unwrap();
                (a, b)
            })
            .unwrap();
        assert_eq!(a.display_row, b.display_row);
        assert!((a.pixel_x - b.pixel_x).abs() < 1e-3);
        assert!((a.screen_top_left - b.screen_top_left).length() < 1e-3);
        assert!((a.world_top_left - b.world_top_left).length() < 1e-3);
    }

    /// Without a `DisplayLayout` (or with one whose
    /// `buffer_to_display` returns `None`), the helper must still
    /// produce a sensible fallback so consumers don't blow up before
    /// the first layout pass. Matches the rope→cell math the example's
    /// `cursor_screen_pos` used pre-API.
    #[test]
    fn fallback_uses_monospace_columns() {
        let viewport = TextViewViewport {
            width: 800,
            height: 600,
            hit_test_position: Vec2::ZERO,
            text_area_left: 50.0,
            text_area_top: 8.0,
            gutter_width: 40.0,
        };
        let scroll = ScrollState::default();
        let font = FontConfig {
            font: Handle::default(),
            font_size: 14.0,
            line_height: 21.0,
            char_width: 8.4,
            font_bold: None,
            font_italic: None,
            font_bold_italic: None,
            font_synthesis: Default::default(),
            inline_bg_hpad_em: 0.25,
        };
        let buffer = TextBuffer::with_text("plain text");

        let mut world = World::new();
        let entity = world.spawn((viewport, scroll, font, buffer)).id();

        let anchor = world
            .run_system_once(move |anchors: BufferAnchorParam| {
                anchors.at_buffer_pos(entity, 0, 5).unwrap()
            })
            .unwrap();
        // No layout → 1:1 buffer-row, monospace columns.
        assert_eq!(anchor.display_row, 0);
        assert!((anchor.pixel_x - 5.0 * 8.4).abs() < 1e-3);
    }

    /// The screen-space anchor the helper produces must match the
    /// math `cursor_screen_pos` did manually before this API existed —
    /// otherwise migrating callers shifts their popups.
    #[test]
    fn screen_pos_matches_legacy_cursor_screen_pos() {
        let (mut world, entity) = make_editor_world();
        let anchor = world
            .run_system_once(move |anchors: BufferAnchorParam| {
                anchors.at_buffer_pos(entity, 1, 3).unwrap()
            })
            .unwrap();

        // Legacy formula from examples/editor_lsp.rs::cursor_screen_pos.
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
            inline_bg_hpad_em: 0.25,
        };
        let metrics = super::row_metrics_with_baseline(&viewport, &scroll, &font, 14.0 * 0.32);
        let expected_screen_x = viewport.text_area_left + 3.0 * font.char_width
            - scroll.horizontal_scroll_offset;
        let expected_screen_y = metrics.row_y_top(1);

        assert!((anchor.screen_top_left.x - expected_screen_x).abs() < 1e-3);
        assert!((anchor.screen_top_left.y - expected_screen_y).abs() < 1e-3);
    }

    /// The world-space anchor the helper produces must match
    /// `RowMetrics::cell_world_pos_at_x` — the helper is a convenience
    /// wrapper, not a parallel code path.
    #[test]
    fn world_pos_matches_row_metrics() {
        let (mut world, entity) = make_editor_world();
        let anchor = world
            .run_system_once(move |anchors: BufferAnchorParam| {
                anchors.at_buffer_pos(entity, 2, 4).unwrap()
            })
            .unwrap();

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
            inline_bg_hpad_em: 0.25,
        };
        let metrics = super::row_metrics_with_baseline(&viewport, &scroll, &font, 14.0 * 0.32);
        let expected = metrics.cell_world_pos_at_x(2, 4.0 * font.char_width);

        assert!((anchor.world_top_left - expected).length() < 1e-3);
    }
}

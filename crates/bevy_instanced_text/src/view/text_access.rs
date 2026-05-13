//! Wrap-aware layout producer.
//!
//! `produce_layouts` is the engine's per-frame layout system. It walks
//! every `TextBuffer<T>` entity, reads its (optional) [`HiddenLines`] /
//! [`LineStyles`] / [`TextBounds`] data Components, shapes the visible
//! window through cosmic-text, and (when soft wrap is enabled) splits long
//! lines on a pixel-budget boundary into multiple `ShapedLine` rows. The
//! result is the per-frame `DisplayLayout` consumed by the renderer and by
//! cursor / selection / overlay producers.

use bevy::prelude::*;
use std::collections::HashMap;
use std::sync::Arc;

use super::font::MonoCellWidth;
use super::pipeline::DisplayLayout;
use super::glyph::{LineShape, ShapedGlyph, ShapedLine, StyleRun};
use super::text::{ContentMetrics, SmoothScroll, TextBuffer, TextContent};
use bevy::ui::ScrollPosition;
use super::text_style::{HiddenLines, LineStyles, RunWithText, TextBounds};
use bevy::ui::ComputedNode;
use crate::gpu::GlyphAtlas;

/// Default extra rows kept above and below the visible window.
pub const VIEWPORT_BUFFER_LINES: u32 = 4;

/// System set for layout production. Editor-side producer systems that
/// write `LineStyles` / `HiddenLines` should run `.before(LayoutProduceSet)`.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct LayoutProduceSet;

/// Per-entity dirty-detection key. Equality means "no rebuild needed".
/// Floats are compared by bit pattern (NaN comparisons aren't a real
/// concern — scroll/viewport never produce NaN under normal use).
///
/// Public so [`produce_layouts`] can be called as a Bevy system from
/// downstream crates / tests via `RunSystemOnce` — the `Local<HashMap<..>>`
/// parameter forces the inner type to be at least as visible as the system.
#[derive(Clone, PartialEq, Eq)]
pub struct LayoutFingerprint {
    /// True when `TextBuffer<T>` was marked changed by Bevy's change detection.
    buffer_changed: bool,
    scroll_bits: u32,
    h_scroll_bits: u32,
    viewport_w: u32,
    viewport_h: u32,
    viewport_top_bits: u32,
    font_size_tenths: u32,
    line_height_tenths: u32,
    style_arc_addr: usize,
    hidden_arc_addr: usize,
    wrap_budget_bits: u64,
    wrap_indent_bits: u32,
}

/// The buffer-line range a layout pass will touch. Shared between the
/// engine's `produce_layouts` and editor-side producer systems (e.g. the
/// syntax-styling system) so both agree on which lines are about to render.
///
/// Walks the buffer skipping hidden lines, returning `[start, end)` —
/// `start` is the first buffer line whose first display row is at or past
/// the visible top, `end` is one past the last buffer line whose first
/// display row is past the visible bottom.
pub fn visible_buffer_range(
    buffer: &impl TextContent,
    scroll_y: f32,
    viewport_height: f32,
    text_area_top: f32,
    line_height: f32,
    char_width: f32,
    wrap: TextBounds,
    hidden: Option<&HiddenLines>,
) -> std::ops::Range<usize> {
    let total = buffer.line_count();
    if total == 0 {
        return 0..0;
    }

    let buf_px = line_height * VIEWPORT_BUFFER_LINES as f32;
    let start_pixels = scroll_y - text_area_top - buf_px;
    let first_visible_display_row = (start_pixels / line_height).floor().max(0.0) as u32;
    let visible_count = ((viewport_height + buf_px * 2.0) / line_height).ceil() as u32;
    let last_visible_display_row = first_visible_display_row + visible_count;

    let approx_wrap_chars = wrap.width.map(|px| (px / char_width).max(1.0) as usize);
    let visible =
        |buffer_line: usize| -> bool { hidden.map(|h| h.is_visible(buffer_line)).unwrap_or(true) };

    // Walk forward to find `start`, the first buffer line whose display row
    // is in the visible window.
    let mut display_row: u32 = 0;
    let mut buffer_line: usize = 0;
    while buffer_line < total && display_row < first_visible_display_row {
        if visible(buffer_line) {
            display_row += approx_display_rows_for_line(buffer, buffer_line, approx_wrap_chars);
        }
        buffer_line += 1;
    }
    let start = buffer_line;

    // Continue forward until we pass the last visible display row.
    while buffer_line < total && display_row <= last_visible_display_row {
        if visible(buffer_line) {
            display_row += approx_display_rows_for_line(buffer, buffer_line, approx_wrap_chars);
        }
        buffer_line += 1;
    }
    let end = buffer_line;

    start..end
}

/// The engine's layout system. Registered by [`TextContentPlugin<T>`].
///
/// Walks every `TextBuffer<T>` entity, fingerprints its inputs, skips when
/// nothing changed, and otherwise rebuilds the entity's `DisplayLayout`.
/// Reads `Option<&HiddenLines>` and `Option<&LineStyles>` for editor-domain
/// folding / styling.
#[allow(clippy::type_complexity)]
pub fn produce_layouts<T: TextContent + Component>(
    mut q: Query<
        (
            Entity,
            Ref<TextBuffer<T>>,
            &ScrollPosition,
            &SmoothScroll,
            &mut ContentMetrics,
            &ComputedNode,
            &TextFont,
            &bevy::text::LineHeight,
            &MonoCellWidth,
            &mut DisplayLayout,
            Option<&HiddenLines>,
            Option<&LineStyles>,
            Option<&TextBounds>,
            Option<&super::measurement::LayoutTuning>,
        ),
    >,
    mut atlas: ResMut<GlyphAtlas>,
    fonts: Res<Assets<bevy::text::Font>>,
    mut last_fingerprints: Local<HashMap<Entity, LayoutFingerprint>>,
) {
    let _span = bevy::prelude::info_span!("produce_layouts").entered();
    let mut alive: std::collections::HashSet<Entity> = std::collections::HashSet::new();
    for (
        entity,
        buffer,
        scroll_pos,
        smooth,
        mut metrics,
        tv_viewport,
        font,
        lh,
        mono,
        mut layout,
        hidden,
        styles,
        wrap,
        tuning,
    ) in q.iter_mut()
    {
        let buffer_lines = tuning
            .map(|t| t.viewport_buffer_lines)
            .unwrap_or(VIEWPORT_BUFFER_LINES);
        alive.insert(entity);
        let wrap = wrap.copied().unwrap_or_default();
        let line_height = crate::view::font::resolve_line_height(*lh, font.font_size);

        // Identity-keyed change detection: a producer that writes a fresh
        // Arc each refresh changes the address; the engine refingerprints.
        let style_arc_addr = styles
            .map(|s| Arc::as_ptr(&s.by_line) as usize)
            .unwrap_or(0);
        let hidden_arc_addr = hidden.map(|h| Arc::as_ptr(&h.0) as usize).unwrap_or(0);

        let inv = tv_viewport.inverse_scale_factor();
        let logical = tv_viewport.size() * inv;
        let text_area_top = tv_viewport.content_inset().min_inset.y * inv;
        let fingerprint = LayoutFingerprint {
            buffer_changed: buffer.is_changed(),
            scroll_bits: scroll_pos.y.to_bits(),
            h_scroll_bits: smooth.horizontal.to_bits(),
            viewport_w: logical.x as u32,
            viewport_h: logical.y as u32,
            viewport_top_bits: text_area_top.to_bits(),
            font_size_tenths: (font.font_size * 10.0) as u32,
            line_height_tenths: (line_height * 10.0) as u32,
            style_arc_addr,
            hidden_arc_addr,
            wrap_budget_bits: wrap
                .width
                .map(|v| v.to_bits() as u64)
                .unwrap_or(u64::MAX),
            wrap_indent_bits: wrap.indent_px.to_bits(),
        };

        // Tracy diagnostic: which fingerprint field changed since last frame?
        // Each cache-miss reason gets its own zone name so Tracy's per-zone
        // counts directly tell us how often each invalidation source fires.
        // Helps catch spurious invalidations (e.g. an upstream producer
        // writing a fresh `Arc` every frame).
        macro_rules! rebuild {
            ($miss_name:literal) => {{
                let _miss = bevy::prelude::info_span!($miss_name).entered();
                let new_layout = build_display_layout(
                    &**buffer,
                    scroll_pos.y,
                    smooth.horizontal,
                    &mut metrics,
                    tv_viewport,
                    font,
                    line_height,
                    mono,
                    wrap,
                    layout.default_fg,
                    hidden,
                    styles,
                    Some(&mut atlas),
                    Some(&fonts),
                    buffer_lines,
                );
                *layout = new_layout;
            }};
        }

        match last_fingerprints.get(&entity) {
            None => rebuild!("layout_miss_first"),
            Some(prev) if prev == &fingerprint => continue,
            Some(prev) => {
                if prev.buffer_changed != fingerprint.buffer_changed && fingerprint.buffer_changed {
                    rebuild!("layout_miss_content");
                } else if prev.scroll_bits != fingerprint.scroll_bits
                    || prev.h_scroll_bits != fingerprint.h_scroll_bits
                {
                    rebuild!("layout_miss_scroll");
                } else if prev.viewport_w != fingerprint.viewport_w
                    || prev.viewport_h != fingerprint.viewport_h
                    || prev.viewport_top_bits != fingerprint.viewport_top_bits
                {
                    rebuild!("layout_miss_viewport");
                } else if prev.font_size_tenths != fingerprint.font_size_tenths
                    || prev.line_height_tenths != fingerprint.line_height_tenths
                {
                    rebuild!("layout_miss_font");
                } else if prev.style_arc_addr != fingerprint.style_arc_addr {
                    rebuild!("layout_miss_styles");
                } else if prev.hidden_arc_addr != fingerprint.hidden_arc_addr {
                    rebuild!("layout_miss_hidden");
                } else {
                    rebuild!("layout_miss_wrap");
                }
            }
        }
        last_fingerprints.insert(entity, fingerprint);
    }
    last_fingerprints.retain(|e, _| alive.contains(e));
}

/// Build a `DisplayLayout` for the visible viewport. Internal — called by
/// `produce_layouts`. Kept as a separate function so consumers wanting a
/// one-shot non-system build (e.g. tests) can call it directly.
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_display_layout(
    buffer: &impl TextContent,
    scroll_y: f32,
    _horizontal_scroll: f32,
    metrics: &mut ContentMetrics,
    viewport: &ComputedNode,
    font: &TextFont,
    line_height: f32,
    mono: &MonoCellWidth,
    wrap: TextBounds,
    default_fg: Color,
    hidden: Option<&HiddenLines>,
    styles: Option<&LineStyles>,
    atlas: Option<&mut GlyphAtlas>,
    fonts: Option<&Assets<bevy::text::Font>>,
    buffer_lines: u32,
) -> DisplayLayout {
    let TextBounds {
        width: wrap_width,
        indent_px: wrap_indent_px,
    } = wrap;
    let char_width = mono.px;
    let baseline_offset = font.font_size * 0.32;
    let total_buffer_lines = buffer.line_count();

    let line_visible =
        |buffer_line: usize| -> bool { hidden.map(|h| h.is_visible(buffer_line)).unwrap_or(true) };
    let line_style_runs = |buffer_line: u32| -> Vec<RunWithText> {
        styles
            .and_then(|s| s.get(buffer_line))
            .cloned()
            .unwrap_or_default()
    };

    // Visible range — same math as the helper, inlined here to also feed
    // first/last_visible_display_row for the y_top calculation.
    let inv = viewport.inverse_scale_factor();
    let logical = viewport.size() * inv;
    let text_area_top = viewport.content_inset().min_inset.y * inv;
    let buf_px = line_height * buffer_lines as f32;
    let start_pixels = scroll_y - text_area_top - buf_px;
    let first_visible_display_row = (start_pixels / line_height).floor().max(0.0) as u32;
    let visible_count = ((logical.y + buf_px * 2.0) / line_height).ceil() as u32;
    let last_visible_display_row = first_visible_display_row + visible_count;

    let approx_wrap_chars = wrap_width.map(|px| (px / char_width).max(1.0) as usize);
    let fast_path_start = (first_visible_display_row as usize).min(total_buffer_lines);
    let folding_in_play =
        approx_wrap_chars.is_none() && (0..fast_path_start).any(|l| !line_visible(l));
    let (start_buffer_line, mut current_display_row) =
        if approx_wrap_chars.is_some() || folding_in_play {
            let mut display_row: u32 = 0;
            let mut buffer_line: usize = 0;
            while buffer_line < total_buffer_lines && display_row < first_visible_display_row {
                if line_visible(buffer_line) {
                    let rows = approx_display_rows_for_line(buffer, buffer_line, approx_wrap_chars);
                    display_row += rows;
                }
                buffer_line += 1;
            }
            (buffer_line, display_row)
        } else {
            (fast_path_start, first_visible_display_row)
        };

    let mut shaped_lines: Vec<ShapedLine> = Vec::with_capacity(visible_count as usize);
    let visible_rows_start = current_display_row;

    let mut atlas_opt = atlas;

    for buffer_line in start_buffer_line..total_buffer_lines {
        if !line_visible(buffer_line) {
            continue;
        }
        if current_display_row > last_visible_display_row {
            break;
        }

        let line_text: String = buffer.line(buffer_line).into_owned();

        let styled = line_style_runs(buffer_line as u32);
        let line_bg = styled.iter().find_map(|s| s.run.bg);

        let mut runs: Vec<StyleRun> = Vec::with_capacity(styled.len());
        let mut byte_cursor = 0usize;
        let mut concat = String::new();
        for r in &styled {
            let len = r.text.len();
            if len == 0 {
                continue;
            }
            concat.push_str(&r.text);
            let mut run = r.run.clone();
            run.byte_range = byte_cursor..byte_cursor + len;
            runs.push(run);
            byte_cursor += len;
        }

        // The text the renderer walks. When runs is non-empty, prefer the
        // concatenation of run texts (matches the byte_range indexing).
        // When runs is empty, fall back to the raw rope line.
        let render_text = if !runs.is_empty() {
            concat
        } else {
            line_text.clone()
        };

        // Shape via cosmic-text when an atlas is available. Strip a trailing
        // newline first — the rope line includes it, but cosmic-text would
        // just emit a zero-advance glyph for it.
        let shape = atlas_opt.as_deref_mut().map(|atlas| {
            let _shape_span = bevy::prelude::info_span!("shape_line").entered();
            let shape_text = render_text.strip_suffix('\n').unwrap_or(&render_text);
            let font_id = fonts.and_then(|fs| atlas.ensure_font(&font.font, fs));
            Arc::new(atlas.shape_line(shape_text, font.font_size, font_id))
        });

        // Track the widest shaped line so far so external scroll UI can read
        // a real pixel extent rather than guessing from char counts.
        if let Some(s) = shape.as_ref() {
            if s.width > metrics.max_content_width {
                metrics.max_content_width = s.width;
            }
        }

        // y_top for a given display_row — the row's actual top edge in
        // screen-Y, with `text_area_top` as origin and `scroll_offset`
        // applied. Renderer + overlay math derive baseline / band positions
        // from this, so `y_top` consistently means "top of the leaded box".
        let y_top_for = |display_row: u32| -> f32 {
            text_area_top - scroll_y + display_row as f32 * line_height
        };

        // When wrap is on and the shaped line exceeds the budget, split into
        // multiple rows. Otherwise emit a single row covering the full text.
        let wrap_split = match (wrap_width, shape.as_ref()) {
            (Some(budget), Some(s)) if s.width > budget => {
                Some(wrap_into_rows(&render_text, &runs, s, budget))
            }
            _ => None,
        };

        match wrap_split {
            Some(rows) if !rows.is_empty() => {
                for (i, row) in rows.iter().enumerate() {
                    let row_shape = Arc::new(LineShape {
                        glyphs: row.glyphs.clone(),
                        width: row.width,
                        font_size: shape
                            .as_ref()
                            .map(|s| s.font_size)
                            .unwrap_or(font.font_size),
                    });
                    shaped_lines.push(ShapedLine {
                        display_row: current_display_row,
                        buffer_row: buffer_line as u32,
                        buffer_byte_offset: row.buffer_byte_offset,
                        is_wrap_continuation: i > 0,
                        y_top: y_top_for(current_display_row),
                        x_offset: if i > 0 { wrap_indent_px } else { 0.0 },
                        text: row.text.clone(),
                        runs: row.runs.clone(),
                        line_bg,
                        line_height: None,
                        padding_top: 0.0,
                        padding_bottom: 0.0,
                        shape: Some(row_shape),
                    });
                    current_display_row += 1;
                }
            }
            _ => {
                shaped_lines.push(ShapedLine {
                    display_row: current_display_row,
                    buffer_row: buffer_line as u32,
                    buffer_byte_offset: 0,
                    is_wrap_continuation: false,
                    y_top: y_top_for(current_display_row),
                    x_offset: 0.0,
                    text: render_text,
                    runs,
                    line_bg,
                    line_height: None,
                    padding_top: 0.0,
                    padding_bottom: 0.0,
                    shape,
                });
                current_display_row += 1;
            }
        }
    }

    let visible_rows_end = current_display_row;

    // With wrap off every line occupies exactly one display row, so the total
    // is just the visible buffer-line count — no need to walk all 150k lines.
    let total_display_rows: u32 = if approx_wrap_chars.is_none() {
        if hidden.is_some() {
            (0..total_buffer_lines).filter(|&l| line_visible(l)).count() as u32
        } else {
            total_buffer_lines as u32
        }
    } else {
        (0..total_buffer_lines)
            .filter(|&l| line_visible(l))
            .map(|l| approx_display_rows_for_line(buffer, l, approx_wrap_chars))
            .sum()
    };

    DisplayLayout {
        lines: Arc::new(shaped_lines),
        visible_rows: visible_rows_start..visible_rows_end,
        total_display_rows,
        line_height,
        char_width,
        baseline_offset,
        default_fg,
        version: 0,
        scroll_version: 0,
    }
}

/// One soft-wrap row's worth of post-shape data, ready to be packaged into a
/// `ShapedLine`. `glyphs` are line-local: each `g.x` has been rebased so the
/// row's first glyph starts at x=0.
#[derive(Clone, Debug)]
pub struct WrapRow {
    pub text: String,
    pub runs: Vec<StyleRun>,
    pub glyphs: Vec<ShapedGlyph>,
    pub width: f32,
    /// Byte offset within the source buffer line where this row's `text` starts.
    pub buffer_byte_offset: usize,
}

/// Split a shaped line into pixel-budgeted rows, preferring word-break
/// boundaries. The input `shape.glyphs[*].byte_index` are byte offsets into
/// `text`; emitted rows carry sliced text/runs and per-row local glyph x.
pub fn wrap_into_rows(
    text: &str,
    runs: &[StyleRun],
    shape: &LineShape,
    budget: f32,
) -> Vec<WrapRow> {
    if shape.glyphs.is_empty() || text.is_empty() {
        return Vec::new();
    }

    let mut rows: Vec<WrapRow> = Vec::new();
    let mut row_start_idx: usize = 0; // index into shape.glyphs
    let mut row_start_x: f32 = 0.0;

    while row_start_idx < shape.glyphs.len() {
        let row_origin_x = row_start_x;
        // First glyph whose left edge exceeds the budget.
        let mut break_idx = shape.glyphs.len();
        for j in row_start_idx + 1..shape.glyphs.len() {
            let local_x = shape.glyphs[j].x - row_origin_x;
            if local_x > budget {
                break_idx = j;
                break;
            }
        }

        if break_idx == shape.glyphs.len() {
            // Remaining glyphs fit — final row.
            let row_glyphs: Vec<ShapedGlyph> = shape.glyphs[row_start_idx..]
                .iter()
                .map(|g| ShapedGlyph {
                    x: g.x - row_origin_x,
                    byte_index: g.byte_index - shape.glyphs[row_start_idx].byte_index,
                    cache_key: g.cache_key,
                })
                .collect();
            let buf_byte_start = shape.glyphs[row_start_idx].byte_index;
            let row_text = text[buf_byte_start..].to_string();
            let row_runs = slice_runs(runs, buf_byte_start..text.len());
            let row_width = shape.width - row_origin_x;
            rows.push(WrapRow {
                text: row_text,
                runs: row_runs,
                glyphs: row_glyphs,
                width: row_width,
                buffer_byte_offset: buf_byte_start,
            });
            break;
        }

        // Try to break at the previous space/tab cluster.
        let mut chosen = break_idx;
        for j in (row_start_idx + 1..break_idx).rev() {
            let g = &shape.glyphs[j];
            if let Some(ch) = text[g.byte_index..].chars().next() {
                if ch == ' ' || ch == '\t' {
                    chosen = j + 1; // break *after* the whitespace
                    break;
                }
            }
        }

        // Avoid infinite loop on a single oversized glyph.
        if chosen <= row_start_idx {
            chosen = (row_start_idx + 1).min(shape.glyphs.len());
        }

        let row_byte_end = if chosen < shape.glyphs.len() {
            shape.glyphs[chosen].byte_index
        } else {
            text.len()
        };
        let row_byte_start = shape.glyphs[row_start_idx].byte_index;
        let row_glyphs: Vec<ShapedGlyph> = shape.glyphs[row_start_idx..chosen]
            .iter()
            .map(|g| ShapedGlyph {
                x: g.x - row_origin_x,
                byte_index: g.byte_index - row_byte_start,
                cache_key: g.cache_key,
            })
            .collect();
        let row_text = text[row_byte_start..row_byte_end].to_string();
        let row_runs = slice_runs(runs, row_byte_start..row_byte_end);
        let row_width = if chosen < shape.glyphs.len() {
            shape.glyphs[chosen].x - row_origin_x
        } else {
            shape.width - row_origin_x
        };
        rows.push(WrapRow {
            text: row_text,
            runs: row_runs,
            glyphs: row_glyphs,
            width: row_width,
            buffer_byte_offset: row_byte_start,
        });

        row_start_idx = chosen;
        row_start_x = if chosen < shape.glyphs.len() {
            shape.glyphs[chosen].x
        } else {
            shape.width
        };
    }

    rows
}

/// Clip and rebase a slice of runs to a byte sub-range.
pub fn slice_runs(runs: &[StyleRun], range: std::ops::Range<usize>) -> Vec<StyleRun> {
    let mut out = Vec::new();
    for run in runs {
        if run.byte_range.end <= range.start || run.byte_range.start >= range.end {
            continue;
        }
        let s = run.byte_range.start.max(range.start) - range.start;
        let e = run.byte_range.end.min(range.end) - range.start;
        if s >= e {
            continue;
        }
        out.push(StyleRun {
            byte_range: s..e,
            fg: run.fg,
            bg: run.bg,
            font_scale: run.font_scale,
            skew: run.skew,
            corner_radius: run.corner_radius,
            font_weight: run.font_weight,
            italic: run.italic,
            font: run.font.clone(),
            decoration: run.decoration,
            link: run.link.clone(),
        });
    }
    out
}

/// Cheap approximate display-row count for a buffer line. Used for
/// off-screen row accounting (sizing external scroll UI, scroll-offset →
/// first-visible-row translation) without paying the cost of full shaping.
pub fn approx_display_rows_for_line(
    buffer: &impl TextContent,
    buffer_line: usize,
    wrap_chars: Option<usize>,
) -> u32 {
    let Some(budget) = wrap_chars else {
        return 1;
    };
    if buffer_line >= buffer.line_count() {
        return 1;
    }
    let len = buffer.line_len_chars(buffer_line);
    if len == 0 {
        1
    } else {
        len.div_ceil(budget) as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_font() -> TextFont {
        TextFont::from_font_size(14.0)
    }

    fn test_mono() -> MonoCellWidth {
        MonoCellWidth { px: 8.0 }
    }

    fn test_line_height() -> f32 {
        21.0
    }

    fn test_computed() -> ComputedNode {
        let mut c = ComputedNode::default();
        c.size = bevy::math::Vec2::new(800.0, 600.0);
        c.inverse_scale_factor = 1.0;
        c
    }

    fn build(text: &str) -> DisplayLayout {
        let mut metrics = ContentMetrics::default();
        build_display_layout(
            &text.to_owned(),
            0.0,
            0.0,
            &mut metrics,
            &test_computed(),
            &test_font(),
            test_line_height(),
            &test_mono(),
            TextBounds::default(),
            Color::WHITE,
            None,
            None,
            None,
            None,
            VIEWPORT_BUFFER_LINES,
        )
    }

    fn line_texts(layout: &DisplayLayout) -> Vec<String> {
        layout.lines.iter().map(|l| l.text.clone()).collect()
    }

    /// Insert a newline in the middle of a line — the resulting layout's
    /// rendered text must reflect both halves of the split exactly.
    #[test]
    fn newline_in_middle_splits_line_in_layout() {
        let before = build("hello world\nsecond line\nthird line\n");
        assert_eq!(before.lines.len(), 4, "before: 4 lines (trailing \\n)");
        let before_texts = line_texts(&before);
        assert!(before_texts[0].starts_with("hello world"));
        assert!(before_texts[1].starts_with("second line"));

        // Mimic pressing Enter in the middle of "hello world" → "hello\n world"
        let after = build("hello\n world\nsecond line\nthird line\n");
        let after_texts = line_texts(&after);
        assert_eq!(after.lines.len(), 5, "after: split adds one row, total 5");
        assert!(
            after_texts[0].starts_with("hello"),
            "row 0 should be 'hello', got {:?}",
            after_texts[0]
        );
        assert!(
            !after_texts[0].starts_with("hello world"),
            "row 0 must NOT still contain the un-split text"
        );
        assert!(
            after_texts[1].starts_with(" world"),
            "row 1 should be ' world' (post-split tail), got {:?}",
            after_texts[1]
        );
        assert!(
            after_texts[2].starts_with("second line"),
            "row 2 must be the previously-row-1 'second line', got {:?}",
            after_texts[2]
        );
    }

    /// Backspace at the start of a line should merge it with the previous one.
    #[test]
    fn backspace_join_merges_lines_in_layout() {
        let before = build("hello\nworld\ntail\n");
        assert_eq!(before.lines.len(), 4, "'hello', 'world', 'tail', ''");

        // Remove the `\n` between "hello" and "world" → "helloworld\ntail\n"
        let after = build("helloworld\ntail\n");
        let after_texts = line_texts(&after);
        assert_eq!(after.lines.len(), 3, "join reduces line count by 1");
        assert!(
            after_texts[0].starts_with("helloworld"),
            "row 0 must be the joined 'helloworld', got {:?}",
            after_texts[0]
        );
        assert!(
            after_texts[1].starts_with("tail"),
            "row 1 must now be 'tail' (shifted up), got {:?}",
            after_texts[1]
        );
        assert!(
            !after_texts.iter().any(|t| t.starts_with("world")),
            "stale 'world' row leaked into layout: {:?}",
            after_texts
        );
    }

    /// Drive `produce_layouts` as a Bevy system across two ticks: tick 1
    /// builds the initial layout, tick 2 runs after an edit. The layout the
    /// system writes must reflect the post-edit buffer.
    #[test]
    fn produce_layouts_system_rebuilds_after_buffer_change() {
        use crate::gpu::GlyphAtlas;
        use bevy::asset::Assets;
        use bevy::ecs::system::RunSystemOnce;
        use bevy::image::Image;
        use bevy::prelude::*;
        use bevy::text::Font;

        let mut world = World::new();
        let mut images = Assets::<Image>::default();
        let atlas = GlyphAtlas::new(&mut images);
        world.insert_resource(images);
        world.insert_resource(atlas);
        world.insert_resource(Assets::<Font>::default());

        let entity = world
            .spawn((
                TextBuffer::new(crate::view::text::TextSpan::new(
                    "hello world\nsecond line\nthird line\n",
                )),
                bevy::ui::ScrollPosition::default(),
                SmoothScroll::default(),
                ContentMetrics::default(),
                test_computed(),
                test_font(),
                test_mono(),
                bevy::text::LineHeight::Px(test_line_height()),
                DisplayLayout::default(),
                TextBounds::default(),
                crate::view::measurement::LayoutTuning::default(),
            ))
            .id();

        world
            .run_system_once(produce_layouts::<crate::view::text::TextSpan>)
            .unwrap();
        let lines1 = world.get::<DisplayLayout>(entity).unwrap().lines.len();
        assert_eq!(lines1, 4, "initial layout: 4 rows");

        // Mimic the edit: replace buffer contents via DerefMut.
        {
            let mut buf = world
                .get_mut::<TextBuffer<crate::view::text::TextSpan>>(entity)
                .unwrap();
            buf.0 = crate::view::text::TextSpan::new(
                "hello\n world\nsecond line\nthird line\n",
            );
        }

        world
            .run_system_once(produce_layouts::<crate::view::text::TextSpan>)
            .unwrap();
        let layout2 = world.get::<DisplayLayout>(entity).unwrap();
        assert_eq!(
            layout2.lines.len(),
            5,
            "post-edit layout: 5 rows (split added one)"
        );
        let texts: Vec<&str> = layout2.lines.iter().map(|l| l.text.as_str()).collect();
        assert!(texts[0].starts_with("hello"), "row 0 = 'hello', got {:?}", texts[0]);
        assert!(texts[1].starts_with(" world"), "row 1 = ' world', got {:?}", texts[1]);
    }

    /// `buffer_row` on each `ShapedLine` must reflect the post-edit buffer.
    #[test]
    fn buffer_rows_reindex_after_line_removal() {
        let before = build("a\nb\nc\nd\ne\n");
        let before_rows: Vec<u32> = before.lines.iter().map(|l| l.buffer_row).collect();
        assert_eq!(before_rows, vec![0, 1, 2, 3, 4, 5]);

        // Delete line "c" entirely → "a\nb\nd\ne\n"
        let after = build("a\nb\nd\ne\n");
        let after_rows: Vec<u32> = after.lines.iter().map(|l| l.buffer_row).collect();
        let after_texts = line_texts(&after);
        assert_eq!(
            after.lines.len(),
            5,
            "after deletion: 'a','b','d','e','' (5 lines), got texts={:?}",
            after_texts
        );
        assert_eq!(
            after_rows,
            vec![0, 1, 2, 3, 4],
            "buffer_rows must be contiguous 0..N after deletion, got {:?} texts={:?}",
            after_rows,
            after_texts
        );
        assert!(after_texts[2].starts_with('d'), "row 2 should be 'd' now");
    }
}


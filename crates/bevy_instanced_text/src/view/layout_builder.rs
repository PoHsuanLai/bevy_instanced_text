//! Wrap-aware layout producer.
//!
//! `produce_layouts` is the engine's per-frame layout system. It walks
//! every `TextView` entity, reads its (optional) [`HiddenLines`] /
//! [`LineStyles`] / [`LayoutWrap`] data Components, shapes the visible
//! window through cosmic-text, and (when soft wrap is enabled) splits long
//! lines on a pixel-budget boundary into multiple `ShapedLine` rows. The
//! result is the per-frame `DisplayLayout` consumed by the renderer and by
//! cursor / selection / overlay producers.
//!
//! These data Components plug editor-domain concepts (folds, syntax) into
//! the engine without making the engine depend on them. Markdown / chat /
//! log-viewer consumers can write the same Components from their own
//! producer systems.

use bevy::prelude::*;
use std::collections::HashMap;
use std::sync::Arc;

use super::font::FontConfig;
use super::layout::DisplayLayout;
use super::plugin::TextView;
use super::snapshot::{Block, BlockLayoutConfig, LineShape, ShapedGlyph, ShapedLine, StyleRun};
use super::state::{ContentMetrics, ScrollState, TextBuffer};
use super::styling::{BlockList, HiddenLines, LayoutWrap, LineStyles, RunWithText};
use super::viewport::TextViewViewport;
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
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct LayoutFingerprint {
    content_version: u64,
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
/// Walks `buffer.rope` skipping hidden lines, returning `[start, end)` —
/// `start` is the first buffer line whose first display row is at or past
/// the visible top, `end` is one past the last buffer line whose first
/// display row is past the visible bottom.
pub fn visible_buffer_range(
    buffer: &TextBuffer,
    scroll: &ScrollState,
    viewport: &TextViewViewport,
    font: &FontConfig,
    wrap: LayoutWrap,
    hidden: Option<&HiddenLines>,
) -> std::ops::Range<usize> {
    let line_height = font.line_height;
    let char_width = font.char_width;
    let total = buffer.line_count();
    if total == 0 {
        return 0..0;
    }

    let buf_px = line_height * VIEWPORT_BUFFER_LINES as f32;
    let scroll_dist = scroll.scroll_offset.abs();
    let start_pixels = scroll_dist - viewport.text_area_top - buf_px;
    let first_visible_display_row = (start_pixels / line_height).floor().max(0.0) as u32;
    let visible_count = ((viewport.height as f32 + buf_px * 2.0) / line_height).ceil() as u32;
    let last_visible_display_row = first_visible_display_row + visible_count;

    let approx_wrap_chars = wrap.budget_px.map(|px| (px / char_width).max(1.0) as usize);
    let visible = |buffer_line: usize| -> bool {
        hidden.map(|h| h.is_visible(buffer_line)).unwrap_or(true)
    };

    // Walk forward to find `start`, the first buffer line whose display row
    // is in the visible window.
    let mut display_row: u32 = 0;
    let mut buffer_line: usize = 0;
    while buffer_line < total && display_row < first_visible_display_row {
        if visible(buffer_line) {
            display_row += approx_display_rows_for_line(
                &buffer.rope,
                buffer_line,
                approx_wrap_chars,
            );
        }
        buffer_line += 1;
    }
    let start = buffer_line;

    // Continue forward until we pass the last visible display row.
    while buffer_line < total && display_row <= last_visible_display_row {
        if visible(buffer_line) {
            display_row += approx_display_rows_for_line(
                &buffer.rope,
                buffer_line,
                approx_wrap_chars,
            );
        }
        buffer_line += 1;
    }
    let end = buffer_line;

    start..end
}

/// The engine's layout system. Registered by `InstancedTextPlugin`.
///
/// Walks every `TextView` entity, fingerprints its inputs, skips when
/// nothing changed, and otherwise rebuilds the entity's `DisplayLayout`.
/// Reads `Option<&HiddenLines>` and `Option<&LineStyles>` for editor-domain
/// folding / styling.
#[allow(clippy::type_complexity)]
pub(crate) fn produce_layouts(
    mut q: Query<
        (
            Entity,
            &TextBuffer,
            &ScrollState,
            &mut ContentMetrics,
            &TextViewViewport,
            &FontConfig,
            &mut DisplayLayout,
            Option<&HiddenLines>,
            Option<&LineStyles>,
            Option<&LayoutWrap>,
            Option<&super::tuning::LayoutTuning>,
        ),
        // Block-driven entities have their layout written by
        // `produce_block_layout`; skip them here to avoid double-writes.
        (With<TextView>, Without<BlockList>),
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
        scroll,
        mut metrics,
        tv_viewport,
        font,
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

        // Identity-keyed change detection: a producer that writes a fresh
        // Arc each refresh changes the address; the engine refingerprints.
        let style_arc_addr = styles
            .map(|s| Arc::as_ptr(&s.by_line) as usize)
            .unwrap_or(0);
        let hidden_arc_addr = hidden.map(|h| Arc::as_ptr(&h.0) as usize).unwrap_or(0);

        let fingerprint = LayoutFingerprint {
            content_version: buffer.content_version,
            scroll_bits: scroll.scroll_offset.to_bits(),
            h_scroll_bits: scroll.horizontal_scroll_offset.to_bits(),
            viewport_w: tv_viewport.width,
            viewport_h: tv_viewport.height,
            viewport_top_bits: tv_viewport.text_area_top.to_bits(),
            font_size_tenths: (font.font_size * 10.0) as u32,
            line_height_tenths: (font.line_height * 10.0) as u32,
            style_arc_addr,
            hidden_arc_addr,
            wrap_budget_bits: wrap
                .budget_px
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
                    buffer,
                    scroll,
                    &mut metrics,
                    tv_viewport,
                    font,
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
                if prev.content_version != fingerprint.content_version {
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
    buffer: &TextBuffer,
    scroll: &ScrollState,
    metrics: &mut ContentMetrics,
    viewport: &TextViewViewport,
    font: &FontConfig,
    wrap: LayoutWrap,
    default_fg: Color,
    hidden: Option<&HiddenLines>,
    styles: Option<&LineStyles>,
    atlas: Option<&mut GlyphAtlas>,
    fonts: Option<&Assets<bevy::text::Font>>,
    buffer_lines: u32,
) -> DisplayLayout {
    let LayoutWrap {
        budget_px: wrap_budget_px,
        indent_px: wrap_indent_px,
    } = wrap;
    let line_height = font.line_height;
    let char_width = font.char_width;
    let baseline_offset = font.font_size * 0.32;
    let total_buffer_lines = buffer.line_count();

    let line_visible = |buffer_line: usize| -> bool {
        hidden.map(|h| h.is_visible(buffer_line)).unwrap_or(true)
    };
    let line_style_runs = |buffer_line: u32| -> Vec<RunWithText> {
        styles
            .and_then(|s| s.get(buffer_line))
            .cloned()
            .unwrap_or_default()
    };

    // Visible range — same math as the helper, inlined here to also feed
    // first/last_visible_display_row for the y_top calculation.
    let buf_px = line_height * buffer_lines as f32;
    let scroll_dist = scroll.scroll_offset.abs();
    let start_pixels = scroll_dist - viewport.text_area_top - buf_px;
    let first_visible_display_row = (start_pixels / line_height).floor().max(0.0) as u32;
    let visible_count =
        ((viewport.height as f32 + buf_px * 2.0) / line_height).ceil() as u32;
    let last_visible_display_row = first_visible_display_row + visible_count;

    let approx_wrap_chars = wrap_budget_px.map(|px| (px / char_width).max(1.0) as usize);
    let fast_path_start = (first_visible_display_row as usize).min(total_buffer_lines);
    let folding_in_play = approx_wrap_chars.is_none()
        && (0..fast_path_start).any(|l| !line_visible(l));
    let (start_buffer_line, mut current_display_row) =
        if approx_wrap_chars.is_some() || folding_in_play {
            let mut display_row: u32 = 0;
            let mut buffer_line: usize = 0;
            while buffer_line < total_buffer_lines && display_row < first_visible_display_row {
                if line_visible(buffer_line) {
                    let rows = approx_display_rows_for_line(
                        &buffer.rope,
                        buffer_line,
                        approx_wrap_chars,
                    );
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

        let rope_line = buffer.rope.line(buffer_line);
        let line_text: String = rope_line.to_string();

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
            viewport.text_area_top + scroll.scroll_offset + display_row as f32 * line_height
        };

        // When wrap is on and the shaped line exceeds the budget, split into
        // multiple rows. Otherwise emit a single row covering the full text.
        let wrap_split = match (wrap_budget_px, shape.as_ref()) {
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
                        font_size: shape.as_ref().map(|s| s.font_size).unwrap_or(font.font_size),
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
            .map(|l| approx_display_rows_for_line(&buffer.rope, l, approx_wrap_chars))
            .sum()
    };

    DisplayLayout {
        lines: Arc::new(shaped_lines),
        block_rects: Arc::new(Vec::new()),
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
    rope: &ropey::Rope,
    buffer_line: usize,
    wrap_chars: Option<usize>,
) -> u32 {
    let Some(budget) = wrap_chars else {
        return 1;
    };
    if buffer_line >= rope.len_lines() {
        return 1;
    }
    let line = rope.line(buffer_line);
    let mut len = line.len_chars();
    if len > 0 && line.char(len - 1) == '\n' {
        len -= 1;
    }
    if len == 0 {
        1
    } else {
        len.div_ceil(budget) as u32
    }
}

/// Per-entity dirty key for block-driven layouts.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct BlockLayoutFingerprint {
    block_arc_addr: usize,
    font_size_tenths: u32,
    line_height_tenths: u32,
    char_width_tenths: u32,
    wrap_chars: u32,
    default_fg_bits: [u32; 4],
}

/// Engine system for the static-content path. Walks every `TextView` entity
/// carrying a [`BlockList`], reads the current block list, and writes the
/// entity's `DisplayLayout` via [`Block::layout`].
///
/// Skips when the `BlockList`'s Arc identity + font / wrap inputs are
/// unchanged from the previous run. Producers that want to update the list
/// should swap in a fresh `BlockList::new(blocks)` so the Arc address moves.
#[allow(clippy::type_complexity)]
pub(crate) fn produce_block_layout(
    mut q: Query<
        (
            Entity,
            &BlockList,
            &FontConfig,
            &mut DisplayLayout,
            Option<&LayoutWrap>,
        ),
        With<TextView>,
    >,
    mut last_fingerprints: Local<HashMap<Entity, BlockLayoutFingerprint>>,
) {
    let mut alive: std::collections::HashSet<Entity> = std::collections::HashSet::new();
    for (entity, blocks, font, mut layout, wrap) in q.iter_mut() {
        alive.insert(entity);
        let wrap = wrap.copied().unwrap_or_default();
        let char_width = font.char_width.max(1.0);
        let wrap_chars = wrap
            .budget_px
            .map(|px| (px / char_width).floor().max(0.0) as u32)
            .unwrap_or(0);

        let fg_l = layout.default_fg.to_linear();
        let fingerprint = BlockLayoutFingerprint {
            block_arc_addr: Arc::as_ptr(&blocks.0) as usize,
            font_size_tenths: (font.font_size * 10.0) as u32,
            line_height_tenths: (font.line_height * 10.0) as u32,
            char_width_tenths: (font.char_width * 10.0) as u32,
            wrap_chars,
            default_fg_bits: [
                fg_l.red.to_bits(),
                fg_l.green.to_bits(),
                fg_l.blue.to_bits(),
                fg_l.alpha.to_bits(),
            ],
        };
        if last_fingerprints.get(&entity) == Some(&fingerprint) {
            continue;
        }

        let cfg = BlockLayoutConfig {
            line_height: font.line_height,
            char_width: font.char_width,
            // Mirror of the editor's baseline-offset convention; ~32% of font size.
            baseline_offset: font.font_size * 0.32,
            default_fg: layout.default_fg,
            default_wrap_chars: if wrap_chars > 0 { Some(wrap_chars as usize) } else { None },
        };
        *layout = Block::layout(&blocks.0, cfg);
        last_fingerprints.insert(entity, fingerprint);
    }

    last_fingerprints.retain(|e, _| alive.contains(e));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Smoke-test the Component → system → DisplayLayout flow. Spawn a
    /// `TextView` entity with a `BlockList`, run `produce_block_layout`
    /// once via a minimal Schedule, verify the entity's `DisplayLayout`
    /// has the right rows.
    #[test]
    fn produce_block_layout_writes_display_layout() {
        let mut world = World::new();
        let blocks = vec![
            Block::new("hello"),
            Block::new("world").with_padding(4.0, 4.0),
        ];
        let entity = world
            .spawn((
                TextView,
                FontConfig::from_size(16.0),
                DisplayLayout::default(),
                BlockList::new(blocks),
            ))
            .id();

        let mut schedule = Schedule::default();
        schedule.add_systems(produce_block_layout);
        schedule.run(&mut world);

        let layout = world.get::<DisplayLayout>(entity).expect("layout missing");
        assert_eq!(layout.lines.len(), 2);
        assert_eq!(layout.lines[0].text, "hello");
        assert_eq!(layout.lines[1].text, "world");
        // padding_top on the second block lifts row 1 by 4px above the
        // baseline of "16px line height + previous row".
        assert!(layout.lines[1].y_top > layout.lines[0].y_top + 16.0);
    }

    /// Re-running the system without swapping the `BlockList` Arc skips
    /// the rebuild.
    #[test]
    fn produce_block_layout_skips_when_arc_unchanged() {
        let mut world = World::new();
        let entity = world
            .spawn((
                TextView,
                FontConfig::from_size(16.0),
                DisplayLayout::default(),
                BlockList::new(vec![Block::new("once")]),
            ))
            .id();

        let mut schedule = Schedule::default();
        schedule.add_systems(produce_block_layout);
        schedule.run(&mut world);
        let first_arc = world.get::<DisplayLayout>(entity).unwrap().lines.clone();

        schedule.run(&mut world);
        let second_arc = world.get::<DisplayLayout>(entity).unwrap().lines.clone();
        // Second run should reuse the same Arc — no rebuild.
        assert!(Arc::ptr_eq(&first_arc, &second_arc));
    }

    /// `BlockList` Component cooperates with `LayoutWrap`: the system
    /// translates `LayoutWrap.budget_px` into a char budget via
    /// `FontConfig.char_width` and applies it as the default wrap.
    #[test]
    fn produce_block_layout_honors_layout_wrap() {
        let mut world = World::new();
        // 16px font, char_width = 8px, budget_px = 80px → 10-char budget.
        let mut font = FontConfig::from_size(16.0);
        font.char_width = 8.0;
        let entity = world
            .spawn((
                TextView,
                font,
                DisplayLayout::default(),
                BlockList::new(vec![Block::new(
                    "the quick brown fox jumps over the lazy dog and runs away.",
                )]),
                LayoutWrap {
                    budget_px: Some(80.0),
                    indent_px: 0.0,
                },
            ))
            .id();

        let mut schedule = Schedule::default();
        schedule.add_systems(produce_block_layout);
        schedule.run(&mut world);

        let layout = world.get::<DisplayLayout>(entity).unwrap();
        // 58 chars / 10-char budget → multiple wrap rows.
        assert!(layout.lines.len() >= 2, "expected wrap, got {}", layout.lines.len());
    }
}

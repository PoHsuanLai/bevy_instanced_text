//! Snapshot types — what to render, owned by `text_view/`.
//!
//! These types are the renderer's data contract: a consumer (editor, chat panel,
//! log viewer) hands us styled lines + display layout; we render them. The renderer
//! does not know where the styling came from (syntax, markdown, plain text).

use bevy::prelude::*;
use std::ops::Range;
use std::sync::Arc;

/// One glyph from cosmic-text shaping. Rendered by looking up `cache_key` in the atlas.
///
/// `byte_index` is the cluster-start byte in the parent `ShapedLine.text` — a single
/// glyph may cover multiple bytes (ligatures, combining marks). Renderer consumers
/// that need a per-glyph color resolve it by binary-searching `ShapedLine.runs` on
/// `byte_index`.
#[derive(Clone, Copy, Debug)]
pub struct ShapedGlyph {
    /// Pen-x at glyph start, line-local in pixels (does not include `ShapedLine.x_offset`).
    pub x: f32,
    /// First byte in `ShapedLine.text` covered by this glyph.
    pub byte_index: usize,
    /// Atlas key — pass to `GlyphAtlas::get_or_rasterize_glyph`.
    pub cache_key: cosmic_text::CacheKey,
}

/// Per-line cosmic-text shaping result. Held by `ShapedLine.shape` as `Arc<LineShape>`
/// so scroll-only frames can reuse the previous frame's shape via `Arc::ptr_eq`.
#[derive(Clone, Debug)]
pub struct LineShape {
    /// Shaped glyphs in visual order. Indices align 1:1 with the cosmic-text
    /// `LayoutLine.glyphs` they were derived from.
    pub glyphs: Vec<ShapedGlyph>,
    /// Total advance of the line in pixels — equals last glyph's pen-x + last advance.
    /// Consumed by the display-map producer to drive `ContentMetrics.max_content_width`
    /// (the horizontal content extent, exposed for external scroll UI).
    pub width: f32,
    /// Font size at which shaping was performed. Renderer compares against its own
    /// font_size and falls back to the char_width path on mismatch.
    pub font_size: f32,
}

bitflags::bitflags! {
    /// Text decorations applied across a `StyleRun`. Flags can be combined
    /// (e.g. `TextDecoration::UNDERLINE | TextDecoration::STRIKETHROUGH`).
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct TextDecoration: u8 {
        const UNDERLINE      = 0b001;
        const STRIKETHROUGH  = 0b010;
        /// Wavy underline (typically for diagnostics).
        const SQUIGGLE       = 0b100;
    }
}

/// A run of text within a shaped line that shares the same style.
///
/// Byte ranges index into the parent `ShapedLine.text` (post-fold/wrap), so
/// runs are sparse and the renderer doesn't materialize per-buffer-line
/// `Vec<Option<…>>` arrays.
///
/// Most fields are `Option`s with `None` meaning "use the layout default."
/// This keeps cheap monospace consumers untouched: an editor that only needs
/// foreground color and italic skew leaves weight / family / decoration / link
/// all `None` and pays nothing extra.
#[derive(Clone, Debug)]
pub struct StyleRun {
    /// Byte range within the parent `ShapedLine.text`. Sorted, non-overlapping.
    pub byte_range: Range<usize>,
    pub fg: Color,
    pub bg: Option<Color>,
    /// 1.0 = normal, 1.3 = header, etc. 0.0 means use line default.
    pub font_scale: f32,
    /// Horizontal skew applied to glyphs in this run (~0.2 = italic-ish).
    /// Set explicitly to force a skew regardless of italic-face availability;
    /// the renderer also writes this when `italic = true` and no italic face
    /// is loaded (synthetic italic, controlled by
    /// `TextFont.font_synthesis.style`).
    pub skew: f32,
    pub corner_radius: f32,
    /// Font weight (100..=900). `None` = layout default. The renderer maps
    /// `Some(w >= 600)` to the entity's bold face when one is loaded;
    /// otherwise (and when `TextFont.font_synthesis.weight` is on) it
    /// synthesizes by stroke-doubling the regular face.
    pub font_weight: Option<u16>,
    /// Italic flag. The renderer maps `true` to the entity's italic (or
    /// bold-italic) face when one is loaded; otherwise (and when
    /// `TextFont.font_synthesis.style` is on) it synthesizes via skew.
    pub italic: bool,
    /// Override font for this run. When set, the renderer registers and shapes
    /// with this handle instead of the entity's `TextFont` slots. `None` =
    /// use the entity default.
    pub font: Option<bevy::asset::Handle<bevy::text::Font>>,
    /// Decorations drawn alongside the text. Combine flags freely:
    /// `TextDecoration::UNDERLINE | TextDecoration::STRIKETHROUGH`.
    /// `TextDecoration::empty()` means no decoration.
    pub decoration: TextDecoration,
    /// URL or anchor target if this run is a link. Click handlers in the
    /// interaction layer can dispatch on this.
    pub link: Option<Arc<str>>,
}

impl StyleRun {
    pub fn fg_only(byte_range: Range<usize>, fg: Color) -> Self {
        Self {
            byte_range,
            fg,
            bg: None,
            font_scale: 0.0,
            skew: 0.0,
            corner_radius: 0.0,
            font_weight: None,
            italic: false,
            font: None,
            decoration: TextDecoration::empty(),
            link: None,
        }
    }
}

/// One display row's worth of text + styling, ready to render.
///
/// Produced by `display_map::build_display_layout`. Folding, soft-wrap, and tab
/// expansion have already been applied — `text` is exactly what appears on screen
/// for this row, `runs` covers it.
#[derive(Clone, Debug)]
pub struct ShapedLine {
    /// Display row index (0-based, post-fold/wrap).
    pub display_row: u32,
    /// Source buffer line. Multiple display rows may share a buffer row when wrapped.
    pub buffer_row: u32,
    /// Byte offset within the buffer line where this row's `text` begins.
    /// Always 0 for non-wrapped rows; for soft-wrap continuations it's the
    /// byte index in the source line at which this row picks up. Lets
    /// consumers convert `(buffer_byte) → (display_row, byte_in_row)` without
    /// re-deriving from row text lengths.
    pub buffer_byte_offset: usize,
    /// True when this row is a soft-wrap continuation of the previous row.
    pub is_wrap_continuation: bool,
    /// Pre-computed Y position in pixels relative to the layout origin. The
    /// renderer trusts this value and does not recompute it from
    /// `display_row * line_height` — important when `line_height` overrides
    /// produce non-uniform row heights (markdown headings, code blocks).
    pub y_top: f32,
    /// Per-line X offset (indent, right-align, soft-wrap continuation).
    pub x_offset: f32,
    /// The text to render for this row, post-fold/wrap/tab expansion.
    pub text: String,
    /// Styled runs covering `text`. Sorted by `byte_range.start`, non-overlapping.
    /// Empty = render as plain text using the layout's default foreground.
    pub runs: Vec<StyleRun>,
    /// Optional full-line background.
    pub line_bg: Option<Color>,
    /// Per-row line-height override in pixels. `None` = use the layout's
    /// global `line_height`. Producers that emit non-uniform rows (markdown
    /// headings, code blocks at a different size) set this; helpers that
    /// stack rows must compute `y_top` accordingly (see `trivial_layout`).
    pub line_height: Option<f32>,
    /// Vertical space in pixels above this row, on top of the row's line
    /// height. Used for block-level spacing — heading top margins, paragraph
    /// breaks, code-block separators. Producers that stack rows must include
    /// this when computing `y_top`. Default 0.
    pub padding_top: f32,
    /// Vertical space in pixels below this row. See `padding_top`.
    pub padding_bottom: f32,
    /// Per-glyph advances from cosmic-text shaping. `None` = use the layout's
    /// `char_width` fallback (cheap path for `trivial_layout` consumers like
    /// chat/log demos that don't want to pay shaping cost).
    pub shape: Option<Arc<LineShape>>,
}

/// Build a `DisplayLayout` from `(text, runs)` pairs without folding, wrapping,
/// or viewport culling. Suitable for standalone consumers without a display map.
pub fn trivial_layout(
    lines: &[(String, Vec<StyleRun>)],
    line_height: f32,
    char_width: f32,
    baseline_offset: f32,
    default_fg: bevy::prelude::Color,
) -> super::layout::DisplayLayout {
    use super::layout::DisplayLayout;
    use std::sync::Arc;

    let shaped: Vec<ShapedLine> = lines
        .iter()
        .enumerate()
        .map(|(i, (text, runs))| ShapedLine {
            display_row: i as u32,
            buffer_row: i as u32,
            buffer_byte_offset: 0,
            is_wrap_continuation: false,
            // y_top is the row's visual top in screen-Y. Caller's render system
            // adds the viewport's text_area_top + scroll_offset on top if needed;
            // for a static demo we just stack rows from y=0.
            y_top: i as f32 * line_height,
            x_offset: 0.0,
            text: text.clone(),
            runs: runs.clone(),
            line_bg: None,
            line_height: None,
            padding_top: 0.0,
            padding_bottom: 0.0,
            shape: None,
        })
        .collect();
    let total = shaped.len() as u32;
    DisplayLayout {
        lines: Arc::new(shaped),
        block_rects: Arc::new(Vec::new()),
        visible_rows: 0..total,
        total_display_rows: total,
        line_height,
        char_width,
        baseline_offset,
        default_fg,
        version: 1,
        scroll_version: 0,
    }
}

/// A border applied around a block's outer rect.
///
/// `width` is in pixels and applies uniformly to all four sides — uniform-width
/// is enough for code blocks, blockquotes, and panels which is what markdown /
/// chat consumers ask for. Per-side widths are out of scope until a consumer
/// actually needs them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BlockBorder {
    pub color: Color,
    pub width: f32,
}

/// Block-level decoration: background fill spanning the block's full vertical
/// extent (including `padding_top` + all wrap rows + `padding_bottom`), plus
/// an optional border.
///
/// Distinct from per-row `ShapedLine.line_bg`, which paints one row at a time
/// and visually splits a wrapped paragraph. Use a `BlockDecoration` when you
/// want a fenced code block / blockquote / chat-message bubble to render as
/// one panel.
#[derive(Clone, Debug, Default)]
pub struct BlockDecoration {
    pub background: Option<Color>,
    pub border: Option<BlockBorder>,
    /// Corner radius applied to the block rect (and inset by border width
    /// on the inside). 0 = sharp corners.
    pub corner_radius: f32,
}

/// One block's footprint inside a [`super::layout::DisplayLayout`]. The
/// renderer uses this to draw block-level backgrounds and borders before any
/// per-row backgrounds or glyphs.
///
/// `display_row_end` is inclusive. `indent` is the block's left x (also stored
/// on each row's `x_offset`); the rect extends to the right edge of the
/// content area, computed by the renderer from the viewport width.
#[derive(Clone, Debug)]
pub struct BlockRect {
    pub display_row_start: u32,
    pub display_row_end: u32,
    pub indent: f32,
    pub padding_top: f32,
    pub padding_bottom: f32,
    pub decoration: BlockDecoration,
}

#[derive(Clone, Debug, Default)]
pub struct Block {
    pub text: String,
    pub runs: Vec<StyleRun>,
    /// Per-row line-height in pixels. `None` = layout default.
    pub line_height: Option<f32>,
    pub padding_top: f32,
    pub padding_bottom: f32,
    pub indent: f32,
    pub line_bg: Option<Color>,
    /// Soft-wrap budget in characters. `None` = inherit from the
    /// `layout_blocks` default. `Some(0)` = no wrap (block stays one row).
    pub wrap_chars: Option<usize>,
    /// Block-level background (spans padding + all wrap rows). Distinct from
    /// `line_bg`; both can coexist (the line bg paints over the block bg).
    pub block_bg: Option<Color>,
    pub block_border: Option<BlockBorder>,
    pub block_corner_radius: f32,
}

impl Block {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            ..Default::default()
        }
    }

    pub fn with_runs(mut self, runs: Vec<StyleRun>) -> Self {
        self.runs = runs;
        self
    }

    pub fn with_line_height(mut self, lh: f32) -> Self {
        self.line_height = Some(lh);
        self
    }

    pub fn with_padding(mut self, top: f32, bottom: f32) -> Self {
        self.padding_top = top;
        self.padding_bottom = bottom;
        self
    }

    pub fn with_indent(mut self, indent: f32) -> Self {
        self.indent = indent;
        self
    }

    /// Wrap this block's text on a per-character budget. Continuation rows
    /// inherit the block's `indent`. Whitespace-aware: breaks at the last
    /// space/tab before the budget when one exists; otherwise hard-breaks.
    /// `chars == 0` disables wrap for this block (overrides the layout default).
    pub fn with_wrap_chars(mut self, chars: usize) -> Self {
        self.wrap_chars = Some(chars);
        self
    }

    /// Block-level background fill. Spans padding_top + all wrap rows +
    /// padding_bottom — the entire block's footprint, not per-row. Use this
    /// for fenced code blocks, blockquotes, chat-message bubbles.
    pub fn with_block_background(mut self, color: Color) -> Self {
        self.block_bg = Some(color);
        self
    }

    /// Border drawn at the outer edge of the block's rect.
    pub fn with_block_border(mut self, color: Color, width: f32) -> Self {
        self.block_border = Some(BlockBorder { color, width });
        self
    }

    /// Corner radius for `with_block_background` / `with_block_border`.
    /// 0 = sharp corners.
    pub fn with_block_corner_radius(mut self, radius: f32) -> Self {
        self.block_corner_radius = radius;
        self
    }
}

/// Metrics + defaults for [`Block::layout`]. In the ECS flow these come from
/// per-entity Components; this struct is the headless entry point for tests and
/// embeddings.
#[derive(Clone, Copy, Debug)]
pub struct BlockLayoutConfig {
    pub line_height: f32,
    pub char_width: f32,
    pub baseline_offset: f32,
    pub default_fg: bevy::prelude::Color,
    /// Soft-wrap budget in characters applied to blocks that don't override
    /// it. `None` ⇒ no wrap. Per-block override via [`Block::with_wrap_chars`].
    pub default_wrap_chars: Option<usize>,
}

impl Block {
    pub fn layout(blocks: &[Block], cfg: BlockLayoutConfig) -> super::layout::DisplayLayout {
        layout_blocks_inner(blocks, cfg)
    }
}

fn layout_blocks_inner(blocks: &[Block], cfg: BlockLayoutConfig) -> super::layout::DisplayLayout {
    use super::layout::DisplayLayout;
    use std::sync::Arc;
    let BlockLayoutConfig {
        line_height,
        char_width,
        baseline_offset,
        default_fg,
        default_wrap_chars,
    } = cfg;

    let mut shaped: Vec<ShapedLine> = Vec::with_capacity(blocks.len());
    let mut block_rects: Vec<BlockRect> = Vec::new();
    let mut y = 0.0_f32;
    let mut display_row: u32 = 0;
    for (i, b) in blocks.iter().enumerate() {
        let row_h = b.line_height.unwrap_or(line_height);
        y += b.padding_top;

        let wrap = b.wrap_chars.or(default_wrap_chars).filter(|&c| c > 0);
        let chunks = match wrap {
            Some(budget) => wrap_text_into_chunks(&b.text, budget),
            None => vec![(0, b.text.clone())],
        };
        let last_idx = chunks.len().saturating_sub(1);

        let block_first_row = display_row;
        for (chunk_idx, (byte_offset, chunk_text)) in chunks.into_iter().enumerate() {
            let is_continuation = chunk_idx > 0;
            let is_last = chunk_idx == last_idx;
            let chunk_len = chunk_text.len();
            let chunk_runs = slice_runs_for_chunk(&b.runs, byte_offset, byte_offset + chunk_len);
            shaped.push(ShapedLine {
                display_row,
                buffer_row: i as u32,
                buffer_byte_offset: byte_offset,
                is_wrap_continuation: is_continuation,
                y_top: y,
                x_offset: b.indent,
                text: chunk_text,
                runs: chunk_runs,
                line_bg: b.line_bg,
                line_height: b.line_height,
                // Padding belongs to the block as a whole; only the first
                // row pays padding_top, only the last row pays padding_bottom.
                padding_top: if is_continuation { 0.0 } else { b.padding_top },
                padding_bottom: if is_last { b.padding_bottom } else { 0.0 },
                shape: None,
            });
            y += row_h;
            display_row += 1;
        }
        y += b.padding_bottom;

        if b.block_bg.is_some() || b.block_border.is_some() {
            block_rects.push(BlockRect {
                display_row_start: block_first_row,
                display_row_end: display_row.saturating_sub(1),
                indent: b.indent,
                padding_top: b.padding_top,
                padding_bottom: b.padding_bottom,
                decoration: BlockDecoration {
                    background: b.block_bg,
                    border: b.block_border,
                    corner_radius: b.block_corner_radius,
                },
            });
        }
    }
    let total = shaped.len() as u32;
    DisplayLayout {
        lines: Arc::new(shaped),
        block_rects: Arc::new(block_rects),
        visible_rows: 0..total,
        total_display_rows: total,
        line_height,
        char_width,
        baseline_offset,
        default_fg,
        version: 1,
        scroll_version: 0,
    }
}

/// Whitespace-aware char-budget wrap. Returns `(byte_offset_in_source, chunk_text)` pairs.
/// Breaks at the last space/tab before the budget; otherwise hard-breaks. Words
/// wider than the budget stay intact (extends past budget, same as shaped wrap).
fn wrap_text_into_chunks(text: &str, budget: usize) -> Vec<(usize, String)> {
    if text.is_empty() || budget == 0 {
        return vec![(0, text.to_string())];
    }

    let mut out: Vec<(usize, String)> = Vec::new();
    let mut chunk_start_byte = 0_usize;
    let mut char_count = 0_usize;
    let mut last_ws_end_byte: Option<usize> = None;

    let mut iter = text.char_indices().peekable();
    while let Some((byte_idx, ch)) = iter.next() {
        // Newlines in the input force a hard break (markdown lists / multi-line
        // bodies pre-split into blocks should already not contain '\n', but
        // keep the helper safe if a caller passes one).
        if ch == '\n' {
            let chunk_text = text[chunk_start_byte..byte_idx + ch.len_utf8()].to_string();
            out.push((chunk_start_byte, chunk_text));
            chunk_start_byte = byte_idx + ch.len_utf8();
            char_count = 0;
            last_ws_end_byte = None;
            continue;
        }

        char_count += 1;
        if ch == ' ' || ch == '\t' {
            last_ws_end_byte = Some(byte_idx + ch.len_utf8());
        }

        if char_count >= budget {
            // Try to break at the last whitespace seen *after* chunk_start.
            let break_byte = match last_ws_end_byte {
                Some(b) if b > chunk_start_byte => b,
                _ => {
                    // No whitespace in this chunk yet. Two options:
                    // (a) we're mid-word — extend until we hit one (don't split)
                    // (b) the next char is whitespace — break at the next iteration
                    // For (a), keep going; the row will exceed the budget but a
                    // word stays whole.
                    if let Some(&(next_byte, next_ch)) = iter.peek() {
                        if next_ch == ' ' || next_ch == '\t' || next_ch == '\n' {
                            next_byte
                        } else {
                            continue;
                        }
                    } else {
                        // End of input — emit the rest as the final chunk below.
                        break;
                    }
                }
            };

            if break_byte > chunk_start_byte {
                let chunk_text = text[chunk_start_byte..break_byte].to_string();
                out.push((chunk_start_byte, chunk_text));
                chunk_start_byte = break_byte;
                // Bytes between break_byte and the iterator's current position
                // (byte_idx + this char's length) belong to the new chunk and
                // were already consumed by the iterator. Seed char_count with
                // their count so the budget check stays accurate.
                let consumed_end = byte_idx + ch.len_utf8();
                char_count = if consumed_end > chunk_start_byte {
                    text[chunk_start_byte..consumed_end].chars().count()
                } else {
                    0
                };
                last_ws_end_byte = None;
            }
        }
    }

    if chunk_start_byte < text.len() {
        out.push((chunk_start_byte, text[chunk_start_byte..].to_string()));
    } else if out.is_empty() {
        out.push((0, String::new()));
    }
    out
}

/// Clip runs to `[start_byte, end_byte)` and rebase to chunk-local numbering.
fn slice_runs_for_chunk(runs: &[StyleRun], start: usize, end: usize) -> Vec<StyleRun> {
    let mut out = Vec::new();
    for r in runs {
        if r.byte_range.end <= start || r.byte_range.start >= end {
            continue;
        }
        let s = r.byte_range.start.max(start) - start;
        let e = r.byte_range.end.min(end) - start;
        if s >= e {
            continue;
        }
        let mut clone = r.clone();
        clone.byte_range = s..e;
        out.push(clone);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::prelude::Color;

    /// Block layout with mixed padding + per-row line-height stacks rows
    /// correctly: each row's `y_top` equals the sum of all prior rows'
    /// `(padding_top + line_height + padding_bottom) + this row's padding_top`.
    #[test]
    fn layout_blocks_stacks_padding_and_line_height() {
        let blocks = vec![
            // body: 16px line height, no padding.
            Block::new("hello"),
            // heading: 24px line height, 8px above, 4px below.
            Block::new("# heading")
                .with_line_height(24.0)
                .with_padding(8.0, 4.0),
            // body again, default line-height (16px), no padding.
            Block::new("more body"),
            // code block row: 16px, 6px above, 6px below for the panel.
            Block::new("fn x() {}").with_padding(6.0, 6.0),
        ];

        let layout = Block::layout(
            &blocks,
            BlockLayoutConfig {
                line_height: 16.0,
                char_width: 8.0,
                baseline_offset: 5.0,
                default_fg: Color::WHITE,
                default_wrap_chars: None,
            },
        );
        let lines = &*layout.lines;
        assert_eq!(lines.len(), 4);

        // Row 0: y = 0
        assert_eq!(lines[0].y_top, 0.0);
        // Row 1: y = 0 + 16 (row0 lh) + 0 (row0 pad-bot) + 8 (row1 pad-top) = 24
        assert_eq!(lines[1].y_top, 24.0);
        // Row 2: y = 24 + 24 (row1 lh) + 4 (row1 pad-bot) + 0 = 52
        assert_eq!(lines[2].y_top, 52.0);
        // Row 3: y = 52 + 16 + 0 + 6 = 74
        assert_eq!(lines[3].y_top, 74.0);
    }

    #[test]
    fn layout_blocks_indent_propagates_to_x_offset() {
        let blocks = vec![
            Block::new("body"),
            Block::new("- list").with_indent(20.0),
            Block::new("  - nested").with_indent(40.0),
            Block::new("> quote").with_indent(16.0),
        ];
        let layout = Block::layout(
            &blocks,
            BlockLayoutConfig {
                line_height: 16.0,
                char_width: 8.0,
                baseline_offset: 5.0,
                default_fg: Color::WHITE,
                default_wrap_chars: None,
            },
        );
        let lines = &*layout.lines;
        assert_eq!(lines[0].x_offset, 0.0);
        assert_eq!(lines[1].x_offset, 20.0);
        assert_eq!(lines[2].x_offset, 40.0);
        assert_eq!(lines[3].x_offset, 16.0);
    }

    #[test]
    fn trivial_layout_no_padding_matches_old_behavior() {
        let blocks: Vec<Block> = ["a", "b", "c"].iter().map(|s| Block::new(*s)).collect();
        let layout = Block::layout(
            &blocks,
            BlockLayoutConfig {
                line_height: 20.0,
                char_width: 8.0,
                baseline_offset: 5.0,
                default_fg: Color::WHITE,
                default_wrap_chars: None,
            },
        );
        let lines = &*layout.lines;
        assert_eq!(lines[0].y_top, 0.0);
        assert_eq!(lines[1].y_top, 20.0);
        assert_eq!(lines[2].y_top, 40.0);
    }

    /// `default_wrap_chars = Some(N)` splits a long body block into multiple
    /// rows. Continuation rows have `is_wrap_continuation = true`, inherit
    /// the block's `indent`, and get the right `buffer_byte_offset` so callers
    /// can map clicks back into the source text.
    #[test]
    fn layout_blocks_wraps_at_word_boundary() {
        let body = "the quick brown fox jumps over the lazy dog and runs away.";
        let blocks = vec![Block::new(body).with_indent(10.0)];
        let layout = Block::layout(
            &blocks,
            BlockLayoutConfig {
                line_height: 16.0,
                char_width: 8.0,
                baseline_offset: 5.0,
                default_fg: Color::WHITE,
                default_wrap_chars: Some(30),
            },
        );
        let lines = &*layout.lines;

        assert!(lines.len() >= 2);
        assert!(!lines[0].is_wrap_continuation);
        assert!(lines[1].is_wrap_continuation);

        for line in lines.iter() {
            assert_eq!(line.buffer_row, 0);
            assert_eq!(line.x_offset, 10.0);
        }

        let recomposed: String = lines.iter().map(|l| l.text.clone()).collect();
        assert_eq!(recomposed, body);

        for line in lines.iter() {
            let chars = line.text.chars().count();
            assert!(
                chars <= 31,
                "chunk {:?} exceeded budget+1, got {chars}",
                line.text
            );
        }
    }

    /// A word longer than the wrap budget stays intact rather than splitting
    /// mid-word. Mirrors the shaped wrap path's behavior for oversized clusters.
    #[test]
    fn layout_blocks_wrap_keeps_long_words_intact() {
        let body = "short pneumonoultramicroscopicsilicovolcanoconiosis end";
        let blocks = vec![Block::new(body)];
        let layout = Block::layout(
            &blocks,
            BlockLayoutConfig {
                line_height: 16.0,
                char_width: 8.0,
                baseline_offset: 5.0,
                default_fg: Color::WHITE,
                default_wrap_chars: Some(10),
            },
        );
        let lines = &*layout.lines;

        let recomposed: String = lines.iter().map(|l| l.text.clone()).collect();
        assert_eq!(recomposed, body);
        assert!(
            lines.iter().any(|l| l
                .text
                .contains("pneumonoultramicroscopicsilicovolcanoconiosis")),
            "long word was split across rows: {:?}",
            lines.iter().map(|l| &l.text).collect::<Vec<_>>()
        );
    }

    /// Per-block `with_wrap_chars(0)` overrides the layout default and
    /// disables wrap for that block alone.
    #[test]
    fn layout_blocks_per_block_wrap_override() {
        let blocks = vec![
            Block::new("alpha bravo charlie delta echo foxtrot"),
            Block::new("uno dos tres cuatro cinco seis siete").with_wrap_chars(0),
        ];
        let layout = Block::layout(
            &blocks,
            BlockLayoutConfig {
                line_height: 16.0,
                char_width: 8.0,
                baseline_offset: 5.0,
                default_fg: Color::WHITE,
                default_wrap_chars: Some(15),
            },
        );
        let lines = &*layout.lines;

        let block0_rows = lines.iter().filter(|l| l.buffer_row == 0).count();
        let block1_rows = lines.iter().filter(|l| l.buffer_row == 1).count();
        assert!(
            block0_rows >= 2,
            "block 0 should wrap, got {block0_rows} rows"
        );
        assert_eq!(block1_rows, 1, "block 1 should not wrap");
    }

    /// `StyleRun`s on a wrapped block are clipped + rebased to each chunk's
    /// local byte numbering, so renderers can index into chunk.text directly.
    #[test]
    fn layout_blocks_wrap_clips_runs_to_chunks() {
        // Crafted so a budget of 10 puts "the quick " on row 0 and
        // "fox" (with its run) entirely inside row 1.
        let text = "the quick fox runs";
        let fox_start = text.find("fox").unwrap(); // 10
        let fox_end = fox_start + "fox".len(); // 13
        let runs = vec![StyleRun {
            byte_range: fox_start..fox_end,
            fg: Color::srgb(1.0, 0.0, 0.0),
            bg: None,
            font_scale: 0.0,
            skew: 0.0,
            corner_radius: 0.0,
            font_weight: None,
            italic: false,
            font: None,
            decoration: TextDecoration::empty(),
            link: None,
        }];
        let blocks = vec![Block::new(text).with_runs(runs)];

        // Budget 10 splits at the space after "quick" (byte 10). Row 0 =
        // "the quick " (bytes 0..10) — pre-fox, no runs. Row 1 = "fox runs"
        // (bytes 10..18) — holds the fox run rebased to local 0..3.
        let layout = Block::layout(
            &blocks,
            BlockLayoutConfig {
                line_height: 16.0,
                char_width: 8.0,
                baseline_offset: 5.0,
                default_fg: Color::WHITE,
                default_wrap_chars: Some(10),
            },
        );
        let lines = &*layout.lines;
        assert_eq!(lines.len(), 2);

        assert!(lines[0].runs.is_empty(), "row 0 runs: {:?}", lines[0].runs);

        assert_eq!(lines[1].runs.len(), 1);
        assert_eq!(lines[1].runs[0].byte_range, 0..3);
        assert_eq!(&lines[1].text[0..3], "fox");
    }

    /// Blocks with `with_block_background` / `with_block_border` get a
    /// matching `BlockRect` in `DisplayLayout::block_rects`. Blocks without
    /// either don't.
    #[test]
    fn layout_blocks_emits_block_rect_for_decorated_blocks() {
        let blocks = vec![
            Block::new("plain body"),
            Block::new("fenced code")
                .with_padding(8.0, 8.0)
                .with_block_background(Color::srgb(0.1, 0.1, 0.1))
                .with_block_corner_radius(4.0),
            Block::new("> blockquote line").with_block_border(Color::srgb(0.5, 0.5, 0.5), 1.0),
        ];
        let layout = Block::layout(
            &blocks,
            BlockLayoutConfig {
                line_height: 16.0,
                char_width: 8.0,
                baseline_offset: 5.0,
                default_fg: Color::WHITE,
                default_wrap_chars: None,
            },
        );

        // Two decorated blocks → two rects (the plain body has neither).
        assert_eq!(layout.block_rects.len(), 2);
        let [code, quote] = &layout.block_rects[..] else {
            panic!("expected 2 rects");
        };

        // The code block lives at display_row 1 (after the plain body).
        assert_eq!(code.display_row_start, 1);
        assert_eq!(code.display_row_end, 1);
        assert_eq!(code.padding_top, 8.0);
        assert_eq!(code.padding_bottom, 8.0);
        assert_eq!(code.decoration.background, Some(Color::srgb(0.1, 0.1, 0.1)));
        assert_eq!(code.decoration.corner_radius, 4.0);
        assert!(code.decoration.border.is_none());

        // Blockquote is at display_row 2.
        assert_eq!(quote.display_row_start, 2);
        assert_eq!(quote.display_row_end, 2);
        assert_eq!(quote.decoration.border.map(|b| b.width), Some(1.0));
    }

    /// Wrapping a decorated block keeps it as one `BlockRect` spanning all
    /// continuation rows — a code block that wraps stays one panel.
    #[test]
    fn layout_blocks_block_rect_spans_wrap_continuation() {
        let body = "alpha bravo charlie delta echo foxtrot golf hotel";
        let blocks = vec![Block::new(body).with_block_background(Color::srgb(0.2, 0.2, 0.3))];
        let layout = Block::layout(
            &blocks,
            BlockLayoutConfig {
                line_height: 16.0,
                char_width: 8.0,
                baseline_offset: 5.0,
                default_fg: Color::WHITE,
                default_wrap_chars: Some(10),
            },
        );

        assert_eq!(layout.block_rects.len(), 1);
        let rect = &layout.block_rects[0];
        assert_eq!(rect.display_row_start, 0);
        // End row index = total wrap rows - 1.
        assert_eq!(rect.display_row_end, layout.lines.len() as u32 - 1);
        assert!(rect.decoration.background.is_some());
    }
}

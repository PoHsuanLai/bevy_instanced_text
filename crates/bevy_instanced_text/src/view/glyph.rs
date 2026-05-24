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
    /// Text decorations applied across a `TextFormat`. Flags can be combined
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
pub struct TextFormat {
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

impl TextFormat {
    /// Foreground-only format. Every other field stays at its layout default;
    /// chain `.with_*` / `.italic()` to layer on more attributes.
    pub fn fg(byte_range: Range<usize>, fg: Color) -> Self {
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

    pub fn with_bg(mut self, bg: Color) -> Self {
        self.bg = Some(bg);
        self
    }

    pub fn with_scale(mut self, font_scale: f32) -> Self {
        self.font_scale = font_scale;
        self
    }

    pub fn with_skew(mut self, skew: f32) -> Self {
        self.skew = skew;
        self
    }

    pub fn with_corner_radius(mut self, corner_radius: f32) -> Self {
        self.corner_radius = corner_radius;
        self
    }

    pub fn with_weight(mut self, font_weight: u16) -> Self {
        self.font_weight = Some(font_weight);
        self
    }

    pub fn italic(mut self) -> Self {
        self.italic = true;
        self
    }

    pub fn with_font(mut self, font: bevy::asset::Handle<bevy::text::Font>) -> Self {
        self.font = Some(font);
        self
    }

    pub fn with_decoration(mut self, decoration: TextDecoration) -> Self {
        self.decoration = decoration;
        self
    }

    pub fn with_link(mut self, link: impl Into<Arc<str>>) -> Self {
        self.link = Some(link.into());
        self
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
    pub runs: Vec<TextFormat>,
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
    /// Byte ranges in `text` that came from `FormattedSpan { is_virtual: true }`
    /// — inline decoration text that shapes with the row but is invisible
    /// to byte-addressed APIs (cursor, selection, `x_at_byte`, `byte_at_x`).
    /// Sorted, non-overlapping, in `text`-relative byte coordinates.
    pub virtual_byte_ranges: Vec<Range<usize>>,
}

impl ShapedLine {
    /// Translate a source-byte offset (a byte the cursor can occupy — one
    /// that exists in the source buffer, not inserted by a virtual span)
    /// into the corresponding concat-byte offset in `text`. Concat bytes
    /// are what `runs[*].byte_range` and `shape.glyphs[*].byte_index`
    /// reference.
    ///
    /// The mapping skips over any virtual ranges whose start is `<=` the
    /// source position: each preceding virtual run shifts the source byte
    /// right by its length.
    pub fn concat_byte_for_source_byte(&self, source_byte: usize) -> usize {
        let mut concat = source_byte;
        for range in &self.virtual_byte_ranges {
            if range.start <= concat {
                concat += range.end - range.start;
            } else {
                break;
            }
        }
        concat
    }

    /// Translate a concat-byte offset (a position in `text` / `runs` /
    /// `shape.glyphs`) into the corresponding source-byte offset. A
    /// concat byte that falls *inside* a virtual range snaps to the
    /// source byte at the range's left edge (`snap_right == false`) or
    /// right edge (`snap_right == true`).
    pub fn source_byte_for_concat_byte(&self, concat_byte: usize, snap_right: bool) -> usize {
        let mut source = concat_byte;
        for range in &self.virtual_byte_ranges {
            if range.end <= concat_byte {
                source -= range.end - range.start;
            } else if range.start <= concat_byte {
                // concat_byte falls inside this virtual range — snap.
                source -= concat_byte - range.start;
                if snap_right {
                    // Don't advance past the range; the source byte at the
                    // right edge of the gap is the next source position.
                }
                break;
            } else {
                break;
            }
        }
        source
    }

    /// `Some(range)` if `concat_byte` is inside a virtual range on this
    /// row. Returned range is in concat-byte coordinates.
    pub fn virtual_range_at_concat_byte(&self, concat_byte: usize) -> Option<Range<usize>> {
        self.virtual_byte_ranges
            .iter()
            .find(|r| r.start <= concat_byte && concat_byte < r.end)
            .cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `ShapedLine` with everything but `virtual_byte_ranges` zeroed — the
    /// helpers under test ignore the rest.
    fn line_with_virtuals(text: &str, virtuals: Vec<Range<usize>>) -> ShapedLine {
        ShapedLine {
            display_row: 0,
            buffer_row: 0,
            buffer_byte_offset: 0,
            is_wrap_continuation: false,
            y_top: 0.0,
            x_offset: 0.0,
            text: text.to_string(),
            runs: Vec::new(),
            line_bg: None,
            line_height: None,
            padding_top: 0.0,
            padding_bottom: 0.0,
            shape: None,
            virtual_byte_ranges: virtuals,
        }
    }

    /// Source byte 0 maps to concat byte 0 when no virtual precedes it,
    /// and to concat byte = virtual_len when one virtual span starts at 0.
    #[test]
    fn source_to_concat_skips_preceding_virtuals() {
        // "fn foo([: i32 v])bar"  — virtual "[: i32 v]" at concat bytes 7..16
        // (length 9). Source bytes: "fn foo(" (0..7) then "bar" (7..10).
        let line = line_with_virtuals("fn foo([: i32 v])bar", vec![7..16]);

        // Source byte 0 → concat 0
        assert_eq!(line.concat_byte_for_source_byte(0), 0);
        // Source byte 7 (start of "bar") → concat 16 (past the virtual run)
        assert_eq!(line.concat_byte_for_source_byte(7), 16);
        // Source byte 10 (end) → concat 19
        assert_eq!(line.concat_byte_for_source_byte(10), 19);
    }

    /// Concat byte before the virtual range round-trips; after it skips
    /// back to the source byte at the virtual range's right edge.
    #[test]
    fn concat_to_source_skips_back_over_virtuals() {
        let line = line_with_virtuals("fn foo([: i32 v])bar", vec![7..16]);

        assert_eq!(line.source_byte_for_concat_byte(0, false), 0);
        assert_eq!(line.source_byte_for_concat_byte(7, false), 7);
        // Concat byte 16 (just past the virtual) → source byte 7 (next source position).
        assert_eq!(line.source_byte_for_concat_byte(16, false), 7);
        assert_eq!(line.source_byte_for_concat_byte(19, false), 10);
    }

    /// Concat bytes inside a virtual range snap to the left or right edge
    /// based on `snap_right`.
    #[test]
    fn concat_to_source_snaps_inside_virtual_range() {
        let line = line_with_virtuals("fn foo([: i32 v])bar", vec![7..16]);

        // concat 10 falls inside 7..16.
        // snap_right=false: snap to the left edge → source 7.
        assert_eq!(line.source_byte_for_concat_byte(10, false), 7);
        // snap_right=true: snap to the right edge → source 7 too (same
        // source byte sits at both edges of the virtual gap).
        assert_eq!(line.source_byte_for_concat_byte(10, true), 7);
    }

    /// Detects whether a concat byte is inside a virtual range.
    #[test]
    fn virtual_range_at_concat_byte_finds_containing_range() {
        let line = line_with_virtuals("abXXXcd", vec![2..5]);

        assert!(line.virtual_range_at_concat_byte(1).is_none());
        assert_eq!(line.virtual_range_at_concat_byte(2), Some(2..5));
        assert_eq!(line.virtual_range_at_concat_byte(4), Some(2..5));
        assert!(line.virtual_range_at_concat_byte(5).is_none());
    }

    /// Multiple virtual ranges compose: cumulative virtual length is
    /// applied each time the source byte crosses one.
    #[test]
    fn multiple_virtuals_compose() {
        // "a[X]b[YY]c" with virtuals 1..4 (`[X]`) and 5..9 (`[YY]`).
        // Source layout: "a" (0..1), "b" (1..2), "c" (2..3).
        let line = line_with_virtuals("a[X]b[YY]c", vec![1..4, 5..9]);

        assert_eq!(line.concat_byte_for_source_byte(0), 0);
        assert_eq!(line.concat_byte_for_source_byte(1), 4); // past [X]
        assert_eq!(line.concat_byte_for_source_byte(2), 9); // past [X] and [YY]
        assert_eq!(line.concat_byte_for_source_byte(3), 10);

        assert_eq!(line.source_byte_for_concat_byte(0, false), 0);
        assert_eq!(line.source_byte_for_concat_byte(4, false), 1);
        assert_eq!(line.source_byte_for_concat_byte(9, false), 2);
        assert_eq!(line.source_byte_for_concat_byte(10, false), 3);
    }
}

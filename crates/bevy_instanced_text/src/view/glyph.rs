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
}


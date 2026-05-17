//! Font configuration components for `TextView` entities.
//!
//! Uses `bevy::text::TextFont` for font handle + size, `bevy::text::LineHeight`
//! for row height (same as Bevy's text pipeline). One companion component carries
//! monospace-specific extensions:
//! - [`MonoFontFaces`] — bold/italic face handles and synthesis settings.
//! - [`MonoCellWidth`] — character advance width (no Bevy equivalent).

use bevy::prelude::*;

/// Faux bold/italic synthesis settings for when a dedicated font face isn't provided.
///
/// `bold_stroke_px`: x-offset used to double-draw glyphs for faux bold (~0.6 px).
/// `italic_skew`: shear slope for faux italic (~0.21 ≈ 12°).
#[derive(Clone, Copy, Debug, PartialEq, Reflect)]
#[reflect(Default, Debug)]
pub struct FontSynthesis {
    pub weight: bool,
    pub style: bool,
    pub bold_stroke_px: f32,
    pub italic_skew: f32,
}

impl Default for FontSynthesis {
    fn default() -> Self {
        Self {
            weight: true,
            style: true,
            bold_stroke_px: 0.6,
            italic_skew: 0.21,
        }
    }
}

/// Bold and italic face handles for a monospace font family, plus synthesis
/// fallback settings.
///
/// Works the same way as Bevy's per-`TextSpan` font swapping: load each face
/// as a separate asset and assign the handle here. The renderer picks the
/// appropriate face per `TextFormat` based on `font_weight` / `italic` flags,
/// falling back toward `TextFont::font` (the regular face) when a slot is
/// empty. When a face is missing and the corresponding synthesis flag is set,
/// the renderer approximates it (double-draw for bold, shear for italic).
#[derive(Component, Clone, Debug, Default, Reflect)]
#[reflect(Component, Default, Debug)]
pub struct MonoFontFaces {
    pub font_bold: Option<Handle<Font>>,
    pub font_italic: Option<Handle<Font>>,
    pub font_bold_italic: Option<Handle<Font>>,
    pub font_synthesis: FontSynthesis,
}

impl MonoFontFaces {
    pub fn with_bold(mut self, handle: Handle<Font>) -> Self {
        self.font_bold = Some(handle);
        self
    }

    pub fn with_italic(mut self, handle: Handle<Font>) -> Self {
        self.font_italic = Some(handle);
        self
    }

    pub fn with_bold_italic(mut self, handle: Handle<Font>) -> Self {
        self.font_bold_italic = Some(handle);
        self
    }
}

/// Advance width of one monospace cell in logical pixels.
///
/// Bevy's text pipeline measures per-glyph advance from font metrics at shape
/// time and has no equivalent concept. For instanced monospace rendering we need
/// this up-front for viewport culling, cursor placement, and wrap budgets.
///
/// Default approximates a 14 px font (`font_size * 0.6`). The editor's
/// `update_font_metrics` system measures the actual advance of `'0'` from the
/// atlas and writes it back here each frame.
///
/// Line height uses Bevy's standard `LineHeight` component (same semantics as
/// Bevy's text pipeline — hosts set `LineHeight::Px` or `LineHeight::RelativeToFont`).
#[derive(Component, Clone, Copy, Debug, Reflect)]
#[reflect(Component, Default, Debug)]
pub struct MonoCellWidth {
    pub px: f32,
}

impl MonoCellWidth {
    pub fn from_font_size(font_size: f32) -> Self {
        Self {
            px: font_size * 0.6,
        }
    }
}

impl Default for MonoCellWidth {
    fn default() -> Self {
        Self::from_font_size(14.0)
    }
}

/// Convenience: resolve `LineHeight` to pixels given a `font_size`.
/// Mirrors `bevy::text::LineHeight` semantics without touching the private `eval` method.
#[inline]
pub fn resolve_line_height(line_height: bevy::text::LineHeight, font_size: f32) -> f32 {
    match line_height {
        bevy::text::LineHeight::Px(px) => px,
        bevy::text::LineHeight::RelativeToFont(scale) => scale * font_size,
    }
}

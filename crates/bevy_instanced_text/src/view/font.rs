//! Per-entity font configuration. The renderer reads this; there is no global
//! font resource.

use bevy::prelude::*;
use bevy::text::Font;

/// Per-entity font configuration.
///
/// Field names mirror Bevy's [`bevy::text::TextFont`] (`font`, `font_size`) so
/// spawn-site syntax matches Bevy text. Multi-face slots (`font_bold`,
/// `font_italic`, `font_bold_italic`) and synthesis are layered on top —
/// Bevy doesn't bundle multi-face natively. `line_height` / `char_width`
/// are layout metrics the renderer falls back to when shaping is off.
#[derive(Component, Clone, Debug, Reflect)]
#[reflect(Component, Default, Debug)]
pub struct FontConfig {
    pub font: Handle<Font>,
    pub font_size: f32,
    pub line_height: f32,
    pub char_width: f32,
    pub font_bold: Option<Handle<Font>>,
    pub font_italic: Option<Handle<Font>>,
    pub font_bold_italic: Option<Handle<Font>>,
    pub font_synthesis: FontSynthesis,
}

/// Whether (and how) to synthesize a bold / italic face when the
/// matching slot on [`FontConfig`] is empty. `weight` / `style` toggles
/// match CSS Fonts L4 `font-synthesis: weight style`. The `*_amount`
/// fields tune the synthesis intensity:
///
/// - `bold_stroke_px`: faux-bold draws each glyph twice with this
///   x-offset. ~0.6 px gives a noticeable weight bump without smearing
///   text — the value typical browsers use for faux-bold. Scale up for
///   very large display sizes if results look thin.
/// - `italic_skew`: faux-italic shears glyphs by this slope (rise/run).
///   ~0.21 (~12°) is the angle FreeType's slant transform and most
///   browsers apply.
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

impl Default for FontConfig {
    fn default() -> Self {
        Self::from_size(14.0)
    }
}

impl FontConfig {
    /// `line_height = font_size * 1.5`, `char_width = font_size * 0.6`,
    /// `font = Handle::default()` (Bevy's FiraMono-subset when the
    /// `default_font` feature is enabled).
    pub fn from_size(font_size: f32) -> Self {
        Self {
            font: Handle::default(),
            font_size,
            line_height: font_size * 1.5,
            char_width: font_size * 0.6,
            font_bold: None,
            font_italic: None,
            font_bold_italic: None,
            font_synthesis: FontSynthesis::default(),
        }
    }

    pub fn with_line_height(mut self, line_height: f32) -> Self {
        self.line_height = line_height;
        self
    }

    pub fn with_line_height_multiplier(mut self, multiplier: f32) -> Self {
        self.line_height = self.font_size * multiplier;
        self
    }

    pub fn with_char_width(mut self, char_width: f32) -> Self {
        self.char_width = char_width;
        self
    }

    pub fn with_font(mut self, handle: Handle<Font>) -> Self {
        self.font = handle;
        self
    }

    pub fn with_bold_font(mut self, handle: Handle<Font>) -> Self {
        self.font_bold = Some(handle);
        self
    }

    pub fn with_italic_font(mut self, handle: Handle<Font>) -> Self {
        self.font_italic = Some(handle);
        self
    }

    pub fn with_bold_italic_font(mut self, handle: Handle<Font>) -> Self {
        self.font_bold_italic = Some(handle);
        self
    }

    pub fn with_font_synthesis(mut self, synthesis: FontSynthesis) -> Self {
        self.font_synthesis = synthesis;
        self
    }

    /// Resolve a handle for `(bold, italic)`, falling back to the closest
    /// available face. Caller applies synthesis when the regular face is
    /// returned for a styled request.
    pub fn font_for(&self, bold: bool, italic: bool) -> &Handle<Font> {
        match (bold, italic) {
            (true, true) => self
                .font_bold_italic
                .as_ref()
                .or(self.font_bold.as_ref())
                .or(self.font_italic.as_ref())
                .unwrap_or(&self.font),
            (true, false) => self.font_bold.as_ref().unwrap_or(&self.font),
            (false, true) => self.font_italic.as_ref().unwrap_or(&self.font),
            (false, false) => &self.font,
        }
    }
}

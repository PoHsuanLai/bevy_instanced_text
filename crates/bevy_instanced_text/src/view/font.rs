//! Per-entity font configuration. The renderer reads this; there is no global
//! font resource.

use bevy::prelude::*;
use bevy::text::Font;

/// Per-entity font configuration: face handles, size, line height, and synthesis settings.
#[derive(Component, Clone, Debug, Reflect)]
#[reflect(Component, Default, Debug)]
pub struct TextFont {
    pub font: Handle<Font>,
    pub font_size: f32,
    pub line_height: f32,
    pub char_width: f32,
    pub font_bold: Option<Handle<Font>>,
    pub font_italic: Option<Handle<Font>>,
    pub font_bold_italic: Option<Handle<Font>>,
    pub font_synthesis: FontSynthesis,
}

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

impl Default for TextFont {
    fn default() -> Self {
        Self::from_font_size(14.0)
    }
}

impl TextFont {
    pub fn from_font_size(font_size: f32) -> Self {
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

    /// Returns the best available face handle for `(bold, italic)`, falling back toward regular.
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

//! Per-entity render colors. Pure rendering — no edit affordances.
//!
//! Background and foreground belong to the rendering substrate: a terminal
//! wants them, a markdown viewer wants them, the editor wants them. Cursor
//! and selection colors live on `bevy_instanced_text_edit` (edit-tier);
//! line-numbers / brackets / indent-guides live on the editor crate
//! (editor-tier).

use bevy::prelude::*;

const DEFAULT_FG: Color = Color::srgb(0.827, 0.827, 0.827);
const DEFAULT_BG: Color = Color::srgb(0.117, 0.117, 0.117);

/// Text foreground color.
#[derive(Component, Clone, Copy, Debug, Reflect, Deref, DerefMut)]
#[reflect(Component, Default, Debug)]
pub struct TextColor(pub Color);

impl Default for TextColor {
    fn default() -> Self {
        Self(DEFAULT_FG)
    }
}

impl<T: Into<Color>> From<T> for TextColor {
    fn from(color: T) -> Self {
        Self(color.into())
    }
}

/// Text background (canvas) color.
#[derive(Component, Clone, Copy, Debug, Reflect, Deref, DerefMut)]
#[reflect(Component, Default, Debug)]
pub struct TextBackgroundColor(pub Color);

impl Default for TextBackgroundColor {
    fn default() -> Self {
        Self(DEFAULT_BG)
    }
}

impl<T: Into<Color>> From<T> for TextBackgroundColor {
    fn from(color: T) -> Self {
        Self(color.into())
    }
}


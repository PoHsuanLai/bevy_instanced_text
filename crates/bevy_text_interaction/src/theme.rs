//! Per-entity edit-affordance colors: the caret and the selection band.

use bevy::prelude::*;

/// Caret color.
#[derive(Component, Clone, Copy, Debug, Reflect, Deref, DerefMut)]
#[reflect(Component, Default, Debug)]
pub struct TextCursorColor(pub Color);

impl Default for TextCursorColor {
    fn default() -> Self {
        Self(Color::srgb(0.933, 0.933, 0.933))
    }
}

impl<T: Into<Color>> From<T> for TextCursorColor {
    fn from(color: T) -> Self {
        Self(color.into())
    }
}

/// Selection highlight background color.
#[derive(Component, Clone, Copy, Debug, Reflect, Deref, DerefMut)]
#[reflect(Component, Default, Debug)]
pub struct TextSelectionColor(pub Color);

impl Default for TextSelectionColor {
    fn default() -> Self {
        Self(Color::srgba(0.231, 0.373, 0.604, 0.4))
    }
}

impl<T: Into<Color>> From<T> for TextSelectionColor {
    fn from(color: T) -> Self {
        Self(color.into())
    }
}

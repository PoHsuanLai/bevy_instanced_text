//! Per-entity render colors. Pure rendering — no edit affordances.
//!
//! Background and foreground belong to the rendering substrate: a terminal
//! wants them, a markdown viewer wants them, the editor wants them. Cursor
//! and selection colors live on `bevy_instanced_text_edit::EditTheme` (edit-tier);
//! line-numbers / brackets / indent-guides live on the editor crate
//! (editor-tier).

use bevy::prelude::*;

/// Per-entity background and foreground colors for the text rendering substrate.
/// Shared by editors, terminals, markdown viewers — anything that renders styled text.
#[derive(Component, Clone, Debug, Reflect)]
#[reflect(Component, Default, Debug)]
pub struct RenderTheme {
    pub background: Color,
    pub foreground: Color,
}

impl Default for RenderTheme {
    fn default() -> Self {
        Self {
            background: Color::srgb(0.117, 0.117, 0.117),
            foreground: Color::srgb(0.827, 0.827, 0.827),
        }
    }
}

/// Decorative chrome shared by any consumer that paints styled blocks of
/// text — code chips, fenced code backgrounds, blockquote bars, horizontal
/// rules. Markdown, terminal command-blocks, log-viewers, editor diagnostic
/// panels can all read from one palette.
#[derive(Component, Clone, Debug, Reflect)]
#[reflect(Component, Default, Debug)]
pub struct BlockDecorTheme {
    pub inline_code_fg: Color,
    pub inline_code_bg: Color,
    pub code_block_fg: Color,
    pub code_block_bg: Color,
    pub blockquote_fg: Color,
    pub blockquote_bar: Color,
    pub rule_color: Color,
    /// Corner radius (px) for fenced code-block backgrounds.
    pub code_corner_radius: f32,
    /// Corner radius (px) for inline `code` chips. Typically smaller
    /// than [`Self::code_corner_radius`] — the chip is short, so a
    /// large radius reads as bubbly rather than crisp.
    pub inline_code_corner_radius: f32,
}

impl Default for BlockDecorTheme {
    fn default() -> Self {
        Self {
            inline_code_fg: Color::srgb(1.0, 0.78, 0.55),
            inline_code_bg: Color::srgba(1.0, 1.0, 1.0, 0.08),
            code_block_fg: Color::srgb(0.90, 0.90, 0.92),
            code_block_bg: Color::srgba(1.0, 1.0, 1.0, 0.05),
            blockquote_fg: Color::srgb(0.75, 0.75, 0.78),
            blockquote_bar: Color::srgba(0.55, 0.65, 0.85, 0.55),
            rule_color: Color::srgba(1.0, 1.0, 1.0, 0.15),
            code_corner_radius: 4.0,
            inline_code_corner_radius: 3.0,
        }
    }
}

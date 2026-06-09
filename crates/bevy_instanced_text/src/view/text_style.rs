//! Plain-data Components that plug into `produce_layouts`.
//!
//! The engine's layout system queries each `TextView` entity for these
//! components. They're optional: an entity without [`HiddenLines`] shows every
//! line; one without [`LineStyles`] renders with `DisplayLayout::default_fg`.

use bevy::prelude::*;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use super::glyph::TextFormat;

/// Optional Component on a `TextView` entity selecting which buffer lines
/// the engine renders. Absent ⇒ every line is visible.
///
/// `Arc<HashSet>` so cloning during change-detection is cheap. Producers
/// write a fresh `Arc::new(set)` on each refresh.
#[derive(Component, Default, Clone)]
pub struct HiddenLines(pub Arc<HashSet<usize>>);

impl HiddenLines {
    pub fn new(lines: HashSet<usize>) -> Self {
        Self(Arc::new(lines))
    }

    pub fn is_visible(&self, buffer_line: usize) -> bool {
        !self.0.contains(&buffer_line)
    }
}

/// Optional Component on a `TextView` entity carrying styled runs per
/// buffer line. Absent ⇒ every line renders with `default_fg`.
///
/// Producers (e.g. the editor's syntax-styling system) compute styled runs
/// for the visible buffer-line window via the shared
/// [`super::text_access::visible_buffer_range`] helper, build a fresh
/// `HashMap`, and write a new `LineStyles` Component.
///
/// **Single-writer rule**: at most one system per entity should write
/// `LineStyles` per frame. Two producers writing to the same entity will
/// silently overwrite each other.
#[derive(Component, Default, Clone)]
pub struct LineStyles {
    /// Maps `buffer_line → styled runs`. Sparse: only the visible window is
    /// populated. Lines absent from the map render plain.
    pub by_line: Arc<HashMap<u32, Vec<FormattedSpan>>>,
}

impl LineStyles {
    pub fn new(by_line: HashMap<u32, Vec<FormattedSpan>>) -> Self {
        Self {
            by_line: Arc::new(by_line),
        }
    }

    /// Returns the runs for `buffer_line`, or `None` if it isn't styled.
    pub fn get(&self, buffer_line: u32) -> Option<&Vec<FormattedSpan>> {
        self.by_line.get(&buffer_line)
    }
}

/// Soft-wrap configuration Component. Mirrors Bevy's `TextBounds` name.
///
/// `width = None` disables wrap (one display row per visible buffer line).
/// When set, lines wider than `width` split into multiple continuation rows,
/// each inset by `indent_px`.
#[derive(Component, Clone, Copy, Debug, Reflect)]
#[reflect(Component, Default)]
pub struct TextBounds {
    /// Pixel width budget for a row. `None` ⇒ no wrap.
    pub width: Option<f32>,
    /// Continuation-row left inset in pixels.
    pub indent_px: f32,
}

impl Default for TextBounds {
    fn default() -> Self {
        Self {
            width: None,
            indent_px: 0.0,
        }
    }
}

/// One styled span: text payload plus its format. The element type of
/// [`LineStyles`].
///
/// Producers concatenate `text` payloads to form the line that gets shaped;
/// the engine then rebases each span's `format.byte_range` to match its
/// position in the concatenation. `format.byte_range` on input is ignored —
/// set it to `0..0` (or anything) when constructing.
///
/// `is_virtual` marks the span as inline decoration text — it participates
/// in shaping (subsequent glyphs are pushed right) but is invisible to
/// byte-addressed APIs like cursor movement, selection, and
/// `DisplayLayout::x_at_byte` / `byte_at_x`. Use for inlay hints, ghost-
/// text autosuggest, inline diff annotations — any text that should render
/// inline but isn't part of the source buffer.
#[derive(Clone, Debug)]
pub struct FormattedSpan {
    pub text: String,
    pub format: TextFormat,
    pub is_virtual: bool,
}

// ---------------------------------------------------------------------------
// Ergonomic styling helpers for hand-authored labels.
//
// These build the same `LineStyles` the bulk producer path uses, but spare
// callers the `HashMap` / `byte_range: 0..0` / `is_virtual: false` ceremony.
// For high-throughput producers (syntax highlighters), keep using
// `LineStyles::new` + `FormattedSpan` directly.
// ---------------------------------------------------------------------------

/// A foreground-only [`TextFormat`] starter for the styling builders. The byte
/// range is left at `0..0`; the engine rebases it from the span's position, so
/// it never has to be filled in by hand. Chain `TextFormat`'s `.with_*` /
/// `.italic()` to layer on more attributes:
///
/// ```rust
/// # use bevy_instanced_text::prelude::*;
/// # use bevy::color::Color;
/// let f = fg(Color::WHITE).italic().with_weight(700);
/// ```
pub fn fg(color: Color) -> TextFormat {
    TextFormat::fg(0..0, color)
}

/// A [`FormattedSpan`] from `(text, format)` — the visible (non-virtual) case.
/// `format.byte_range` is ignored on input; the engine assigns it from the
/// span's position in its line.
pub fn span(text: impl Into<String>, format: TextFormat) -> FormattedSpan {
    FormattedSpan {
        text: text.into(),
        format,
        is_virtual: false,
    }
}

/// Build the `(text, LineStyles)` pair for a **single** styled line from
/// `(text, format)` segments. The returned `String` is the concatenation of
/// the segment texts — spawn it as the `InstancedText` content so the buffer's
/// indexing matches the rendered runs:
///
/// ```rust
/// # use bevy::prelude::*;
/// # use bevy_instanced_text::prelude::*;
/// # fn sys(mut commands: Commands) {
/// let (text, styles) = styled_line([
///     ("normal ", fg(Color::WHITE)),
///     ("bold blue", fg(Color::srgb(0.4, 0.6, 1.0)).with_weight(700)),
/// ]);
/// commands.spawn((InstancedText::<String>::new(text), styles));
/// # }
/// ```
///
/// Segment texts must not contain `\n` — use [`styled_lines`] for multi-line
/// content (debug builds assert this).
pub fn styled_line<S>(segments: impl IntoIterator<Item = (S, TextFormat)>) -> (String, LineStyles)
where
    S: Into<String>,
{
    styled_lines([segments])
}

/// Build the `(text, LineStyles)` pair for **multiple** styled lines. Each
/// inner iterator is one line's `(text, format)` segments; lines are joined
/// with `\n` to form the returned buffer string, and each line's runs are keyed
/// to its line index. Spawn the returned `String` as the `InstancedText`
/// content:
///
/// ```rust
/// # use bevy::prelude::*;
/// # use bevy_instanced_text::prelude::*;
/// # fn sys(mut commands: Commands) {
/// let (kw, name, dim) = (Color::srgb(0.8,0.5,0.9), Color::WHITE, Color::srgb(0.5,0.5,0.5));
/// let (text, styles) = styled_lines([
///     vec![("fn ", fg(kw)), ("main", fg(name))],
///     vec![("  body", fg(dim))],
/// ]);
/// // text == "fn main\n  body"
/// commands.spawn((InstancedText::<String>::new(text), styles));
/// # }
/// ```
pub fn styled_lines<S, Line, Lines>(lines: Lines) -> (String, LineStyles)
where
    S: Into<String>,
    Line: IntoIterator<Item = (S, TextFormat)>,
    Lines: IntoIterator<Item = Line>,
{
    let mut buffer = String::new();
    let mut by_line: HashMap<u32, Vec<FormattedSpan>> = HashMap::new();

    for (line_index, segments) in lines.into_iter().enumerate() {
        if line_index > 0 {
            buffer.push('\n');
        }
        let mut runs = Vec::new();
        for (seg_text, format) in segments {
            let seg: String = seg_text.into();
            debug_assert!(
                !seg.contains('\n'),
                "styled span text must not contain '\\n' (line {line_index}); \
                 split it across lines in `styled_lines` instead",
            );
            buffer.push_str(&seg);
            runs.push(span(seg, format));
        }
        if !runs.is_empty() {
            by_line.insert(line_index as u32, runs);
        }
    }

    (buffer, LineStyles::new(by_line))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn styled_line_derives_text_from_segments() {
        let blue = Color::srgb(0.4, 0.6, 1.0);
        let (text, styles) = styled_line([("normal ", fg(Color::WHITE)), ("blue", fg(blue))]);
        // Buffer string is the concatenation of segment texts — no second source.
        assert_eq!(text, "normal blue");
        // One line keyed at 0, two runs.
        let runs = styles.get(0).expect("line 0 styled");
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].text, "normal ");
        assert_eq!(runs[1].text, "blue");
        assert!(!runs[0].is_virtual);
    }

    #[test]
    fn styled_lines_joins_with_newline_and_keys_per_line() {
        let blue = Color::srgb(0.4, 0.6, 1.0);
        let (text, styles) = styled_lines([
            vec![("fn ", fg(Color::WHITE)), ("main", fg(blue))],
            vec![("  body", fg(Color::srgb(0.5, 0.5, 0.5)))],
        ]);
        assert_eq!(text, "fn main\n  body");
        assert_eq!(styles.get(0).map(|r| r.len()), Some(2));
        assert_eq!(styles.get(1).map(|r| r.len()), Some(1));
        assert_eq!(styles.get(1).unwrap()[0].text, "  body");
    }

    #[test]
    fn empty_line_is_not_keyed() {
        // A line with no segments contributes a blank buffer line but no runs.
        let (text, styles) = styled_lines([vec![("a", fg(Color::WHITE))], Vec::<(&str, _)>::new()]);
        assert_eq!(text, "a\n");
        assert!(styles.get(0).is_some());
        assert!(styles.get(1).is_none());
    }

    #[test]
    fn fg_leaves_byte_range_for_the_engine() {
        // The builder never asks callers to fill byte_range; it stays 0..0
        // until the layout pass rebases it.
        let f = fg(Color::WHITE);
        assert_eq!(f.byte_range, 0..0);
    }
}

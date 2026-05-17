//! TextContent trait, generic TextBuffer<T>, and content metrics.
//!
//! Scroll state is `bevy::ui::ScrollPosition` — read it directly from the
//! same entity. The engine performs no animation; hosts that want smooth
//! scroll write `ScrollPosition` themselves (via `bevy_tweening`, a custom
//! animator, or however they like).

use std::borrow::Cow;
use std::ops::{Deref, DerefMut, Range};

use bevy::prelude::*;

/// Minimum interface the layout engine and picking observers need from a
/// text buffer.
///
/// Implement this on any type to use it as the backing store for a
/// [`TextBuffer`]. The engine calls the three required methods during
/// layout; the four default-implemented methods support hit-testing and
/// selection by rendering-layer observers. A rope-backed type should
/// override the defaults for O(log n) indexing — `String` / [`TextSpan`]
/// fall back to per-line scans, which is fine for short content.
pub trait TextContent: Send + Sync + 'static {
    /// Total number of lines, including a trailing empty line when the
    /// content ends with `\n` (matching ropey's `len_lines()` convention).
    fn line_count(&self) -> usize;
    /// Text of line `i` (0-based), including its trailing `\n` if present.
    fn line(&self, i: usize) -> Cow<'_, str>;
    /// Character count of line `i`, excluding the trailing `\n`.
    fn line_len_chars(&self, i: usize) -> usize;

    /// Total character count across all lines (including trailing `\n` chars).
    fn char_count(&self) -> usize {
        (0..self.line_count())
            .map(|i| self.line(i).chars().count())
            .sum()
    }

    /// Char offset where line `line` begins. `line == line_count()` returns
    /// the total char count (one-past-the-end convention).
    fn line_to_char(&self, line: usize) -> usize {
        let n = self.line_count();
        let upper = line.min(n);
        (0..upper).map(|i| self.line(i).chars().count()).sum()
    }

    /// Line that contains char offset `ch`. Returns the last line index
    /// when `ch >= char_count()`.
    fn char_to_line(&self, ch: usize) -> usize {
        let mut acc = 0usize;
        let n = self.line_count();
        for i in 0..n {
            let len = self.line(i).chars().count();
            if ch < acc + len {
                return i;
            }
            acc += len;
        }
        n.saturating_sub(1)
    }

    /// Char range as a string. Default impl walks lines and concatenates
    /// the relevant character slice — O(range_len) plus O(line_count) line
    /// walking. Rope-backed implementations should override.
    fn slice_chars(&self, range: Range<usize>) -> Cow<'_, str> {
        let total = self.char_count();
        let start = range.start.min(total);
        let end = range.end.min(total).max(start);
        if start == end {
            return Cow::Owned(String::new());
        }
        let mut out = String::with_capacity(end - start);
        let mut acc = 0usize;
        for i in 0..self.line_count() {
            let line = self.line(i);
            let len = line.chars().count();
            let line_end = acc + len;
            if line_end <= start {
                acc = line_end;
                continue;
            }
            if acc >= end {
                break;
            }
            let local_start = start.saturating_sub(acc);
            let local_end = (end - acc).min(len);
            let s: String = line
                .chars()
                .skip(local_start)
                .take(local_end - local_start)
                .collect();
            out.push_str(&s);
            acc = line_end;
        }
        Cow::Owned(out)
    }
}

/// Re-export Bevy's [`TextSpan`] so users don't need a separate import.
/// `TextContent` is implemented for it below.
pub use bevy::text::TextSpan;

/// Compute `line(i)` for a `&str` body following the ropey convention:
/// the slice **includes its trailing `\n`** when one is present. The final
/// virtual empty line after a trailing newline is reported as `""`.
fn line_slice(body: &str, i: usize) -> &str {
    let mut start = 0usize;
    let mut current_line = 0usize;
    let bytes = body.as_bytes();
    while start <= bytes.len() {
        if current_line == i {
            // Find next '\n' at-or-after `start`; include it in the slice.
            let rest = &body[start..];
            let end_byte = match rest.find('\n') {
                Some(p) => start + p + 1, // include the '\n'
                None => bytes.len(),
            };
            return &body[start..end_byte];
        }
        // Advance to the next line start (one past the next '\n').
        let rest = &body[start..];
        match rest.find('\n') {
            Some(p) => {
                start += p + 1;
                current_line += 1;
            }
            None => return "", // Past the last line
        }
    }
    ""
}

/// Count lines in a `&str` using the ropey convention: a trailing `\n` adds
/// a virtual empty line, and an empty string has one line.
fn line_count_of(body: &str) -> usize {
    if body.is_empty() {
        return 1;
    }
    // Each '\n' separates a line from the next. A trailing '\n' contributes
    // its own virtual empty line, which is also (newlines + 1) — for both
    // "a\nb" (1 nl → 2 lines) and "a\nb\n" (2 nls → 3 lines).
    body.as_bytes().iter().filter(|&&b| b == b'\n').count() + 1
}

impl TextContent for bevy::text::TextSpan {
    fn line_count(&self) -> usize {
        line_count_of(&self.0)
    }

    fn line(&self, i: usize) -> Cow<'_, str> {
        Cow::Borrowed(line_slice(&self.0, i))
    }

    fn line_len_chars(&self, i: usize) -> usize {
        let l = line_slice(&self.0, i);
        let stripped = l.strip_suffix('\n').unwrap_or(l);
        stripped.chars().count()
    }
}

impl TextContent for String {
    fn line_count(&self) -> usize {
        line_count_of(self)
    }

    fn line(&self, i: usize) -> Cow<'_, str> {
        Cow::Borrowed(line_slice(self, i))
    }

    fn line_len_chars(&self, i: usize) -> usize {
        let l = line_slice(self, i);
        let stripped = l.strip_suffix('\n').unwrap_or(l);
        stripped.chars().count()
    }
}

/// The engine's text content component. Wraps any [`TextContent`] type.
///
/// Spawning this component (with a registered [`TextContentPlugin<T>`])
/// is sufficient to get instanced text rendering. Change detection is
/// handled by Bevy's standard `Changed<TextBuffer<T>>` — mutations go
/// through [`DerefMut`] which marks the component changed automatically.
///
/// # Examples
///
/// ```rust,ignore
/// // Simple label — no rope needed
/// commands.spawn(TextBuffer::<TextSpan>::new("Track 1"));
///
/// // Editor — rope-backed, impl TextContent for Rope in your crate
/// commands.spawn(TextBuffer::<RopeBuffer>::new(my_rope));
/// ```
#[derive(Component)]
pub struct TextBuffer<T: TextContent>(pub T);

impl<T: TextContent> TextBuffer<T> {
    /// Construct from anything that can convert into the content type `T`.
    ///
    /// When `T` isn't obvious from the argument, use a turbofish:
    ///
    /// ```rust,ignore
    /// // Label: TextSpan: From<&str>, so &str is enough once T is named.
    /// commands.spawn(TextBuffer::<TextSpan>::new("hello"));
    ///
    /// // Editor: pass the rope value directly.
    /// commands.spawn(TextBuffer::<RopeBuffer>::new(my_rope));
    /// ```
    pub fn new(content: impl Into<T>) -> Self {
        Self(content.into())
    }
}

impl<T: TextContent> Deref for TextBuffer<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T: TextContent> DerefMut for TextBuffer<T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.0
    }
}

impl<T: TextContent + Default> Default for TextBuffer<T> {
    fn default() -> Self {
        Self(T::default())
    }
}

/// Recomputable layout cache — widest shaped line, used by external scroll UI to size horizontal extent.
#[derive(Component, Default, Reflect)]
#[reflect(Component, Default)]
pub struct ContentMetrics {
    pub max_content_width: f32,
}

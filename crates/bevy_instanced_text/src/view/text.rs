//! `TextContent` trait, generic `InstancedText<T>`, and content metrics.
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
/// [`InstancedText`]. The engine calls the three required methods during
/// layout; the four default-implemented methods support hit-testing and
/// selection by rendering-layer observers. A rope-backed type should
/// override the defaults for O(log n) indexing — the built-in `String` impl
/// falls back to per-line scans, which is fine for short content.
pub trait TextContent: Send + Sync + 'static {
    /// Total number of lines, including a trailing empty line when the
    /// content ends with `\n` (matching ropey's `len_lines()` convention).
    fn line_count(&self) -> usize;
    /// Text of line `i` (0-based), including its trailing `\n` if present.
    fn line(&self, i: usize) -> Cow<'_, str>;
    /// Character count of line `i`, excluding the trailing `\n`.
    fn line_len_chars(&self, i: usize) -> usize;

    /// The whole buffer as one `&str`, when it is stored contiguously.
    ///
    /// Lets whole-buffer passes walk the text once rather than calling
    /// [`line`](Self::line) per index, which is O(i) for contiguous types and
    /// so quadratic over a full scan. Chunked types (ropes, grids) return
    /// `None` — their `line` is already cheap.
    fn as_contiguous_str(&self) -> Option<&str> {
        None
    }

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

/// Compute `line(i)` for a `&str` body following the ropey convention:
/// the slice **includes its trailing `\n`** when one is present. The final
/// virtual empty line after a trailing newline is reported as `""`.
fn line_slice(body: &str, i: usize) -> &str {
    let bytes = body.as_bytes();
    let mut start = 0usize;
    for _ in 0..i {
        match memchr(b'\n', &bytes[start..]) {
            Some(p) => start += p + 1,
            None => return "", // Past the last line
        }
    }
    if start > bytes.len() {
        return "";
    }
    let end = match memchr(b'\n', &bytes[start..]) {
        Some(p) => start + p + 1, // include the '\n'
        None => bytes.len(),
    };
    &body[start..end]
}

/// Byte-wise `\n` search. `str::find('\n')` goes through `CharSearcher`, which
/// is markedly slower than a raw byte scan on the hot measure path.
fn memchr(needle: u8, haystack: &[u8]) -> Option<usize> {
    haystack.iter().position(|&b| b == needle)
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

impl TextContent for String {
    fn line_count(&self) -> usize {
        line_count_of(self)
    }

    fn as_contiguous_str(&self) -> Option<&str> {
        Some(self)
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

/// The engine's text-view component — the instanced-text analog of Bevy UI's
/// [`bevy::ui::widget::Text`]. Wraps any [`TextContent`] type: use `String`
/// for labels (implemented below), plug in a rope for editors, a grid for
/// terminals.
///
/// Spawning this component (with a registered `TextContentPlugin<T>`) is
/// sufficient to get instanced rendering — `TextFont`, `TextColor`, and
/// `TextLayout` are auto-inserted like Bevy's `Text`. Change detection is
/// handled by Bevy's standard `Changed<InstancedText<T>>` — mutations go
/// through [`DerefMut`] which marks the component changed automatically.
///
/// The content type is named explicitly via the turbofish, so the backing
/// store is visible at every spawn site:
///
/// ```rust,ignore
/// // Label — string-backed.
/// commands.spawn(InstancedText::<String>::new("Track 1"));
///
/// // Editor — rope-backed, impl TextContent for Rope in your crate.
/// commands.spawn(InstancedText::<RopeBuffer>::new(my_rope));
/// ```
#[derive(Component)]
pub struct InstancedText<T: TextContent>(pub T);

impl<T: TextContent> InstancedText<T> {
    /// Construct from anything that converts into the content type `T`. Name
    /// `T` with a turbofish so the backing store is explicit:
    ///
    /// ```rust,ignore
    /// // String: From<&str>, so &str is enough once T is named.
    /// commands.spawn(InstancedText::<String>::new("hello"));
    ///
    /// // Editor: pass the rope value directly.
    /// commands.spawn(InstancedText::<RopeBuffer>::new(my_rope));
    /// ```
    pub fn new(content: impl Into<T>) -> Self {
        Self(content.into())
    }
}

impl<T: TextContent> Deref for InstancedText<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T: TextContent> DerefMut for InstancedText<T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.0
    }
}

impl<T: TextContent + Default> Default for InstancedText<T> {
    fn default() -> Self {
        Self(T::default())
    }
}

impl<T: TextContent> From<T> for InstancedText<T> {
    fn from(content: T) -> Self {
        Self(content)
    }
}

/// Recomputable layout cache — widest shaped line, used by external scroll UI to size horizontal extent.
#[derive(Component, Default, Reflect)]
#[reflect(Component, Default)]
pub struct ContentMetrics {
    pub max_content_width: f32,
}

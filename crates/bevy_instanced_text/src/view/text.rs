//! TextContent trait, generic TextBuffer<T>, scroll state, and content metrics.

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
            let s: String = line.chars().skip(local_start).take(local_end - local_start).collect();
            out.push_str(&s);
            acc = line_end;
        }
        Cow::Owned(out)
    }
}

/// A simple string-backed [`TextContent`] for labels, HUD values, DAW track
/// names, and any other short text that doesn't need rope-level editing.
///
/// Mirrors Bevy's own `TextSpan(pub String)` naming convention. Spawning
/// `TextBuffer::<TextSpan>::new(TextSpan::new("hello"))` is the simplest
/// way to render instanced text.
#[derive(Component, Clone, Default, Debug, Reflect)]
#[reflect(Component, Default)]
pub struct TextSpan(pub String);

impl TextSpan {
    pub fn new(text: impl Into<String>) -> Self {
        Self(text.into())
    }
}

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
    let newlines = body.as_bytes().iter().filter(|&&b| b == b'\n').count();
    // Each '\n' separates a line from the next; if the string ends with '\n'
    // there is a virtual empty line after.
    if body.ends_with('\n') {
        newlines + 1
    } else {
        newlines + 1
    }
}

impl TextContent for TextSpan {
    fn line_count(&self) -> usize {
        line_count_of(&self.0)
    }

    fn line(&self, i: usize) -> Cow<'_, str> {
        Cow::Borrowed(line_slice(&self.0, i))
    }

    fn line_len_chars(&self, i: usize) -> usize {
        let l = line_slice(&self.0, i);
        // Spec: exclude trailing '\n'.
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
/// commands.spawn(TextBuffer::new("Track 1"));
///
/// // Editor — rope-backed, impl TextContent for Rope in your crate
/// commands.spawn(TextBuffer::new(my_rope));
/// ```
#[derive(Component)]
pub struct TextBuffer<T: TextContent>(pub T);

impl<T: TextContent> TextBuffer<T> {
    pub fn new(content: T) -> Self {
        Self(content)
    }
}

impl TextBuffer<String> {
    pub fn from_str(s: &str) -> Self {
        Self(s.to_owned())
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

/// Smooth-scroll animation targets and current offsets.
///
/// Hosts write `target_y` / `target_x` to request scroll. The engine's
/// `animate_text_view_scroll` system drives `offset_y` and `horizontal`
/// toward those targets each frame.
///
/// For instant (non-animated) scroll, write both `offset_y` and `target_y`
/// to the same value (and similarly for horizontal).
///
/// Sign convention: positive = down / right.
#[derive(Component, Default, Reflect)]
#[reflect(Component, Default)]
pub struct SmoothScroll {
    /// Vertical smooth-scroll target in logical pixels, positive = down.
    pub target_y: f32,
    /// Horizontal smooth-scroll target in logical pixels, positive = right.
    pub target_x: f32,
    /// Current animated vertical offset. Written by the engine; read by
    /// renderers. Not the same as `target_y` when an animation is in flight.
    pub offset_y: f32,
    /// Current animated horizontal offset. Written by the engine; read by
    /// renderers. Not the same as `target_x` when an animation is in flight.
    pub horizontal: f32,
    /// Animation duration in seconds. Synced from `ScrollConfig::smooth_scroll_duration`.
    pub duration: f32,
    #[reflect(ignore)]
    pub(crate) vertical_anim: Option<ScrollAnimation>,
    #[reflect(ignore)]
    pub(crate) horizontal_anim: Option<ScrollAnimation>,
}

#[derive(Clone, Debug)]
pub(crate) struct ScrollAnimation {
    pub from: f32,
    pub to: f32,
    pub elapsed: f32,
    pub duration: f32,
    pub composite: Option<CompositeStops>,
}

/// Two-stage composite curve for jumps > 2.5× viewport; avoids the floaty
/// tail that a single easeOutCubic produces over large distances.
#[derive(Clone, Debug)]
pub(crate) struct CompositeStops {
    pub stop1: f32,
    pub stop2: f32,
    pub split: f32,
}

/// Recomputable layout cache — widest shaped line, used by external scroll UI to size horizontal extent.
#[derive(Component, Default, Reflect)]
#[reflect(Component, Default)]
pub struct ContentMetrics {
    pub max_content_width: f32,
}

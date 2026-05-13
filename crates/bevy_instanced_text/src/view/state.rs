//! TextContent trait, generic TextBuffer<T>, scroll state, and content metrics.

use std::borrow::Cow;
use std::ops::{Deref, DerefMut};

use bevy::prelude::*;

/// Minimum interface the layout engine needs from a text buffer.
///
/// Implement this on any type to use it as the backing store for a
/// [`TextBuffer`]. The engine calls only these three methods during
/// layout — everything else (rope edits, cursor math, LSP position
/// mapping) stays in the crate that owns the concrete type.
///
/// A built-in impl for [`String`] is provided so label / DAW / HUD use
/// cases work without any extra dependencies.
pub trait TextContent: Send + Sync + 'static {
    /// Total number of lines, including a trailing empty line when the
    /// content ends with `\n` (matching ropey's `len_lines()` convention).
    fn line_count(&self) -> usize;
    /// Text of line `i` (0-based), including its trailing `\n` if present.
    fn line(&self, i: usize) -> Cow<'_, str>;
    /// Character count of line `i`, excluding the trailing `\n`.
    fn line_len_chars(&self, i: usize) -> usize;
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

impl TextContent for TextSpan {
    fn line_count(&self) -> usize {
        if self.0.is_empty() {
            1
        } else {
            let n = self.0.lines().count();
            if self.0.ends_with('\n') { n + 1 } else { n }
        }
    }

    fn line(&self, i: usize) -> Cow<'_, str> {
        let mut lines = self.0.split('\n');
        Cow::Borrowed(lines.nth(i).unwrap_or(""))
    }

    fn line_len_chars(&self, i: usize) -> usize {
        self.0.split('\n')
            .nth(i)
            .map(|l| l.chars().count())
            .unwrap_or(0)
    }
}

impl TextContent for String {
    fn line_count(&self) -> usize {
        if self.is_empty() {
            1
        } else {
            let n = self.lines().count();
            if self.ends_with('\n') { n + 1 } else { n }
        }
    }

    fn line(&self, i: usize) -> Cow<'_, str> {
        let mut lines = self.split('\n');
        Cow::Borrowed(lines.nth(i).unwrap_or(""))
    }

    fn line_len_chars(&self, i: usize) -> usize {
        self.split('\n')
            .nth(i)
            .map(|l| l.chars().count())
            .unwrap_or(0)
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

/// Vertical and horizontal scroll offsets and their smooth-scroll targets.
#[derive(Component, Reflect)]
#[reflect(Component)]
pub struct ScrollState {
    pub scroll_offset: f32,
    pub target_scroll_offset: f32,
    pub horizontal_scroll_offset: f32,
    pub target_horizontal_scroll_offset: f32,
    /// Animation duration in seconds. Synced from `ScrollConfig::smooth_scroll_duration`.
    pub smooth_scroll_duration: f32,
    #[reflect(ignore)]
    pub vertical_anim: Option<ScrollAnimation>,
    #[reflect(ignore)]
    pub horizontal_anim: Option<ScrollAnimation>,
}

impl Default for ScrollState {
    fn default() -> Self {
        Self {
            scroll_offset: 0.0,
            target_scroll_offset: 0.0,
            horizontal_scroll_offset: 0.0,
            target_horizontal_scroll_offset: 0.0,
            smooth_scroll_duration: 0.125,
            vertical_anim: None,
            horizontal_anim: None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ScrollAnimation {
    pub from: f32,
    pub to: f32,
    pub elapsed: f32,
    pub duration: f32,
    pub composite: Option<CompositeStops>,
}

/// Two-stage composite curve for jumps > 2.5× viewport; avoids the floaty
/// tail that a single easeOutCubic produces over large distances.
#[derive(Clone, Debug)]
pub struct CompositeStops {
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

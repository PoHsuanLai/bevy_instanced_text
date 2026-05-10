//! GPU-accelerated text rendering engine for Bevy.
//!
//! Rasterizes glyphs via [cosmic-text](https://docs.rs/cosmic-text), shapes
//! lines, and issues one instanced GPU draw call per text view. The crate is
//! pure rendering infrastructure — it owns no cursor, no selection, no input
//! handling, and no application-level concepts. Feed it styled text; it draws it.
//!
//! ## Concepts
//!
//! A **[`TextView`]** is the entity marker. Pair it with:
//!
//! - **[`TextBuffer`]** — the rope-backed text content and a version counter.
//! - **[`TextViewport`]** — size, scroll offsets, and gutter geometry.
//! - **[`TextFont`]** — font path, size, and line height.
//! - **[`TextColor`]** / **[`TextBackgroundColor`]** — foreground and background colors.
//! - **[`LineStyles`]** — per-line [`StyleRun`] lists (colors, bold, italic,
//!   inline backgrounds). Producers write this; the engine reads it.
//! - **[`HiddenLines`]** — which buffer lines to skip (e.g. folded regions).
//! - **[`LayoutWrap`]** — optional soft-wrap budget in pixels.
//! - **[`TextViewOverlays`]** — decoration rectangles (cursors, selections,
//!   highlights) written by the host each frame.
//!
//! The engine produces a **[`DisplayLayout`]** — an immutable per-frame snapshot
//! of shaped lines — and renders it. Hosts that need pixel-accurate hit-testing
//! or overlay placement read `DisplayLayout` and the [`RowMetrics`] /
//! [`BufferAnchorParam`] helpers.
//!
//! **Scroll state** ([`ScrollState`], [`ContentMetrics`]) is data only; the
//! engine does not render a scrollbar — attach your own or skip it.
//!
//! ## Quick start
//!
//! ```rust,no_run
//! use bevy::prelude::*;
//! use bevy_instanced_text::prelude::*;
//!
//! App::new()
//!     .add_plugins(DefaultPlugins)
//!     .add_plugins(InstancedTextPlugins)
//!     .add_systems(Startup, |mut commands: Commands| {
//!         commands.spawn((
//!             TextView,
//!             TextBuffer::new("hello world"),
//!             TextViewport::default(),
//!             TextFont::default(),
//!         ));
//!     })
//!     .run();
//! ```

pub mod gpu;
pub mod view;

pub use gpu::*;
pub use view::*;

pub mod prelude {
    //! Common types for spawning and rendering text views.
    pub use crate::gpu::{GlyphAtlasPlugin, InstancedTextRenderPlugin};
    pub use crate::view::{
        row_metrics, row_metrics_with_baseline, AnchorPoint, Block, BlockDecorTheme,
        BlockLayoutConfig, BlockList, BufferAnchorParam, ContentMetrics, DisplayLayout,
        CornerRadii, FontSynthesis, HiddenLines, InstancedTextPlugin, InstancedTextPlugins,
        LayoutWrap, LineStyles, RectOverlay, RowMetrics, RowMetricsParam, RowVertical, RunWithText,
        ScrollState, StyleRun, TextBackgroundColor, TextBuffer, TextColor, TextFont, TextView,
        TextViewOverlays, TextViewport,
    };
}

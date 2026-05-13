//! GPU-accelerated text rendering engine for Bevy.
//!
//! Rasterizes glyphs via [cosmic-text](https://docs.rs/cosmic-text), shapes
//! lines, and issues one instanced GPU draw call per text view. The crate is
//! pure rendering infrastructure — it owns no cursor, no selection, no input
//! handling, and no application-level concepts. Feed it styled text; it draws it.
//!
//! ## Concepts
//!
//! A text view entity is a standard Bevy UI `Node` carrying a [`TextBuffer<T>`].
//! Size it with `Node::width`/`height`; add `Node::padding` to inset the text
//! area from the node edges. Everything else is standard Bevy UI — hit-testing,
//! picking, and layout are all handled by the UI system automatically.
//!
//! The content type `T` is anything that implements [`TextContent`]. The crate
//! ships [`TextSpan`] (a `String` wrapper) for simple labels; editors plug in a
//! rope-backed type and terminals plug in a grid-derived type.
//!
//! Components paired with [`TextBuffer<T>`]:
//!
//! - **[`TextFont`]** — font path, size, and line height.
//! - **[`TextColor`]** / **[`TextBackgroundColor`]** — foreground and background colors.
//! - **[`LineStyles`]** — per-line [`StyleRun`] lists (colors, bold, italic,
//!   inline backgrounds). Producers write this; the engine reads it.
//! - **[`HiddenLines`]** — which buffer lines to skip (e.g. folded regions).
//! - **[`TextBounds`]** — optional soft-wrap budget in pixels.
//! - **[`TextViewOverlays`]** — decoration rectangles (cursors, selections,
//!   highlights) written by the host each frame.
//!
//! The engine produces a **[`DisplayLayout`]** — an immutable per-frame snapshot
//! of shaped lines — and renders it. Hosts that need pixel-accurate hit-testing
//! or overlay placement read `DisplayLayout` and the [`RowMetrics`] /
//! [`BufferAnchorParam`] helpers.
//!
//! **Scroll state** ([`bevy::ui::ScrollPosition`] + [`SmoothScroll`], [`ContentMetrics`]) is data
//! only; the engine does not render a scrollbar — attach your own or skip it.
//!
//! ## Camera setup
//!
//! The engine renders glyphs as GPU instances in world space via a `Camera2d`.
//! For a single full-window view, spawn one `Camera2d` at the default origin —
//! no extra configuration needed.
//!
//! For split-pane layouts, give each camera a `Camera::viewport` rect (in
//! physical pixels) so it only renders into its portion of the window. Each
//! `TextView` entity uses `RenderLayers` to target the right camera:
//!
//! ```rust,no_run
//! # use bevy::prelude::*;
//! # use bevy_camera::visibility::RenderLayers;
//! # fn setup(mut commands: Commands, window: Query<&Window>) {
//! let window = window.single().unwrap();
//! let scale = window.scale_factor();
//! let half_w = (window.width() * scale / 2.0) as u32;
//! let full_h = (window.height() * scale) as u32;
//!
//! // Left camera.
//! commands.spawn((
//!     Camera2d,
//!     Camera {
//!         viewport: Some(bevy::camera::Viewport {
//!             physical_position: UVec2::ZERO,
//!             physical_size: UVec2::new(half_w, full_h),
//!             ..default()
//!         }),
//!         ..default()
//!     },
//!     RenderLayers::layer(0),
//! ));
//!
//! // Left text view — sized to half the window in logical pixels.
//! commands.spawn((
//!     bevy_instanced_text::TextBuffer::new(bevy_instanced_text::TextSpan::new("left pane")),
//!     Node { width: Val::Px(window.width() / 2.0), height: Val::Px(window.height()), ..default() },
//!     RenderLayers::layer(0),
//! ));
//! # }
//! ```
//!
//! ## Quick start
//!
//! ```rust,no_run
//! use bevy::prelude::*;
//! use bevy_instanced_text::prelude::*;
//! use bevy_instanced_text::TextSpan; // disambiguate from `bevy::text::TextSpan`
//!
//! App::new()
//!     .add_plugins(DefaultPlugins)
//!     .add_plugins(InstancedTextPlugins)
//!     .add_systems(Startup, |mut commands: Commands| {
//!         // Camera — one Camera2d is all that's needed for a single view.
//!         commands.spawn(Camera2d);
//!         // Text view — size it with Node; padding insets the text area.
//!         commands.spawn((
//!             TextBuffer::new(TextSpan::new("hello world")),
//!             Node {
//!                 width: Val::Vw(100.0),
//!                 height: Val::Vh(100.0),
//!                 ..default()
//!             },
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
        row_metrics, row_metrics_with_baseline, ContentMetrics, CornerRadii, DisplayLayout,
        FontSynthesis, HiddenLines, InstancedTextPlugin, InstancedTextPlugins, LineStyles,
        MonoCellWidth, MonoFontFaces, RectOverlay, RowMetrics, RowMetricsParam, RowVertical,
        RunWithText, SmoothScroll, StyleRun, TextBackgroundColor, TextBounds, TextBuffer,
        TextColor, TextContent, TextContentPlugin, TextSpan, TextViewOverlays,
    };
}

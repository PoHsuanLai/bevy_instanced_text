//! View primitives: generic text state, paint-ready layout, overlays, renderer.

pub mod bounds;
pub mod cursor;
pub mod font;
pub mod pipeline;
pub mod text_access;
pub mod overlay;
pub mod plugin;
pub mod render;
pub mod glyph;
pub mod text;
pub mod text_style;
pub mod measurement;

pub use bounds::{row_metrics, row_metrics_with_baseline, RowMetrics, RowMetricsParam};
pub use cursor::{AnchorPoint, BufferAnchorParam};
pub use font::{FontSynthesis, MonoCellWidth, MonoFontFaces, resolve_line_height};
pub use pipeline::DisplayLayout;
pub use text_access::{visible_buffer_range, LayoutProduceSet};

pub use overlay::{
    for_each_row_in_buffer_span, CornerRadii, RectOverlay, RowPosition, RowVertical,
    TextOverlays, TextUnderlays,
};
pub use plugin::{
    InstancedTextPlugin, InstancedTextPlugins, TextContentPlugin, TextViewBatchEntity,
    TextViewRenderSet,
};
pub use render::{BatchTransform, GlyphBatchComponent, GlyphInstance};
pub use glyph::{ShapedLine, StyleRun, TextDecoration};
pub use text::{ContentMetrics, TextBuffer, TextContent, TextSpan};
pub use text_style::{HiddenLines, LineStyles, RunWithText, TextBounds};
pub use bevy::text::{TextBackgroundColor, TextColor};

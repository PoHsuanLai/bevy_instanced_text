//! View primitives: generic text state, paint-ready layout, overlays, renderer.

pub mod bounds;
pub mod cursor;
pub mod font;
pub mod glyph;
pub mod measurement;
pub mod overlay;
pub mod pipeline;
pub mod plugin;
pub mod render;
pub mod text;
pub mod text_access;
pub mod text_style;

pub use bounds::{row_metrics, row_metrics_with_baseline, RowMetrics, RowMetricsParam};
pub use cursor::{AnchorPoint, BufferAnchorParam};
pub use font::{resolve_line_height, FontSynthesis, MonoCellWidth, MonoFontFaces};
pub use pipeline::DisplayLayout;
pub use text_access::{visible_buffer_range, LayoutProduceSet};

pub use bevy::text::{TextBackgroundColor, TextColor};
pub use glyph::{ShapedLine, TextDecoration, TextFormat};
pub use overlay::{
    for_each_row_in_buffer_span, CornerRadii, RectOverlay, RowPosition, RowVertical, TextOverlays,
    TextUnderlays,
};
pub use plugin::{
    InstancedTextPlugin, InstancedTextPlugins, TextContentPlugin, TextViewBatchEntity,
    TextViewRenderSet,
};
pub use render::{BatchTransform, GlyphBatchComponent, GlyphInstance};
pub use text::{ContentMetrics, TextBuffer, TextContent, TextSpan};
pub use text_style::{FormattedSpan, HiddenLines, LineStyles, TextBounds};

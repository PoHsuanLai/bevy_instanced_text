//! View primitives: generic text state, paint-ready layout, overlays, renderer.

pub mod anchor;
pub mod font;
pub mod layout;
pub mod layout_builder;
pub mod overlay;
pub mod plugin;
pub mod render;
pub mod snapshot;
pub mod state;
pub mod styling;
pub mod theme;
pub mod tuning;

pub use anchor::{row_metrics, row_metrics_with_baseline, RowMetrics, RowMetricsParam};
pub use font::{FontSynthesis, MonoCellWidth, MonoFontFaces, resolve_line_height};
pub use layout::DisplayLayout;
pub use layout_builder::{visible_buffer_range, LayoutProduceSet};

pub use overlay::{
    for_each_row_in_buffer_span, CornerRadii, RectOverlay, RowPosition, RowVertical,
    TextViewOverlays,
};
pub use plugin::{
    InstancedTextPlugin, InstancedTextPlugins, TextContentPlugin, TextViewBatchEntity,
    TextViewRenderSet,
};
pub use render::{BatchTransform, GlyphBatchComponent, GlyphInstance};
pub use snapshot::{ShapedLine, StyleRun, TextDecoration};
pub use state::{ContentMetrics, SmoothScroll, TextBuffer, TextContent, TextSpan};
pub use styling::{HiddenLines, LineStyles, RunWithText, TextBounds};
pub use theme::{TextBackgroundColor, TextColor};

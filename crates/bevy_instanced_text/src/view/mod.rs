//! View primitives: rope-backed text state, viewport, paint-ready layout, overlays, renderer.

pub mod anchor;
pub mod buffer_anchor;
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
pub mod viewport;

pub use anchor::{
    row_metrics, row_metrics_with_baseline, RowMetrics, RowMetricsParam,
    DEFAULT_BASELINE_OFFSET_RATIO,
};
pub use buffer_anchor::{AnchorPoint, BufferAnchorParam};
pub use font::{FontSynthesis, TextFont};
pub use layout::DisplayLayout;
pub use layout_builder::{
    approx_display_rows_for_line, slice_runs, visible_buffer_range, wrap_into_rows,
    LayoutProduceSet, WrapRow,
};

pub use overlay::{
    for_each_row_in_buffer_span, CornerRadii, RectOverlay, RowPosition, RowVertical,
    TextViewOverlays,
};
pub use plugin::{
    InstancedTextPlugin, InstancedTextPlugins, TextView, TextViewBatchEntity, TextViewRenderSet,
};
pub use render::{render_layout, FontFaces, GlyphBatchComponent, GlyphInstance, TextViewBatch};
pub use snapshot::{ShapedLine, StyleRun, TextDecoration};
pub use state::{ContentMetrics, ScrollState, TextBuffer};
pub use styling::{HiddenLines, LineStyles, RunWithText, TextBounds};
pub use theme::{TextBackgroundColor, TextColor};
pub use tuning::LayoutTuning;
pub use viewport::TextViewport;

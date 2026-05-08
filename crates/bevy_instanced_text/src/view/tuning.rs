//! Per-`TextView` rendering performance knobs.
//!
//! Defaults are tuned for a typical desktop IDE on a 1080p–4K monitor.
//! Override per-entity to suit unusual workloads — chat panels, log
//! tails, embedded surfaces, multi-megabyte source files:
//!
//! ```rust,ignore
//! commands.spawn((
//!     TextView,
//!     LayoutTuning { viewport_buffer_lines: 8 },
//!     // ...font, viewport, etc
//! ));
//! ```
//!
//! Cascaded by `TextView`'s `#[require]`, so leaving it off uses the
//! defaults. The atlas-side shape-cache cap is a separate process-wide
//! concern — see [`crate::gpu::DEFAULT_SHAPE_CACHE_CAPACITY`] and the
//! [`crate::gpu::GlyphAtlas::new_with_font_and_capacity`] constructor.

use bevy::prelude::*;

/// Per-view layout perf tunables.
#[derive(Component, Clone, Copy, Debug, Reflect)]
#[reflect(Component, Default, Debug)]
pub struct LayoutTuning {
    /// Extra display rows kept above and below the visible window during
    /// layout. More = smoother fast-scroll into view, fewer mid-frame
    /// rebuilds; less = lower steady-state shaping cost on huge files.
    pub viewport_buffer_lines: u32,
}

impl Default for LayoutTuning {
    fn default() -> Self {
        Self {
            viewport_buffer_lines: 4,
        }
    }
}

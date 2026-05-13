//! GPU text rendering via instanced draw calls and a glyph atlas.
//!
//! 1. **Glyph Atlas** — rasterizes glyphs to a texture atlas once via cosmic_text/swash.
//! 2. **Instanced Rendering** — renders all visible glyphs in a single draw call.
//!
//! The atlas keys exclusively on `cosmic_text::CacheKey`; the renderer in
//! `crate::view::render` reads cache_keys off pre-shaped `LineShape`s and
//! looks them up via `GlyphAtlas::get_or_rasterize_glyph`.

mod font_atlas;
mod pipeline;

use bevy::prelude::*;

pub use font_atlas::{
    GlyphAtlas, GlyphAtlasPlugin, GlyphInfo, PlacementInfo, ATLAS_SIZE, DEFAULT_RASTER_SCALE,
    DEFAULT_SHAPE_CACHE_CAPACITY,
};

pub use pipeline::InstancedTextRenderPlugin;

/// System condition: true once the glyph atlas resource exists.
pub fn atlas_ready(atlas: Option<Res<GlyphAtlas>>) -> bool {
    atlas.is_some()
}

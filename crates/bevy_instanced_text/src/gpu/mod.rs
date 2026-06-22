//! GPU text rendering via instanced draw calls and a glyph atlas.
//!
//! 1. **Glyph Atlas** — shapes lines and rasterizes glyphs by driving Bevy
//!    0.19's built-in `TextPipeline` + `FontAtlasSet` (parley). No direct
//!    cosmic-text dependency.
//! 2. **Instanced Rendering** — renders all visible glyphs in a single draw call.
//!
//! The atlas keys glyphs on [`GlyphKey`]; the renderer in
//! `crate::view::render` reads keys off pre-shaped `LineShape`s and resolves
//! their atlas info via `GlyphAtlas::get_or_rasterize_glyph`.

mod font_atlas;
mod pipeline;

use bevy::prelude::*;

pub use font_atlas::{
    GlyphAtlas, GlyphAtlasPlugin, GlyphInfo, GlyphKey, PlacementInfo, ATLAS_SIZE,
    DEFAULT_RASTER_SCALE, DEFAULT_SHAPE_CACHE_CAPACITY,
};

pub use pipeline::InstancedTextRenderPlugin;

/// System condition: true once the glyph atlas resource exists.
pub fn atlas_ready(atlas: Option<Res<GlyphAtlas>>) -> bool {
    atlas.is_some()
}

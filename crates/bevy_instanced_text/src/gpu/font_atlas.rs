//! Glyph atlas: shapes lines and rasterizes glyphs by driving Bevy 0.19's
//! built-in text pipeline (`TextPipeline` + parley) and caching the rasterized
//! glyphs in Bevy's `FontAtlasSet`.
//!
//! This is a thin shim over `bevy_text`: `shape_line` lays out a single line via
//! `TextPipeline::update_buffer`, then `update_text_layout_info` rasterizes the
//! glyphs into `FontAtlasSet` and yields per-glyph atlas rects. The crate keeps
//! its own GPU instanced draw, fed from the resulting [`LineShape`] /
//! [`GlyphInfo`] data — no direct `cosmic-text` dependency.

use bevy::asset::{AssetId, RenderAssetUsages};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy::text::{
    ComputedTextBlock, Font, FontAtlasSet, FontCx, FontHinting, FontSize, FontSource, Justify,
    LayoutCx, LetterSpacing, LineBreak, LineHeight, ScaleCx, TextBounds, TextFont, TextLayoutInfo,
    TextPipeline,
};
use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};
use std::sync::Arc;

/// Solid-pixel texture size. A tiny dedicated texture holds one white pixel for
/// background / overlay quads (the glyph atlas textures live in `FontAtlasSet`).
pub const ATLAS_SIZE: u32 = 4;

/// Fallback supersampling factor when no window scale factor is available
/// (e.g. headless tests). Matches the typical Retina display.
pub const DEFAULT_RASTER_SCALE: f32 = 2.0;

/// Stable identity for a shaped+rasterized glyph. Replaces the old
/// `cosmic_text::CacheKey`: it identifies an atlas entry produced by Bevy's
/// text pipeline so `get_or_rasterize_glyph` is a pure lookup.
///
/// `position` already includes the swash placement offset baked back out into
/// logical pixels; `uv_*` / `size` are resolved against the owning
/// `FontAtlas` texture dimensions.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlyphKey {
    /// Atlas image this glyph lives in.
    pub texture: AssetId<Image>,
    /// Resolved glyph info (UVs, logical size, placement offset).
    pub info: GlyphInfo,
}

impl Eq for GlyphKey {}

impl Hash for GlyphKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.texture.hash(state);
        self.info.uv_min.x.to_bits().hash(state);
        self.info.uv_min.y.to_bits().hash(state);
        self.info.uv_max.x.to_bits().hash(state);
        self.info.uv_max.y.to_bits().hash(state);
    }
}

/// Atlas entry for one rasterized glyph: UV rect, size, placement offset, and advance.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlyphInfo {
    /// UV coordinates in the atlas (0.0 to 1.0)
    pub uv_min: Vec2,
    pub uv_max: Vec2,
    /// Size in logical pixels (atlas stores high-res, rendering uses logical size)
    pub size: Vec2,
    /// Offset from the baseline in logical pixels. `offset.x` is the left side
    /// bearing; `offset.y` is the distance from the baseline to the glyph's top
    /// (positive = above the baseline), matching swash placement `top`.
    pub offset: Vec2,
    pub advance: f32,
}

/// The image handle this glyph's atlas lives in. Carried alongside `GlyphInfo`
/// so the renderer can resolve the correct `Handle<Image>` per glyph.
pub use self::placement::PlacementInfo;

mod placement {
    /// Swash placement in logical pixels — left/top of the glyph bitmap
    /// relative to the pen origin / baseline.
    #[derive(Clone, Copy, Debug)]
    pub struct PlacementInfo {
        pub left: f32,
        pub top: f32,
    }
}

/// GPU glyph atlas resource.
///
/// Owns Bevy 0.19's text-pipeline plumbing (`TextPipeline`, `FontAtlasSet`,
/// parley contexts) and drives it to shape lines and rasterize glyphs. The
/// rasterized glyphs live in `FontAtlasSet`'s textures; this resource also owns
/// a tiny `solid` texture for background / overlay quads.
///
/// Shaped `LineShape`s are cached keyed on `(text, font_size, font, bold,
/// italic)` to avoid re-driving the pipeline for unchanged lines on scroll.
#[derive(Resource)]
pub struct GlyphAtlas {
    /// Tiny image holding a single white pixel for solid background quads.
    pub texture: Handle<Image>,
    pub dirty: bool,
    /// Generation counter — bumped on clear for cache invalidation.
    pub generation: u64,
    /// UV info for a solid white pixel — used for background rectangles. Its
    /// texture is [`GlyphAtlas::texture`].
    pub solid_uv: GlyphInfo,

    // --- Bevy text pipeline plumbing ---
    text_pipeline: TextPipeline,
    font_atlas_set: FontAtlasSet,
    font_cx: FontCx,
    layout_cx: LayoutCx,
    scale_cx: ScaleCx,
    /// Scratch block reused across `shape_line` calls.
    computed: ComputedTextBlock,
    /// Scratch layout-info reused across `shape_line` calls.
    layout_info: TextLayoutInfo,

    /// Cache of shaped lines keyed by `(content_hash, font_size_bits, font, …)`.
    shape_cache: HashMap<u64, Arc<crate::view::glyph::LineShape>>,
    shape_cache_order: VecDeque<u64>,
    /// Glyph rasterization scale. Tracks the host window's scale_factor.
    raster_scale: f32,
    /// Per-instance cap on `shape_cache`.
    shape_cache_capacity: usize,
    /// AssetId of the last glyph atlas texture resolved via
    /// [`GlyphAtlas::get_or_rasterize_glyph`]. The renderer reads this after a
    /// `render_layout` pass to pick the batch's glyph atlas image.
    last_glyph_texture: Option<AssetId<Image>>,
}

/// Default FIFO cap on the shaped-line cache.
pub const DEFAULT_SHAPE_CACHE_CAPACITY: usize = 8192;

impl GlyphAtlas {
    pub fn new(images: &mut Assets<Image>) -> Self {
        Self::new_with_capacity(images, DEFAULT_SHAPE_CACHE_CAPACITY)
    }

    /// Construct with a custom shape-cache capacity.
    pub fn new_with_capacity(images: &mut Assets<Image>, shape_cache_capacity: usize) -> Self {
        // A small RGBA texture with a single white pixel for solid quads.
        let pixels = vec![255u8; (ATLAS_SIZE * ATLAS_SIZE * 4) as usize];
        let image = Image::new(
            Extent3d {
                width: ATLAS_SIZE,
                height: ATLAS_SIZE,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            pixels,
            TextureFormat::Rgba8UnormSrgb,
            RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
        );
        let texture = images.add(image);

        // Sample the middle of the solid texture so filtering never bleeds an
        // edge texel.
        let solid_uv = GlyphInfo {
            uv_min: Vec2::splat(0.5 / ATLAS_SIZE as f32),
            uv_max: Vec2::splat((ATLAS_SIZE as f32 - 0.5) / ATLAS_SIZE as f32),
            size: Vec2::ONE,
            offset: Vec2::ZERO,
            advance: 0.0,
        };

        Self {
            texture,
            dirty: false,
            generation: 0,
            solid_uv,
            text_pipeline: TextPipeline::default(),
            font_atlas_set: FontAtlasSet::default(),
            font_cx: FontCx::default(),
            layout_cx: LayoutCx::default(),
            scale_cx: ScaleCx::default(),
            computed: ComputedTextBlock::default(),
            layout_info: TextLayoutInfo::default(),
            shape_cache: HashMap::with_capacity(shape_cache_capacity),
            shape_cache_order: VecDeque::with_capacity(shape_cache_capacity),
            raster_scale: DEFAULT_RASTER_SCALE,
            shape_cache_capacity,
            last_glyph_texture: None,
        }
    }

    /// No-op in the FontAtlasSet world: fonts are resolved by handle through
    /// Bevy's text pipeline at shape time. Returns the handle's `AssetId` when
    /// the font asset is loaded so callers can detect availability, else `None`.
    ///
    /// Kept for API compatibility with the old cosmic-text font registration.
    pub fn ensure_font(
        &mut self,
        handle: &Handle<Font>,
        fonts: &Assets<Font>,
    ) -> Option<AssetId<Font>> {
        let id = handle.id();
        fonts.get(handle).map(|_| id)
    }

    /// Texture upload is handled by Bevy's `Assets<Image>` directly (the atlas
    /// textures live in `FontAtlasSet`). Kept as a no-op for API compatibility.
    pub fn update_texture(&mut self, _images: &mut Assets<Image>) {
        self.dirty = false;
    }

    /// Clear the shaped-line cache. The underlying `FontAtlasSet` is left
    /// intact (Bevy manages its own atlas growth); we only drop our shape cache
    /// so stale advances/positions don't survive a scale change.
    pub fn clear(&mut self) {
        self.shape_cache.clear();
        self.shape_cache_order.clear();
        self.generation += 1;
    }

    /// Current glyph rasterization scale.
    pub fn raster_scale(&self) -> f32 {
        self.raster_scale
    }

    /// Set the rasterization scale and invalidate everything keyed on it.
    /// No-op if the scale is unchanged.
    pub fn set_raster_scale(&mut self, scale: f32) {
        let scale = scale.max(0.1);
        if (self.raster_scale - scale).abs() < f32::EPSILON {
            return;
        }
        self.raster_scale = scale;
        self.clear();
    }

    /// Pre-rasterize a batch of glyphs into the atlas. With Bevy's pipeline the
    /// glyphs are rasterized eagerly during `shape_line` (which fills the
    /// `FontAtlasSet`), so this is a no-op retained for API compatibility:
    /// `GlyphKey` already carries its resolved `GlyphInfo`.
    pub fn ensure_glyphs<I: IntoIterator<Item = GlyphKey>>(&mut self, keys: I) {
        for _ in keys {}
    }

    /// Look up a glyph's atlas info. Because [`GlyphKey`] carries its resolved
    /// [`GlyphInfo`] (filled by `shape_line` when the glyph was rasterized),
    /// this is a pure unwrap — kept as a method so callers don't change shape.
    pub fn get_or_rasterize_glyph(&mut self, key: GlyphKey) -> Option<(GlyphInfo, PlacementInfo)> {
        self.last_glyph_texture = Some(key.texture);
        let placement = PlacementInfo {
            left: key.info.offset.x,
            top: key.info.offset.y,
        };
        Some((key.info, placement))
    }

    /// Resolve the `Handle<Image>` for the most recently looked-up glyph's
    /// atlas texture (see [`GlyphAtlas::get_or_rasterize_glyph`]). The renderer
    /// uses this to bind the batch's glyph atlas image.
    pub fn last_glyph_texture_handle(&self) -> Option<Handle<Image>> {
        self.last_glyph_texture
            .and_then(|id| self.atlas_image_handle(id))
    }
}

/// Inserts [`GlyphAtlas`] at startup and keeps its raster scale synced to
/// the primary window's `scale_factor` each frame.
pub struct GlyphAtlasPlugin;

impl Plugin for GlyphAtlasPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, setup_glyph_atlas)
            .add_systems(PreUpdate, sync_atlas_scale);
    }
}

fn setup_glyph_atlas(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    windows: Query<&bevy::window::Window, With<bevy::window::PrimaryWindow>>,
) {
    let mut atlas = GlyphAtlas::new(&mut images);
    if let Ok(window) = windows.single() {
        atlas.set_raster_scale(window.scale_factor());
    }
    commands.insert_resource(atlas);
}

/// Every frame, compare the primary window's `scale_factor` to the atlas's
/// cached value and re-rasterize on mismatch. The setter short-circuits when
/// stable.
fn sync_atlas_scale(
    atlas: Option<ResMut<GlyphAtlas>>,
    windows: Query<&bevy::window::Window, With<bevy::window::PrimaryWindow>>,
) {
    let Some(mut atlas) = atlas else { return };
    let Ok(window) = windows.single() else { return };
    atlas.set_raster_scale(window.scale_factor());
}

mod shaping {
    use super::*;
    use crate::view::glyph::{LineShape, ShapedGlyph};

    impl GlyphAtlas {
        /// Resolve the strong `Handle<Image>` for one of the `FontAtlasSet`'s
        /// atlas textures by `AssetId`. The set holds the strong handle, so the
        /// image is guaranteed alive while the glyph references it.
        pub fn atlas_image_handle(&self, id: AssetId<Image>) -> Option<Handle<Image>> {
            for atlases in self.font_atlas_set.values() {
                for atlas in atlases {
                    if atlas.texture.id() == id {
                        return Some(atlas.texture.clone());
                    }
                }
            }
            None
        }

        /// Shape a single line and rasterize its glyphs into the `FontAtlasSet`.
        ///
        /// Returns a [`LineShape`] whose glyphs carry a [`GlyphKey`] resolving
        /// to the rasterized atlas entry. `font` selects the face (a
        /// `Handle<Font>`); `None` falls back to the default font handle.
        ///
        /// Cached on `(text, font_size, font, bold, italic)`.
        pub fn shape_line(
            &mut self,
            text: &str,
            font_size: f32,
            font: Option<&Handle<Font>>,
            fonts: &Assets<Font>,
            images: &mut Assets<Image>,
        ) -> LineShape {
            self.shape_line_styled(text, font_size, font, false, false, fonts, images)
        }

        #[allow(clippy::too_many_arguments)]
        pub fn shape_line_styled(
            &mut self,
            text: &str,
            font_size: f32,
            font: Option<&Handle<Font>>,
            bold: bool,
            italic: bool,
            fonts: &Assets<Font>,
            images: &mut Assets<Image>,
        ) -> LineShape {
            let font_handle = font.cloned().unwrap_or_default();

            let key = {
                let mut hasher = std::collections::hash_map::DefaultHasher::new();
                text.hash(&mut hasher);
                font_size.to_bits().hash(&mut hasher);
                font_handle.id().hash(&mut hasher);
                bold.hash(&mut hasher);
                italic.hash(&mut hasher);
                hasher.finish()
            };

            if let Some(cached) = self.shape_cache.get(&key) {
                return (**cached).clone();
            }

            let shape = self
                .drive_pipeline(text, font_size, &font_handle, bold, italic, fonts, images)
                .unwrap_or(LineShape {
                    glyphs: Vec::new(),
                    width: 0.0,
                    font_size,
                });

            if self.shape_cache_order.len() >= self.shape_cache_capacity {
                if let Some(victim) = self.shape_cache_order.pop_front() {
                    self.shape_cache.remove(&victim);
                }
            }
            let arc = Arc::new(shape);
            self.shape_cache.insert(key, arc.clone());
            self.shape_cache_order.push_back(key);
            (*arc).clone()
        }

        /// Drive Bevy's `TextPipeline` for a single span and translate the
        /// resulting `TextLayoutInfo` into a `LineShape` (logical-pixel coords).
        #[allow(clippy::too_many_arguments)]
        fn drive_pipeline(
            &mut self,
            text: &str,
            font_size: f32,
            font_handle: &Handle<Font>,
            bold: bool,
            italic: bool,
            fonts: &Assets<Font>,
            images: &mut Assets<Image>,
        ) -> Option<LineShape> {
            if text.is_empty() {
                return Some(LineShape {
                    glyphs: Vec::new(),
                    width: 0.0,
                    font_size,
                });
            }

            let scale = self.raster_scale;
            let mut text_font = TextFont {
                font: FontSource::Handle(font_handle.clone()),
                font_size: FontSize::Px(font_size),
                ..default()
            };
            if bold {
                text_font.weight = bevy::text::FontWeight(700);
            }
            if italic {
                text_font.style = bevy::text::FontStyle::Italic;
            }

            // Bevy requires the resolved font asset to exist; bail to the
            // fallback shape (empty) if it's not loaded yet.
            fonts.get(font_handle.id())?;

            let entity = Entity::PLACEHOLDER;
            let spans = core::iter::once((
                entity,
                0usize,
                text,
                &text_font,
                Color::WHITE,
                LineHeight::default(),
                LetterSpacing::default(),
            ));

            self.text_pipeline
                .update_buffer(
                    fonts,
                    spans,
                    LineBreak::NoWrap,
                    Justify::Left,
                    TextBounds::UNBOUNDED,
                    scale,
                    &mut self.computed,
                    &mut self.font_cx,
                    &mut self.layout_cx,
                    Vec2::ZERO,
                    16.0,
                )
                .ok()?;

            self.text_pipeline
                .update_text_layout_info(
                    &mut self.layout_info,
                    &mut self.font_atlas_set,
                    images,
                    &mut self.computed,
                    &mut self.scale_cx,
                    TextBounds::UNBOUNDED,
                    Justify::Left,
                    FontHinting::default(),
                )
                .ok()?;

            self.dirty = true;

            let inv = 1.0 / scale;
            let mut glyphs: Vec<ShapedGlyph> = Vec::with_capacity(self.layout_info.glyphs.len());
            // Resolve atlas image dimensions once per texture.
            let mut tex_dims: HashMap<AssetId<Image>, Vec2> = HashMap::new();

            for g in &self.layout_info.glyphs {
                let atlas = &g.atlas_info;
                let dims = *tex_dims.entry(atlas.texture).or_insert_with(|| {
                    images
                        .get(atlas.texture)
                        .map(|i| {
                            let s = i.size();
                            Vec2::new(s.x as f32, s.y as f32)
                        })
                        .unwrap_or(Vec2::ONE)
                });
                let rect = atlas.rect;
                let size_phys = rect.size();
                // Bevy bakes: position = size/2 + glyph_pos + offset.
                // Recover the pen origin (glyph_pos) in physical px.
                let pen_phys = g.position - size_phys / 2.0 - atlas.offset;
                let pen_x_logical = pen_phys.x * inv;

                let info = GlyphInfo {
                    uv_min: Vec2::new(rect.min.x / dims.x, rect.min.y / dims.y),
                    uv_max: Vec2::new(rect.max.x / dims.x, rect.max.y / dims.y),
                    size: size_phys * inv,
                    // Bevy offset = (left, -top); the crate wants (left, top).
                    offset: Vec2::new(atlas.offset.x * inv, -atlas.offset.y * inv),
                    advance: 0.0,
                };

                glyphs.push(ShapedGlyph {
                    x: pen_x_logical,
                    byte_index: 0, // filled below
                    cache_key: GlyphKey {
                        texture: atlas.texture,
                        info,
                    },
                });
            }

            // Bevy's `PositionedGlyph` does not expose a cluster byte index.
            // Glyphs come back in visual (left-to-right for LTR) order, which
            // matches the byte order for the monospace/LTR content this engine
            // targets. Assign byte indices by walking the source string's
            // char boundaries in order. For ligatures/clusters this is an
            // approximation, same as the previous cosmic-text cluster mapping
            // for the common case.
            let char_byte_starts: Vec<usize> = text
                .char_indices()
                .map(|(i, _)| i)
                .chain(core::iter::once(text.len()))
                .collect();
            for (gi, glyph) in glyphs.iter_mut().enumerate() {
                glyph.byte_index = char_byte_starts.get(gi).copied().unwrap_or(text.len());
            }

            let width = self.layout_info.size.x * inv;

            Some(LineShape {
                glyphs,
                width,
                font_size,
            })
        }
    }
}

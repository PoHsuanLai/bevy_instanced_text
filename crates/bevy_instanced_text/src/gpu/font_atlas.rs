//! Glyph atlas: rasterizes glyphs once via cosmic_text and caches them in a GPU texture.

use bevy::asset::{AssetId, RenderAssetUsages};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy::text::Font;
use cosmic_text::{FontSystem, SwashCache};
use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};
use std::sync::Arc;

/// Power of 2 for GPU efficiency.
pub const ATLAS_SIZE: u32 = 2048;

const GLYPH_PADDING: u32 = 2;

/// Fallback supersampling factor when no window scale factor is available
/// (e.g. headless tests). Matches the typical Retina display.
pub const DEFAULT_RASTER_SCALE: f32 = 2.0;

/// Atlas entry for one rasterized glyph: UV rect, size, placement offset, and advance.
#[derive(Clone, Copy, Debug)]
pub struct GlyphInfo {
    /// UV coordinates in the atlas (0.0 to 1.0)
    pub uv_min: Vec2,
    pub uv_max: Vec2,
    /// Size in logical pixels (atlas stores high-res, rendering uses logical size)
    pub size: Vec2,
    /// Offset from the baseline in logical pixels
    pub offset: Vec2,
    pub advance: f32,
}

/// Shelf-based row packing for the atlas.
struct AtlasRow {
    y: u32,
    height: u32,
    x_cursor: u32,
}

/// GPU glyph atlas resource. Rasterizes glyphs once via cosmic-text/swash and
/// caches them in a single `ATLAS_SIZE × ATLAS_SIZE` RGBA texture. Also caches
/// shaped `LineShape`s to avoid repeated cosmic-text shaping on scroll.
#[derive(Resource)]
pub struct GlyphAtlas {
    pub texture: Handle<Image>,
    rows: Vec<AtlasRow>,
    current_y: u32,
    pixels: Vec<u8>,
    pub dirty: bool,
    font_system: FontSystem,
    swash_cache: SwashCache,
    /// `bevy_text::Font` handles registered with the cosmic-text fontdb,
    /// keyed by AssetId so re-registration is a no-op on subsequent frames.
    loaded_fonts: HashMap<AssetId<Font>, cosmic_text::fontdb::ID>,
    /// Cache keyed by cosmic_text CacheKey — populated by `get_or_rasterize_glyph`.
    cache: HashMap<cosmic_text::CacheKey, GlyphInfo>,
    /// Generation counter — incremented on atlas clear for cache invalidation
    pub generation: u64,
    /// Dirty row range for partial texture upload (min_y..max_y in pixels)
    dirty_min_y: u32,
    dirty_max_y: u32,
    /// UV info for a solid white pixel — used for background rectangles
    pub solid_uv: GlyphInfo,
    /// Cache of shaped lines keyed by `(content_hash, font_size_bits, font_id)`.
    /// Cosmic-text's `ShapeLine::new` is ~1 ms per line on a typical line of
    /// code, so for big files (150k+ lines) re-shaping the visible window on
    /// every scroll-driven layout rebuild dominates frame time. Identical
    /// (text, font_size, font_id) tuples produce identical shapes, so a
    /// content-hash cache turns scrolling into a series of hash hits.
    shape_cache: HashMap<u64, Arc<crate::view::glyph::LineShape>>,
    /// FIFO insertion order so we can cap `shape_cache` at `SHAPE_CACHE_CAPACITY`
    /// without an LRU. Workload is "scroll past lines once" — recency vs.
    /// frequency doesn't matter much at this size.
    shape_cache_order: VecDeque<u64>,
    /// Glyph rasterization scale. Tracks the host window's scale_factor so
    /// HiDPI displays get crisp text and 1x displays don't pay 4x atlas cost.
    /// Synced each frame by `sync_atlas_scale` via [`GlyphAtlas::set_raster_scale`].
    raster_scale: f32,
    /// Per-instance cap on `shape_cache`. Set at construction from
    /// Defaults to [`DEFAULT_SHAPE_CACHE_CAPACITY`].
    shape_cache_capacity: usize,
}

/// Default FIFO cap on the shaped-line cache. Override via
/// Override via `Performance::viewport_buffer_lines` on the editor entity.
pub const DEFAULT_SHAPE_CACHE_CAPACITY: usize = 8192;

impl GlyphAtlas {
    pub fn new(images: &mut Assets<Image>) -> Self {
        Self::new_with_capacity(images, DEFAULT_SHAPE_CACHE_CAPACITY)
    }

    /// Construct with a custom shape-cache capacity. Hosts that work in huge
    /// files (250k+ lines) benefit from a larger cap; embedded / chat
    /// scenarios can shrink it.
    pub fn new_with_capacity(images: &mut Assets<Image>, shape_cache_capacity: usize) -> Self {
        let pixels = vec![0u8; (ATLAS_SIZE * ATLAS_SIZE * 4) as usize];

        let image = Image::new(
            Extent3d {
                width: ATLAS_SIZE,
                height: ATLAS_SIZE,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            pixels.clone(),
            TextureFormat::Rgba8UnormSrgb,
            // Keep in both worlds so we can update the data and have it re-upload
            RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
        );

        let texture = images.add(image);

        let font_system = FontSystem::new();
        let swash_cache = SwashCache::new();

        let mut atlas = Self {
            texture,
            rows: Vec::new(),
            current_y: 0,
            pixels,
            dirty: false,
            font_system,
            swash_cache,
            loaded_fonts: HashMap::new(),
            cache: HashMap::new(),
            generation: 0,
            dirty_min_y: ATLAS_SIZE,
            dirty_max_y: 0,
            solid_uv: GlyphInfo {
                uv_min: Vec2::ZERO,
                uv_max: Vec2::ZERO,
                size: Vec2::ONE,
                offset: Vec2::ZERO,
                advance: 0.0,
            },
            shape_cache: HashMap::with_capacity(shape_cache_capacity),
            shape_cache_order: VecDeque::with_capacity(shape_cache_capacity),
            raster_scale: DEFAULT_RASTER_SCALE,
            shape_cache_capacity,
        };

        atlas.reserve_solid_pixel();
        atlas.dirty = true;
        atlas.dirty_min_y = 0;
        atlas.dirty_max_y = 2;

        atlas
    }

    /// Register a `bevy_text::Font` asset's bytes into the cosmic-text font
    /// system on first use; subsequent calls are O(1) cache hits. Returns
    /// the `fontdb::ID` to feed into `shape_line`.
    pub fn ensure_font(
        &mut self,
        handle: &Handle<Font>,
        fonts: &Assets<Font>,
    ) -> Option<cosmic_text::fontdb::ID> {
        let id = handle.id();
        if let Some(font_id) = self.loaded_fonts.get(&id) {
            return Some(*font_id);
        }
        let font = fonts.get(handle)?;
        let bytes: Vec<u8> = (*font.data).clone();
        let db = self.font_system.db_mut();
        let count_before = db.faces().count();
        db.load_font_data(bytes);
        let font_id = db.faces().nth(count_before).map(|f| f.id)?;
        self.loaded_fonts.insert(id, font_id);
        Some(font_id)
    }

    fn allocate(&mut self, width: u32, height: u32) -> Option<(u32, u32)> {
        if width == 0 || height == 0 {
            return Some((0, 0));
        }

        let padded_width = width + GLYPH_PADDING;
        let padded_height = height + GLYPH_PADDING;

        for row in &mut self.rows {
            if row.height >= padded_height && row.x_cursor + padded_width <= ATLAS_SIZE {
                let x = row.x_cursor;
                let y = row.y;
                row.x_cursor += padded_width;
                return Some((x, y));
            }
        }

        if self.current_y + padded_height <= ATLAS_SIZE {
            let y = self.current_y;
            self.current_y += padded_height;
            self.rows.push(AtlasRow {
                y,
                height: padded_height,
                x_cursor: padded_width,
            });
            return Some((0, y));
        }

        None
    }

    /// Partial upload: only the dirty row range.
    pub fn update_texture(&mut self, images: &mut Assets<Image>) {
        if !self.dirty || self.dirty_min_y >= self.dirty_max_y {
            self.dirty = false;
            return;
        }

        let min_y = self.dirty_min_y.min(ATLAS_SIZE);
        let max_y = self.dirty_max_y.min(ATLAS_SIZE);

        if let Some(image) = images.get_mut(&self.texture) {
            if let Some(ref mut data) = image.data {
                let row_bytes = (ATLAS_SIZE * 4) as usize;
                let start_byte = min_y as usize * row_bytes;
                let end_byte = max_y as usize * row_bytes;

                if end_byte <= data.len() && end_byte <= self.pixels.len() {
                    data[start_byte..end_byte].copy_from_slice(&self.pixels[start_byte..end_byte]);
                } else {
                    data.copy_from_slice(&self.pixels);
                }
            }
        } else {
            // No existing image — create fresh (first frame).
            let new_image = Image::new(
                Extent3d {
                    width: ATLAS_SIZE,
                    height: ATLAS_SIZE,
                    depth_or_array_layers: 1,
                },
                TextureDimension::D2,
                self.pixels.clone(),
                TextureFormat::Rgba8UnormSrgb,
                RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
            );
            let _ = images.insert(&self.texture, new_image);
        }

        self.dirty = false;
        self.dirty_min_y = ATLAS_SIZE;
        self.dirty_max_y = 0;
    }

    /// Clear the atlas when font changes or atlas is full.
    pub fn clear(&mut self) {
        self.rows.clear();
        self.current_y = 0;
        self.pixels.fill(0);
        self.dirty = true;
        self.cache.clear();
        self.generation += 1;
        self.dirty_min_y = 0;
        self.dirty_max_y = ATLAS_SIZE;
        self.reserve_solid_pixel();
    }

    /// Current glyph rasterization scale. Glyphs are rasterized at this factor
    /// times the requested font size and rendered at logical (1x) size.
    pub fn raster_scale(&self) -> f32 {
        self.raster_scale
    }

    /// Set the rasterization scale and invalidate everything keyed on it.
    /// `cosmic_text::CacheKey` bakes in the scale via `Glyph::physical`, so
    /// both the glyph atlas cache and the shape cache must be cleared.
    /// No-op if the scale is unchanged.
    pub fn set_raster_scale(&mut self, scale: f32) {
        let scale = scale.max(0.1);
        if (self.raster_scale - scale).abs() < f32::EPSILON {
            return;
        }
        self.raster_scale = scale;
        self.shape_cache.clear();
        self.shape_cache_order.clear();
        self.clear();
    }

    /// Reserve a 2×2 white pixel region for solid-fill backgrounds.
    fn reserve_solid_pixel(&mut self) {
        if let Some((sx, sy)) = self.allocate(2, 2) {
            for dy in 0..2u32 {
                for dx in 0..2u32 {
                    let idx = (((sy + dy) * ATLAS_SIZE + sx + dx) * 4) as usize;
                    self.pixels[idx] = 255;
                    self.pixels[idx + 1] = 255;
                    self.pixels[idx + 2] = 255;
                    self.pixels[idx + 3] = 255;
                }
            }
            self.solid_uv = GlyphInfo {
                uv_min: Vec2::new(
                    (sx as f32 + 0.5) / ATLAS_SIZE as f32,
                    (sy as f32 + 0.5) / ATLAS_SIZE as f32,
                ),
                uv_max: Vec2::new(
                    (sx as f32 + 1.5) / ATLAS_SIZE as f32,
                    (sy as f32 + 1.5) / ATLAS_SIZE as f32,
                ),
                size: Vec2::ONE,
                offset: Vec2::ZERO,
                advance: 0.0,
            };
        }
    }

    /// Pre-rasterize a batch of `cosmic_text::CacheKey`s into the atlas,
    /// ignoring the result. Used by `display_map` to warm the atlas before
    /// the renderer runs, so the renderer's paint pass never triggers
    /// mid-frame texture uploads. Cache hits are O(1) and skip the work.
    pub fn ensure_glyphs<I: IntoIterator<Item = cosmic_text::CacheKey>>(&mut self, keys: I) {
        for key in keys {
            if self.cache.contains_key(&key) {
                continue;
            }
            // Drop the result; we just need the side effect of insertion.
            let _ = self.get_or_rasterize_glyph(key);
        }
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
    fonts: Res<Assets<Font>>,
    windows: Query<&bevy::window::Window, With<bevy::window::PrimaryWindow>>,
) {
    let mut atlas = GlyphAtlas::new(&mut images);
    if let Ok(window) = windows.single() {
        atlas.set_raster_scale(window.scale_factor());
    }
    // Seed the atlas's private `FontSystem` with whichever font `bevy_text`
    // registered at `Handle::default()` (its bundled `FiraMono-subset.ttf`
    // when the `default_font` feature is on). cosmic-text's `ShapeLine::new`
    // panics with `"no default font found"` if the system has zero faces —
    // on native this never bites because `FontSystem::new()` scans the OS,
    // but `wasm32-unknown-unknown` ships no fonts and the very first shape
    // crashes before any consumer-supplied font asset finishes loading.
    let default_handle: Handle<Font> = Handle::default();
    atlas.ensure_font(&default_handle, &fonts);
    commands.insert_resource(atlas);
}

/// Mirror Bevy's own text pipeline (PR #16264): every frame, compare the
/// primary window's `scale_factor` to the atlas's cached value and
/// re-rasterize on mismatch. The setter short-circuits when stable.
fn sync_atlas_scale(
    atlas: Option<ResMut<GlyphAtlas>>,
    windows: Query<&bevy::window::Window, With<bevy::window::PrimaryWindow>>,
) {
    let Some(mut atlas) = atlas else { return };
    let Ok(window) = windows.single() else { return };
    atlas.set_raster_scale(window.scale_factor());
}

pub use instanced_extensions::*;

mod instanced_extensions {
    use super::*;
    use cosmic_text::{Attrs, AttrsList, ShapeBuffer, ShapeLine, Shaping};

    #[derive(Clone, Copy, Debug)]
    pub struct PlacementInfo {
        pub left: f32,
        pub top: f32,
    }

    impl GlyphAtlas {
        pub(crate) fn pack(&mut self, width: u32, height: u32) -> Option<(u32, u32)> {
            self.allocate(width, height)
        }

        pub(crate) fn write_glyph_data(
            &mut self,
            x: u32,
            y: u32,
            width: u32,
            height: u32,
            data: &[u8],
        ) {
            if width == 0 || height == 0 {
                return;
            }

            for gy in 0..height {
                for gx in 0..width {
                    let src_idx = ((gy * width + gx) * 4) as usize;
                    let dst_x = x + gx;
                    let dst_y = y + gy;
                    let dst_idx = ((dst_y * ATLAS_SIZE + dst_x) * 4) as usize;

                    if dst_idx + 3 < self.pixels.len() && src_idx + 3 < data.len() {
                        self.pixels[dst_idx] = data[src_idx];
                        self.pixels[dst_idx + 1] = data[src_idx + 1];
                        self.pixels[dst_idx + 2] = data[src_idx + 2];
                        self.pixels[dst_idx + 3] = data[src_idx + 3];
                    }
                }
            }

            self.dirty_min_y = self.dirty_min_y.min(y);
            self.dirty_max_y = self.dirty_max_y.max(y + height);
            self.dirty = true;
        }

        /// Shape a line into the engine's owned `LineShape`. Pass a
        /// `fontdb::ID` to pin shaping to a specific face (e.g. one
        /// returned by [`GlyphAtlas::ensure_font`]); pass `None` to use
        /// the constructor's `font_path` font, falling back to system fonts.
        ///
        /// Cached: identical `(text, font_size, font_id)` triples reuse the
        /// previously shaped result. Cosmic-text's `ShapeLine::new` runs full
        /// BiDi/script analysis (~1 ms per line of code) so re-shaping the
        /// same line every scroll-driven layout rebuild dominates frame time
        /// on big files. The cache turns scroll into a series of hash hits.
        pub fn shape_line(
            &mut self,
            text: &str,
            font_size: f32,
            font_id: Option<cosmic_text::fontdb::ID>,
        ) -> crate::view::glyph::LineShape {
            self.shape_line_styled(text, font_size, font_id, false, false)
        }

        pub fn shape_line_styled(
            &mut self,
            text: &str,
            font_size: f32,
            font_id: Option<cosmic_text::fontdb::ID>,
            bold: bool,
            italic: bool,
        ) -> crate::view::glyph::LineShape {
            use crate::view::glyph::{LineShape, ShapedGlyph};

            let pinned = font_id;

            let key = {
                let mut hasher = std::collections::hash_map::DefaultHasher::new();
                text.hash(&mut hasher);
                font_size.to_bits().hash(&mut hasher);
                pinned.hash(&mut hasher);
                bold.hash(&mut hasher);
                italic.hash(&mut hasher);
                hasher.finish()
            };

            if let Some(cached) = self.shape_cache.get(&key) {
                let _hit = bevy::prelude::info_span!("shape_line_hit").entered();
                return (**cached).clone();
            }
            let _miss = bevy::prelude::info_span!("shape_line_miss").entered();

            let mut attrs = Attrs::new();
            let pinned_family = pinned.and_then(|id| {
                self.font_system
                    .db()
                    .face(id)
                    .and_then(|f| f.families.first().map(|fam| fam.0.clone()))
            });
            if let Some(ref family) = pinned_family {
                attrs = attrs.family(cosmic_text::Family::Name(family.as_str()));
            }
            if bold {
                attrs = attrs.weight(cosmic_text::fontdb::Weight::BOLD);
            }
            if italic {
                attrs = attrs.style(cosmic_text::Style::Italic);
            }
            let attrs_list = AttrsList::new(attrs);

            let line = ShapeLine::new(
                &mut self.font_system,
                text,
                &attrs_list,
                Shaping::Advanced,
                4,
            );

            let mut layout_lines = Vec::with_capacity(1);
            let mut scratch = ShapeBuffer::default();

            line.layout_to_buffer(
                &mut scratch,
                font_size,
                None,
                cosmic_text::Wrap::None,
                None,
                &mut layout_lines,
                None,
            );

            let shape = if layout_lines.is_empty() {
                LineShape {
                    glyphs: Vec::new(),
                    width: 0.0,
                    font_size,
                }
            } else {
                let layout = &layout_lines[0];
                let mut glyphs = Vec::with_capacity(layout.glyphs.len());
                let scale = self.raster_scale;
                for g in &layout.glyphs {
                    let physical = g.physical((0.0, 0.0), scale);
                    glyphs.push(ShapedGlyph {
                        x: g.x,
                        byte_index: g.start,
                        cache_key: physical.cache_key,
                    });
                }
                LineShape {
                    glyphs,
                    width: layout.w,
                    font_size,
                }
            };

            // Insert + FIFO bound. Empty lines get cached too (pre-formatted
            // empty `LineShape` is cheap, lookup is still a win).
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

        pub fn get_or_rasterize_glyph(
            &mut self,
            cache_key: cosmic_text::CacheKey,
        ) -> Option<(GlyphInfo, PlacementInfo)> {
            use swash::scale::image::Content;

            // Check cache first. `PlacementInfo` is reconstructed from
            // `GlyphInfo.offset` (which already stores left/top in logical
            // pixels), so we don't need to cache it separately.
            if let Some(info) = self.cache.get(&cache_key) {
                let placement = PlacementInfo {
                    left: info.offset.x,
                    top: info.offset.y,
                };
                return Some((*info, placement));
            }

            let image = self
                .swash_cache
                .get_image(&mut self.font_system, cache_key)
                .clone()?;

            if image.placement.width == 0 || image.placement.height == 0 {
                return None;
            }

            let width = image.placement.width as usize;
            let height = image.placement.height as usize;

            let mut rgba_data = Vec::with_capacity(width * height * 4);
            match image.content {
                Content::Mask => {
                    for &alpha in &image.data {
                        rgba_data.extend_from_slice(&[255, 255, 255, alpha]);
                    }
                }
                Content::SubpixelMask | Content::Color => {
                    rgba_data.extend_from_slice(&image.data);
                }
            }

            // Pack into atlas, with generation-based recovery on full
            let pack_result = self.pack(width as u32, height as u32).or_else(|| {
                warn!(
                    "Glyph atlas full in get_or_rasterize_glyph, clearing (generation {})",
                    self.generation
                );
                self.clear();
                self.pack(width as u32, height as u32)
            });
            if let Some((x, y)) = pack_result {
                self.write_glyph_data(x, y, width as u32, height as u32, &rgba_data);

                let glyph_info = GlyphInfo {
                    uv_min: Vec2::new(x as f32 / ATLAS_SIZE as f32, y as f32 / ATLAS_SIZE as f32),
                    uv_max: Vec2::new(
                        (x + width as u32) as f32 / ATLAS_SIZE as f32,
                        (y + height as u32) as f32 / ATLAS_SIZE as f32,
                    ),
                    size: Vec2::new(
                        width as f32 / self.raster_scale,
                        height as f32 / self.raster_scale,
                    ),
                    offset: Vec2::new(
                        image.placement.left as f32 / self.raster_scale,
                        image.placement.top as f32 / self.raster_scale,
                    ),
                    advance: 0.0,
                };

                let placement = PlacementInfo {
                    left: image.placement.left as f32 / self.raster_scale,
                    top: image.placement.top as f32 / self.raster_scale,
                };

                self.cache.insert(cache_key, glyph_info);

                Some((glyph_info, placement))
            } else {
                warn!("Atlas full! Cannot pack glyph {}x{}", width, height);
                None
            }
        }
    }
}

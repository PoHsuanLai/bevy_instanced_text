//! Generic text view rendering — produces `GlyphInstance` batches from a
//! `DisplayLayout` plus below/above overlay slices.
//!
//! Pure function over an immutable snapshot: no cursors, selections, syntax
//! highlighting, or other editor concepts here. The producer (`display_map`)
//! has already shaped each visible row through Bevy's text pipeline and resolved
//! per-line/per-run colors into the `ShapedLine.runs`. This module turns
//! that into per-glyph quads ready for the instanced pipeline.

use std::collections::HashMap;
use std::sync::Arc;

use bevy::asset::AssetId;
use bevy::prelude::*;

use crate::gpu::GlyphAtlas;

use super::font::FontSynthesis;
use super::glyph::{ShapedLine, TextDecoration, TextFormat};
use super::overlay::RectOverlay;
use super::pipeline::{line_x_at_byte, DisplayLayout};
use bevy::ui::ComputedNode;

/// Resolved font faces for one render call. The renderer picks per-run
/// based on `TextFormat.font_weight` (≥600 ⇒ bold) and `TextFormat.italic`,
/// falling back to `regular` and synthesizing the missing axis when
/// `synthesis` permits.
///
/// Built once per text-view per frame in `update_text_views`
/// from the entity's `TextFont`. Each slot is a `Handle<Font>` that Bevy's
/// text pipeline resolves at shape time.
#[derive(Clone, Debug, Default)]
pub struct FontFaces {
    pub regular: Option<bevy::asset::Handle<bevy::text::Font>>,
    pub bold: Option<bevy::asset::Handle<bevy::text::Font>>,
    pub italic: Option<bevy::asset::Handle<bevy::text::Font>>,
    pub bold_italic: Option<bevy::asset::Handle<bevy::text::Font>>,
    pub synthesis: FontSynthesis,
}

impl FontFaces {
    /// Single-face shorthand. Bold/italic slots empty, synthesis on.
    /// Used by trivial-layout consumers that don't care about styling.
    pub fn single(regular: Option<bevy::asset::Handle<bevy::text::Font>>) -> Self {
        Self {
            regular,
            bold: None,
            italic: None,
            bold_italic: None,
            synthesis: FontSynthesis::default(),
        }
    }

    /// Resolve a face for a `(bold, italic)` request. Returns the matching
    /// loaded face when available, else falls back to the closest loaded
    /// face on the requested axis (bold-italic → bold → regular, etc.).
    fn pick(&self, bold: bool, italic: bool) -> Option<bevy::asset::Handle<bevy::text::Font>> {
        match (bold, italic) {
            (true, true) => self
                .bold_italic
                .clone()
                .or_else(|| self.bold.clone())
                .or_else(|| self.italic.clone())
                .or_else(|| self.regular.clone()),
            (true, false) => self.bold.clone().or_else(|| self.regular.clone()),
            (false, true) => self.italic.clone().or_else(|| self.regular.clone()),
            (false, false) => self.regular.clone(),
        }
    }

    /// Whether this `(bold, italic)` request needs synthesis: weight when
    /// the bold slot is empty, style when the italic slot is empty.
    fn needs_synth(&self, bold: bool, italic: bool) -> (bool, bool) {
        let bold_synth = bold
            && self.synthesis.weight
            && match (bold, italic) {
                (true, true) => self.bold_italic.is_none() && self.bold.is_none(),
                (true, false) => self.bold.is_none(),
                _ => false,
            };
        let italic_synth = italic
            && self.synthesis.style
            && match (bold, italic) {
                (true, true) => self.bold_italic.is_none() && self.italic.is_none(),
                (false, true) => self.italic.is_none(),
                _ => false,
            };
        (bold_synth, italic_synth)
    }
}

/// Map a `TextFormat.font_weight` to "bold or not". CSS-style threshold:
/// `>= 600` is bold (semibold and above).
fn run_is_bold(run: &TextFormat) -> bool {
    matches!(run.font_weight, Some(w) if w >= 600)
}

/// Glyph instance data for GPU rendering. `pub` for test inspection in downstream crates.
///
/// `corner_radii` carries per-corner radii `[tl, tr, bl, br]`. Background
/// rects use this for asymmetric rounding (e.g. multi-row code-block
/// panels — first row rounds top corners, last row rounds bottom).
/// Glyphs leave it `[0.0; 4]` since text never rounds. Layout is sized
/// to match the WGSL vertex inputs in `text.wgsl`.
#[derive(Clone, Copy, Debug, Default, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
pub struct GlyphInstance {
    pub position: Vec2,
    pub uv_min: Vec2,
    pub uv_max: Vec2,
    pub size: Vec2,
    pub color: [f32; 4],
    /// Per-corner radii in pixels: `[top_left, top_right, bottom_left, bottom_right]`.
    /// `[0.0; 4]` = sharp.
    pub corner_radii: [f32; 4],
    pub z_index: f32,
    /// Horizontal skew factor for italic simulation (0.0 = normal, ~0.2 = italic)
    pub skew: f32,
    /// Pad to 16-byte alignment so `array_stride` matches WebGPU's
    /// vertex buffer requirements.
    pub _padding: [f32; 2],
}

#[derive(Component, Clone)]
pub struct TextViewBatch {
    pub built_at_scroll: f32,
    pub built_at_horizontal_scroll: f32,
    pub first_line: usize,
    pub last_line: usize,
    pub built_at_width: u32,
    pub built_at_height: u32,
}

/// Per-`TextView` render component holding the glyph instance buffer for the
/// current frame. Written by `update_text_views`; consumed by the GPU pipeline.
#[derive(Component, Clone)]
pub struct GlyphBatchComponent {
    pub instances: Vec<GlyphInstance>,
    pub atlas_texture: Handle<Image>,
    /// Which render layer this batch belongs to (for multi-viewport filtering).
    /// None = layer 0 (default).
    pub render_layer: Option<u8>,
}

/// Per-batch affine + clip + UI camera routing for one `TextView`.
///
/// The affine maps node-local logical px → screen physical px and is
/// composed each frame in `update_text_views`. `clip` mirrors Bevy UI's
/// `CalculatedClip`. `stack_index` and `target_camera` mirror
/// `ComputedNode::stack_index` / `ComputedUiTargetCamera` on the
/// `TextView`.
#[derive(Component, Clone, Copy, Debug)]
pub struct BatchTransform {
    /// 2×3 affine, row-major: `[m00, m01, t0, m10, m11, t1]`.
    pub affine: [f32; 6],
    pub clip: Option<Rect>,
    pub stack_index: u32,
    pub target_camera: Option<Entity>,
    /// Sub-ordering within a single text view's `stack_index`. One text view
    /// now emits several batch entities (one per atlas texture × painter
    /// tier), all sharing the same `stack_index`. This monotonically
    /// increasing offset (assigned in draw order: below → text → above) is
    /// folded into the GPU sort key so the sub-batches keep their relative
    /// layering regardless of ECS query order. Kept tiny (≪ 1.0) so it never
    /// crosses into an adjacent UI element's stack index.
    pub sub_order: f32,
}

pub struct RenderContext {
    pub content_start_x: f32,
    /// Padding-right inset (in logical px); used by `Justify::Right`/`Center`
    /// to stop before the right padding boundary.
    pub content_end_inset_x: f32,
    pub horizontal_scroll_offset: f32,
    /// Logical font size in pixels (resolved from `TextFont.font_size`).
    pub font_size: f32,
    pub faces: FontFaces,
    pub justify: bevy::text::Justify,
}

/// The painter's tier a [`GlyphBatch`] belongs to. The renderer layers
/// `Below` (selection/line backgrounds) under `Text` (glyphs + per-run
/// backgrounds + decorations) under `Above` (carets, brackets). Sub-batches
/// must preserve this relative ordering even when each tier spans several
/// atlas textures, so the consumer drives the GPU sort key off this rank.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BatchTier {
    Below,
    Text,
    Above,
}

impl BatchTier {
    /// Monotonic rank used to order tiers when composing the final draw
    /// sequence: lower draws first (further back).
    pub fn rank(self) -> u8 {
        match self {
            BatchTier::Below => 0,
            BatchTier::Text => 1,
            BatchTier::Above => 2,
        }
    }
}

/// One sub-batch: a run of [`GlyphInstance`]s that all sample a single atlas
/// texture, tagged with the painter's [`BatchTier`] they belong to.
///
/// Bevy 0.19's `FontAtlasSet` shards glyphs into a separate texture per
/// `(font, size, …)`, so a view that mixes sizes (headings, `font_scale`
/// runs) produces glyphs across several textures. Each distinct texture
/// becomes its own batch — one bound texture, one draw — so every glyph
/// samples the texture it was actually rasterized into.
pub struct GlyphBatch {
    pub tier: BatchTier,
    pub texture: Handle<Image>,
    pub instances: Vec<GlyphInstance>,
}

/// Output of [`render_layout`]: the glyph + overlay instances split into
/// per-texture sub-batches.
///
/// Each [`GlyphBatch`] binds exactly one atlas texture. Solid quads
/// (selection / line / run backgrounds, overlays, decorations) form their
/// own batch bound to the atlas's tiny solid texture; glyph quads are
/// grouped by the `FontAtlasSet` texture they were rasterized into. The
/// batches are returned in draw order: all `Below` first, then `Text`,
/// then `Above`.
pub struct RenderOutput {
    pub batches: Vec<GlyphBatch>,
}

/// Per-texture instance buckets for one painter's tier.
///
/// Glyph instances are grouped by the `AssetId<Image>` of the atlas texture
/// they sample. Solid quads (backgrounds, overlays, decorations) all land in
/// the bucket keyed by the atlas's solid texture. Each bucket carries the
/// strong `Handle<Image>` so the final batch can bind it. Insertion order is
/// preserved (`order`) so the painter's sequence within a tier is stable —
/// the solid texture is inserted first by the caller so backgrounds emitted
/// before glyphs keep drawing under them.
#[derive(Default)]
struct TierBucket {
    by_texture: HashMap<AssetId<Image>, (Handle<Image>, Vec<GlyphInstance>)>,
    order: Vec<AssetId<Image>>,
}

impl TierBucket {
    /// Append `inst` to the bucket for `texture`, registering the texture's
    /// strong handle on first sight.
    fn push(&mut self, texture: AssetId<Image>, handle: &Handle<Image>, inst: GlyphInstance) {
        let entry = self.by_texture.entry(texture).or_insert_with(|| {
            self.order.push(texture);
            (handle.clone(), Vec::new())
        });
        entry.1.push(inst);
    }

    /// Drain into `GlyphBatch`es tagged with `tier`, preserving insertion order
    /// and dropping empty buckets.
    fn into_batches(self, tier: BatchTier, out: &mut Vec<GlyphBatch>) {
        let mut by_texture = self.by_texture;
        for id in self.order {
            if let Some((handle, instances)) = by_texture.remove(&id) {
                if !instances.is_empty() {
                    out.push(GlyphBatch {
                        tier,
                        texture: handle,
                        instances,
                    });
                }
            }
        }
    }
}

/// Render a `DisplayLayout` into glyph instances.
///
/// `below` is painted before glyphs (selections, backgrounds); `above` after (carets, brackets).
#[allow(clippy::too_many_arguments)]
pub fn render_layout(
    layout: &DisplayLayout,
    below: &[RectOverlay],
    above: &[RectOverlay],
    viewport: &ComputedNode,
    atlas: &mut GlyphAtlas,
    fonts: &bevy::asset::Assets<bevy::text::Font>,
    images: &mut bevy::asset::Assets<Image>,
    ctx: RenderContext,
) -> RenderOutput {
    let RenderContext {
        content_start_x,
        content_end_inset_x,
        horizontal_scroll_offset,
        font_size,
        faces,
        justify,
    } = ctx;
    let default_line_height = layout.line_height;
    let char_width = layout.char_width;
    let baseline_offset = layout.baseline_offset;

    // Emit in node-local logical px, top-left origin, +Y down — the
    // shader composes with the per-batch affine to land on screen px.
    let inv = viewport.inverse_scale_factor();
    let logical = viewport.size() * inv;
    let content_width = logical.x - content_start_x - content_end_inset_x;
    let base_line_start_x = content_start_x - horizontal_scroll_offset;

    // Single anchor: `line.y_top` is the row's visual top in screen pixels.
    //   - Glyph baseline = y_top + line_height/2 + baseline_offset (legacy ascent ratio).
    //   - Overlay full-line rect spans y_top..y_top+line_height.
    //   - Sub-line overlays (`y_range: Some(0..2)`) are y_top-relative.
    // Per-row overrides: `ShapedLine.line_height = Some(h)` lets a row paint
    // taller / shorter than the layout default (markdown headings, code blocks).

    // Per-tier, per-texture buckets. Seed each tier's solid texture first so
    // that solid quads (backgrounds, overlays, decorations) draw under the
    // glyphs emitted into the same tier and the solid group stays at a stable
    // position in the painter's sequence.
    let solid_uv = atlas.solid_uv;
    let solid_id = atlas.texture.id();
    let solid_handle = atlas.texture.clone();
    let mut text_bucket = TierBucket::default();
    let mut below_bucket = TierBucket::default();
    let mut above_bucket = TierBucket::default();

    // Below-text overlays first (selection bg, line highlight)
    for rect in below {
        push_overlay_quad(
            rect,
            layout,
            base_line_start_x,
            logical.x,
            solid_uv,
            solid_id,
            &solid_handle,
            &mut below_bucket,
        );
    }

    // Glyphs and per-line/per-run backgrounds
    for line in layout.lines.iter() {
        let line_height = line.line_height.unwrap_or(default_line_height);
        // Glyph baseline derived from row top.
        let base_y = line.y_top + line_height * 0.5 + baseline_offset;
        let line_width = line
            .shape
            .as_ref()
            .map_or(line.text.len() as f32 * char_width, |s| s.width);
        let justify_offset = match justify {
            // `Start`/`Justified` treated as left; RTL not handled by this engine.
            bevy::text::Justify::Left
            | bevy::text::Justify::Start
            | bevy::text::Justify::Justified => 0.0,
            bevy::text::Justify::Center => (content_width - line_width).max(0.0) * 0.5,
            bevy::text::Justify::Right | bevy::text::Justify::End => {
                (content_width - line_width).max(0.0)
            }
        };
        let line_x = base_line_start_x + line.x_offset + justify_offset;

        // Line background (full-width quad) — full row, top-anchored on y_top.
        if let Some(bg) = line.line_bg {
            let margin = content_start_x;
            let bg_pad = if line.x_offset > 0.0 {
                char_width * 1.5
            } else {
                0.0
            };
            let bg_x_start = (line.x_offset - bg_pad).max(0.0);
            // Pick the largest corner radius among runs sharing this background.
            let line_corner_radius = line
                .runs
                .iter()
                .filter(|r| r.bg == Some(bg))
                .map(|r| r.corner_radius)
                .fold(0.0_f32, f32::max);
            below_bucket.push(
                solid_id,
                &solid_handle,
                GlyphInstance {
                    position: Vec2::new(margin + bg_x_start, line.y_top),
                    uv_min: solid_uv.uv_min,
                    uv_max: solid_uv.uv_max,
                    size: Vec2::new(logical.x - margin * 2.0 - bg_x_start, line_height),
                    color: linear_rgba(bg),
                    z_index: 0.0,
                    corner_radii: [line_corner_radius; 4],
                    skew: 0.0,
                    _padding: [0.0; 2],
                },
            );
        }

        // Pre-compose the row anchor; threads through every emitter call.
        let anchor = RowAnchor { line_x, base_y };

        // Shape-driven path is used when the line carries a `LineShape` shaped
        // at this font_size, no run wants a font_scale override, and no run
        // requests a non-regular face. Bold / italic runs need re-shaping
        // against the matching face — they fall through to the unshaped
        // path which picks the right `font_id` per run.
        let shape_usable = line
            .shape
            .as_ref()
            .filter(|s| (s.font_size - font_size).abs() < f32::EPSILON)
            .filter(|_| {
                line.runs.iter().all(|r| {
                    (r.font_scale == 0.0 || r.font_scale == 1.0) && !run_is_bold(r) && !r.italic
                })
            });

        if line.runs.is_empty() {
            let style = RunStyle {
                color: linear_rgba(layout.default_fg),
                skew: 0.0,
                stroke_offset_px: 0.0,
            };
            if let Some(shape) = shape_usable {
                emit_shaped_run_glyphs(
                    &shape.glyphs,
                    0..line.text.len(),
                    anchor,
                    style,
                    atlas,
                    &mut text_bucket,
                );
            } else {
                emit_unshaped_run_glyphs(
                    line,
                    0..line.text.len(),
                    RunPaint {
                        anchor,
                        style,
                        face: RunFace {
                            id: faces.regular.clone(),
                            bold: false,
                            italic: false,
                        },
                        seg_x_start: 0.0,
                        seg_font_size: font_size,
                    },
                    atlas,
                    fonts,
                    images,
                    &mut text_bucket,
                );
            }
        } else {
            for run in &line.runs {
                if line.text.get(run.byte_range.clone()).is_none() {
                    continue;
                }

                let seg_x_start = line_byte_to_x(line, run.byte_range.start, char_width);
                let seg_x_end = line_byte_to_x(line, run.byte_range.end, char_width);

                let bold = run_is_bold(run);
                let italic = run.italic;
                let (synth_bold, synth_italic) = faces.needs_synth(bold, italic);
                let face = RunFace {
                    id: if let Some(ref handle) = run.font {
                        Some(handle.clone())
                    } else {
                        faces.pick(bold, italic)
                    },
                    bold,
                    italic,
                };

                // Synthesis: if no italic face is loaded, use the run's
                // explicit skew or apply the synthetic-italic default.
                // Bold synthesis is a glyph-level stroke double.
                let effective_skew = if synth_italic && run.skew == 0.0 {
                    faces.synthesis.italic_skew
                } else {
                    run.skew
                };
                let style = RunStyle {
                    color: linear_rgba(run.fg),
                    skew: effective_skew,
                    stroke_offset_px: if synth_bold {
                        faces.synthesis.bold_stroke_px
                    } else {
                        0.0
                    },
                };
                let seg_font_size = if run.font_scale > 0.0 {
                    font_size * run.font_scale
                } else {
                    font_size
                };
                let paint = RunPaint {
                    anchor,
                    style,
                    face,
                    seg_x_start,
                    seg_font_size,
                };

                // For runs with a background, emit the bg and the glyphs
                // from the same shape so the bg can size itself to the
                // glyphs' actual ink bounds (not the advance edges, which
                // include trailing side-bearing whitespace). For runs
                // without a bg, take the cheap shaped-or-unshaped path.
                if let Some(bg) = run.bg {
                    if line.line_bg != Some(bg) {
                        emit_run_with_bg(
                            line,
                            run,
                            paint,
                            BgBand {
                                seg_x_end,
                                line_height,
                                baseline_offset,
                                color: bg,
                            },
                            atlas,
                            fonts,
                            images,
                            shape_usable,
                            RenderTargets {
                                below: &mut below_bucket,
                                text: &mut text_bucket,
                            },
                        );
                    } else {
                        emit_run_glyphs_only(
                            line,
                            run,
                            paint,
                            atlas,
                            fonts,
                            images,
                            shape_usable,
                            &mut text_bucket,
                        );
                    }
                } else {
                    emit_run_glyphs_only(
                        line,
                        run,
                        paint,
                        atlas,
                        fonts,
                        images,
                        shape_usable,
                        &mut text_bucket,
                    );
                }

                if !run.decoration.is_empty() {
                    emit_run_decoration(
                        run.decoration,
                        run.fg,
                        RowBand {
                            anchor,
                            line_height,
                            baseline_offset,
                            x_start: seg_x_start,
                            x_end: seg_x_end,
                        },
                        solid_uv,
                        solid_id,
                        &solid_handle,
                        &mut text_bucket,
                    );
                }
            }
        }
    }

    // Above-text overlays (carets, brackets)
    for rect in above {
        push_overlay_quad(
            rect,
            layout,
            base_line_start_x,
            logical.x,
            solid_uv,
            solid_id,
            &solid_handle,
            &mut above_bucket,
        );
    }

    // Painter's order: all below batches → all text batches → all above
    // batches. Each tier may span several atlas textures; ordering the tiers
    // here keeps backgrounds under text and overlays/carets over it even
    // though they are now separate per-texture draws.
    let mut batches = Vec::new();
    below_bucket.into_batches(BatchTier::Below, &mut batches);
    text_bucket.into_batches(BatchTier::Text, &mut batches);
    above_bucket.into_batches(BatchTier::Above, &mut batches);

    RenderOutput { batches }
}

/// Pixel offset of `byte` within `line.text`. Uses shaped advances when present
/// and falls back to a `char_width` walk otherwise. Tab handling matches the
/// shaper's behavior (it inflates `\t` advance by `tab_width = 4`).
fn line_byte_to_x(line: &ShapedLine, byte: usize, char_width: f32) -> f32 {
    line_x_at_byte(line, byte, char_width)
}

/// Where a row paints in node-local logical px. Threaded through the
/// glyph and quad emitters so they share one anchor.
#[derive(Clone, Copy)]
struct RowAnchor {
    /// Pen origin X, already including `text_area_left + scroll + line.x_offset`.
    line_x: f32,
    /// Glyph baseline Y.
    base_y: f32,
}

/// Per-run paint attributes. `color` is pre-linearized. `skew` carries
/// italic simulation. `stroke_offset_px > 0.0` triggers synthetic bold:
/// each glyph is drawn twice with this x-offset.
#[derive(Clone, Copy)]
struct RunStyle {
    color: [f32; 4],
    skew: f32,
    stroke_offset_px: f32,
}

/// Build a glyph quad from an atlas hit. Emits the glyph's top-left
/// corner in node-local logical pixels (+Y down).
fn glyph_quad(
    info: crate::gpu::GlyphInfo,
    pen_x: f32,
    anchor: RowAnchor,
    style: RunStyle,
) -> GlyphInstance {
    let node_x = anchor.line_x + pen_x + info.offset.x;
    let node_y = anchor.base_y - info.offset.y;
    GlyphInstance {
        position: Vec2::new(node_x, node_y),
        uv_min: info.uv_min,
        uv_max: info.uv_max,
        size: info.size,
        color: style.color,
        z_index: 0.0,
        corner_radii: [0.0; 4],
        skew: style.skew,
        _padding: [0.0; 2],
    }
}

/// Emit a glyph (and its synthetic-bold twin if requested) into the bucket for
/// its atlas `texture`. `handle` is the texture's strong handle.
fn push_glyph(
    info: crate::gpu::GlyphInfo,
    texture: AssetId<Image>,
    handle: &Handle<Image>,
    pen_x: f32,
    anchor: RowAnchor,
    style: RunStyle,
    out: &mut TierBucket,
) {
    out.push(texture, handle, glyph_quad(info, pen_x, anchor, style));
    if style.stroke_offset_px > 0.0 {
        out.push(
            texture,
            handle,
            glyph_quad(info, pen_x + style.stroke_offset_px, anchor, style),
        );
    }
}

/// Emit glyphs whose `byte_index` lies inside `range` from a pre-shaped line.
/// `glyphs[i].x` is line-local; `glyph_quad` adds the row anchor.
fn emit_shaped_run_glyphs(
    glyphs: &[super::glyph::ShapedGlyph],
    range: std::ops::Range<usize>,
    anchor: RowAnchor,
    style: RunStyle,
    atlas: &mut GlyphAtlas,
    out: &mut TierBucket,
) {
    for g in glyphs {
        if g.byte_index < range.start || g.byte_index >= range.end {
            continue;
        }
        let texture = g.cache_key.texture;
        let Some((info, _)) = atlas.get_or_rasterize_glyph(g.cache_key) else {
            continue;
        };
        let Some(handle) = atlas.atlas_image_handle(texture) else {
            continue;
        };
        push_glyph(info, texture, &handle, g.x, anchor, style, out);
    }
}

/// The resolved font face for a run plus the synthesis flags the shaper needs.
/// These three always travel together — the face is picked from `bold`/`italic`
/// (or the run's explicit font), and the shaper re-applies the flags to select
/// the matching style within that face. Bundled so the emit functions take one
/// argument instead of three.
#[derive(Clone)]
struct RunFace {
    id: Option<bevy::asset::Handle<bevy::text::Font>>,
    bold: bool,
    italic: bool,
}

/// Everything an emitter needs to paint one run's glyphs: where the row sits
/// (`anchor`), how it looks (`style`), which face to shape with (`face`), and
/// the run's horizontal/size geometry. `seg_font_size` is the rasterizer size
/// (may differ from the line default under a `font_scale` run); `seg_x_start`
/// is the run's pen-x relative to the line origin. Computed once per run and
/// threaded through the emit functions so their signatures stay legible.
#[derive(Clone)]
struct RunPaint {
    anchor: RowAnchor,
    style: RunStyle,
    face: RunFace,
    seg_x_start: f32,
    seg_font_size: f32,
}

/// The vertical band a per-run background quad fills, plus its color. `seg_x_end`
/// closes the advance range opened by [`RunPaint::seg_x_start`]; `line_height`
/// and `baseline_offset` place the band against the glyph row.
#[derive(Clone, Copy)]
struct BgBand {
    seg_x_end: f32,
    line_height: f32,
    baseline_offset: f32,
    color: bevy::prelude::Color,
}

/// The two instance buffers a run can paint into: `below` for backgrounds that
/// sit under the glyphs, `text` for the glyphs themselves. Painter's order is
/// applied later by the caller.
struct RenderTargets<'a> {
    below: &'a mut TierBucket,
    text: &'a mut TierBucket,
}

/// Emit glyphs for a byte range by shaping it on demand. Used when no
/// `LineShape` is attached (`trivial_layout` consumers) or when a
/// `TextFormat.font_scale` override requires re-shaping at a different size.
///
/// Shaping a per-frame slice is cheap — it's the same code path the producer
/// uses, just narrowed to the run. For monospace ASCII it yields advances
/// byte-identical to the old `col * char_width` walk.
fn emit_unshaped_run_glyphs(
    line: &ShapedLine,
    range: std::ops::Range<usize>,
    paint: RunPaint,
    atlas: &mut GlyphAtlas,
    fonts: &bevy::asset::Assets<bevy::text::Font>,
    images: &mut bevy::asset::Assets<Image>,
    out: &mut TierBucket,
) {
    let Some(slice) = line.text.get(range) else {
        return;
    };
    let shape_text = slice.strip_suffix('\n').unwrap_or(slice);
    let shape = atlas.shape_line_styled(
        shape_text,
        paint.seg_font_size,
        paint.face.id.as_ref(),
        paint.face.bold,
        paint.face.italic,
        fonts,
        images,
    );
    for g in &shape.glyphs {
        let texture = g.cache_key.texture;
        let Some((info, _)) = atlas.get_or_rasterize_glyph(g.cache_key) else {
            continue;
        };
        let Some(handle) = atlas.atlas_image_handle(texture) else {
            continue;
        };
        push_glyph(
            info,
            texture,
            &handle,
            paint.seg_x_start + g.x,
            paint.anchor,
            paint.style,
            out,
        );
    }
}

/// Emit a per-run background quad spanning the run's advance bounds, then
/// emit the glyphs. Using advance bounds (not ink bounds) means the chip
/// extends to the same edges as the surrounding text's character cells,
/// matching how browsers render `<code>` — no side-bearing gap on either side.
#[allow(clippy::too_many_arguments)]
fn emit_run_with_bg(
    line: &ShapedLine,
    run: &TextFormat,
    paint: RunPaint,
    band: BgBand,
    atlas: &mut GlyphAtlas,
    fonts: &bevy::asset::Assets<bevy::text::Font>,
    images: &mut bevy::asset::Assets<Image>,
    shape_usable: Option<&Arc<super::glyph::LineShape>>,
    targets: RenderTargets,
) {
    let baseline_y_off = band.line_height * 0.5 + band.baseline_offset;
    let cap_to_descender = baseline_y_off + band.baseline_offset * 0.6;
    let text_band_above = cap_to_descender * 0.25;
    let band_top_y_off = baseline_y_off - text_band_above;
    let bg_w = (band.seg_x_end - paint.seg_x_start).max(0.0);
    let solid_id = atlas.texture.id();
    let solid_handle = atlas.texture.clone();
    // Spans the glyph band, matching `RectOverlay { vertical: Full }`.
    targets.below.push(
        solid_id,
        &solid_handle,
        GlyphInstance {
            position: Vec2::new(
                paint.anchor.line_x + paint.seg_x_start,
                line.y_top + band_top_y_off - cap_to_descender * 0.5,
            ),
            uv_min: atlas.solid_uv.uv_min,
            uv_max: atlas.solid_uv.uv_max,
            size: Vec2::new(bg_w, cap_to_descender),
            color: linear_rgba(band.color),
            z_index: 0.0,
            corner_radii: [run.corner_radius; 4],
            skew: 0.0,
            _padding: [0.0; 2],
        },
    );

    emit_run_glyphs_only(
        line,
        run,
        paint,
        atlas,
        fonts,
        images,
        shape_usable,
        targets.text,
    );
}

/// Emit just the run's glyphs — shape-or-unshaped path, no bg, no extra work.
#[allow(clippy::too_many_arguments)]
fn emit_run_glyphs_only(
    line: &ShapedLine,
    run: &TextFormat,
    paint: RunPaint,
    atlas: &mut GlyphAtlas,
    fonts: &bevy::asset::Assets<bevy::text::Font>,
    images: &mut bevy::asset::Assets<Image>,
    shape_usable: Option<&Arc<super::glyph::LineShape>>,
    out: &mut TierBucket,
) {
    if let Some(shape) = shape_usable {
        emit_shaped_run_glyphs(
            &shape.glyphs,
            run.byte_range.clone(),
            paint.anchor,
            paint.style,
            atlas,
            out,
        );
    } else {
        emit_unshaped_run_glyphs(
            line,
            run.byte_range.clone(),
            paint,
            atlas,
            fonts,
            images,
            out,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn push_overlay_quad(
    rect: &super::overlay::RectOverlay,
    layout: &DisplayLayout,
    line_start_x: f32,
    viewport_width: f32,
    solid_uv: crate::gpu::GlyphInfo,
    solid_id: AssetId<Image>,
    solid_handle: &Handle<Image>,
    out: &mut TierBucket,
) {
    let Some(line) = layout
        .lines
        .iter()
        .find(|l| l.display_row == rect.display_row)
    else {
        return;
    };
    let line_height = line.line_height.unwrap_or(layout.line_height);
    let baseline_offset = layout.baseline_offset;
    // Negative x_range.start is intentional for overlays that need to
    // render to the left of the block's own indent (e.g. blockquote bars).
    let x0 = rect.x_range.start;
    let x1 = if rect.x_range.end >= f32::MAX / 2.0 {
        // Sentinel: stretch to the viewport's right edge.
        (viewport_width - line_start_x - line.x_offset).max(x0 + 1.0)
    } else {
        rect.x_range.end
    };
    let width = (x1 - x0).max(1.0);

    // Resolve semantic vertical placement against the row's geometry.
    // y_off is from the row's top edge (line.y_top); height is the rect height.
    //
    // Important: the row "box" (`y_top..y_top + line_height`) includes leading
    // above/below the actual glyph cap-to-descender extent. Carets and full-line
    // backgrounds should align with the *text* (centered on the baseline), not
    // with the leaded box, so they sit visually on the row of text rather than
    // straddling line-spacing whitespace.
    let baseline_y_off = line_height * 0.5 + baseline_offset;
    // Cap-to-descender extent ≈ ascent + descent, the visual "text band" within
    // the leaded line. Approximated from the same heuristic used for baseline_offset
    // (~32% of font size for descender-ish region above baseline). This gives a
    // tight box around glyphs, used as the canonical text-aligned span for
    // carets and full-line backgrounds.
    let cap_to_descender = baseline_y_off + baseline_offset * 0.6;
    // Bias text-aligned overlays slightly below the baseline so they cover the
    // descender region (where the eye perceives the text "sitting"), not just
    // float around the baseline midpoint. Used uniformly by Full + Caret so
    // selection backgrounds and carets stack to the same Y.
    let text_band_above_baseline = cap_to_descender * 0.25;
    let (y_off, height) = match rect.vertical {
        super::overlay::RowVertical::Full => {
            (baseline_y_off - text_band_above_baseline, cap_to_descender)
        }
        super::overlay::RowVertical::FullLeaded => {
            // Span the row's full leaded box. `y_top` is the box top, so
            // the rect runs `[y_top, y_top + line_height]` in screen-Y.
            // Quad center sits at `y_top + line_height / 2`, which gives
            // adjacent rows a flush seam.
            (line_height * 0.5, line_height)
        }
        super::overlay::RowVertical::Caret { height_fraction } => {
            let h = (cap_to_descender * height_fraction).max(1.0);
            (baseline_y_off - h * 0.25, h)
        }
        super::overlay::RowVertical::TopBand { thickness } => (0.0, thickness.max(1.0)),
        super::overlay::RowVertical::BottomBand { thickness } => {
            let t = thickness.max(1.0);
            (line_height - t, t)
        }
        super::overlay::RowVertical::UnderBaseline { thickness, gap } => {
            (baseline_y_off + gap, thickness.max(1.0))
        }
        // Strikethrough: mid-cap height (~40% above baseline within the cap-to-descender band).
        super::overlay::RowVertical::Strikethrough { thickness } => {
            let t = thickness.max(1.0);
            (baseline_y_off - cap_to_descender * 0.4, t)
        }
        // Underline: just below the baseline.
        super::overlay::RowVertical::Underline { thickness, gap } => {
            (baseline_y_off + gap, thickness.max(1.0))
        }
    };

    let node_x = line_start_x + line.x_offset + x0;
    let node_y = line.y_top + y_off - height * 0.5;

    out.push(
        solid_id,
        solid_handle,
        GlyphInstance {
            position: Vec2::new(node_x, node_y),
            uv_min: solid_uv.uv_min,
            uv_max: solid_uv.uv_max,
            size: Vec2::new(width, height),
            color: linear_rgba(rect.color),
            z_index: 0.0,
            corner_radii: [
                rect.corners.tl,
                rect.corners.tr,
                rect.corners.bl,
                rect.corners.br,
            ],
            skew: 0.0,
            _padding: [0.0; 2],
        },
    );
}

/// Pre-multiplied linear RGBA suitable for `GlyphInstance.color`.
fn linear_rgba(color: Color) -> [f32; 4] {
    let l = color.to_linear();
    [l.red, l.green, l.blue, l.alpha]
}

/// A horizontal band on a row: where the row sits (`anchor`), its vertical
/// metrics (`line_height`, `baseline_offset`), and the line-local x-span it
/// covers. `x_start`/`x_end` come from `line_byte_to_x` — the same coordinate
/// space as selection `x_range`.
#[derive(Clone, Copy)]
struct RowBand {
    anchor: RowAnchor,
    line_height: f32,
    baseline_offset: f32,
    x_start: f32,
    x_end: f32,
}

/// Emit thin quads for each active `TextDecoration` flag on a run.
/// Y is derived from `band.anchor.base_y` (the glyph baseline) using the same
/// math as `push_overlay_quad`.
fn emit_run_decoration(
    decoration: TextDecoration,
    fg: Color,
    band: RowBand,
    solid_uv: crate::gpu::GlyphInfo,
    solid_id: AssetId<Image>,
    solid_handle: &Handle<Image>,
    out: &mut TierBucket,
) {
    let RowBand {
        anchor,
        line_height,
        baseline_offset,
        x_start,
        x_end,
    } = band;
    let thickness = (line_height * 0.07).max(1.0);
    let color = linear_rgba(fg);
    let width = (x_end - x_start).max(1.0);
    // anchor.line_x already includes line_start_x + line.x_offset.
    let node_x = anchor.line_x + x_start;

    // Derive y_top from base_y: base_y = y_top + line_height*0.5 + baseline_offset
    let y_top = anchor.base_y - line_height * 0.5 - baseline_offset;
    let baseline_y_off = line_height * 0.5 + baseline_offset;
    let cap_to_descender = baseline_y_off + baseline_offset * 0.6;

    let mut push = |y_off: f32| {
        let node_y = y_top + y_off;
        out.push(
            solid_id,
            solid_handle,
            GlyphInstance {
                position: Vec2::new(node_x, node_y),
                uv_min: solid_uv.uv_min,
                uv_max: solid_uv.uv_max,
                size: Vec2::new(width, thickness),
                color,
                z_index: 1.0,
                corner_radii: [0.0; 4],
                skew: 0.0,
                _padding: [0.0; 2],
            },
        );
    };

    if decoration.contains(TextDecoration::STRIKETHROUGH) {
        push(baseline_y_off - cap_to_descender * 0.4);
    }
    if decoration.contains(TextDecoration::UNDERLINE) {
        push(baseline_y_off + thickness);
    }
}

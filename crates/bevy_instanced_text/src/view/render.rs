//! Generic text view rendering — produces `GlyphInstance` batches from a
//! `DisplayLayout` and (optional) `TextViewOverlays`.
//!
//! Pure function over an immutable snapshot: no cursors, selections, syntax
//! highlighting, or other editor concepts here. The producer (`display_map`)
//! has already shaped each visible row through cosmic-text and resolved
//! per-line/per-run colors into the `ShapedLine.runs`. This module turns
//! that into per-glyph quads ready for the instanced pipeline.

use std::sync::Arc;

use bevy::prelude::*;

use crate::gpu::GlyphAtlas;

use super::font::FontSynthesis;
use super::layout::{line_x_at_byte, DisplayLayout};
use super::overlay::TextViewOverlays;
use super::snapshot::{ShapedLine, StyleRun, TextDecoration};
use bevy::ui::ComputedNode;

/// Resolved font faces for one render call. The renderer picks per-run
/// based on `StyleRun.font_weight` (≥600 ⇒ bold) and `StyleRun.italic`,
/// falling back to `regular` and synthesizing the missing axis when
/// `synthesis` permits.
///
/// Built once per text-view per frame in `update_text_views`
/// from the entity's `TextFont` (each `Handle<Font>` is registered with
/// the atlas's cosmic-text font system on first use).
#[derive(Clone, Copy, Debug, Default)]
pub struct FontFaces {
    pub regular: Option<cosmic_text::fontdb::ID>,
    pub bold: Option<cosmic_text::fontdb::ID>,
    pub italic: Option<cosmic_text::fontdb::ID>,
    pub bold_italic: Option<cosmic_text::fontdb::ID>,
    pub synthesis: FontSynthesis,
}

impl FontFaces {
    /// Single-face shorthand. Bold/italic slots empty, synthesis on.
    /// Used by trivial-layout consumers that don't care about styling.
    pub fn single(regular: Option<cosmic_text::fontdb::ID>) -> Self {
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
    fn pick(&self, bold: bool, italic: bool) -> Option<cosmic_text::fontdb::ID> {
        match (bold, italic) {
            (true, true) => self
                .bold_italic
                .or(self.bold)
                .or(self.italic)
                .or(self.regular),
            (true, false) => self.bold.or(self.regular),
            (false, true) => self.italic.or(self.regular),
            (false, false) => self.regular,
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

/// Map a `StyleRun.font_weight` to "bold or not". CSS-style threshold:
/// `>= 600` is bold (semibold and above).
fn run_is_bold(run: &StyleRun) -> bool {
    matches!(run.font_weight, Some(w) if w >= 600)
}

/// Glyph instance data for GPU rendering.
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

#[derive(Component)]
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

pub struct RenderContext {
    pub content_start_x: f32,
    pub horizontal_scroll_offset: f32,
    pub font_size: f32,
    pub faces: FontFaces,
}

/// Render a `DisplayLayout` into glyph instances. Pure function over an immutable
/// snapshot — folding, wrapping, culling, and styling are already done by the producer.
/// Overlays via `RectOverlay`: negative-z renders below text, positive-z above.
pub fn render_layout(
    layout: &DisplayLayout,
    overlays: Option<&TextViewOverlays>,
    viewport: &ComputedNode,
    atlas: &mut GlyphAtlas,
    fonts: &bevy::asset::Assets<bevy::text::Font>,
    ctx: RenderContext,
) -> Vec<GlyphInstance> {
    let RenderContext {
        content_start_x,
        horizontal_scroll_offset,
        font_size,
        faces,
    } = ctx;
    let default_line_height = layout.line_height;
    let char_width = layout.char_width;
    let baseline_offset = layout.baseline_offset;

    // Glyph instances are emitted in world-space pixels under the
    // centered-ortho convention: viewport's top-left is at
    // `(-width/2, +height/2)` relative to the camera origin. The shader
    // projects directly through the camera's `view.clip_from_world`.
    let inv = viewport.inverse_scale_factor();
    let logical = viewport.size() * inv;
    let viewport_world_left = -logical.x / 2.0;
    let viewport_world_top = logical.y / 2.0;
    let line_start_x = content_start_x - horizontal_scroll_offset;

    // Single anchor: `line.y_top` is the row's visual top in screen pixels.
    //   - Glyph baseline = y_top + line_height/2 + baseline_offset (legacy ascent ratio).
    //   - Overlay full-line rect spans y_top..y_top+line_height.
    //   - Sub-line overlays (`y_range: Some(0..2)`) are y_top-relative.
    // Per-row overrides: `ShapedLine.line_height = Some(h)` lets a row paint
    // taller / shorter than the layout default (markdown headings, code blocks).

    let mut text_instances: Vec<GlyphInstance> = Vec::with_capacity(layout.lines.len() * 80);
    let mut below_instances: Vec<GlyphInstance> = Vec::new();
    let mut above_instances: Vec<GlyphInstance> = Vec::new();

    // Below-text overlays first (selection bg, line highlight)
    if let Some(ovs) = overlays {
        for rect in ovs.rects.iter().filter(|r| r.z < 0) {
            push_overlay_quad(
                rect,
                layout,
                viewport_world_left,
                viewport_world_top,
                line_start_x,
                logical.x,
                atlas.solid_uv,
                &mut below_instances,
            );
        }
    }

    // Glyphs and per-line/per-run backgrounds
    for line in layout.lines.iter() {
        let line_height = line.line_height.unwrap_or(default_line_height);
        // Glyph baseline derived from row top.
        let base_y = line.y_top + line_height * 0.5 + baseline_offset;
        let line_x = line_start_x + line.x_offset;

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
            below_instances.push(GlyphInstance {
                position: Vec2::new(
                    viewport_world_left + margin + bg_x_start,
                    viewport_world_top - line.y_top - line_height * 0.5,
                ),
                uv_min: atlas.solid_uv.uv_min,
                uv_max: atlas.solid_uv.uv_max,
                size: Vec2::new(
                    logical.x - margin * 2.0 - bg_x_start,
                    line_height,
                ),
                color: linear_rgba(bg),
                z_index: 0.0,
                corner_radii: [line_corner_radius; 4],
                skew: 0.0,
                _padding: [0.0; 2],
            });
        }

        // Pre-compose the row anchor; threads through every emitter call.
        let anchor = RowAnchor {
            viewport_world_left,
            viewport_world_top,
            line_x,
            base_y,
        };

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
                    &mut text_instances,
                );
            } else {
                emit_unshaped_run_glyphs(
                    line,
                    0..line.text.len(),
                    anchor,
                    style,
                    RunMetrics {
                        font_size,
                        start_x: 0.0,
                    },
                    atlas,
                    faces.regular,
                    false,
                    false,
                    &mut text_instances,
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
                let run_face = if let Some(ref handle) = run.font {
                    atlas.ensure_font(handle, fonts)
                } else {
                    faces.pick(bold, italic)
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
                            anchor,
                            style,
                            seg_x_start,
                            seg_x_end,
                            seg_font_size,
                            line_height,
                            baseline_offset,
                            bg,
                            atlas,
                            run_face,
                            bold,
                            italic,
                            shape_usable,
                            &mut below_instances,
                            &mut text_instances,
                        );
                    } else {
                        emit_run_glyphs_only(
                            line,
                            run,
                            anchor,
                            style,
                            seg_x_start,
                            seg_font_size,
                            atlas,
                            run_face,
                            bold,
                            italic,
                            shape_usable,
                            &mut text_instances,
                        );
                    }
                } else {
                    emit_run_glyphs_only(
                        line,
                        run,
                        anchor,
                        style,
                        seg_x_start,
                        seg_font_size,
                        atlas,
                        run_face,
                        bold,
                        italic,
                        shape_usable,
                        &mut text_instances,
                    );
                }

                if !run.decoration.is_empty() {
                    emit_run_decoration(
                        run.decoration,
                        run.fg,
                        anchor,
                        line_height,
                        baseline_offset,
                        seg_x_start,
                        seg_x_end,
                        atlas.solid_uv,
                        &mut text_instances,
                    );
                }
            }
        }
    }

    // Above-text overlays (carets)
    if let Some(ovs) = overlays {
        for rect in ovs.rects.iter().filter(|r| r.z >= 0) {
            push_overlay_quad(
                rect,
                layout,
                viewport_world_left,
                viewport_world_top,
                line_start_x,
                logical.x,
                atlas.solid_uv,
                &mut above_instances,
            );
        }
    }

    // Painter's order: below → text → above
    let mut out = below_instances;
    out.append(&mut text_instances);
    out.append(&mut above_instances);
    out
}

/// Pixel offset of `byte` within `line.text`. Uses shaped advances when present
/// and falls back to a `char_width` walk otherwise. Tab handling matches the
/// shaper's behavior (it inflates `\t` advance by `tab_width = 4`).
fn line_byte_to_x(line: &ShapedLine, byte: usize, char_width: f32) -> f32 {
    line_x_at_byte(line, byte, char_width)
}

/// Where a row paints — viewport origin + line-local offsets. Composed once
/// per row in `render_layout` and threaded into both glyph emitters and quad
/// pushers, eliminating repeated 4-float parameter lists.
#[derive(Clone, Copy)]
struct RowAnchor {
    /// Viewport's world-space top-left X (Bevy's center-origin Y is inverted).
    viewport_world_left: f32,
    /// Viewport's world-space top edge (Y inversion baseline).
    viewport_world_top: f32,
    /// Line-local origin X in screen pixels, includes scroll + line.x_offset.
    line_x: f32,
    /// Glyph baseline Y in screen pixels (top-down origin).
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

/// Build a glyph quad from an atlas hit. Centralizes the legacy/shaped paint
/// math so both emitters share one expression for screen→world conversion.
fn glyph_quad(
    info: crate::gpu::GlyphInfo,
    pen_x: f32,
    anchor: RowAnchor,
    style: RunStyle,
) -> GlyphInstance {
    let screen_x = anchor.line_x + pen_x + info.offset.x;
    let screen_y = anchor.base_y - info.offset.y;
    GlyphInstance {
        position: Vec2::new(
            anchor.viewport_world_left + screen_x,
            anchor.viewport_world_top - screen_y - info.size.y,
        ),
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

/// Emit a glyph (and its synthetic-bold twin if requested) into `out`.
fn push_glyph(
    info: crate::gpu::GlyphInfo,
    pen_x: f32,
    anchor: RowAnchor,
    style: RunStyle,
    out: &mut Vec<GlyphInstance>,
) {
    out.push(glyph_quad(info, pen_x, anchor, style));
    if style.stroke_offset_px > 0.0 {
        out.push(glyph_quad(
            info,
            pen_x + style.stroke_offset_px,
            anchor,
            style,
        ));
    }
}

/// Emit glyphs whose `byte_index` lies inside `range` from a pre-shaped line.
/// `glyphs[i].x` is line-local; `glyph_quad` adds the row anchor.
fn emit_shaped_run_glyphs(
    glyphs: &[super::snapshot::ShapedGlyph],
    range: std::ops::Range<usize>,
    anchor: RowAnchor,
    style: RunStyle,
    atlas: &mut GlyphAtlas,
    out: &mut Vec<GlyphInstance>,
) {
    for g in glyphs {
        if g.byte_index < range.start || g.byte_index >= range.end {
            continue;
        }
        let Some((info, _)) = atlas.get_or_rasterize_glyph(g.cache_key) else {
            continue;
        };
        push_glyph(info, g.x, anchor, style, out);
    }
}

/// Per-run metrics for the shape-on-demand fallback path. `font_size` is the
/// rasterizer size (may differ from the line default when a `font_scale` run
/// overrides); `start_x` is the run's pen-x relative to the line origin (where
/// it picks up after any preceding runs).
#[derive(Clone, Copy)]
struct RunMetrics {
    font_size: f32,
    start_x: f32,
}

/// Emit glyphs for a byte range by shaping it on demand. Used when no
/// `LineShape` is attached (`trivial_layout` consumers) or when a
/// `StyleRun.font_scale` override requires re-shaping at a different size.
///
/// Shaping a per-frame slice is cheap — it's the same code path the producer
/// uses, just narrowed to the run. For monospace ASCII it yields advances
/// byte-identical to the old `col * char_width` walk.
#[allow(clippy::too_many_arguments)]
fn emit_unshaped_run_glyphs(
    line: &ShapedLine,
    range: std::ops::Range<usize>,
    anchor: RowAnchor,
    style: RunStyle,
    metrics: RunMetrics,
    atlas: &mut GlyphAtlas,
    font_id: Option<cosmic_text::fontdb::ID>,
    bold: bool,
    italic: bool,
    out: &mut Vec<GlyphInstance>,
) {
    let Some(slice) = line.text.get(range) else {
        return;
    };
    let shape_text = slice.strip_suffix('\n').unwrap_or(slice);
    let shape = atlas.shape_line_styled(shape_text, metrics.font_size, font_id, bold, italic);
    for g in &shape.glyphs {
        let Some((info, _)) = atlas.get_or_rasterize_glyph(g.cache_key) else {
            continue;
        };
        push_glyph(info, metrics.start_x + g.x, anchor, style, out);
    }
}

/// Emit a per-run background quad spanning the run's advance bounds, then
/// emit the glyphs. Using advance bounds (not ink bounds) means the chip
/// extends to the same edges as the surrounding text's character cells,
/// matching how browsers render `<code>` — no side-bearing gap on either side.
#[allow(clippy::too_many_arguments)]
fn emit_run_with_bg(
    line: &ShapedLine,
    run: &StyleRun,
    anchor: RowAnchor,
    style: RunStyle,
    seg_x_start: f32,
    seg_x_end: f32,
    seg_font_size: f32,
    line_height: f32,
    baseline_offset: f32,
    bg: bevy::prelude::Color,
    atlas: &mut GlyphAtlas,
    run_face: Option<cosmic_text::fontdb::ID>,
    bold: bool,
    italic: bool,
    shape_usable: Option<&Arc<super::snapshot::LineShape>>,
    below: &mut Vec<GlyphInstance>,
    text: &mut Vec<GlyphInstance>,
) {
    let baseline_y_off = line_height * 0.5 + baseline_offset;
    let cap_to_descender = baseline_y_off + baseline_offset * 0.6;
    let text_band_above = cap_to_descender * 0.25;
    let band_top_y_off = baseline_y_off - text_band_above;
    let bg_w = (seg_x_end - seg_x_start).max(0.0);
    below.push(GlyphInstance {
        position: Vec2::new(
            anchor.viewport_world_left + anchor.line_x + seg_x_start,
            anchor.viewport_world_top - line.y_top - band_top_y_off - cap_to_descender * 0.5,
        ),
        uv_min: atlas.solid_uv.uv_min,
        uv_max: atlas.solid_uv.uv_max,
        size: Vec2::new(bg_w, cap_to_descender),
        color: linear_rgba(bg),
        z_index: 0.0,
        corner_radii: [run.corner_radius; 4],
        skew: 0.0,
        _padding: [0.0; 2],
    });

    emit_run_glyphs_only(
        line,
        run,
        anchor,
        style,
        seg_x_start,
        seg_font_size,
        atlas,
        run_face,
        bold,
        italic,
        shape_usable,
        text,
    );
}

/// Emit just the run's glyphs — shape-or-unshaped path, no bg, no extra work.
#[allow(clippy::too_many_arguments)]
fn emit_run_glyphs_only(
    line: &ShapedLine,
    run: &StyleRun,
    anchor: RowAnchor,
    style: RunStyle,
    seg_x_start: f32,
    seg_font_size: f32,
    atlas: &mut GlyphAtlas,
    run_face: Option<cosmic_text::fontdb::ID>,
    bold: bool,
    italic: bool,
    shape_usable: Option<&Arc<super::snapshot::LineShape>>,
    out: &mut Vec<GlyphInstance>,
) {
    if let Some(shape) = shape_usable {
        emit_shaped_run_glyphs(
            &shape.glyphs,
            run.byte_range.clone(),
            anchor,
            style,
            atlas,
            out,
        );
    } else {
        emit_unshaped_run_glyphs(
            line,
            run.byte_range.clone(),
            anchor,
            style,
            RunMetrics {
                font_size: seg_font_size,
                start_x: seg_x_start,
            },
            atlas,
            run_face,
            bold,
            italic,
            out,
        );
    }
}


#[allow(clippy::too_many_arguments)]
fn push_overlay_quad(
    rect: &super::overlay::RectOverlay,
    layout: &DisplayLayout,
    world_left: f32,
    world_top: f32,
    line_start_x: f32,
    viewport_width: f32,
    solid_uv: crate::gpu::GlyphInfo,
    out: &mut Vec<GlyphInstance>,
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
        // Sentinel: extend to the viewport's right edge.
        // world_x_right == world_left + viewport_width
        // world_x_right == world_left + line_start_x + line.x_offset + x1
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

    let world_x = world_left + line_start_x + line.x_offset + x0;
    // Single anchor: line.y_top is the row's screen-Y top. Quad position is the
    // *center* of the rect; world_top inverts Y.
    let world_y = world_top - line.y_top - y_off - height * 0.5;

    out.push(GlyphInstance {
        position: Vec2::new(world_x, world_y),
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
    });
}

/// Pre-multiplied linear RGBA suitable for `GlyphInstance.color`.
fn linear_rgba(color: Color) -> [f32; 4] {
    let l = color.to_linear();
    [l.red, l.green, l.blue, l.alpha]
}

/// Emit thin quads for each active `TextDecoration` flag on a run.
/// `x_start`/`x_end` are line-local pixels from `line_byte_to_x` — the same
/// coordinate space as selection `x_range`. Y is derived from `anchor.base_y`
/// (the glyph baseline) using the same math as `push_overlay_quad`.
#[allow(clippy::too_many_arguments)]
fn emit_run_decoration(
    decoration: TextDecoration,
    fg: Color,
    anchor: RowAnchor,
    line_height: f32,
    baseline_offset: f32,
    x_start: f32,
    x_end: f32,
    solid_uv: crate::gpu::GlyphInfo,
    out: &mut Vec<GlyphInstance>,
) {
    let thickness = (line_height * 0.07).max(1.0);
    let color = linear_rgba(fg);
    let width = (x_end - x_start).max(1.0);
    // anchor.line_x already includes line_start_x + line.x_offset.
    let world_x = anchor.viewport_world_left + anchor.line_x + x_start;

    // Derive y_top from base_y: base_y = y_top + line_height*0.5 + baseline_offset
    let y_top = anchor.base_y - line_height * 0.5 - baseline_offset;
    let baseline_y_off = line_height * 0.5 + baseline_offset;
    let cap_to_descender = baseline_y_off + baseline_offset * 0.6;

    let mut push = |y_off: f32| {
        let world_y = anchor.viewport_world_top - y_top - y_off - thickness;
        out.push(GlyphInstance {
            position: Vec2::new(world_x, world_y),
            uv_min: solid_uv.uv_min,
            uv_max: solid_uv.uv_max,
            size: Vec2::new(width, thickness),
            color,
            z_index: 1.0,
            corner_radii: [0.0; 4],
            skew: 0.0,
            _padding: [0.0; 2],
        });
    };

    if decoration.contains(TextDecoration::STRIKETHROUGH) {
        push(baseline_y_off - cap_to_descender * 0.4);
    }
    if decoration.contains(TextDecoration::UNDERLINE) {
        push(baseline_y_off + thickness);
    }
}

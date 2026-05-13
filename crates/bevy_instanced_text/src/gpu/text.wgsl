// GPU instanced text rendering shader for Bevy UI.
//
// Glyph instances arrive in node-local logical px (top-left origin,
// +Y down). The vertex shader composes the per-batch affine and
// `view.clip_from_world` (UI ortho, physical px → NDC).

#import bevy_render::view::View

@group(0) @binding(0) var<uniform> view: View;

// Per-batch affine packed as rows of a 2×3 matrix:
//   affine_row0 = (m00, m01, t0)  → screen_x = m00*x + m01*y + t0
//   affine_row1 = (m10, m11, t1)  → screen_y = m10*x + m11*y + t1
// `clip_max.x < 0` signals no clipping.
struct Batch {
    affine_row0: vec3<f32>,
    affine_row1: vec3<f32>,
    clip_min: vec2<f32>,
    clip_max: vec2<f32>,
};

@group(2) @binding(0) var<uniform> batch: Batch;

struct FragmentInput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    // Local position within the quad (0..size) for rounded corner SDF.
    @location(2) local_pos: vec2<f32>,
    @location(3) quad_size: vec2<f32>,
    // Per-corner radii: [top_left, top_right, bottom_left, bottom_right].
    @location(4) corner_radii: vec4<f32>,
}

@vertex
fn vertex(
    @builtin(vertex_index) vertex_index: u32,
    @location(0) glyph_position: vec2<f32>,
    @location(1) uv_min: vec2<f32>,
    @location(2) uv_max: vec2<f32>,
    @location(3) size: vec2<f32>,
    @location(4) color: vec4<f32>,
    @location(5) corner_radii: vec4<f32>,
    @location(6) z_index: f32,
    @location(7) skew: f32,
) -> FragmentInput {
    // Unit-quad corners in triangle-list order with +Y down.
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, 0.0),
        vec2<f32>(0.0, 0.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(1.0, 0.0),
    );
    let unit_vertex = corners[vertex_index];

    var node_xy = glyph_position + unit_vertex * size;
    // Italic shear — top of the glyph leans right.
    node_xy.x += skew * (1.0 - unit_vertex.y) * size.y;

    let screen_xy = vec2<f32>(
        batch.affine_row0.x * node_xy.x + batch.affine_row0.y * node_xy.y + batch.affine_row0.z,
        batch.affine_row1.x * node_xy.x + batch.affine_row1.y * node_xy.y + batch.affine_row1.z,
    );

    // Bevy UI ortho: physical px → NDC. Small per-glyph Z preserves
    // overlay ordering (selection bg beneath text, caret above) under
    // sort key. UI camera Z extends to UI_CAMERA_FAR (1000).
    let world_pos = vec4<f32>(screen_xy.x, screen_xy.y, z_index * 1e-4, 1.0);
    let clip_pos = view.clip_from_world * world_pos;

    let uv = mix(uv_min, uv_max, unit_vertex);

    var out: FragmentInput;
    out.position = clip_pos;
    out.uv = uv;
    out.color = color;
    out.local_pos = unit_vertex * size;
    out.quad_size = size;
    out.corner_radii = corner_radii;
    return out;
}

@group(1) @binding(0) var atlas_texture: texture_2d<f32>;
@group(1) @binding(1) var atlas_sampler: sampler;

// Rounded-rectangle SDF with per-corner radii. `pos` is centered on the
// quad. Quadrant selection: pos.y < 0 picks the top corners (radii.xy),
// pos.y > 0 picks the bottom corners (radii.zw).
fn rounded_rect_sdf_per_corner(
    pos: vec2<f32>,
    half_size: vec2<f32>,
    radii: vec4<f32>,
) -> f32 {
    let r = select(
        select(radii.x, radii.y, pos.x > 0.0),
        select(radii.z, radii.w, pos.x > 0.0),
        pos.y > 0.0,
    );
    let q = abs(pos) - half_size + vec2<f32>(r);
    return min(max(q.x, q.y), 0.0) + length(max(q, vec2<f32>(0.0))) - r;
}

// Kept in sync with `rounded_rect_sdf_per_corner` so the AA gate uses
// the same radius as the SDF.
fn pick_corner_radius(pos: vec2<f32>, radii: vec4<f32>) -> f32 {
    return select(
        select(radii.x, radii.y, pos.x > 0.0),
        select(radii.z, radii.w, pos.x > 0.0),
        pos.y > 0.0,
    );
}

@fragment
fn fragment(in: FragmentInput) -> @location(0) vec4<f32> {
    // Scissor-style clip against `batch.clip_*` (negative max disables).
    if batch.clip_max.x >= 0.0 {
        if in.position.x < batch.clip_min.x || in.position.x > batch.clip_max.x
            || in.position.y < batch.clip_min.y || in.position.y > batch.clip_max.y {
            discard;
        }
    }

    let max_radius = max(
        max(in.corner_radii.x, in.corner_radii.y),
        max(in.corner_radii.z, in.corner_radii.w),
    );
    if max_radius > 0.0 {
        let center = in.quad_size * 0.5;
        let pos = in.local_pos - center;
        let d = rounded_rect_sdf_per_corner(pos, center, in.corner_radii);
        if d > 0.5 {
            discard;
        }
        // Anti-alias only the curved region; sharp edges keep alpha=1
        // so adjacent rows don't get half-transparent seams.
        let r = pick_corner_radius(pos, in.corner_radii);
        let q = abs(pos) - center + vec2<f32>(r);
        let in_curve = r > 0.0 && q.x > 0.0 && q.y > 0.0;
        let edge_alpha = select(1.0, 1.0 - smoothstep(-0.5, 0.5, d), in_curve);

        let atlas_sample = textureSample(atlas_texture, atlas_sampler, in.uv);
        let alpha = atlas_sample.a * in.color.a * edge_alpha;

        if alpha < 0.01 {
            discard;
        }
        return vec4<f32>(in.color.rgb * alpha, alpha);
    }

    let atlas_sample = textureSample(atlas_texture, atlas_sampler, in.uv);
    let alpha = atlas_sample.a * in.color.a;

    if alpha < 0.01 {
        discard;
    }

    return vec4<f32>(in.color.rgb * alpha, alpha);
}

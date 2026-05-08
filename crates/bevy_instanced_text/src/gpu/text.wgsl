// GPU instanced text rendering shader.
//
// Each glyph is one instance. The vertex shader expands a unit quad from
// `vertex_index` for each instance, then projects through the camera's
// view-projection matrix from `Mesh2dPipeline`'s view bind group.
//
// Note: this shader deliberately does NOT use `mesh2d_position_local_to_clip`
// or `get_world_from_local(instance_index)`. That helper indexes into a
// per-entity `mesh[]` array using `@builtin(instance_index)` — but our
// `instance_index` is the per-GLYPH instance, not the per-entity one, so
// each glyph would read a different (invalid) entity transform. Instead we
// project glyph positions directly through `view.clip_from_world`. Glyph
// instance positions are emitted by `render_layout` in world-space pixels.

#import bevy_sprite::mesh2d_view_bindings::view

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
    // Per-instance glyph attributes (slot 0).
    @location(0) glyph_position: vec2<f32>,
    @location(1) uv_min: vec2<f32>,
    @location(2) uv_max: vec2<f32>,
    @location(3) size: vec2<f32>,
    @location(4) color: vec4<f32>,
    @location(5) corner_radii: vec4<f32>,
    @location(6) z_index: f32,
    @location(7) skew: f32,
) -> FragmentInput {
    // Six unit-quad vertices in triangle-list order: BL,BR,TL, TL,BR,TR.
    // The shader expands the quad on the GPU from `vertex_index`; no
    // mesh asset is bound.
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), // 0: BL
        vec2<f32>(1.0, 0.0), // 1: BR
        vec2<f32>(0.0, 1.0), // 2: TL
        vec2<f32>(0.0, 1.0), // 3: TL
        vec2<f32>(1.0, 0.0), // 4: BR
        vec2<f32>(1.0, 1.0), // 5: TR
    );
    let unit_vertex = corners[vertex_index];

    // Glyph position is the quad's bottom-left corner in world-space
    // pixels (Y up). `render_layout` emits these positions directly.
    var world_xy = glyph_position + unit_vertex * size;
    // Italic skew — top of the glyph shifts right by `skew * size.y`.
    world_xy.x += skew * unit_vertex.y * size.y;
    // Small per-glyph Z offset preserves overlay ordering (selection bg
    // beneath text, caret above) under depth test.
    let world_pos = vec4<f32>(world_xy.x, world_xy.y, z_index * 1e-4, 1.0);

    let clip_pos = view.clip_from_world * world_pos;

    // Atlas UV interpolation. Flip V so 0=top in atlas matches 0=top of the
    // glyph quad (atlas stores glyphs with origin at top-left, but our
    // unit_vertex has origin at bottom-left, so we feed `1 - v`).
    let uv = mix(uv_min, uv_max, vec2<f32>(unit_vertex.x, 1.0 - unit_vertex.y));

    var out: FragmentInput;
    out.position = clip_pos;
    out.uv = uv;
    out.color = color;
    out.local_pos = unit_vertex * size;
    out.quad_size = size;
    out.corner_radii = corner_radii;
    return out;
}

// Atlas binding lives in the second bind group (after view).
@group(1) @binding(0) var atlas_texture: texture_2d<f32>;
@group(1) @binding(1) var atlas_sampler: sampler;

// SDF for a rounded rectangle with per-corner radii. `pos` is centered
// (origin at quad center). `half_size` is half the quad's extent. The
// radius used is selected by which quadrant `pos` is in:
//   - top-left  (x<0, y>0): radii.x
//   - top-right (x>0, y>0): radii.y
//   - bot-left  (x<0, y<0): radii.z
//   - bot-right (x>0, y<0): radii.w
//
// In the geometry-space coordinate system the vertex shader uses
// `unit_vertex.y` increases bottom→top, so positive local_pos.y is the
// top of the quad — matches the [tl, tr, bl, br] mapping in `radii`.
fn rounded_rect_sdf_per_corner(
    pos: vec2<f32>,
    half_size: vec2<f32>,
    radii: vec4<f32>,
) -> f32 {
    // Pick this fragment's corner radius based on its quadrant.
    let r = select(
        select(radii.z, radii.w, pos.x > 0.0),
        select(radii.x, radii.y, pos.x > 0.0),
        pos.y > 0.0,
    );
    let q = abs(pos) - half_size + vec2<f32>(r);
    return min(max(q.x, q.y), 0.0) + length(max(q, vec2<f32>(0.0))) - r;
}

// Pick the corner radius for a fragment's quadrant. Mirrors
// `rounded_rect_sdf_per_corner`'s quadrant selection — kept in sync
// so the AA gate uses the same radius the SDF uses.
fn pick_corner_radius(pos: vec2<f32>, radii: vec4<f32>) -> f32 {
    return select(
        select(radii.z, radii.w, pos.x > 0.0),
        select(radii.x, radii.y, pos.x > 0.0),
        pos.y > 0.0,
    );
}

@fragment
fn fragment(in: FragmentInput) -> @location(0) vec4<f32> {
    // Rounded corner clipping when any corner has a non-zero radius.
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
        // Anti-alias only the curved region (where both `q.x > 0` and
        // `q.y > 0` for the corner's bounding square). Sharp edges keep
        // alpha=1 so adjacent rows don't get half-transparent seams.
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

    // Standard path (no rounded corners).
    let atlas_sample = textureSample(atlas_texture, atlas_sampler, in.uv);
    let alpha = atlas_sample.a * in.color.a;

    if alpha < 0.01 {
        discard;
    }

    return vec4<f32>(in.color.rgb * alpha, alpha);
}

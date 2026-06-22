//! Text view plugin — registers the rendering systems that turn
//! `InstancedText<T>` entities into GPU draw batches.
//!
//! [`InstancedTextPlugin`] sets up the core rendering infrastructure.
//! [`TextContentPlugin<T>`] registers `produce_layouts::<T>` for a specific
//! content type — add one per `T` you use. [`InstancedTextPlugins`] bundles
//! everything including the `String` content type for simple labels.
//!
//! Scroll is `bevy::ui::ScrollPosition`. The engine reads it; it never
//! writes it. Smooth scroll, if you want it, belongs in the host crate.

use std::marker::PhantomData;

use bevy::app::{PluginGroup, PluginGroupBuilder};
use bevy::math::Affine2;
use bevy::prelude::*;
use bevy::ui::{
    ui_transform::UiGlobalTransform, CalculatedClip, ComputedNode, ComputedUiTargetCamera,
    ContentSize, IsDefaultUiCamera, Measure, MeasureArgs, NodeMeasure, ScrollPosition, UiSystems,
};

use super::font::{resolve_line_height, MonoCellWidth, MonoFontFaces};
use super::measurement::LayoutTuning;
use super::overlay::{TextOverlays, TextUnderlays};
use super::pipeline::DisplayLayout;
use super::render::{render_layout, BatchTransform, GlyphBatchComponent, TextViewBatch};
use super::text::{ContentMetrics, InstancedText, TextContent};
use super::text_access::{produce_layouts, LayoutProduceSet};
use super::text_style::TextBounds;
use crate::gpu::{atlas_ready, GlyphAtlas, GlyphAtlasPlugin, InstancedTextRenderPlugin};
pub use bevy::text::{TextBackgroundColor, TextColor};

/// Contains `update_text_views`. Order downstream `.after(TextViewRenderSet)`.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct TextViewRenderSet;

/// Taffy `Measure` reporting an `InstancedText`'s intrinsic size so flex
/// containers can size the node without an explicit `Node::width`/`height` —
/// mirroring how Bevy UI's `Text` measures. `line_height` is the cross-axis
/// hint; `max_content_width` / `min_content_width` are approximate intrinsic
/// widths (char count × cell width) used to resolve `width: auto` shrink-to-fit.
///
/// Widths are monospace estimates, not shaped extents — exact for the
/// monospace fonts this engine targets, a close approximation otherwise. An
/// explicit `Node::width` always wins; this only contributes when the parent
/// asks for `MinContent` / `MaxContent`.
#[derive(Clone, Copy)]
struct InstancedTextMeasure {
    line_height: f32,
    max_content_width: f32,
    min_content_width: f32,
}

impl Measure for InstancedTextMeasure {
    fn measure(&mut self, args: MeasureArgs<'_>) -> bevy::math::Vec2 {
        use bevy::ui::AvailableSpace;
        let width = args
            .known_width
            .unwrap_or_else(|| match args.available_width {
                AvailableSpace::Definite(w) => w.min(self.max_content_width),
                AvailableSpace::MinContent => self.min_content_width,
                AvailableSpace::MaxContent => self.max_content_width,
            });
        bevy::math::Vec2::new(width, self.line_height)
    }
}

/// Approximate intrinsic widths (max-content, min-content) for a buffer at a
/// given cell width. Max-content = widest line; min-content = longest run
/// between whitespace break opportunities. Both in logical pixels.
fn intrinsic_widths<T: TextContent>(buffer: &T, cell_px: f32) -> (f32, f32) {
    let mut max_chars = 0usize;
    let mut min_chars = 0usize;
    for i in 0..buffer.line_count() {
        let line = buffer.line(i);
        let trimmed = line.strip_suffix('\n').unwrap_or(&line);
        max_chars = max_chars.max(trimmed.chars().count());
        for word in trimmed.split([' ', '\t']) {
            min_chars = min_chars.max(word.chars().count());
        }
    }
    (max_chars as f32 * cell_px, min_chars as f32 * cell_px)
}

/// Links a text view to its batch rendering entities. Managed by
/// `update_text_views`.
///
/// A view emits one batch entity per atlas-texture × painter-tier group
/// (Bevy 0.19's `FontAtlasSet` shards glyphs into separate textures per
/// font/size, so a mixed-size view spans several). The entities are listed in
/// draw order (below → text → above); each carries a `BatchTransform.sub_order`
/// that preserves that layering on the GPU.
#[derive(Component, Reflect)]
#[reflect(Component)]
pub struct TextViewBatchEntity(pub Vec<Entity>);

/// Registers `produce_layouts::<T>` for a specific [`TextContent`] type.
///
/// Add one of these per content type you use. [`InstancedTextPlugin`]
/// automatically adds `TextContentPlugin::<String>` for the simple label
/// use case. Editor / terminal hosts add their own (e.g.
/// `TextContentPlugin::<Rope>`).
pub struct TextContentPlugin<T: TextContent>(PhantomData<T>);

impl<T: TextContent> Default for TextContentPlugin<T> {
    fn default() -> Self {
        Self(PhantomData)
    }
}

impl<T: TextContent> Plugin for TextContentPlugin<T> {
    fn build(&self, app: &mut App) {
        // Register required components so spawning InstancedText<T> alone is enough.
        app.world_mut()
            .register_required_components_with::<InstancedText<T>, bevy::text::LineHeight>(|| {
                bevy::text::LineHeight::RelativeToFont(1.5)
            });
        app.world_mut()
            .register_required_components::<InstancedText<T>, ScrollPosition>();
        app.world_mut()
            .register_required_components::<InstancedText<T>, ContentMetrics>();
        app.world_mut()
            .register_required_components::<InstancedText<T>, DisplayLayout>();
        app.world_mut()
            .register_required_components::<InstancedText<T>, TextUnderlays>();
        app.world_mut()
            .register_required_components::<InstancedText<T>, TextOverlays>();
        app.world_mut()
            .register_required_components::<InstancedText<T>, TextFont>();
        app.world_mut()
            .register_required_components::<InstancedText<T>, TextColor>();
        app.world_mut()
            .register_required_components::<InstancedText<T>, bevy::text::TextLayout>();
        app.world_mut()
            .register_required_components::<InstancedText<T>, TextBackgroundColor>();
        app.world_mut()
            .register_required_components::<InstancedText<T>, MonoFontFaces>();
        app.world_mut()
            .register_required_components::<InstancedText<T>, MonoCellWidth>();

        app.world_mut()
            .register_required_components::<InstancedText<T>, TextBounds>();
        app.world_mut()
            .register_required_components::<InstancedText<T>, super::text_style::LineStyles>();
        app.world_mut()
            .register_required_components::<InstancedText<T>, super::text_style::HiddenLines>();
        app.world_mut()
            .register_required_components::<InstancedText<T>, LayoutTuning>();
        app.world_mut()
            .register_required_components::<InstancedText<T>, Node>();
        app.world_mut()
            .register_required_components::<InstancedText<T>, ContentSize>();
        app.world_mut()
            .register_required_components::<InstancedText<T>, Visibility>();
        app.world_mut()
            .register_required_components_with::<InstancedText<T>, InheritedVisibility>(|| {
                InheritedVisibility::VISIBLE
            });
        app.world_mut()
            .register_required_components::<InstancedText<T>, bevy::picking::Pickable>();

        app.add_systems(
            PostUpdate,
            (
                measure_text_buffer::<T>.in_set(UiSystems::Content),
                produce_layouts::<T>
                    .run_if(atlas_ready)
                    .in_set(LayoutProduceSet)
                    .after(UiSystems::Layout)
                    .before(prewarm_atlas_for_layout),
            ),
        );
    }
}

/// The per-entity columns `measure_text_buffer` reads/writes. Bundled into a
/// `QueryData` so the system signature stays legible.
#[derive(bevy::ecs::query::QueryData)]
#[query_data(mutable)]
struct MeasureRow<T: TextContent> {
    content_size: &'static mut ContentSize,
    buffer: &'static InstancedText<T>,
    line_height: &'static bevy::text::LineHeight,
    font: &'static TextFont,
    mono: &'static MonoCellWidth,
}

/// Change filter for [`measure_text_buffer`] — re-measure when any input that
/// affects the intrinsic size moves, or when `ContentSize` is first added.
type MeasureChanged<T> = Or<(
    Changed<InstancedText<T>>,
    Changed<bevy::text::LineHeight>,
    Changed<TextFont>,
    Changed<MonoCellWidth>,
    Added<ContentSize>,
)>;

/// Installs a [`InstancedTextMeasure`] on every `InstancedText<T>` entity so bevy_ui
/// knows their intrinsic line height and width. Runs in `UiSystems::Content`,
/// before taffy lays out the tree. Only updates when an input that affects the
/// measured size changes so layout invalidation stays minimal.
fn measure_text_buffer<T: TextContent>(mut q: Query<MeasureRow<T>, MeasureChanged<T>>) {
    for mut row in q.iter_mut() {
        let lh = resolve_line_height(*row.line_height, row.font.font_size);
        let (max_content_width, min_content_width) = intrinsic_widths(&**row.buffer, row.mono.px);
        row.content_size
            .set(NodeMeasure::Custom(Box::new(InstancedTextMeasure {
                line_height: lh,
                max_content_width,
                min_content_width,
            })));
    }
}

/// Registers the core rendering and scroll animation systems. Does not add GPU
/// plugins — use [`InstancedTextPlugins`] for the full bundle.
///
/// Also registers [`TextContentPlugin::<String>`] for simple label use cases.
#[derive(Default)]
pub struct InstancedTextPlugin;

impl Plugin for InstancedTextPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<LayoutTuning>();

        app.register_type::<MonoFontFaces>()
            .register_type::<MonoCellWidth>()
            .register_type::<super::overlay::RectOverlay>()
            .register_type::<super::overlay::RowVertical>()
            .register_type::<TextBounds>()
            // TextColor / TextBackgroundColor are bevy::text types — Bevy registers them.
            .register_type::<TextViewBatchEntity>()
            .register_type::<TextUnderlays>()
            .register_type::<TextOverlays>()
            .register_type::<ContentMetrics>();

        // Register the String content type so simple labels work out of the box.
        // (Bevy already registers String's reflection.)
        app.add_plugins(TextContentPlugin::<String>::default());

        // Ensure there is always a camera marked as the default UI camera so
        // Bevy UI layout can resolve Val::Percent sizes for InstancedText<T> Node entities.
        app.add_systems(Startup, ensure_default_ui_camera);

        app.add_systems(
            PostUpdate,
            (
                prewarm_atlas_for_layout
                    .run_if(atlas_ready)
                    .before(update_text_views),
                update_text_views
                    .run_if(atlas_ready)
                    .in_set(TextViewRenderSet),
            ),
        );
    }
}

/// Full bundle: [`GlyphAtlasPlugin`] + [`InstancedTextRenderPlugin`]
/// + [`InstancedTextPlugin`].
pub struct InstancedTextPlugins;

impl PluginGroup for InstancedTextPlugins {
    fn build(self) -> PluginGroupBuilder {
        PluginGroupBuilder::start::<Self>()
            .add(GlyphAtlasPlugin)
            .add(InstancedTextRenderPlugin)
            .add(InstancedTextPlugin)
    }
}

/// The per-text-view columns `update_text_views` reads. Bundled into a
/// `QueryData` so the system signature stays legible.
#[derive(bevy::ecs::query::QueryData)]
pub struct TextViewRow {
    entity: Entity,
    scroll: &'static ScrollPosition,
    computed: &'static ComputedNode,
    stack_index: &'static bevy::ui::ComputedStackIndex,
    ui_transform: Ref<'static, UiGlobalTransform>,
    clip: Option<&'static CalculatedClip>,
    target_cam: Option<&'static ComputedUiTargetCamera>,
    font: &'static TextFont,
    faces_cfg: &'static MonoFontFaces,
    text_layout: Option<&'static bevy::text::TextLayout>,
    layout: Ref<'static, DisplayLayout>,
    underlays: Ref<'static, TextUnderlays>,
    overlays: Ref<'static, TextOverlays>,
    batch_entity: Option<&'static TextViewBatchEntity>,
    render_layers: Option<&'static bevy_camera::visibility::RenderLayers>,
    inherited_vis: Ref<'static, InheritedVisibility>,
}

pub fn update_text_views(
    mut commands: Commands,
    mut text_views: Query<TextViewRow, With<DisplayLayout>>,
    mut atlas: ResMut<GlyphAtlas>,
    mut images: ResMut<Assets<Image>>,
    fonts: Res<Assets<bevy::text::Font>>,
) {
    let _span = bevy::prelude::info_span!("update_text_views").entered();
    for row in text_views.iter_mut() {
        let TextViewRowItem {
            entity: tv_entity,
            scroll,
            computed,
            stack_index,
            ui_transform,
            clip,
            target_cam,
            font,
            faces_cfg,
            text_layout,
            layout,
            underlays,
            overlays,
            batch_entity: batch_entity_opt,
            render_layers,
            inherited_vis,
        } = row;
        let justify = text_layout.map(|t| t.justify).unwrap_or_default();
        // Bevy's text pipeline resolves fonts by `Handle<Font>` at shape time;
        // the renderer just needs the per-axis handles.
        let regular = match &font.font {
            bevy::text::FontSource::Handle(h) => Some(h.clone()),
            _ => Some(Handle::<bevy::text::Font>::default()),
        };
        let bold = faces_cfg.font_bold.clone();
        let italic = faces_cfg.font_italic.clone();
        let bold_italic = faces_cfg.font_bold_italic.clone();
        let faces = super::render::FontFaces {
            regular,
            bold,
            italic,
            bold_italic,
            synthesis: faces_cfg.font_synthesis,
        };
        // Skip the rebuild if nothing changed — the GPU batch is still valid.
        // `InheritedVisibility` must be checked so that a parent toggling
        // `Display::None` → `Display::Flex` triggers a batch rebuild (the
        // extraction layer gates on visibility, so the batch goes stale
        // while hidden).
        if !layout.is_changed()
            && !underlays.is_changed()
            && !overlays.is_changed()
            && !ui_transform.is_changed()
            && !inherited_vis.is_changed()
            && batch_entity_opt.is_some()
        {
            continue;
        }
        let layout: &DisplayLayout = &layout;
        let inv = computed.inverse_scale_factor();
        let inset = computed.content_inset();
        let content_start_x = inset.min_inset.x * inv;
        let content_end_inset_x = inset.max_inset.x * inv;

        // `UiGlobalTransform` is anchored at the node's center (in
        // screen physical px), so map top-left logical px → center
        // physical px before applying it. Mirrors Bevy UI's text-extract
        // `Affine2::from(*transform) * Affine2::from_translation(-0.5 * size)`.
        let scale = 1.0 / inv;
        let size_phys = computed.size();
        let ui_affine: Affine2 = **ui_transform;
        let composed = ui_affine
            * Affine2::from_translation(-0.5 * size_phys)
            * Affine2::from_scale(Vec2::splat(scale));
        let base_batch_transform = BatchTransform {
            affine: [
                composed.matrix2.x_axis.x,
                composed.matrix2.y_axis.x,
                composed.translation.x,
                composed.matrix2.x_axis.y,
                composed.matrix2.y_axis.y,
                composed.translation.y,
            ],
            clip: clip.map(|c| c.clip),
            stack_index: stack_index.0,
            target_camera: target_cam.and_then(|c| c.get()),
            // Set per sub-batch below.
            sub_order: 0.0,
        };

        let render_output = {
            let _render_span = bevy::prelude::info_span!("render_layout").entered();
            render_layout(
                layout,
                &underlays.0,
                &overlays.0,
                computed,
                &mut atlas,
                &fonts,
                &mut images,
                super::render::RenderContext {
                    content_start_x,
                    content_end_inset_x,
                    horizontal_scroll_offset: scroll.x,
                    font_size: super::font::font_size_px(font.font_size),
                    faces,
                    justify,
                },
            )
        };
        let super::render::RenderOutput { batches } = render_output;

        atlas.update_texture(&mut images);

        let logical = computed.size() * inv;
        let text_area_top = computed.content_inset().min_inset.y * inv;
        let line_height = layout.line_height;
        let start_pixels = scroll.y - text_area_top;
        let first_visible = (start_pixels / line_height).floor().max(0.0) as usize;
        let visible_count = (logical.y / line_height).ceil() as usize;
        let last_visible = first_visible + visible_count;

        let batch_data = TextViewBatch {
            built_at_scroll: scroll.y,
            built_at_horizontal_scroll: scroll.x,
            first_line: first_visible,
            last_line: last_visible,
            built_at_width: logical.x as u32,
            built_at_height: logical.y as u32,
        };

        // A view now emits one batch entity per atlas-texture × painter tier
        // group (`render_layout` returns them in draw order). Despawn last
        // frame's batch entities and respawn — the group count varies with the
        // mix of font sizes on screen, so reusing entities 1:1 isn't reliable.
        let prev_batches = batch_entity_opt.map(|b| b.0.as_slice()).unwrap_or(&[]);

        if batches.is_empty() {
            for &batch_e in prev_batches {
                commands.entity(batch_e).insert(Visibility::Hidden);
            }
            continue;
        }

        for &batch_e in prev_batches {
            commands.entity(batch_e).despawn();
        }

        let layer = render_layers.and_then(|l| {
            (0u8..=31)
                .find(|&i| l.intersects(&bevy_camera::visibility::RenderLayers::layer(i as usize)))
        });

        // Spacing between sub-batch sort offsets. `stack_z_offsets::TEXT` is
        // 0.06 and adjacent UI stack indices differ by 1.0, so a tiny step per
        // sub-batch keeps the whole group inside this view's slot while
        // preserving below → text → above ordering.
        const SUB_ORDER_STEP: f32 = 0.0001;

        let mut new_batch_entities = Vec::with_capacity(batches.len());
        for (i, group) in batches.into_iter().enumerate() {
            let batch_comp = GlyphBatchComponent {
                instances: group.instances,
                // Each group binds exactly the texture its instances sample —
                // the `FontAtlasSet` texture for glyphs, or the solid texture
                // for background/overlay quads.
                atlas_texture: group.texture,
                render_layer: layer,
            };
            let batch_transform = BatchTransform {
                sub_order: i as f32 * SUB_ORDER_STEP,
                ..base_batch_transform
            };

            // Parent the batch under the text-view so Bevy's
            // `propagate_visibility` cascade reaches its
            // `InheritedVisibility`. Our custom render-world extract
            // (`extract_visible_ui_components`) then gates on that —
            // matching how bevy_ui_render handles UI element
            // visibility. We deliberately don't use bevy's
            // `extract_visible()` helper since it gates on
            // `ViewVisibility`, which is set by `check_visibility`
            // and that system requires `GlobalTransform` (UI nodes
            // have `UiGlobalTransform`, not `GlobalTransform`).
            //
            // `Inherited` (not `Visible`) so the parent text-view's
            // visibility can hide this batch via the propagate cascade.
            let mut entity_cmds = commands.spawn((
                batch_comp,
                batch_transform,
                batch_data.clone(),
                Name::new("TextViewBatch"),
                Visibility::Inherited,
                InheritedVisibility::default(),
                ChildOf(tv_entity),
            ));
            if let Some(layers) = render_layers {
                entity_cmds.insert(layers.clone());
            }
            new_batch_entities.push(entity_cmds.id());
        }

        commands
            .entity(tv_entity)
            .insert(TextViewBatchEntity(new_batch_entities));
    }
}

/// Mark one camera as the default UI camera if none is marked yet.
/// This lets Bevy UI resolve `Val::Percent` sizes for `TextView` `Node` entities
/// without requiring hosts to manually add `IsDefaultUiCamera` to their camera.
fn ensure_default_ui_camera(
    mut commands: Commands,
    cameras: Query<Entity, With<Camera>>,
    already_marked: Query<(), With<IsDefaultUiCamera>>,
) {
    if !already_marked.is_empty() {
        return;
    }
    if let Some(entity) = cameras.iter().next() {
        commands.entity(entity).insert(IsDefaultUiCamera);
    }
}

/// Pre-rasterize every glyph in a freshly-built `DisplayLayout` so the renderer
/// never triggers atlas mutation during the paint pass (eliminates scroll stutter).
pub(crate) fn prewarm_atlas_for_layout(
    layouts: Query<Ref<DisplayLayout>>,
    mut atlas: ResMut<GlyphAtlas>,
) {
    for layout in &layouts {
        if !layout.is_changed() {
            continue;
        }
        // ShapedLine.shape already carries cache_keys; per-run font_scale overrides
        // re-shape at paint time (rare enough that mid-paint rasterization is fine).
        atlas.ensure_glyphs(layout.lines.iter().flat_map(|l| {
            l.shape
                .as_ref()
                .map(|s| s.glyphs.iter().map(|g| g.cache_key))
                .into_iter()
                .flatten()
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intrinsic_width_max_is_widest_line() {
        // Three lines; the middle one ("fourteen chars" = 14) is widest. Cell 10px.
        let buffer = String::from("short\nfourteen chars\nmid");
        let (max_w, _min_w) = intrinsic_widths(&buffer, 10.0);
        assert_eq!(
            max_w, 140.0,
            "max-content = widest line's char count × cell"
        );
    }

    #[test]
    fn intrinsic_width_min_is_longest_word() {
        // Longest whitespace-delimited run is "wrapping" (8 chars).
        let buffer = String::from("a soft wrapping budget");
        let (_max_w, min_w) = intrinsic_widths(&buffer, 10.0);
        assert_eq!(min_w, 80.0, "min-content = longest unbreakable run × cell");
    }

    #[test]
    fn intrinsic_width_ignores_trailing_newline() {
        // Trailing '\n' adds a virtual empty line; it must not inflate width.
        let buffer = String::from("abcd\n");
        let (max_w, _min_w) = intrinsic_widths(&buffer, 10.0);
        assert_eq!(max_w, 40.0);
    }
}

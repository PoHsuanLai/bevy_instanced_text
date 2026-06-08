//! Text view plugin — registers the rendering systems that turn
//! `TextBuffer<T>` entities into GPU draw batches.
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
use super::text::{ContentMetrics, TextBuffer, TextContent};
use super::text_access::{produce_layouts, LayoutProduceSet};
use super::text_style::TextBounds;
use crate::gpu::{atlas_ready, GlyphAtlas, GlyphAtlasPlugin, InstancedTextRenderPlugin};
pub use bevy::text::{TextBackgroundColor, TextColor};

/// Contains `update_text_views`. Order downstream `.after(TextViewRenderSet)`.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct TextViewRenderSet;

/// Taffy `Measure` reporting a `TextBuffer`'s intrinsic size so flex
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
struct TextBufferMeasure {
    line_height: f32,
    max_content_width: f32,
    min_content_width: f32,
}

impl Measure for TextBufferMeasure {
    fn measure(&mut self, args: MeasureArgs<'_>, _style: &taffy::Style) -> bevy::math::Vec2 {
        use bevy::ui::AvailableSpace;
        let width = args.width.unwrap_or_else(|| match args.available_width {
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
        for word in trimmed.split(|c: char| c == ' ' || c == '\t') {
            min_chars = min_chars.max(word.chars().count());
        }
    }
    (max_chars as f32 * cell_px, min_chars as f32 * cell_px)
}

/// Links a text view to its batch rendering entity. Managed by `update_text_views`.
#[derive(Component, Reflect)]
#[reflect(Component)]
pub struct TextViewBatchEntity(pub Entity);

/// Registers `produce_layouts::<T>` for a specific [`TextContent`] type.
///
/// Add one of these per content type you use. [`InstancedTextPlugin`]
/// automatically adds `TextContentPlugin::<String>` for the simple label
/// use case. Editor / terminal hosts add their own (e.g.
/// `TextContentPlugin::<Rope>`).
pub struct TextContentPlugin<T: TextContent + Component>(PhantomData<T>);

impl<T: TextContent + Component> Default for TextContentPlugin<T> {
    fn default() -> Self {
        Self(PhantomData)
    }
}

impl<T: TextContent + Component> Plugin for TextContentPlugin<T> {
    fn build(&self, app: &mut App) {
        // Register required components so spawning TextBuffer<T> alone is enough.
        app.world_mut()
            .register_required_components_with::<TextBuffer<T>, bevy::text::LineHeight>(|| {
                bevy::text::LineHeight::RelativeToFont(1.5)
            });
        app.world_mut()
            .register_required_components::<TextBuffer<T>, ScrollPosition>();
        app.world_mut()
            .register_required_components::<TextBuffer<T>, ContentMetrics>();
        app.world_mut()
            .register_required_components::<TextBuffer<T>, DisplayLayout>();
        app.world_mut()
            .register_required_components::<TextBuffer<T>, TextUnderlays>();
        app.world_mut()
            .register_required_components::<TextBuffer<T>, TextOverlays>();
        app.world_mut()
            .register_required_components::<TextBuffer<T>, TextFont>();
        app.world_mut()
            .register_required_components::<TextBuffer<T>, TextColor>();
        app.world_mut()
            .register_required_components::<TextBuffer<T>, bevy::text::TextLayout>();
        app.world_mut()
            .register_required_components::<TextBuffer<T>, TextBackgroundColor>();
        app.world_mut()
            .register_required_components::<TextBuffer<T>, MonoFontFaces>();
        app.world_mut()
            .register_required_components::<TextBuffer<T>, MonoCellWidth>();

        app.world_mut()
            .register_required_components::<TextBuffer<T>, TextBounds>();
        app.world_mut()
            .register_required_components::<TextBuffer<T>, super::text_style::LineStyles>();
        app.world_mut()
            .register_required_components::<TextBuffer<T>, super::text_style::HiddenLines>();
        app.world_mut()
            .register_required_components::<TextBuffer<T>, LayoutTuning>();
        app.world_mut()
            .register_required_components::<TextBuffer<T>, Node>();
        app.world_mut()
            .register_required_components::<TextBuffer<T>, ContentSize>();
        app.world_mut()
            .register_required_components::<TextBuffer<T>, Visibility>();
        app.world_mut()
            .register_required_components_with::<TextBuffer<T>, InheritedVisibility>(|| {
                InheritedVisibility::VISIBLE
            });
        app.world_mut()
            .register_required_components::<TextBuffer<T>, bevy::picking::Pickable>();

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

/// Installs a [`TextBufferMeasure`] on every `TextBuffer<T>` entity so bevy_ui
/// knows their intrinsic line height and width. Runs in `UiSystems::Content`,
/// before taffy lays out the tree. Only updates when an input that affects the
/// measured size changes so layout invalidation stays minimal.
fn measure_text_buffer<T: TextContent + Component>(
    mut q: Query<
        (
            &mut ContentSize,
            &TextBuffer<T>,
            &bevy::text::LineHeight,
            &TextFont,
            &MonoCellWidth,
        ),
        (
            With<TextBuffer<T>>,
            Or<(
                Changed<TextBuffer<T>>,
                Changed<bevy::text::LineHeight>,
                Changed<TextFont>,
                Changed<MonoCellWidth>,
                Added<ContentSize>,
            )>,
        ),
    >,
) {
    for (mut content_size, buffer, line_height, font, mono) in q.iter_mut() {
        let lh = resolve_line_height(*line_height, font.font_size);
        let (max_content_width, min_content_width) = intrinsic_widths(&**buffer, mono.px);
        content_size.set(NodeMeasure::Custom(Box::new(TextBufferMeasure {
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

        app.register_type::<super::text::TextSpan>();
        // Register the TextSpan content type so simple labels work out of the box.
        app.add_plugins(TextContentPlugin::<super::text::TextSpan>::default());

        // Ensure there is always a camera marked as the default UI camera so
        // Bevy UI layout can resolve Val::Percent sizes for TextBuffer<T> Node entities.
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
        let regular = atlas.ensure_font(&font.font, &fonts);
        let bold = faces_cfg
            .font_bold
            .as_ref()
            .and_then(|h| atlas.ensure_font(h, &fonts));
        let italic = faces_cfg
            .font_italic
            .as_ref()
            .and_then(|h| atlas.ensure_font(h, &fonts));
        let bold_italic = faces_cfg
            .font_bold_italic
            .as_ref()
            .and_then(|h| atlas.ensure_font(h, &fonts));
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
        let batch_transform = BatchTransform {
            affine: [
                composed.matrix2.x_axis.x,
                composed.matrix2.y_axis.x,
                composed.translation.x,
                composed.matrix2.x_axis.y,
                composed.matrix2.y_axis.y,
                composed.translation.y,
            ],
            clip: clip.map(|c| c.clip),
            stack_index: computed.stack_index,
            target_camera: target_cam.and_then(|c| c.get()),
        };

        let instances = {
            let _render_span = bevy::prelude::info_span!("render_layout").entered();
            render_layout(
                layout,
                &underlays.0,
                &overlays.0,
                computed,
                &mut atlas,
                &fonts,
                super::render::RenderContext {
                    content_start_x,
                    content_end_inset_x,
                    horizontal_scroll_offset: scroll.x,
                    font_size: font.font_size,
                    faces,
                    justify,
                },
            )
        };

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

        if instances.is_empty() {
            if let Some(batch_e) = batch_entity_opt {
                commands.entity(batch_e.0).insert(Visibility::Hidden);
            }
            continue;
        }

        let layer = render_layers.and_then(|l| {
            (0u8..=31)
                .find(|&i| l.intersects(&bevy_camera::visibility::RenderLayers::layer(i as usize)))
        });
        let batch_comp = GlyphBatchComponent {
            instances,
            atlas_texture: atlas.texture.clone(),
            render_layer: layer,
        };

        if let Some(batch_e) = batch_entity_opt {
            let mut cmds = commands.entity(batch_e.0);
            // `Inherited` (not `Visible`) so the parent text-view's
            // visibility can hide this batch via the propagate cascade.
            // `Visible` here would override and force-draw regardless
            // of any ancestor `Visibility::Hidden`.
            cmds.insert(batch_comp)
                .insert(batch_transform)
                .insert(Visibility::Inherited)
                .insert(batch_data);
            if let Some(layers) = render_layers {
                cmds.insert(layers.clone());
            }
        } else {
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
            let mut entity_cmds = commands.spawn((
                batch_comp,
                batch_transform,
                batch_data,
                Name::new("TextViewBatch"),
                Visibility::Inherited,
                InheritedVisibility::default(),
                ChildOf(tv_entity),
            ));
            if let Some(layers) = render_layers {
                entity_cmds.insert(layers.clone());
            }
            let batch_entity = entity_cmds.id();
            commands
                .entity(tv_entity)
                .insert(TextViewBatchEntity(batch_entity));
        }
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
        assert_eq!(max_w, 140.0, "max-content = widest line's char count × cell");
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

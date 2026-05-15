//! Text view plugin — registers the rendering and scroll animation systems
//! that turn `TextBuffer<T>` entities into GPU draw batches.
//!
//! [`InstancedTextPlugin`] sets up the core rendering infrastructure.
//! [`TextContentPlugin<T>`] registers `produce_layouts::<T>` for a specific
//! content type — add one per `T` you use. [`InstancedTextPlugins`] bundles
//! everything including the `String` content type for simple labels.

use std::marker::PhantomData;

use bevy::app::{PluginGroup, PluginGroupBuilder};
use bevy::prelude::*;
use bevy::math::Affine2;
use bevy::ui::{ui_transform::UiGlobalTransform, CalculatedClip, ComputedNode, ComputedUiTargetCamera, IsDefaultUiCamera, UiSystems};

use super::font::{MonoCellWidth, MonoFontFaces};
use super::pipeline::DisplayLayout;
use super::text_access::{produce_layouts, LayoutProduceSet};
use super::overlay::{TextOverlays, TextUnderlays};
use super::render::{render_layout, BatchTransform, GlyphBatchComponent, TextViewBatch};
use super::text::{CompositeStops, ContentMetrics, HorizontalScroll, ScrollAnimation, ScrollAxis, TextBuffer, TextContent, VerticalScroll};
use super::text_style::TextBounds;
use super::color::{TextBackgroundColor, TextColor};
use super::measurement::LayoutTuning;
use crate::gpu::{atlas_ready, GlyphAtlas, GlyphAtlasPlugin, InstancedTextRenderPlugin};

/// Contains `update_text_views`. Order downstream `.after(TextViewRenderSet)`.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct TextViewRenderSet;

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
            .register_required_components::<TextBuffer<T>, VerticalScroll>();
        app.world_mut()
            .register_required_components::<TextBuffer<T>, HorizontalScroll>();
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
            .register_required_components::<TextBuffer<T>, Transform>();
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
            produce_layouts::<T>
                .run_if(atlas_ready)
                .in_set(LayoutProduceSet)
                .after(UiSystems::Layout)
                .before(prewarm_atlas_for_layout),
        );
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
            .register_type::<TextColor>()
            .register_type::<TextBackgroundColor>()
            .register_type::<TextViewBatchEntity>()
            .register_type::<TextUnderlays>()
            .register_type::<TextOverlays>()
            .register_type::<VerticalScroll>()
            .register_type::<HorizontalScroll>()
            .register_type::<ContentMetrics>();

        app.register_type::<super::text::TextSpan>();
        // Register the TextSpan content type so simple labels work out of the box.
        app.add_plugins(TextContentPlugin::<super::text::TextSpan>::default());

        app.add_systems(
            PostUpdate,
            (animate_vertical_scroll, animate_horizontal_scroll).before(UiSystems::Layout),
        );

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

// +0.010: pretend the animation already started 10ms ago for an instant visual response.
// VSCode does the same: startTime = Date.now() - 10, duration = base + 10.
const SCROLL_BACKDATE_SECS: f32 = 0.010;
const SCROLL_BACKDATE_DURATION: f32 = 0.010;
const COMPOSITE_SPLIT: f32 = 0.33;
const COMPOSITE_VIEWPORT_THRESHOLD: f32 = 2.5;
const COMPOSITE_STOP_INSET: f32 = 0.75;

pub(crate) fn build_animation(
    from: f32,
    to: f32,
    duration: f32,
    viewport_size: f32,
) -> ScrollAnimation {
    let composite = if viewport_size > 0.0
        && (to - from).abs() > COMPOSITE_VIEWPORT_THRESHOLD * viewport_size
    {
        let inset = COMPOSITE_STOP_INSET * viewport_size;
        let (stop1, stop2) = if from < to {
            (from + inset, to - inset)
        } else {
            (from - inset, to + inset)
        };
        Some(CompositeStops {
            stop1,
            stop2,
            split: COMPOSITE_SPLIT,
        })
    } else {
        None
    };
    ScrollAnimation {
        from,
        to,
        elapsed: SCROLL_BACKDATE_SECS,
        duration: (duration + SCROLL_BACKDATE_DURATION).max(0.001),
        composite,
    }
}

/// Single-axis scroll step. Reads the existing `axis`, advances or rebuilds
/// the easing animation, returns the new state. Pure — no Bevy types.
///
/// `dt` is per-frame delta, `viewport_size` is the relevant axis dimension
/// (height for vertical, width for horizontal — used to detect "huge jumps"
/// that warrant the composite curve).
///
/// Returns `Some(new_axis)` if the state changed (so the caller writes through
/// `Mut` and triggers change detection), `None` if nothing moved.
fn step_axis(axis: &ScrollAxis, dt: f32, viewport_size: f32) -> Option<ScrollAxis> {
    let mut anim = match &axis.anim {
        Some(a) if (a.to - axis.target).abs() <= f32::EPSILON => a.clone(),
        _ if (axis.target - axis.current).abs() > 0.5 => {
            build_animation(axis.current, axis.target, axis.duration, viewport_size)
        }
        // No anim in progress and target is already close to current — nothing to do.
        _ => return None,
    };

    let (new_current, finished) = anim.advance(dt);
    let new_anim = if finished { None } else { Some(anim) };

    let current_changed = (new_current - axis.current).abs() > 1e-4;
    let anim_state_changed = axis.anim.is_some() != new_anim.is_some();
    if !current_changed && !anim_state_changed {
        return None;
    }

    Some(ScrollAxis {
        target: axis.target,
        current: new_current,
        duration: axis.duration,
        anim: new_anim,
    })
}

fn animate_vertical_scroll(
    mut query: Query<(&mut VerticalScroll, &ComputedNode), With<DisplayLayout>>,
    time: Res<Time>,
) {
    let dt = time.delta_secs();
    for (mut axis, computed) in query.iter_mut() {
        let viewport_h = computed.size().y * computed.inverse_scale_factor();
        if let Some(next) = step_axis(&axis.0, dt, viewport_h) {
            axis.0 = next;
        }
    }
}

fn animate_horizontal_scroll(
    mut query: Query<(&mut HorizontalScroll, &ComputedNode), With<DisplayLayout>>,
    time: Res<Time>,
) {
    let dt = time.delta_secs();
    for (mut axis, computed) in query.iter_mut() {
        let viewport_w = computed.size().x * computed.inverse_scale_factor();
        if let Some(next) = step_axis(&axis.0, dt, viewport_w) {
            axis.0 = next;
        }
    }
}

#[allow(clippy::type_complexity)]
pub fn update_text_views(
    mut commands: Commands,
    mut text_views: Query<
        (
            Entity,
            &VerticalScroll,
            &HorizontalScroll,
            &ComputedNode,
            &UiGlobalTransform,
            Option<&CalculatedClip>,
            Option<&ComputedUiTargetCamera>,
            &TextFont,
            &MonoFontFaces,
            Option<&bevy::text::TextLayout>,
            Ref<DisplayLayout>,
            Ref<TextUnderlays>,
            Ref<TextOverlays>,
            Option<&TextViewBatchEntity>,
            Option<&bevy_camera::visibility::RenderLayers>,
        ),
        With<DisplayLayout>,
    >,
    mut atlas: ResMut<GlyphAtlas>,
    mut images: ResMut<Assets<Image>>,
    fonts: Res<Assets<bevy::text::Font>>,
) {
    let _span = bevy::prelude::info_span!("update_text_views").entered();
    for (tv_entity, v_scroll, h_scroll, computed, ui_transform, clip, target_cam, font, faces_cfg, text_layout, layout, underlays, overlays, batch_entity_opt, render_layers) in
        text_views.iter_mut()
    {
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
        if !layout.is_changed() && !underlays.is_changed() && !overlays.is_changed() && batch_entity_opt.is_some() {
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
                composed.matrix2.x_axis.x, composed.matrix2.y_axis.x, composed.translation.x,
                composed.matrix2.x_axis.y, composed.matrix2.y_axis.y, composed.translation.y,
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
                    horizontal_scroll_offset: h_scroll.current,
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
        let start_pixels = v_scroll.current - text_area_top;
        let first_visible = (start_pixels / line_height).floor().max(0.0) as usize;
        let visible_count = (logical.y / line_height).ceil() as usize;
        let last_visible = first_visible + visible_count;

        let batch_data = TextViewBatch {
            built_at_scroll: v_scroll.current,
            built_at_horizontal_scroll: h_scroll.current,
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
            cmds.insert(batch_comp)
                .insert(batch_transform)
                .insert(Visibility::Visible)
                .insert(batch_data);
            if let Some(layers) = render_layers {
                cmds.insert(layers.clone());
            }
        } else {
            let mut entity_cmds = commands.spawn((
                batch_comp,
                batch_transform,
                batch_data,
                Name::new("TextViewBatch"),
                Visibility::Visible,
                InheritedVisibility::default(),
                ViewVisibility::default(),
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

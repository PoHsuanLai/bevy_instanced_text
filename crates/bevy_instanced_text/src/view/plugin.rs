//! Text view plugin — registers the rendering and scroll animation systems
//! that turn `TextView` entities into GPU draw batches.
//!
//! This module also defines [`InstancedTextPlugins`], a [`PluginGroup`] that
//! bundles the GPU plugins from [`crate::gpu`] together with the view-side
//! [`InstancedTextPlugin`]. Hosts that just want "render styled text" should
//! add `InstancedTextPlugins`; those that already manage the GPU pipeline
//! themselves can add [`InstancedTextPlugin`] alone.

use bevy::app::{PluginGroup, PluginGroupBuilder};
use bevy::prelude::*;

use super::font::FontConfig;
use super::layout::DisplayLayout;
use super::layout_builder::{produce_block_layout, produce_layouts, LayoutProduceSet};
use super::overlay::TextViewOverlays;
use super::render::{render_layout, GlyphBatchComponent, TextViewBatch};
use super::state::{CompositeStops, ContentMetrics, ScrollAnimation, ScrollState, TextBuffer};
use super::styling::LayoutWrap;
use super::theme::{BlockDecorTheme, RenderTheme};
use super::tuning::LayoutTuning;
use super::viewport::TextViewViewport;
use crate::gpu::{atlas_ready, GlyphAtlas, GlyphAtlasPlugin, InstancedTextRenderPlugin};

/// Contains `update_text_views`. Order downstream `.after(TextViewRenderSet)`.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct TextViewRenderSet;

/// Marker for a text view rendered by [`InstancedTextPlugin`]. `#[require]`
/// cascades the rest of the rendering machinery — spawning `TextView` alone
/// is enough. Includes `Pickable` so `bevy_instanced_text_edit::picking` can produce
/// `PointerHits` without the engine needing to register the backend itself.
#[derive(Component, Default, Reflect)]
#[reflect(Component, Default)]
#[require(
    TextBuffer,
    ScrollState,
    ContentMetrics,
    TextViewViewport,
    DisplayLayout,
    TextViewOverlays,
    FontConfig,
    LayoutWrap,
    LayoutTuning,
    Transform,
    Visibility,
    bevy::picking::Pickable,
)]
pub struct TextView;

/// Links a text view to its batch rendering entity. Managed by `update_text_views`.
#[derive(Component, Reflect)]
#[reflect(Component)]
pub struct TextViewBatchEntity(pub Entity);

/// Registers the rendering and scroll animation systems. Does not add GPU
/// plugins — use [`InstancedTextPlugins`] for the full bundle.
#[derive(Default)]
pub struct InstancedTextPlugin;

impl Plugin for InstancedTextPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<LayoutTuning>();

        app.register_type::<FontConfig>()
            .register_type::<super::overlay::RectOverlay>()
            .register_type::<super::overlay::RowVertical>()
            .register_type::<LayoutWrap>()
            .register_type::<RenderTheme>()
            .register_type::<BlockDecorTheme>()
            .register_type::<TextView>()
            .register_type::<TextViewBatchEntity>()
            .register_type::<TextViewOverlays>()
            .register_type::<TextBuffer>()
            .register_type::<ScrollState>()
            .register_type::<ContentMetrics>()
            .register_type::<TextViewViewport>()
            .register_type::<super::viewport::ViewportOrigin>();

        app.add_systems(
            Update,
            (
                animate_text_view_scroll,
                produce_layouts
                    .run_if(atlas_ready)
                    .in_set(LayoutProduceSet)
                    .before(prewarm_atlas_for_layout),
                // Mutually exclusive with `produce_layouts` at the entity level —
                // `produce_layouts` filters out entities that have `BlockList`.
                produce_block_layout
                    .in_set(LayoutProduceSet)
                    .before(prewarm_atlas_for_layout),
                prewarm_atlas_for_layout
                    .run_if(atlas_ready)
                    .before(update_text_views),
                update_text_views
                    .run_if(atlas_ready)
                    .in_set(TextViewRenderSet),
            )
                .chain(),
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

// -0.010: backdate so the first rendered frame already shows motion (VSCode does the same).
const SCROLL_BACKDATE_SECS: f32 = -0.010;
const COMPOSITE_SPLIT: f32 = 0.33;
const COMPOSITE_VIEWPORT_THRESHOLD: f32 = 2.5;
const COMPOSITE_STOP_INSET: f32 = 0.75;

#[inline]
fn ease_out_cubic(t: f32) -> f32 {
    let inv = 1.0 - t;
    1.0 - inv * inv * inv
}

#[inline]
fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn sample_animation(anim: &ScrollAnimation) -> f32 {
    let t = (anim.elapsed / anim.duration).clamp(0.0, 1.0);
    match &anim.composite {
        None => lerp(anim.from, anim.to, ease_out_cubic(t)),
        Some(c) => {
            if t < c.split {
                let local = t / c.split;
                lerp(anim.from, c.stop1, ease_out_cubic(local))
            } else {
                let local = (t - c.split) / (1.0 - c.split);
                lerp(c.stop2, anim.to, ease_out_cubic(local))
            }
        }
    }
}

fn build_animation(from: f32, to: f32, duration: f32, viewport_size: f32) -> ScrollAnimation {
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
        duration: duration.max(0.001),
        composite,
    }
}

fn animate_text_view_scroll(
    mut query: Query<(&mut ScrollState, &TextViewViewport), With<TextView>>,
    time: Res<Time>,
) {
    let dt = time.delta_secs();

    for (mut state, viewport) in query.iter_mut() {
        let viewport_h = viewport.height as f32;
        let viewport_w = viewport.width as f32;
        let duration = state.smooth_scroll_duration;

        let needs_new_v = match &state.vertical_anim {
            Some(anim) => (anim.to - state.target_scroll_offset).abs() > f32::EPSILON,
            None => (state.target_scroll_offset - state.scroll_offset).abs() > 0.5,
        };
        if needs_new_v {
            let from = state.scroll_offset;
            let to = state.target_scroll_offset;
            state.vertical_anim = Some(build_animation(from, to, duration, viewport_h));
        }
        if let Some(mut anim) = state.vertical_anim.take() {
            anim.elapsed += dt;
            if anim.elapsed >= anim.duration {
                state.scroll_offset = anim.to;
                state.vertical_anim = None;
            } else {
                state.scroll_offset = sample_animation(&anim);
                state.vertical_anim = Some(anim);
            }
        }

        let needs_new_h = match &state.horizontal_anim {
            Some(anim) => (anim.to - state.target_horizontal_scroll_offset).abs() > f32::EPSILON,
            None => {
                (state.target_horizontal_scroll_offset - state.horizontal_scroll_offset).abs() > 0.5
            }
        };
        if needs_new_h {
            let from = state.horizontal_scroll_offset;
            let to = state.target_horizontal_scroll_offset;
            state.horizontal_anim = Some(build_animation(from, to, duration, viewport_w));
        }
        if let Some(mut anim) = state.horizontal_anim.take() {
            anim.elapsed += dt;
            if anim.elapsed >= anim.duration {
                state.horizontal_scroll_offset = anim.to;
                state.horizontal_anim = None;
            } else {
                state.horizontal_scroll_offset = sample_animation(&anim);
                state.horizontal_anim = Some(anim);
            }
        }
    }
}

#[allow(clippy::type_complexity)]
pub(crate) fn update_text_views(
    mut commands: Commands,
    mut text_views: Query<
        (
            Entity,
            &ScrollState,
            &TextViewViewport,
            &FontConfig,
            Ref<DisplayLayout>,
            Option<Ref<TextViewOverlays>>,
            Option<&TextViewBatchEntity>,
            Option<&bevy_camera::visibility::RenderLayers>,
        ),
        With<TextView>,
    >,
    mut atlas: ResMut<GlyphAtlas>,
    mut images: ResMut<Assets<Image>>,
    fonts: Res<Assets<bevy::text::Font>>,
) {
    for (tv_entity, scroll, viewport, font, layout, overlays, batch_entity_opt, render_layers) in
        text_views.iter_mut()
    {
        let regular = atlas.ensure_font(&font.font, &fonts);
        let bold = font
            .font_bold
            .as_ref()
            .and_then(|h| atlas.ensure_font(h, &fonts));
        let italic = font
            .font_italic
            .as_ref()
            .and_then(|h| atlas.ensure_font(h, &fonts));
        let bold_italic = font
            .font_bold_italic
            .as_ref()
            .and_then(|h| atlas.ensure_font(h, &fonts));
        let faces = super::render::FontFaces {
            regular,
            bold,
            italic,
            bold_italic,
            synthesis: font.font_synthesis,
        };
        // Skip the rebuild if neither layout nor overlays changed — the GPU batch is still valid.
        let overlays_changed = overlays.as_ref().map(|o| o.is_changed()).unwrap_or(false);
        if !layout.is_changed() && !overlays_changed && batch_entity_opt.is_some() {
            continue;
        }
        let layout: &DisplayLayout = &layout;
        let overlays = overlays.as_deref();
        let content_start_x = if viewport.gutter_width > 0.0 {
            viewport.text_area_left.max(viewport.gutter_width)
        } else {
            viewport.text_area_left
        };

        let instances = render_layout(
            layout,
            overlays,
            viewport,
            &mut atlas,
            content_start_x,
            scroll.horizontal_scroll_offset,
            font.font_size,
            faces,
        );

        atlas.update_texture(&mut images);

        let line_height = layout.line_height;
        let scroll_dist = scroll.scroll_offset.abs();
        let start_pixels = scroll_dist - viewport.text_area_top;
        let first_visible = (start_pixels / line_height).floor().max(0.0) as usize;
        let visible_count = ((viewport.height as f32) / line_height).ceil() as usize;
        let last_visible = first_visible + visible_count;

        let batch_data = TextViewBatch {
            built_at_scroll: scroll.scroll_offset,
            built_at_horizontal_scroll: scroll.horizontal_scroll_offset,
            first_line: first_visible,
            last_line: last_visible,
            built_at_width: viewport.width,
            built_at_height: viewport.height,
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
                .insert(Visibility::Visible)
                .insert(batch_data);
            if let Some(layers) = render_layers {
                cmds.insert(layers.clone());
            }
        } else {
            let mut entity_cmds = commands.spawn((
                batch_comp,
                Transform::default(),
                GlobalTransform::default(),
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

/// Pre-rasterize every glyph in a freshly-built `DisplayLayout` so the renderer
/// never triggers atlas mutation during the paint pass (eliminates scroll stutter).
pub(crate) fn prewarm_atlas_for_layout(
    layouts: Query<Ref<DisplayLayout>, With<TextView>>,
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

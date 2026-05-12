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
use bevy::ui::{ComputedNode, IsDefaultUiCamera, UiSystems};

use super::font::TextFont;
use super::layout::DisplayLayout;
use super::layout_builder::{produce_layouts, LayoutProduceSet};
use super::overlay::TextViewOverlays;
use super::render::{render_layout, GlyphBatchComponent, TextViewBatch};
use super::state::{CompositeStops, ContentMetrics, ScrollAnimation, ScrollState, TextBuffer};
use super::styling::TextBounds;
use super::theme::{TextBackgroundColor, TextColor};
use super::tuning::LayoutTuning;
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
    DisplayLayout,
    TextViewOverlays,
    TextFont,
    TextBounds,
    LayoutTuning,
    Node,
    Transform,
    Visibility,
    bevy::picking::Pickable
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

        app.register_type::<TextFont>()
            .register_type::<super::overlay::RectOverlay>()
            .register_type::<super::overlay::RowVertical>()
            .register_type::<TextBounds>()
            .register_type::<TextColor>()
            .register_type::<TextBackgroundColor>()
            .register_type::<TextView>()
            .register_type::<TextViewBatchEntity>()
            .register_type::<TextViewOverlays>()
            .register_type::<TextBuffer>()
            .register_type::<ScrollState>()
            .register_type::<ContentMetrics>()
            ;

        app.add_systems(Update, animate_text_view_scroll);

        // Ensure there is always a camera marked as the default UI camera so
        // Bevy UI layout can resolve Val::Percent sizes for TextView Node entities.
        // Runs in PostStartup so all Startup camera-spawning systems have completed.
        // Hosts that want a specific camera can add IsDefaultUiCamera themselves.
        app.add_systems(PostStartup, ensure_default_ui_camera);

        app.add_systems(
            PostUpdate,
            (
                produce_layouts
                    .run_if(atlas_ready)
                    .in_set(LayoutProduceSet)
                    .after(UiSystems::Layout)
                    .before(prewarm_atlas_for_layout),
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
        duration: (duration + SCROLL_BACKDATE_DURATION).max(0.001),
        composite,
    }
}

fn animate_text_view_scroll(
    mut query: Query<(&mut ScrollState, &ComputedNode), With<TextView>>,
    time: Res<Time>,
) {
    let dt = time.delta_secs();

    for (mut state, computed) in query.iter_mut() {
        let inv = computed.inverse_scale_factor();
        let logical = computed.size() * inv;
        let viewport_h = logical.y;
        let viewport_w = logical.x;

        // Read current values without triggering change detection.
        let (duration, target_v, scroll_v, target_h, scroll_h, has_v_anim, has_h_anim) = {
            let s = state.bypass_change_detection();
            (
                s.smooth_scroll_duration,
                s.target_scroll_offset,
                s.scroll_offset,
                s.target_horizontal_scroll_offset,
                s.horizontal_scroll_offset,
                s.vertical_anim.is_some(),
                s.horizontal_anim.is_some(),
            )
        };

        let needs_new_v = if has_v_anim {
            let to = state
                .bypass_change_detection()
                .vertical_anim
                .as_ref()
                .unwrap()
                .to;
            (to - target_v).abs() > f32::EPSILON
        } else {
            (target_v - scroll_v).abs() > 0.5
        };

        let needs_new_h = if has_h_anim {
            let to = state
                .bypass_change_detection()
                .horizontal_anim
                .as_ref()
                .unwrap()
                .to;
            (to - target_h).abs() > f32::EPSILON
        } else {
            (target_h - scroll_h).abs() > 0.5
        };

        // Determine new anim state without writing yet.
        // Both fresh starts and mid-animation combines use `scroll_v` as `from`
        // (the current visual position, i.e. `this._state` in VS Code terms),
        // with the 10ms backdate. VS Code's combine() calls start() with the
        // same signature — no special from-preservation.
        let v_anim_next = if needs_new_v {
            Some(build_animation(scroll_v, target_v, duration, viewport_h))
        } else {
            state.bypass_change_detection().vertical_anim.clone()
        };
        let h_anim_next = if needs_new_h {
            Some(build_animation(scroll_h, target_h, duration, viewport_w))
        } else {
            state.bypass_change_detection().horizontal_anim.clone()
        };

        // Step animations and compute new scroll values.
        let (new_scroll_v, new_v_anim) = match v_anim_next {
            Some(mut anim) => {
                anim.elapsed += dt;
                if anim.elapsed >= anim.duration {
                    (anim.to, None)
                } else {
                    let v = sample_animation(&anim);
                    (v, Some(anim))
                }
            }
            None => (scroll_v, None),
        };

        let (new_scroll_h, new_h_anim) = match h_anim_next {
            Some(mut anim) => {
                anim.elapsed += dt;
                if anim.elapsed >= anim.duration {
                    (anim.to, None)
                } else {
                    let v = sample_animation(&anim);
                    (v, Some(anim))
                }
            }
            None => (scroll_h, None),
        };

        // Only write through the real Mut (triggering change detection) when
        // values actually changed. This prevents spurious Changed<ScrollState>
        // on idle frames, which was causing produce_line_styles to rebuild the
        // full visible window every frame.
        let scroll_v_changed = (new_scroll_v - scroll_v).abs() > 1e-4;
        let scroll_h_changed = (new_scroll_h - scroll_h).abs() > 1e-4;
        let v_anim_changed = needs_new_v || has_v_anim != new_v_anim.is_some();
        let h_anim_changed = needs_new_h || has_h_anim != new_h_anim.is_some();

        if scroll_v_changed || scroll_h_changed || v_anim_changed || h_anim_changed {
            state.scroll_offset = new_scroll_v;
            state.horizontal_scroll_offset = new_scroll_h;
            state.vertical_anim = new_v_anim;
            state.horizontal_anim = new_h_anim;
        }
    }
}

#[allow(clippy::type_complexity)]
pub fn update_text_views(
    mut commands: Commands,
    mut text_views: Query<
        (
            Entity,
            &ScrollState,
            &ComputedNode,
            &TextFont,
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
    let _span = bevy::prelude::info_span!("update_text_views").entered();
    for (tv_entity, scroll, computed, font, layout, overlays, batch_entity_opt, render_layers) in
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
        // text_area_left = padding.left from ComputedNode (set by host via Node::padding).
        // gutter_width is host-managed and always <= text_area_left, so content_start_x = text_area_left.
        let inv = computed.inverse_scale_factor();
        let content_start_x = computed.content_inset().min_inset.x * inv;

        let instances = {
            let _render_span = bevy::prelude::info_span!("render_layout").entered();
            render_layout(
                layout,
                overlays,
                computed,
                &mut atlas,
                &fonts,
                super::render::RenderContext {
                    content_start_x,
                    horizontal_scroll_offset: scroll.horizontal_scroll_offset,
                    font_size: font.font_size,
                    faces,
                },
            )
        };

        atlas.update_texture(&mut images);

        let logical = computed.size() * inv;
        let text_area_top = computed.content_inset().min_inset.y * inv;
        let line_height = layout.line_height;
        let scroll_dist = scroll.scroll_offset.abs();
        let start_pixels = scroll_dist - text_area_top;
        let first_visible = (start_pixels / line_height).floor().max(0.0) as usize;
        let visible_count = (logical.y / line_height).ceil() as usize;
        let last_visible = first_visible + visible_count;

        let batch_data = TextViewBatch {
            built_at_scroll: scroll.scroll_offset,
            built_at_horizontal_scroll: scroll.horizontal_scroll_offset,
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

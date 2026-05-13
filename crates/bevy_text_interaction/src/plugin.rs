//! Pointer + keyboard interaction plugin for text-view entities.
//!
//! [`InstancedTextInteractionPlugin<T>`] is generic over the content type
//! `T: TextContent + Component`. Each consumer registers the plugin once for
//! every content type they want interactive — terminals add `<TextSpan>`,
//! editors add `<RopeBuffer>` (via [`bevy_text_editor`]'s editor plugin).

use std::marker::PhantomData;

use bevy::input_focus::InputDispatchPlugin;
use bevy::picking::DefaultPickingPlugins;
use bevy::prelude::*;
use bevy_instanced_text::TextContent;

use crate::interaction_states::{ScrollConfig, TextViewDragState};
use crate::focus::{
    on_focused_keyboard, on_pointer_drag, on_pointer_press, on_pointer_release, on_pointer_scroll,
};

/// Picking + keyboard interaction for text-view entities of content type `T`.
/// Pair with [`bevy_instanced_text::InstancedTextPlugins`] for the rendering side.
pub struct InstancedTextInteractionPlugin<T: TextContent + Component>(PhantomData<T>);

impl<T: TextContent + Component> Default for InstancedTextInteractionPlugin<T> {
    fn default() -> Self {
        Self(PhantomData)
    }
}

impl<T: TextContent + Component> Plugin for InstancedTextInteractionPlugin<T> {
    fn build(&self, app: &mut App) {
        // Idempotent base infra — only register on first content-type instance.
        if !app.is_plugin_added::<bevy::picking::PickingPlugin>() {
            app.add_plugins(DefaultPickingPlugins);
        }
        if !app.is_plugin_added::<InputDispatchPlugin>() {
            app.add_plugins(InputDispatchPlugin);
        }

        app.register_type::<ScrollConfig>()
            .register_type::<TextViewDragState>()
            .register_type::<crate::focus::InteractionSettings>()
            .register_type::<crate::cursor::CursorSettings>()
            .register_type::<crate::cursor::CursorStyle>()
            .register_type::<crate::color::TextCursorColor>()
            .register_type::<crate::color::TextSelectionColor>();

        // Default clipboard backend; embedders override by inserting their own
        // `ClipboardResource` before plugin setup.
        app.init_resource::<crate::clipboard::ClipboardResource>();

        // Per-content-type observers. Multiple registrations of
        // InstancedTextInteractionPlugin::<T> are protected by the bool below
        // so duplicate observers don't fire.
        let key = std::any::TypeId::of::<T>();
        let already = app.world().get_resource::<RegisteredContentTypes>().map(|r| r.0.contains(&key)).unwrap_or(false);
        if !already {
            app.add_observer(on_pointer_press::<T>);
            app.add_observer(on_pointer_drag::<T>);
            app.add_observer(on_pointer_scroll::<T>);
            app.add_observer(on_focused_keyboard::<T>);
            app.world_mut()
                .get_resource_or_insert_with(RegisteredContentTypes::default)
                .0
                .push(key);
        }

        // The release observer is non-generic — only register it once.
        if !app
            .world()
            .get_resource::<ReleaseObserverRegistered>()
            .is_some()
        {
            app.add_observer(on_pointer_release);
            app.insert_resource(ReleaseObserverRegistered);
        }

        // Instant snap for entities with `ScrollConfig.smooth = false`. Runs
        // before the engine's smooth-scroll animator, so the animator sees
        // target == offset and is a no-op for those entities. Registered once.
        if !app
            .world()
            .get_resource::<InstantScrollRegistered>()
            .is_some()
        {
            app.add_systems(
                Update,
                apply_instant_scroll
                    .before(bevy_instanced_text::view::plugin::TextViewRenderSet),
            );
            app.insert_resource(InstantScrollRegistered);
        }
    }
}

#[derive(Resource, Default)]
struct RegisteredContentTypes(Vec<std::any::TypeId>);

#[derive(Resource)]
struct ReleaseObserverRegistered;

#[derive(Resource)]
struct InstantScrollRegistered;

fn apply_instant_scroll(
    mut q: Query<(&mut bevy_instanced_text::SmoothScroll, &ScrollConfig)>,
) {
    for (mut smooth, cfg) in q.iter_mut() {
        if (smooth.duration - cfg.smooth_scroll_duration).abs() > f32::EPSILON {
            smooth.duration = cfg.smooth_scroll_duration;
        }
        if cfg.smooth {
            continue;
        }
        if (smooth.target_y - smooth.offset_y).abs() > 0.001 {
            smooth.offset_y = smooth.target_y;
        }
        if (smooth.target_x - smooth.horizontal).abs() > 0.001 {
            smooth.horizontal = smooth.target_x;
        }
    }
}

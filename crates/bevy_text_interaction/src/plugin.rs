//! Interaction plugin for `TextView` entities.
//!
//! Wires the picking backend and observer-based handlers that turn pointer +
//! focused-keyboard events into scroll, drag-selection, and clipboard copy
//! on any entity carrying [`bevy_text_engine::TextView`]. The rendering side
//! lives in the engine crate ([`bevy_text_engine::TextEnginePlugin`] /
//! [`bevy_text_engine::TextEnginePlugins`]).
//!
//! The plugin idempotently adds [`bevy::picking::DefaultPickingPlugins`] and
//! [`bevy::input_focus::InputDispatchPlugin`] if the host hasn't already.
//! That keeps the "drop-in" experience working while letting hosts that
//! manage these subsystems themselves stay in control.
//!
//! Interaction state ([`crate::TextViewDragState`],
//! [`crate::TextViewSelectionState`]) is attached by the editor's
//! `#[require]` cascade. Plain `TextView` entities (chat panels, log
//! viewers) don't get those components by default — host code can attach
//! them explicitly when interactivity is desired.

use bevy::input_focus::InputDispatchPlugin;
use bevy::picking::{DefaultPickingPlugins, PickingSystems};
use bevy::prelude::*;

use crate::interaction::{
    on_focused_keyboard, on_pointer_drag, on_pointer_press, on_pointer_release, on_pointer_scroll,
};
use crate::picking::text_view_picking_backend;

/// Plugin registering pointer + keyboard interaction for `TextView`
/// entities. Pair with [`bevy_text_engine::TextEnginePlugins`] which
/// supplies the rendering side.
#[derive(Default)]
pub struct TextInteractionPlugin;

impl Plugin for TextInteractionPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<bevy::picking::PickingPlugin>() {
            app.add_plugins(DefaultPickingPlugins);
        }
        if !app.is_plugin_added::<InputDispatchPlugin>() {
            app.add_plugins(InputDispatchPlugin);
        }

        app.add_systems(
            PreUpdate,
            text_view_picking_backend.in_set(PickingSystems::Backend),
        );

        app.add_observer(on_pointer_press);
        app.add_observer(on_pointer_drag);
        app.add_observer(on_pointer_release);
        app.add_observer(on_pointer_scroll);
        app.add_observer(on_focused_keyboard);
    }
}

//! `TextViewport` — internal cache of resolved viewport dimensions.
//!
//! Populated every frame by `sync_viewport_from_node` from `ComputedNode`
//! and `UiGlobalTransform`. Internal code reads this; hosts set `Node`
//! padding/size and never touch `TextViewport` directly.

use bevy::prelude::*;

/// Internal per-entity viewport cache. Written by `sync_viewport_from_node`
/// from Bevy UI layout; read by layout, rendering, and anchor systems.
/// Hosts should not set this manually — set `Node` size and padding instead.
#[derive(Component, Clone, Copy, Debug, Reflect)]
#[reflect(Component, Debug)]
pub struct TextViewport {
    pub width: u32,
    pub height: u32,
    /// Screen-space top-left of the node, derived from `UiGlobalTransform`.
    pub hit_test_position: bevy::math::Vec2,
    /// Resolved from `Node::padding.left` via `ComputedNode::content_inset`.
    pub text_area_left: f32,
    /// Resolved from `Node::padding.top` via `ComputedNode::content_inset`.
    pub text_area_top: f32,
    /// Kept for internal gutter-width tracking; populated by `bevscode`
    /// via `sync_viewport_from_node` override or direct insert.
    pub gutter_width: f32,
}

impl Default for TextViewport {
    fn default() -> Self {
        Self {
            width: 800,
            height: 600,
            hit_test_position: bevy::math::Vec2::ZERO,
            text_area_left: 0.0,
            text_area_top: 8.0,
            gutter_width: 0.0,
        }
    }
}

impl TextViewport {
    pub fn world_left(&self) -> f32 {
        -(self.width as f32) / 2.0
    }

    pub fn world_top(&self) -> f32 {
        self.height as f32 / 2.0
    }
}


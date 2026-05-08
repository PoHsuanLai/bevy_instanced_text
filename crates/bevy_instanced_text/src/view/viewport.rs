//! TextViewViewport — per-instance viewport dimensions and layout.
//!
//! With the SpecializedMeshPipeline rewrite, the engine no longer cares
//! about the viewport's *world* position — every glyph emits in entity-
//! local pixel space and the entity's `Transform` does the world placement.
//! This component now carries only the rect-bound state the layout pass
//! needs: the visible width/height, hit-test position, content margins,
//! gutter width.
//!
//! `world_left` / `world_top` survive as deprecated zero-returning
//! shims so external consumers that compose viewport offsets with their
//! own coordinates keep building. Callers should migrate to using the
//! `TextView` entity's `Transform` / `GlobalTransform` instead.

use bevy::prelude::*;

/// Per-entity viewport dimensions. Component (not Resource) so each text view
/// has its own.
#[derive(Component, Clone, Copy, Debug, Reflect)]
#[reflect(Component, Debug)]
pub struct TextViewViewport {
    pub width: u32,
    pub height: u32,
    /// Screen-space hit-test position — set this even for render-to-texture views.
    pub hit_test_position: bevy::math::Vec2,
    pub text_area_left: f32,
    pub text_area_top: f32,
    /// 0 for views without a gutter. Editor IDE chrome (the line numbers
    /// gutter) draws its separator at this x; non-editor views ignore it.
    pub gutter_width: f32,
}

impl Default for TextViewViewport {
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

impl TextViewViewport {
    /// World-space X of the viewport's left edge, relative to the camera
    /// origin. Centered ortho convention: `-width / 2`. Renderer +
    /// downstream sprite positioning consumers compose this with their
    /// content's local x offset.
    pub fn world_left(&self) -> f32 {
        -(self.width as f32) / 2.0
    }

    /// World-space Y of the viewport's top edge, relative to the camera
    /// origin. Centered ortho convention: `+height / 2`.
    pub fn world_top(&self) -> f32 {
        self.height as f32 / 2.0
    }
}

/// Deprecated stub kept so consumers re-exporting `bevy_instanced_text::ViewportOrigin`
/// keep building. The engine no longer reads it. Will be removed in a follow-up.
#[derive(Clone, Copy, Debug, Default, PartialEq, Reflect)]
#[reflect(Default, PartialEq)]
pub enum ViewportOrigin {
    #[default]
    CenteredOrtho,
    ScreenAbsolute(bevy::math::Vec2),
}

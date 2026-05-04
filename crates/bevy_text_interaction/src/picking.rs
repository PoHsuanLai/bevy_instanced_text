//! Custom `bevy_picking` backend for [`TextView`] entities.
//!
//! Hit-tests pointer locations against each `TextView`'s
//! [`TextViewViewport`] rect (using the screen-space `hit_test_position` +
//! `width`/`height`). Emits one `PointerHits` per pointer per frame
//! containing every text view under that pointer.
//!
//! The hit `position` is reported in **screen pixels relative to the
//! viewport top-left** (i.e. viewport-local). Observer code that needs to
//! convert to a character index can feed the hit position straight into
//! [`crate::interaction::screen_to_char_pos`] without further translation.
//!
//! Picking order is `1.0`, slightly above the default backends so a text
//! view inside a `bevy_ui` panel gets the click before the panel itself.

use bevy::picking::backend::{HitData, PointerHits};
use bevy::picking::pointer::{PointerId, PointerLocation};
use bevy::picking::Pickable;
use bevy::prelude::*;

use bevy_text_engine::{TextView, TextViewViewport};

/// Picking-backend system: produce `PointerHits` for `TextView` entities.
///
/// Registered by [`crate::plugin::TextInteractionPlugin`] in
/// `PickingSystems::Backend` (PreUpdate).
pub fn text_view_picking_backend(
    pointers: Query<(&PointerId, &PointerLocation)>,
    text_views: Query<(Entity, &TextViewViewport, Option<&Pickable>), With<TextView>>,
    mut output: MessageWriter<PointerHits>,
) {
    for (pointer_id, pointer_location) in pointers.iter() {
        let Some(location) = pointer_location.location() else {
            continue;
        };
        let pointer_pos = location.position;

        let mut picks: Vec<(Entity, HitData)> = Vec::new();
        for (entity, viewport, pickable) in text_views.iter() {
            // `Pickable::IGNORE` opts out entirely.
            if let Some(p) = pickable {
                if !p.is_hoverable && !p.should_block_lower {
                    continue;
                }
            }

            let vp_pos = viewport.hit_test_position;
            let vp_rect = Rect::new(
                vp_pos.x,
                vp_pos.y,
                vp_pos.x + viewport.width as f32,
                vp_pos.y + viewport.height as f32,
            );

            if !vp_rect.contains(pointer_pos) {
                continue;
            }

            // Report viewport-local position so observers can hit-test
            // against rope chars without re-subtracting the viewport origin.
            let local = pointer_pos - vp_pos;
            let hit = HitData::new(
                Entity::PLACEHOLDER,
                0.0,
                Some(Vec3::new(local.x, local.y, 0.0)),
                None,
            );
            picks.push((entity, hit));
        }

        if !picks.is_empty() {
            output.write(PointerHits::new(*pointer_id, picks, 1.0));
        }
    }
}

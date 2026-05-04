//! Shared text-view interactions — scroll, selection, copy.
//!
//! Implemented as observers on `Pointer<…>` events (from `bevy_picking`)
//! and `FocusedInput<KeyboardInput>` (from `bevy_input_focus`), routed by
//! the custom backend in [`crate::picking`]. The polling systems that used
//! to live here (manual cursor-rect hit-testing) are gone — picking +
//! focus dispatch handle entity routing for us.

use bevy::input::keyboard::{KeyCode, KeyboardInput};
use bevy::input_focus::{FocusedInput, InputFocus};
use bevy::input::ButtonState;
use bevy::picking::events::{Drag, Pointer, Press, Release, Scroll};
use bevy::picking::pointer::PointerButton;
use bevy::prelude::*;
use ropey::Rope;

use bevy_text_engine::{DisplayLayout, FontConfig, TextView, TextViewState, TextViewViewport};

use crate::components::{ScrollConfig, TextViewDragState, TextViewSelectionState};

// =============================================================================
// Utilities (kept public for hosts that build their own click handlers)
// =============================================================================

/// Convert screen coordinates (viewport-local, 0,0 at top-left) to a character
/// position in the rope. Used for click-to-position and drag selection.
///
/// `layout` is consulted when available so proportional fonts hit-test
/// correctly via shaped per-glyph advances; falls back to `font.char_width`
/// column math otherwise.
pub fn screen_to_char_pos(
    screen_pos: Vec2,
    rope: &Rope,
    layout: Option<&DisplayLayout>,
    current_scroll_offset: f32,
    font: &FontConfig,
    viewport: &TextViewViewport,
    scroll_offset_override: Option<f32>,
) -> usize {
    let relative_x = screen_pos.x - viewport.text_area_left;
    let scroll_offset = scroll_offset_override.unwrap_or(current_scroll_offset);
    let relative_y = screen_pos.y - viewport.text_area_top - scroll_offset;

    let line_height = font.line_height;
    let display_row = (relative_y / line_height).max(0.0) as usize;

    let line_count = rope.len_lines();
    if display_row >= line_count {
        return rope.len_chars();
    }

    let line_start_char = rope.line_to_char(display_row);

    if let Some(layout) = layout {
        if let Some(byte_in_row) = layout.byte_at_x(display_row as u32, relative_x) {
            // Use the row's buffer_row + buffer_byte_offset to translate the
            // row-local byte offset to a rope byte. Trivial layouts always
            // have buffer_byte_offset=0 and buffer_row==display_row, so this
            // collapses to the prior behavior; with soft wrap, multiple rows
            // share a buffer line and the offset becomes load-bearing.
            let row = layout
                .lines
                .iter()
                .find(|l| l.display_row == display_row as u32);
            let buffer_line = row.map(|r| r.buffer_row as usize).unwrap_or(display_row);
            let buffer_byte_offset = row.map(|r| r.buffer_byte_offset).unwrap_or(0);
            let line_start_byte = rope.line_to_byte(buffer_line.min(rope.len_lines()));
            let abs_byte =
                (line_start_byte + buffer_byte_offset + byte_in_row).min(rope.len_bytes());
            return rope.byte_to_char(abs_byte);
        }
    }

    let col = (relative_x / font.char_width).max(0.0) as usize;
    let line_len = rope.line(display_row).len_chars().saturating_sub(1);
    let char_in_line = col.min(line_len);
    line_start_char + char_in_line
}

/// Copy the current selection to the system clipboard.
/// Returns true if text was copied, false if no selection.
pub fn copy_selection(sel: &TextViewSelectionState, tv: &TextViewState) -> bool {
    if let (Some(s), Some(e)) = (sel.selection_start, sel.selection_end) {
        let (start, end) = if s < e { (s, e) } else { (e, s) };
        let start = start.min(tv.rope.len_chars());
        let end = end.min(tv.rope.len_chars());
        if start == end {
            return false;
        }
        let text = tv.rope.slice(start..end).to_string();
        if let Ok(mut clipboard) = arboard::Clipboard::new() {
            let _ = clipboard.set_text(text);
            return true;
        }
    }
    false
}

// =============================================================================
// Observers (registered globally by `TextInteractionPlugin`)
// =============================================================================

/// Pointer scroll observer for `TextView` entities.
///
/// Picking already routed this event to the entity under the cursor, so the
/// hit-test loop the old `handle_text_view_scroll` did is gone — we just
/// look up the target entity's components and apply the scroll.
pub fn on_pointer_scroll(
    trigger: On<Pointer<Scroll>>,
    mut views: Query<
        (
            &mut TextViewState,
            &TextViewViewport,
            &FontConfig,
            Option<&ScrollConfig>,
        ),
        With<TextView>,
    >,
) {
    let entity = trigger.event().entity;
    let Ok((mut tv, viewport, font, scroll_cfg)) = views.get_mut(entity) else {
        return;
    };

    let default_scroll = ScrollConfig::default();
    let scroll_cfg = scroll_cfg.unwrap_or(&default_scroll);

    let dy = trigger.event().y;
    if dy.abs() <= 0.0 {
        return;
    }

    let scroll_delta = dy * font.line_height * scroll_cfg.speed;
    let line_count = tv.rope.len_lines();
    let content_height = line_count as f32 * font.line_height;
    let viewport_height = viewport.height as f32;
    let max_scroll = (-(content_height - viewport_height + viewport.text_area_top)).min(0.0);

    if scroll_cfg.smooth {
        tv.target_scroll_offset += scroll_delta;
        tv.target_scroll_offset = tv.target_scroll_offset.min(0.0).max(max_scroll);
    } else {
        tv.scroll_offset += scroll_delta;
        tv.scroll_offset = tv.scroll_offset.min(0.0).max(max_scroll);
    }
}

/// Pointer-press observer: focus the view and start a selection drag.
///
/// Only the primary button starts a selection. Position is taken from the
/// hit data, which the picking backend reports in viewport-local coords.
pub fn on_pointer_press(
    trigger: On<Pointer<Press>>,
    mut views: Query<
        (
            &mut TextViewSelectionState,
            &mut TextViewDragState,
            &TextViewState,
            &TextViewViewport,
            &FontConfig,
            Option<&DisplayLayout>,
        ),
        With<TextView>,
    >,
    mut input_focus: ResMut<InputFocus>,
) {
    if trigger.event().button != PointerButton::Primary {
        return;
    }
    let entity = trigger.event().entity;
    let Ok((mut sel, mut drag_state, tv, viewport, font, layout)) = views.get_mut(entity) else {
        return;
    };

    let local_pos = match trigger.event().hit.position {
        Some(p) => Vec2::new(p.x, p.y),
        None => return,
    };

    let char_pos = screen_to_char_pos(
        local_pos,
        &tv.rope,
        layout.as_deref(),
        tv.scroll_offset,
        font,
        viewport,
        None,
    );

    sel.selection_start = Some(char_pos);
    sel.selection_end = None;
    drag_state.is_dragging = true;
    drag_state.drag_start_pos = Some(char_pos);
    drag_state.drag_start_scroll_offset = tv.scroll_offset;
    // Reconstruct screen-space pointer position from hit + viewport origin.
    drag_state.last_screen_pos = Some(viewport.hit_test_position + local_pos);
    input_focus.set(entity);
}

/// Drag observer: extend the selection while the primary button is held.
///
/// Picking dispatches `Pointer<Drag>` to the entity that received the
/// initial press, so this stays scoped to the view that started the drag
/// even if the cursor moves out of its viewport.
pub fn on_pointer_drag(
    trigger: On<Pointer<Drag>>,
    mut views: Query<
        (
            &mut TextViewSelectionState,
            &mut TextViewDragState,
            &TextViewState,
            &TextViewViewport,
            &FontConfig,
            Option<&DisplayLayout>,
        ),
        With<TextView>,
    >,
) {
    if trigger.event().button != PointerButton::Primary {
        return;
    }
    let entity = trigger.event().entity;
    let Ok((mut sel, mut drag_state, tv, viewport, font, layout)) = views.get_mut(entity) else {
        return;
    };
    if !drag_state.is_dragging {
        return;
    }

    // Resolve current pointer position in screen space from picking event.
    let cursor_pos = trigger.event().pointer_location.position;

    if let Some(last_pos) = drag_state.last_screen_pos {
        if (cursor_pos - last_pos).length() < 2.0 {
            return;
        }
    }

    let local_pos = cursor_pos - viewport.hit_test_position;
    let char_pos = screen_to_char_pos(
        local_pos,
        &tv.rope,
        layout.as_deref(),
        tv.scroll_offset,
        font,
        viewport,
        Some(drag_state.drag_start_scroll_offset),
    );

    sel.selection_start = drag_state.drag_start_pos;
    sel.selection_end = Some(char_pos);
    drag_state.last_screen_pos = Some(cursor_pos);
}

/// Release observer: clear the drag flag.
pub fn on_pointer_release(
    trigger: On<Pointer<Release>>,
    mut views: Query<&mut TextViewDragState, With<TextView>>,
) {
    if trigger.event().button != PointerButton::Primary {
        return;
    }
    let entity = trigger.event().entity;
    if let Ok(mut drag_state) = views.get_mut(entity) {
        drag_state.is_dragging = false;
    }
}

/// Focused-keyboard observer: copy the selection on Cmd/Ctrl+C.
///
/// Replaces the global `Res<ButtonInput<KeyCode>>` poll with a routed
/// `FocusedInput<KeyboardInput>` event, so only the focused text view's
/// selection is copied. The Ctrl modifier check keys off
/// `Res<ButtonInput<KeyCode>>` since `KeyboardInput` carries a single
/// key per event.
pub fn on_focused_keyboard(
    trigger: On<FocusedInput<KeyboardInput>>,
    views: Query<(&TextViewSelectionState, &TextViewState), With<TextView>>,
    keyboard: Res<ButtonInput<KeyCode>>,
) {
    let entity = trigger.event().focused_entity;
    let Ok((sel, tv)) = views.get(entity) else {
        return;
    };

    let event = &trigger.event().input;
    if event.state != ButtonState::Pressed {
        return;
    }
    if event.key_code != KeyCode::KeyC {
        return;
    }
    let ctrl = keyboard.pressed(KeyCode::SuperLeft)
        || keyboard.pressed(KeyCode::SuperRight)
        || keyboard.pressed(KeyCode::ControlLeft)
        || keyboard.pressed(KeyCode::ControlRight);
    if !ctrl {
        return;
    }

    copy_selection(sel, tv);
}

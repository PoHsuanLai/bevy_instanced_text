//! Shared text view interactions — scroll, selection, copy.
//!
//! These systems work on any entity with `TextView` + `TextViewState` + `TextViewViewport`.
//! Used by the code editor (via delegation) and standalone text views (chat, logs).

use bevy::input::mouse::MouseWheel;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use ropey::Rope;

use bevy_text_engine::{DisplayLayout, FontConfig, TextView, TextViewState, TextViewViewport};

use crate::components::{ScrollConfig, TextViewDragState, TextViewSelectionState};

// =============================================================================
// Utilities
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

    // Shaped path: ask the layout where pixel `relative_x` falls inside the row,
    // then convert byte offset → char offset via the rope. Only takes this path
    // when `display_row` falls within the layout's visible window — clicks above
    // or below scroll fall through to the column math fallback.
    if let Some(layout) = layout {
        if let Some(byte_in_line) = layout.byte_at_x(display_row as u32, relative_x) {
            let line_start_byte = rope.line_to_byte(display_row);
            let abs_byte = (line_start_byte + byte_in_line).min(rope.len_bytes());
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
// Systems
// =============================================================================

/// Mouse wheel scroll for all `TextView` entities.
/// Hit-tests against each viewport to only scroll the hovered view.
///
/// Per-view scroll behaviour: `FontConfig` is per-entity. `ScrollConfig` is
/// optional — `CodeEditor` entities provide one via their `#[require]`
/// cascade; standalone `TextView`s (chat, logs) fall back to defaults.
pub fn handle_text_view_scroll(
    mut views: Query<
        (
            &mut TextViewState,
            &TextViewViewport,
            &FontConfig,
            Option<&ScrollConfig>,
        ),
        With<TextView>,
    >,
    mut mouse_wheel_events: MessageReader<MouseWheel>,
    windows: Query<&Window, With<PrimaryWindow>>,
) {
    let Ok(window) = windows.single() else { return };
    let Some(cursor_pos) = window.cursor_position() else {
        return;
    };

    // Collect events (can only iterate once)
    let events: Vec<_> = mouse_wheel_events.read().cloned().collect();
    if events.is_empty() {
        return;
    }

    let default_scroll = ScrollConfig::default();
    for (mut tv, viewport, font, scroll_cfg) in views.iter_mut() {
        let scroll_cfg = scroll_cfg.unwrap_or(&default_scroll);
        // Hit-test: is the cursor within this viewport?
        let vp_pos = viewport.hit_test_position;
        let vp_rect = bevy::math::Rect::new(
            vp_pos.x,
            vp_pos.y,
            vp_pos.x + viewport.width as f32,
            vp_pos.y + viewport.height as f32,
        );

        if !vp_rect.contains(cursor_pos) {
            continue;
        }

        for event in &events {
            if event.y.abs() > 0.0 {
                let scroll_delta = event.y * font.line_height * scroll_cfg.speed;
                let line_count = tv.rope.len_lines();
                let content_height = line_count as f32 * font.line_height;
                let viewport_height = viewport.height as f32;
                let max_scroll =
                    (-(content_height - viewport_height + viewport.text_area_top)).min(0.0);

                if scroll_cfg.smooth {
                    tv.target_scroll_offset += scroll_delta;
                    tv.target_scroll_offset = tv.target_scroll_offset.min(0.0).max(max_scroll);
                } else {
                    tv.scroll_offset += scroll_delta;
                    tv.scroll_offset = tv.scroll_offset.min(0.0).max(max_scroll);
                }

            }
        }
    }
}

/// Mouse click + drag selection for all `TextView` entities.
///
/// Drag state is per-view; each entity tracks its own selection drag so two
/// text views can be interacted with independently. On press, the hit view
/// becomes the [`InputFocus`] target so keyboard input routes to it.
pub fn handle_text_view_mouse(
    mut views: Query<
        (
            Entity,
            &mut TextViewSelectionState,
            &mut TextViewDragState,
            &TextViewState,
            &TextViewViewport,
            &FontConfig,
            Option<&DisplayLayout>,
        ),
        With<TextView>,
    >,
    mut input_focus: ResMut<bevy::input_focus::InputFocus>,
    mouse_button: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window, With<PrimaryWindow>>,
) {
    let Ok(window) = windows.single() else { return };
    let Some(cursor_pos) = window.cursor_position() else {
        return;
    };

    // Handle release: clear drag flag on every view that thought it was dragging.
    if mouse_button.just_released(MouseButton::Left) {
        for (_, _, mut drag_state, _, _, _, _) in views.iter_mut() {
            drag_state.is_dragging = false;
        }
        return;
    }

    // Handle press: hit-test each view; the one under the cursor begins a drag
    // and acquires keyboard focus.
    if mouse_button.just_pressed(MouseButton::Left) {
        for (entity, mut sel, mut drag_state, tv, viewport, font, layout) in views.iter_mut() {
            let vp_pos = viewport.hit_test_position;
            let vp_rect = bevy::math::Rect::new(
                vp_pos.x,
                vp_pos.y,
                vp_pos.x + viewport.width as f32,
                vp_pos.y + viewport.height as f32,
            );

            if !vp_rect.contains(cursor_pos) {
                continue;
            }

            let local_pos = Vec2::new(cursor_pos.x - vp_pos.x, cursor_pos.y - vp_pos.y);
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
            drag_state.last_screen_pos = Some(cursor_pos);
            input_focus.set(entity);
        }
        return;
    }

    // Handle drag — only the view that started the drag extends its selection.
    if mouse_button.pressed(MouseButton::Left) {
        for (_, mut sel, mut drag_state, tv, viewport, font, layout) in views.iter_mut() {
            if !drag_state.is_dragging {
                continue;
            }
            // Skip tiny movements to avoid jitter.
            if let Some(last_pos) = drag_state.last_screen_pos {
                if (cursor_pos - last_pos).length() < 2.0 {
                    continue;
                }
            }

            let vp_pos = viewport.hit_test_position;
            let local_pos = Vec2::new(cursor_pos.x - vp_pos.x, cursor_pos.y - vp_pos.y);
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
    }
}

/// Copy selection on Cmd/Ctrl+C for `TextView` entities.
pub fn handle_text_view_copy(
    views: Query<(&TextViewSelectionState, &TextViewState), With<TextView>>,
    keyboard: Res<ButtonInput<KeyCode>>,
) {
    let ctrl = keyboard.pressed(KeyCode::SuperLeft)
        || keyboard.pressed(KeyCode::SuperRight)
        || keyboard.pressed(KeyCode::ControlLeft)
        || keyboard.pressed(KeyCode::ControlRight);

    if ctrl && keyboard.just_pressed(KeyCode::KeyC) {
        for (sel, tv) in views.iter() {
            if copy_selection(sel, tv) {
                break; // Only copy from first view with a selection
            }
        }
    }
}

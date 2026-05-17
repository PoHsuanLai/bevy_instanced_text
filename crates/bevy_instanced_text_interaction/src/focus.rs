//! Pointer + keyboard observers shared by every interactive text view.
//!
//! All observers are generic over `T: TextContent`, so the same code path
//! drives click-to-place, drag-select, scroll, and Cmd+C copy for terminals
//! (`TextBuffer<TextSpan>`), labels, and rope-backed editors
//! (`TextBuffer<RopeBuffer>`). Hit-testing uses [`DisplayLayout`] (when
//! present) for proportional fonts and falls back to monospace cell math.
//!
//! The observers write directly into [`SelectionState`] / [`CursorState`].
//! Editor crates that want richer behavior (multi-cursor on Alt-click,
//! goto-definition on Ctrl-click, fold-aware hit testing) add **their own**
//! observers on the same picking events and run after these.

use bevy::input::keyboard::{KeyCode, KeyboardInput};
use bevy::input::mouse::MouseScrollUnit;
use bevy::input::ButtonState;
use bevy::input_focus::{FocusedInput, InputFocus};
use bevy::picking::events::{Drag, Pointer, Press, Release, Scroll};
use bevy::picking::pointer::PointerButton;
use bevy::prelude::*;
use bevy::ui::ui_transform::UiGlobalTransform;

use bevy::ui::{ComputedNode, ScrollPosition};
use bevy_instanced_text::{ContentMetrics, DisplayLayout, MonoCellWidth, TextBuffer, TextContent};

use crate::interaction_states::{ScrollConfig, TextViewDragState};
use crate::text_state::{CursorState, SelectionState};

type ScrollQuery<'w, 's, T> = Query<
    'w,
    's,
    (
        &'static TextBuffer<T>,
        &'static mut ScrollPosition,
        &'static ContentMetrics,
        &'static ComputedNode,
        &'static TextFont,
        &'static bevy::text::LineHeight,
        &'static MonoCellWidth,
        Option<&'static ScrollConfig>,
    ),
    With<DisplayLayout>,
>;

type PressQuery<'w, 's, T> = Query<
    'w,
    's,
    (
        &'static mut TextViewDragState,
        &'static TextBuffer<T>,
        &'static ScrollPosition,
        &'static TextFont,
        &'static bevy::text::LineHeight,
        &'static MonoCellWidth,
        &'static ComputedNode,
        Option<&'static DisplayLayout>,
        Option<&'static mut SelectionState>,
        Option<&'static mut CursorState>,
        Option<&'static InteractionSettings>,
    ),
    With<DisplayLayout>,
>;

type DragQuery<'w, 's, T> = Query<
    'w,
    's,
    (
        &'static mut TextViewDragState,
        &'static TextBuffer<T>,
        &'static ScrollPosition,
        &'static TextFont,
        &'static bevy::text::LineHeight,
        &'static MonoCellWidth,
        &'static ComputedNode,
        &'static UiGlobalTransform,
        Option<&'static DisplayLayout>,
        Option<&'static mut SelectionState>,
        Option<&'static mut CursorState>,
    ),
    With<DisplayLayout>,
>;

type CopyQuery<'w, 's, T> =
    Query<'w, 's, (&'static SelectionState, &'static TextBuffer<T>), With<DisplayLayout>>;

/// Convert screen coordinates (viewport-local, 0,0 at top-left) to a character
/// position in the text. Generic over any [`TextContent`]; uses the trait's
/// char-index methods so terminals, labels, and editors share one hit-test.
///
/// `layout` is consulted when available so proportional fonts hit-test
/// correctly via shaped per-glyph advances; falls back to `font.char_width`
/// column math otherwise.
pub fn screen_to_char_pos<T: TextContent>(
    screen_pos: Vec2,
    content: &T,
    layout: Option<&DisplayLayout>,
    current_scroll_y: f32,
    mono: &MonoCellWidth,
    line_height: f32,
    text_area_left: f32,
    text_area_top: f32,
    scroll_y_override: Option<f32>,
) -> usize {
    let relative_x = screen_pos.x - text_area_left;
    let scroll_y = scroll_y_override.unwrap_or(current_scroll_y);
    // scroll_y is positive-downward: subtract to shift content up relative to viewport.
    let relative_y = screen_pos.y - text_area_top + scroll_y;

    let display_row = (relative_y / line_height).max(0.0) as usize;

    let line_count = content.line_count();
    if display_row >= line_count {
        return content.char_count();
    }

    let line_start_char = content.line_to_char(display_row);

    if let Some(layout) = layout {
        if let Some(byte_in_row) = layout.byte_at_x(display_row as u32, relative_x) {
            // Use the row's buffer_row + buffer_byte_offset to translate the
            // row-local byte offset to a buffer byte. Trivial layouts always
            // have buffer_byte_offset=0 and buffer_row==display_row, so this
            // collapses to the prior behavior; with soft wrap, multiple rows
            // share a buffer line and the offset becomes load-bearing.
            let row = layout
                .lines
                .iter()
                .find(|l| l.display_row == display_row as u32);
            let buffer_line = row.map(|r| r.buffer_row as usize).unwrap_or(display_row);
            let buffer_byte_offset = row.map(|r| r.buffer_byte_offset).unwrap_or(0);
            // Compute byte offset within the buffer line for the click; convert
            // to a char offset via the trait's line() text.
            let line_text = content.line(buffer_line.min(line_count.saturating_sub(1)));
            let target_byte = buffer_byte_offset + byte_in_row;
            let target_byte = target_byte.min(line_text.len());
            let chars_into_line = line_text[..target_byte].chars().count();
            return content
                .line_to_char(buffer_line)
                .saturating_add(chars_into_line)
                .min(content.char_count());
        }
    }

    let col = (relative_x / mono.px).max(0.0) as usize;
    let line_len = content.line_len_chars(display_row);
    let char_in_line = col.min(line_len);
    line_start_char + char_in_line
}

/// Extract the primary selection's text in its `SelectionMode`-honored
/// shape, ready for the clipboard. Returns `None` when there's nothing
/// to copy. Generic over any [`TextContent`].
///
/// - `Simple` / `Semantic` — char-range slice.
/// - `Block` — column-aligned rectangular slice across visited lines,
///   joined with `\n`.
/// - `Line` — full-line slice (already snapped to whole lines by
///   `expand_to_lines`).
pub fn selection_text<T: TextContent>(sel: &SelectionState, content: &T) -> Option<String> {
    let (start, end) = sel.primary_range()?;
    let mode = sel.selections.primary().mode;
    let len = content.char_count();
    let start = start.min(len);
    let end = end.min(len);
    if start == end {
        return None;
    }
    let text = match mode {
        crate::selection::SelectionMode::Block => block_slice(content, start, end),
        _ => content.slice_chars(start..end).into_owned(),
    };
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

/// Copy the primary selection's text to the given clipboard backend.
/// Returns `true` if anything was copied. Generic over any [`TextContent`].
pub fn copy_selection<T: TextContent>(
    sel: &SelectionState,
    content: &T,
    clipboard: &crate::clipboard::ClipboardResource,
) -> bool {
    if let Some(text) = selection_text(sel, content) {
        clipboard.set_text(&text);
        true
    } else {
        false
    }
}

/// Return the rectangular slice between `start` and `end` (char offsets),
/// one row per source line, joined with `\n`. The column range is
/// `[min_col, max_col)` in *characters*, derived from the two endpoints'
/// columns within their lines.
fn block_slice<T: TextContent>(content: &T, start: usize, end: usize) -> String {
    if start >= end {
        return String::new();
    }
    let (start, end) = (start.min(end), start.max(end));
    let start_line = content.char_to_line(start);
    let end_line = content.char_to_line(end);
    let start_col = start - content.line_to_char(start_line);
    let end_col = end - content.line_to_char(end_line);
    let (col_lo, col_hi) = if start_col <= end_col {
        (start_col, end_col)
    } else {
        (end_col, start_col)
    };
    if col_lo == col_hi {
        return String::new();
    }
    let mut out = String::new();
    for line_idx in start_line..=end_line {
        let line = content.line(line_idx);
        // Strip trailing '\n' if present so block selection columns line up.
        let line_str = line.trim_end_matches('\n');
        let line_len = line_str.chars().count();
        let lo = col_lo.min(line_len);
        let hi = col_hi.min(line_len);
        if lo < hi {
            let s: String = line_str.chars().skip(lo).take(hi - lo).collect();
            out.push_str(&s);
        }
        if line_idx != end_line {
            out.push('\n');
        }
    }
    out
}

/// Pointer scroll observer for text-view entities — handles both vertical
/// (scroll wheel / two-finger swipe) and horizontal (shift+wheel / two-finger
/// swipe sideways) scrolling. Generic over content type.
///
/// # Per-panel routing
///
/// Bevy's picking backend routes [`Pointer<Scroll>`] events to whichever
/// entity is currently under the cursor, so multi-panel layouts get
/// per-panel scrolling for free — no need to compare mouse position against
/// each panel's node bounds. Add [`InstancedTextInteractionPlugin`] (which
/// installs this observer) and every [`TextBuffer<T>`] entity scrolls
/// independently when hovered.
///
/// Hosts that want different scroll behavior per panel can add their own
/// [`Pointer<Scroll>`] observer alongside this one — both fire, and the
/// host's observer can short-circuit by mutating the same
/// [`ScrollPosition`] component.
///
/// [`InstancedTextInteractionPlugin`]: crate::InstancedTextInteractionPlugin
/// [`TextBuffer<T>`]: bevy_instanced_text::TextBuffer
pub fn on_pointer_scroll<T: TextContent + Component>(
    trigger: On<Pointer<Scroll>>,
    mut views: ScrollQuery<T>,
) {
    let entity = trigger.event().entity;
    let Ok((buffer, mut scroll, metrics, computed, font, lh, mono, scroll_cfg)) =
        views.get_mut(entity)
    else {
        return;
    };

    let default_scroll = ScrollConfig::default();
    let scroll_cfg = scroll_cfg.unwrap_or(&default_scroll);

    let unit = trigger.event().unit;
    let dx = trigger.event().x;
    let dy = trigger.event().y;

    let inv = computed.inverse_scale_factor();
    let viewport_width = computed.size().x * inv;
    let viewport_height = computed.size().y * inv;
    let text_area_left = computed.content_inset().min_inset.x * inv;
    let text_area_top = computed.content_inset().min_inset.y * inv;
    let line_height = bevy_instanced_text::resolve_line_height(*lh, font.font_size);

    let (v_delta_per_dy, h_delta_per_dx) = match unit {
        MouseScrollUnit::Line => (line_height * scroll_cfg.speed, mono.px * scroll_cfg.speed),
        // Pixel-unit deltas are already in logical pixels; no speed multiplier.
        MouseScrollUnit::Pixel => (1.0, 1.0),
    };

    // Horizontal scroll — only when content overflows.
    if dx.abs() > 0.0 {
        let available_text_width = viewport_width - text_area_left;
        if metrics.max_content_width > available_text_width {
            let scroll_delta = dx * h_delta_per_dx;
            let max_h = (metrics.max_content_width - available_text_width).max(0.0);
            scroll.x = (scroll.x + scroll_delta).clamp(0.0, max_h);
        }
    }

    // Vertical scroll. Positive = down.
    if dy.abs() > 0.0 {
        let scroll_delta = -dy * v_delta_per_dy;
        let line_count = buffer.line_count();
        let content_height = line_count as f32 * line_height;
        let max_scroll = (content_height - viewport_height + text_area_top).max(0.0);
        scroll.y = (scroll.y + scroll_delta).clamp(0.0, max_scroll);
    }
}

/// Pointer-press observer: focus the view and start a selection drag.
///
/// Only the primary button starts a selection. Position is taken from the
/// hit data, which the picking backend reports in viewport-local coords.
/// Writes through to `SelectionState`/`CursorState` when present.
pub fn on_pointer_press<T: TextContent + Component>(
    trigger: On<Pointer<Press>>,
    mut views: PressQuery<T>,
    keyboard: Res<ButtonInput<KeyCode>>,
    time: Res<Time>,
    mut input_focus: ResMut<InputFocus>,
) {
    if trigger.event().button != PointerButton::Primary {
        return;
    }
    let entity = trigger.event().entity;
    let Ok((
        mut drag_state,
        buffer,
        scroll,
        font,
        lh,
        mono,
        computed,
        layout,
        sel,
        cursor,
        settings,
    )) = views.get_mut(entity)
    else {
        return;
    };
    let interaction = settings.copied().unwrap_or_default();

    // Bevy UI picking reports position as normalized (-0.5,-0.5)→(0.5,0.5)
    // relative to node center in physical pixels. Convert to viewport-local
    // logical pixels (0,0 = top-left) — screen_to_char_pos works in logical.
    let local_pos = match trigger.event().hit.position {
        Some(p) => (Vec2::new(p.x, p.y) + 0.5) * computed.size() * computed.inverse_scale_factor(),
        None => return,
    };

    let inv = computed.inverse_scale_factor();
    let text_area_left = computed.content_inset().min_inset.x * inv;
    let text_area_top = computed.content_inset().min_inset.y * inv;
    let line_height = bevy_instanced_text::resolve_line_height(*lh, font.font_size);
    let char_pos = screen_to_char_pos(
        local_pos,
        &**buffer,
        layout,
        scroll.y,
        mono,
        line_height,
        text_area_left,
        text_area_top,
        None,
    );

    let alt_held = keyboard.pressed(KeyCode::AltLeft) || keyboard.pressed(KeyCode::AltRight);
    let ctrl_or_cmd_held = keyboard.pressed(KeyCode::ControlLeft)
        || keyboard.pressed(KeyCode::ControlRight)
        || keyboard.pressed(KeyCode::SuperLeft)
        || keyboard.pressed(KeyCode::SuperRight);

    // Ctrl/Cmd-click is a navigation gesture — let higher-level observers handle it.
    if ctrl_or_cmd_held {
        input_focus.set(entity);
        return;
    }

    // Click-count detection: same-position click within `interaction.multi_click_secs` bumps the count.
    let now = time.elapsed_secs_f64();
    let near_last = drag_state
        .last_press_pos
        .map(|p| (p - local_pos).length() <= interaction.multi_click_radius_px)
        .unwrap_or(false);
    drag_state.click_count =
        if near_last && (now - drag_state.last_press_time) <= interaction.multi_click_secs {
            (drag_state.click_count + 1).min(3)
        } else {
            1
        };
    drag_state.last_press_time = now;
    drag_state.last_press_pos = Some(local_pos);

    let mode = if alt_held {
        crate::selection::SelectionMode::Block
    } else {
        match drag_state.click_count {
            2 => crate::selection::SelectionMode::Semantic,
            3 => crate::selection::SelectionMode::Line,
            _ => crate::selection::SelectionMode::Simple,
        }
    };
    drag_state.mode = mode;

    if let Some(mut sel) = sel {
        match mode {
            crate::selection::SelectionMode::Semantic => {
                let mut s = crate::selection::Selection::cursor(char_pos);
                s.expand_semantic(&**buffer, crate::selection::DEFAULT_SEMANTIC_ESCAPE_CHARS);
                sel.selections.clear_secondary();
                *sel.selections.primary_mut() = s;
            }
            crate::selection::SelectionMode::Line => {
                let mut s = crate::selection::Selection::cursor(char_pos);
                s.expand_to_lines(&**buffer);
                sel.selections.clear_secondary();
                *sel.selections.primary_mut() = s;
            }
            _ => {
                sel.selections.set_cursor(char_pos);
                sel.selections.primary_mut().mode = mode;
            }
        }
    }
    if let Some(mut cursor) = cursor {
        cursor.cursor_pos = char_pos;
    }
    drag_state.is_dragging = true;
    drag_state.drag_start_pos = Some(char_pos);
    drag_state.drag_start_scroll_offset = scroll.y;
    drag_state.last_screen_pos = Some(trigger.event().pointer_location.position);
    input_focus.set(entity);
}

/// Per-entity click-gesture tuning. Cascaded onto interactive views via
/// `#[require]`, so every interactive view starts with sensible defaults
/// (~OS conventions). Override on spawn to fit the surface — touch-style
/// chat panels want a larger `multi_click_radius_px` and longer
/// `multi_click_secs` than a precision code editor.
#[derive(Clone, Copy, Debug, Component, Reflect)]
#[reflect(Component, Default, Debug)]
pub struct InteractionSettings {
    /// Two consecutive clicks must fall within this window to count as a
    /// multi-click. Defaults to 0.5s — matches macOS / typical Linux DEs.
    /// Windows uses ~0.53s; touch UIs may want 0.75s+.
    pub multi_click_secs: f64,
    /// Two consecutive clicks must fall within this radius (viewport-local
    /// pixels) to count as a multi-click. Defaults to 4 px.
    pub multi_click_radius_px: f32,
}

impl Default for InteractionSettings {
    fn default() -> Self {
        Self {
            multi_click_secs: 0.5,
            multi_click_radius_px: 4.0,
        }
    }
}

/// Drag observer: extend the selection while the primary button is held.
///
/// Picking dispatches `Pointer<Drag>` to the entity that received the
/// initial press, so this stays scoped to the view that started the drag
/// even if the cursor moves out of its viewport.
pub fn on_pointer_drag<T: TextContent + Component>(
    trigger: On<Pointer<Drag>>,
    mut views: DragQuery<T>,
) {
    if trigger.event().button != PointerButton::Primary {
        return;
    }
    let entity = trigger.event().entity;
    let Ok((
        mut drag_state,
        buffer,
        scroll,
        font,
        lh,
        mono,
        computed,
        ui_transform,
        layout,
        sel,
        cursor,
    )) = views.get_mut(entity)
    else {
        return;
    };
    if !drag_state.is_dragging {
        return;
    }

    let cursor_pos = trigger.event().pointer_location.position;

    if let Some(last_pos) = drag_state.last_screen_pos {
        if (cursor_pos - last_pos).length() < 2.0 {
            return;
        }
    }

    let inv_scale = computed.inverse_scale_factor();
    let node_top_left_logical = (ui_transform.translation.xy() - computed.size() * 0.5) * inv_scale;
    let local_pos = cursor_pos - node_top_left_logical;
    let text_area_left = computed.content_inset().min_inset.x * inv_scale;
    let text_area_top = computed.content_inset().min_inset.y * inv_scale;
    let line_height = bevy_instanced_text::resolve_line_height(*lh, font.font_size);
    let char_pos = screen_to_char_pos(
        local_pos,
        &**buffer,
        layout,
        scroll.y,
        mono,
        line_height,
        text_area_left,
        text_area_top,
        Some(drag_state.drag_start_scroll_offset),
    );

    if let (Some(mut sel), Some(start)) = (sel, drag_state.drag_start_pos) {
        let mode = drag_state.mode;
        if start == char_pos && mode == crate::selection::SelectionMode::Simple {
            sel.selections.set_cursor(char_pos);
        } else {
            let mut s = crate::selection::Selection::with_mode(char_pos, start, mode);
            match mode {
                crate::selection::SelectionMode::Semantic => {
                    s.expand_semantic(&**buffer, crate::selection::DEFAULT_SEMANTIC_ESCAPE_CHARS);
                }
                crate::selection::SelectionMode::Line => {
                    s.expand_to_lines(&**buffer);
                }
                _ => {}
            }
            sel.selections.clear_secondary();
            *sel.selections.primary_mut() = s;
        }
    }
    if let Some(mut cursor) = cursor {
        cursor.cursor_pos = char_pos;
    }
    drag_state.last_screen_pos = Some(cursor_pos);
}

/// Release observer: clear the drag flag.
pub fn on_pointer_release(
    trigger: On<Pointer<Release>>,
    mut views: Query<&mut TextViewDragState, With<DisplayLayout>>,
) {
    if trigger.event().button != PointerButton::Primary {
        return;
    }
    let entity = trigger.event().entity;
    if let Ok(mut drag_state) = views.get_mut(entity) {
        drag_state.is_dragging = false;
        drag_state.mode = crate::selection::SelectionMode::Simple;
    }
}

/// Focused-keyboard observer: copy the selection on Cmd/Ctrl+C.
///
/// Generic over [`TextContent`] so terminals, labels, and editors all share
/// this Cmd+C path. Editors with leafwing-driven `CopyRequested` flows can
/// still run their own handler — both paths are idempotent on the clipboard.
pub fn on_focused_keyboard<T: TextContent + Component>(
    trigger: On<FocusedInput<KeyboardInput>>,
    views: CopyQuery<T>,
    keyboard: Res<ButtonInput<KeyCode>>,
    clipboard: Res<crate::clipboard::ClipboardResource>,
) {
    let entity = trigger.event().focused_entity;
    let Ok((sel, buffer)) = views.get(entity) else {
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

    copy_selection(sel, &**buffer, &clipboard);
}

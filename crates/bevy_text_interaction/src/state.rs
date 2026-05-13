//! Per-entity cursor + selection state.
//!
//! These components are shared by every interactive text view: terminals
//! use [`SelectionState`] for copy-selection, editors use both [`CursorState`]
//! and [`SelectionState`] for caret + multi-cursor. Neither component knows
//! about ropey or any specific backing — they hold pure offset data and
//! delegate any content-dependent operations to a [`bevy_instanced_text::TextContent`].

use bevy::prelude::*;
use bevy_instanced_text::TextContent;

use crate::selection::SelectionCollection;

/// Primary cursor position and blink-tracking state.
///
/// `cursor_pos` mirrors `selections.primary().head_offset()`; handlers mutate it
/// during a keystroke and apply the full `SelectionCollection` update afterward.
/// `last_cursor_pos` drives auto-scroll detection; `last_cursor_pos_for_blink`
/// is tracked separately to avoid racing with the auto-scroll system, and is
/// paired with the [`crate::BlinkPhase`] timestamp on the same entity.
#[derive(Component, Default)]
pub struct CursorState {
    pub cursor_pos: usize,
    pub last_cursor_pos: usize,
    pub last_cursor_pos_for_blink: usize,
}

/// Multi-cursor selection state. Wraps a [`SelectionCollection`] so a single
/// component can carry an arbitrary number of selections and a primary.
#[derive(Component, Default)]
pub struct SelectionState {
    pub selections: SelectionCollection,
}

impl SelectionState {
    /// Push the imperative `cursor.cursor_pos` (mutated by movement and edit
    /// helpers) into the primary `Selection` in the collection. Drops any
    /// secondary cursors and collapses the selection.
    pub fn apply_primary_cursor(&mut self, cursor: &CursorState) {
        self.selections.set_cursor(cursor.cursor_pos);
    }

    /// Like [`Self::apply_primary_cursor`] but preserves an existing selection
    /// anchor (or, if there is no selection, uses `default_anchor` as the
    /// new anchor — typically the cursor position from before the move).
    pub fn apply_primary_with_anchor(&mut self, cursor: &CursorState, default_anchor: usize) {
        let anchor = if self.selections.primary().has_selection() {
            self.selections.primary().anchor_offset()
        } else {
            default_anchor
        };
        self.selections.set_selection(cursor.cursor_pos, anchor);
    }

    /// Read back the primary cursor's head into `cursor.cursor_pos` so the
    /// imperative-update helpers see the up-to-date offset.
    pub fn refresh_primary_cursor(&self, cursor: &mut CursorState) {
        cursor.cursor_pos = self.selections.primary().head_offset();
    }

    /// Add a new cursor at the given position (clamped to the content length).
    pub fn add_cursor_at<T: TextContent>(&mut self, content: &T, position: usize) {
        let position = position.min(content.char_count());
        self.selections.add_cursor(position);
    }

    /// Add a new cursor with a selection range (both endpoints clamped).
    pub fn add_cursor_with_range<T: TextContent>(&mut self, content: &T, head: usize, anchor: usize) {
        let max = content.char_count();
        let head = head.min(max);
        let anchor = anchor.min(max);
        self.selections.add_selection_range(head, anchor);
    }

    /// Remove all cursors except the primary, then refresh `cursor.cursor_pos`.
    pub fn clear_secondary_cursors(&mut self, cursor: &mut CursorState) {
        self.selections.clear_secondary();
        self.refresh_primary_cursor(cursor);
    }

    /// Whether the collection currently holds more than one cursor / selection.
    pub fn has_multiple_cursors(&self) -> bool {
        self.selections.is_multiple()
    }

    /// Number of cursors / selections currently active.
    pub fn cursor_count(&self) -> usize {
        self.selections.len()
    }

    /// Apply pending edits to the selection collection.
    pub fn apply_selection_edits(&mut self) {
        self.selections.apply_pending_edits();
    }

    /// Get the primary selection from the collection.
    pub fn primary_selection(&self) -> &crate::selection::Selection {
        self.selections.primary()
    }

    /// Get all selection ranges as (start, end) tuples.
    pub fn selection_ranges(&self) -> Vec<(usize, usize)> {
        self.selections.ranges()
    }

    /// Range of the primary selection if non-empty (otherwise `None`).
    pub fn primary_range(&self) -> Option<(usize, usize)> {
        let primary = self.selections.primary();
        if primary.has_selection() {
            Some(primary.range())
        } else {
            None
        }
    }
}

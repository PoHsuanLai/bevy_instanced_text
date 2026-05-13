//! Selection and cursor types.
//!
//! Selections are anchor-based and edit-resilient. They work over **any**
//! [`bevy_instanced_text::TextContent`] backing — terminals, labels, and
//! rope-backed editors share the same selection model. The handful of
//! content-aware operations ([`Selection::expand_semantic`],
//! [`Selection::expand_to_lines`]) are generic over `&impl TextContent`
//! and use the trait's char-index methods.

use std::sync::atomic::{AtomicU64, Ordering};

use bevy::reflect::Reflect;
use bevy_instanced_text::TextContent;

use crate::text_edit::{Anchor, AnchorSet, TextEdit};

/// Default semantic-boundary characters: word breakers + brackets + quotes.
/// Matches the alacritty default and is a sensible cross-domain choice
/// (editor word selection, terminal double-click expansion, log viewer).
pub const DEFAULT_SEMANTIC_ESCAPE_CHARS: &str = ",│`|:\"' ()[]{}<>\t";

/// What kind of region a selection covers.
///
/// `Simple` is the editor default (free-form char range). `Block` is
/// rectangular (column-aligned), useful for column edits and reading
/// terminal output. `Line` is whole-line (triple-click). `Semantic`
/// expands to word/symbol boundaries (double-click).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Reflect)]
pub enum SelectionMode {
    #[default]
    Simple,
    Block,
    Line,
    Semantic,
}

/// Cursor + optional selection anchor, both edit-resilient. `head` is the
/// cursor (Left bias); `anchor` is where the selection started (Right bias).
///
/// When `head == anchor`, there's no selection (just a cursor).
/// The head and anchor can be in any order - head can be before or after anchor.
#[derive(Clone, Debug)]
pub struct Selection {
    /// The cursor position (where the cursor blinks)
    /// Uses Left bias so it stays before inserted text
    pub head: Anchor,
    /// The selection anchor (where selection started)
    /// Uses Right bias so selection expands to include inserted text at the boundary
    pub anchor: Anchor,
    /// Selection shape — how the (start, end) range is interpreted.
    pub mode: SelectionMode,
    /// Unique ID for this selection (for tracking across operations)
    id: u64,
}

/// Global counter for generating unique selection IDs
static SELECTION_ID_COUNTER: AtomicU64 = AtomicU64::new(0);

impl Selection {
    /// Create a new selection with just a cursor (no selection)
    pub fn cursor(offset: usize) -> Self {
        Self {
            head: Anchor::at(offset),
            anchor: Anchor::at(offset),
            mode: SelectionMode::Simple,
            id: SELECTION_ID_COUNTER.fetch_add(1, Ordering::Relaxed),
        }
    }

    /// Create a new selection with a range
    /// `head` is where the cursor is, `anchor` is where the selection started
    pub fn new(head: usize, anchor: usize) -> Self {
        Self {
            head: Anchor::at(head),
            anchor: Anchor::at_right(anchor),
            mode: SelectionMode::Simple,
            id: SELECTION_ID_COUNTER.fetch_add(1, Ordering::Relaxed),
        }
    }

    /// Create a new selection with an explicit mode.
    pub fn with_mode(head: usize, anchor: usize, mode: SelectionMode) -> Self {
        Self {
            head: Anchor::at(head),
            anchor: Anchor::at_right(anchor),
            mode,
            id: SELECTION_ID_COUNTER.fetch_add(1, Ordering::Relaxed),
        }
    }

    /// Create a selection from anchor objects
    pub fn from_anchors(head: Anchor, anchor: Anchor) -> Self {
        Self {
            head,
            anchor,
            mode: SelectionMode::Simple,
            id: SELECTION_ID_COUNTER.fetch_add(1, Ordering::Relaxed),
        }
    }

    /// Get the unique ID of this selection
    pub fn id(&self) -> u64 {
        self.id
    }

    /// Get the head (cursor) position
    pub fn head_offset(&self) -> usize {
        self.head.offset
    }

    /// Get the anchor position
    pub fn anchor_offset(&self) -> usize {
        self.anchor.offset
    }

    /// Get the start position (minimum of head and anchor)
    pub fn start(&self) -> usize {
        self.head.offset.min(self.anchor.offset)
    }

    /// Get the end position (maximum of head and anchor)
    pub fn end(&self) -> usize {
        self.head.offset.max(self.anchor.offset)
    }

    /// Get the range as (start, end) tuple, always ordered
    pub fn range(&self) -> (usize, usize) {
        (self.start(), self.end())
    }

    /// Check if this is just a cursor (no selection)
    pub fn is_cursor(&self) -> bool {
        self.head.offset == self.anchor.offset
    }

    /// Check if there is an actual selection (head != anchor)
    pub fn has_selection(&self) -> bool {
        self.head.offset != self.anchor.offset
    }

    /// Check if the selection is "reversed" (anchor is after head)
    pub fn is_reversed(&self) -> bool {
        self.anchor.offset > self.head.offset
    }

    /// Check if a position is within the selected range
    pub fn contains(&self, offset: usize) -> bool {
        let (start, end) = self.range();
        offset >= start && offset < end
    }

    /// Check if this selection overlaps with another
    pub fn overlaps(&self, other: &Selection) -> bool {
        let (s1, e1) = self.range();
        let (s2, e2) = other.range();
        s1 < e2 && s2 < e1
    }

    /// Check if this selection is adjacent to another (touching but not overlapping)
    pub fn is_adjacent(&self, other: &Selection) -> bool {
        let (_, e1) = self.range();
        let (s2, _) = other.range();
        e1 == s2
    }

    /// Check if this selection can be merged with another (overlapping or adjacent)
    pub fn can_merge(&self, other: &Selection) -> bool {
        self.overlaps(other) || self.is_adjacent(other) || other.is_adjacent(self)
    }

    /// Merge this selection with another, returning the merged selection
    /// The head position comes from `self` (the "primary" selection in the merge)
    pub fn merge(&self, other: &Selection) -> Selection {
        let new_start = self.start().min(other.start());
        let new_end = self.end().max(other.end());

        // Preserve the head direction from self.
        // Mode follows self — the "primary" half of the merge.
        if self.is_reversed() {
            Selection::with_mode(new_start, new_end, self.mode)
        } else {
            Selection::with_mode(new_end, new_start, self.mode)
        }
    }

    /// Expand this selection to whole-word boundaries on both ends.
    ///
    /// Walks left from `start()` and right from `end()` until a boundary
    /// character (`escape_chars`) or content edge. After expansion `mode`
    /// is set to `Semantic`. For terminal use the alacritty default
    /// [`DEFAULT_SEMANTIC_ESCAPE_CHARS`] is appropriate.
    pub fn expand_semantic<T: TextContent>(&mut self, content: &T, escape_chars: &str) {
        let len = content.char_count();
        if len == 0 {
            return;
        }
        let (start, end) = self.range();
        let new_start = walk_back_to_boundary(content, start, escape_chars);
        let new_end = walk_forward_to_boundary(content, end, escape_chars).min(len);

        // Preserve direction.
        if self.is_reversed() {
            self.head.offset = new_start;
            self.anchor.offset = new_end;
        } else {
            self.head.offset = new_end;
            self.anchor.offset = new_start;
        }
        self.mode = SelectionMode::Semantic;
    }

    /// Expand this selection to whole lines.
    ///
    /// `start()` snaps to its line start, `end()` snaps to the start of
    /// the line after `end()` (so the trailing newline is included).
    /// Mode is set to `Line`.
    pub fn expand_to_lines<T: TextContent>(&mut self, content: &T) {
        let len = content.char_count();
        if len == 0 {
            return;
        }
        let (start, end) = self.range();
        let line_start = content.line_to_char(content.char_to_line(start));
        let end_line = content.char_to_line(end.min(len));
        let next_line_start = content
            .line_to_char((end_line + 1).min(content.line_count()))
            .min(len);

        if self.is_reversed() {
            self.head.offset = line_start;
            self.anchor.offset = next_line_start;
        } else {
            self.head.offset = next_line_start;
            self.anchor.offset = line_start;
        }
        self.mode = SelectionMode::Line;
    }

    /// Adjust this selection based on a text edit
    pub fn adjust(&mut self, edit: &TextEdit) {
        self.head.offset = AnchorSet::adjust_offset(self.head.offset, self.head.bias, edit);
        self.anchor.offset = AnchorSet::adjust_offset(self.anchor.offset, self.anchor.bias, edit);
    }

    /// Move the head to a new position, optionally extending the selection
    pub fn move_head(&mut self, offset: usize, extend: bool) {
        self.head.offset = offset;
        if !extend {
            self.anchor.offset = offset;
        }
    }

    /// Collapse the selection to just a cursor at the head position
    pub fn collapse_to_head(&mut self) {
        self.anchor.offset = self.head.offset;
    }

    /// Collapse the selection to just a cursor at the start position
    pub fn collapse_to_start(&mut self) {
        let start = self.start();
        self.head.offset = start;
        self.anchor.offset = start;
    }

    /// Collapse the selection to just a cursor at the end position
    pub fn collapse_to_end(&mut self) {
        let end = self.end();
        self.head.offset = end;
        self.anchor.offset = end;
    }

    /// Get the length of the selection (0 if just a cursor)
    pub fn len(&self) -> usize {
        self.end() - self.start()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

fn is_boundary(ch: char, escape_chars: &str) -> bool {
    ch.is_whitespace() || escape_chars.contains(ch)
}

/// Helper for [`Selection::expand_semantic`]: walk left until a boundary char.
/// Generic over any [`TextContent`] — terminals, ropes, and label strings all work.
fn walk_back_to_boundary<T: TextContent>(content: &T, offset: usize, escape_chars: &str) -> usize {
    if offset == 0 {
        return 0;
    }
    let slice = content.slice_chars(0..offset);
    let mut new_offset = offset;
    for ch in slice.chars().rev() {
        if is_boundary(ch, escape_chars) {
            break;
        }
        new_offset -= 1;
    }
    new_offset
}

/// Helper for [`Selection::expand_semantic`]: walk right until a boundary char.
fn walk_forward_to_boundary<T: TextContent>(content: &T, offset: usize, escape_chars: &str) -> usize {
    let len = content.char_count();
    if offset >= len {
        return len;
    }
    let slice = content.slice_chars(offset..len);
    let mut new_offset = offset;
    for ch in slice.chars() {
        if is_boundary(ch, escape_chars) {
            break;
        }
        new_offset += 1;
    }
    new_offset
}

impl PartialEq for Selection {
    fn eq(&self, other: &Self) -> bool {
        self.head.offset == other.head.offset && self.anchor.offset == other.anchor.offset
    }
}

impl Eq for Selection {}

impl PartialOrd for Selection {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Selection {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // Sort by start position, then by end position
        self.start()
            .cmp(&other.start())
            .then_with(|| self.end().cmp(&other.end()))
    }
}

/// A collection of non-overlapping selections, maintained in sorted order.
///
/// This is the primary interface for managing multiple selections in the editor.
/// It automatically:
/// - Keeps selections sorted by position
/// - Merges overlapping and adjacent selections
/// - Adjusts all selections when text is edited
///
/// The first selection (index 0) is the "primary" selection that determines
/// the main cursor position for scrolling and other operations.
#[derive(Clone, Debug)]
pub struct SelectionCollection {
    /// The selections, maintained in sorted order by start position
    /// Index 0 is the "primary" selection
    selections: Vec<Selection>,
    /// Pending edits to apply to all selections
    pending_edits: Vec<TextEdit>,
    /// Version counter for tracking changes
    version: u64,
}

impl Default for SelectionCollection {
    fn default() -> Self {
        Self::new()
    }
}

impl SelectionCollection {
    /// Create a new collection with a single cursor at position 0
    pub fn new() -> Self {
        Self {
            selections: vec![Selection::cursor(0)],
            pending_edits: Vec::new(),
            version: 0,
        }
    }

    /// Create a collection with a single cursor at the given position
    pub fn with_cursor(offset: usize) -> Self {
        Self {
            selections: vec![Selection::cursor(offset)],
            pending_edits: Vec::new(),
            version: 0,
        }
    }

    /// Create a collection with a single selection
    pub fn with_selection(head: usize, anchor: usize) -> Self {
        Self {
            selections: vec![Selection::new(head, anchor)],
            pending_edits: Vec::new(),
            version: 0,
        }
    }

    /// Get the primary selection (first selection)
    pub fn primary(&self) -> &Selection {
        // There's always at least one selection
        &self.selections[0]
    }

    /// Get a mutable reference to the primary selection
    pub fn primary_mut(&mut self) -> &mut Selection {
        &mut self.selections[0]
    }

    /// Get the primary cursor position (head of primary selection)
    pub fn cursor_pos(&self) -> usize {
        self.primary().head_offset()
    }

    /// Get the number of selections
    pub fn len(&self) -> usize {
        self.selections.len()
    }

    pub fn is_empty(&self) -> bool {
        self.selections.is_empty()
    }

    /// Check if there's only a single cursor (no multi-selection, no text selected)
    pub fn is_single_cursor(&self) -> bool {
        self.selections.len() == 1 && self.selections[0].is_cursor()
    }

    /// Check if any selection has text selected
    pub fn has_selection(&self) -> bool {
        self.selections.iter().any(|s| s.has_selection())
    }

    /// Check if there are multiple selections
    pub fn is_multiple(&self) -> bool {
        self.selections.len() > 1
    }

    /// Iterate over all selections
    pub fn iter(&self) -> impl Iterator<Item = &Selection> {
        self.selections.iter()
    }

    /// Iterate over all selections mutably
    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut Selection> {
        self.selections.iter_mut()
    }

    /// Get a selection by index
    pub fn get(&self, index: usize) -> Option<&Selection> {
        self.selections.get(index)
    }

    /// Get a mutable selection by index
    pub fn get_mut(&mut self, index: usize) -> Option<&mut Selection> {
        self.selections.get_mut(index)
    }

    /// Add a new selection (cursor only) at the given position
    /// Returns the index of the new selection after sorting/merging
    pub fn add_cursor(&mut self, offset: usize) -> usize {
        self.add_selection(Selection::cursor(offset))
    }

    /// Add a new selection with a range
    /// Returns the index of the new selection after sorting/merging
    pub fn add_selection_range(&mut self, head: usize, anchor: usize) -> usize {
        self.add_selection(Selection::new(head, anchor))
    }

    /// Add a selection to the collection
    /// Automatically sorts and merges overlapping selections
    /// Returns the index of the added (or merged) selection
    pub fn add_selection(&mut self, selection: Selection) -> usize {
        self.selections.push(selection);
        self.sort_and_merge();
        self.version += 1;

        // Find the index of the selection we just added (it might have been merged)
        // For now, return the last index which is where we added it
        self.selections.len().saturating_sub(1)
    }

    /// Remove all selections except the primary
    pub fn clear_secondary(&mut self) {
        if self.selections.len() > 1 {
            self.selections.truncate(1);
            self.version += 1;
        }
    }

    /// Replace all selections with a single cursor
    pub fn set_cursor(&mut self, offset: usize) {
        self.selections.clear();
        self.selections.push(Selection::cursor(offset));
        self.version += 1;
    }

    /// Replace all selections with a single selection
    pub fn set_selection(&mut self, head: usize, anchor: usize) {
        self.selections.clear();
        self.selections.push(Selection::new(head, anchor));
        self.version += 1;
    }

    /// Move the primary selection's head, optionally extending
    pub fn move_primary(&mut self, offset: usize, extend: bool) {
        self.selections[0].move_head(offset, extend);
        self.version += 1;
    }

    /// Move all selection heads by applying a function
    pub fn move_all<F>(&mut self, mut f: F, extend: bool)
    where
        F: FnMut(usize) -> usize,
    {
        for selection in &mut self.selections {
            let new_pos = f(selection.head_offset());
            selection.move_head(new_pos, extend);
        }
        if !extend {
            // If not extending, selections might now be at the same position
            // and should be deduplicated
            self.sort_and_merge();
        }
        self.version += 1;
    }

    /// Collapse all selections to cursors at their head positions
    pub fn collapse_all_to_head(&mut self) {
        for selection in &mut self.selections {
            selection.collapse_to_head();
        }
        self.sort_and_merge();
        self.version += 1;
    }

    /// Collapse all selections to cursors at their start positions
    pub fn collapse_all_to_start(&mut self) {
        for selection in &mut self.selections {
            selection.collapse_to_start();
        }
        self.sort_and_merge();
        self.version += 1;
    }

    /// Collapse all selections to cursors at their end positions
    pub fn collapse_all_to_end(&mut self) {
        for selection in &mut self.selections {
            selection.collapse_to_end();
        }
        self.sort_and_merge();
        self.version += 1;
    }

    /// Record a text edit to adjust all selections
    pub fn record_edit(&mut self, edit: TextEdit) {
        self.pending_edits.push(edit);
        self.version += 1;
    }

    /// Apply all pending edits to selections
    pub fn apply_pending_edits(&mut self) {
        if self.pending_edits.is_empty() {
            return;
        }

        for selection in &mut self.selections {
            for edit in &self.pending_edits {
                selection.adjust(edit);
            }
        }

        self.pending_edits.clear();

        // Re-sort and merge after adjustments (edits might cause overlaps)
        self.sort_and_merge();
    }

    /// Sort selections by position and merge overlapping/adjacent ones
    fn sort_and_merge(&mut self) {
        if self.selections.len() <= 1 {
            return;
        }

        // Sort by start position
        self.selections.sort();

        // Merge overlapping and adjacent selections
        let mut merged: Vec<Selection> = Vec::with_capacity(self.selections.len());

        for selection in self.selections.drain(..) {
            if let Some(last) = merged.last_mut() {
                if last.can_merge(&selection) {
                    // Merge into the existing selection
                    *last = last.merge(&selection);
                } else {
                    merged.push(selection);
                }
            } else {
                merged.push(selection);
            }
        }

        self.selections = merged;

        // Ensure we always have at least one selection
        if self.selections.is_empty() {
            self.selections.push(Selection::cursor(0));
        }
    }

    /// Get the ranges of all selections as (start, end) tuples
    pub fn ranges(&self) -> Vec<(usize, usize)> {
        self.selections.iter().map(|s| s.range()).collect()
    }

    /// Get the version (incremented on changes)
    pub fn version(&self) -> u64 {
        self.version
    }

    /// Check if any selection contains the given offset
    pub fn any_contains(&self, offset: usize) -> bool {
        self.selections.iter().any(|s| s.contains(offset))
    }

    /// Find the selection containing the given offset (if any)
    pub fn selection_at(&self, offset: usize) -> Option<&Selection> {
        self.selections.iter().find(|s| s.contains(offset))
    }

    /// Convert to a Vec of (head, anchor) tuples for compatibility
    pub fn to_head_anchor_pairs(&self) -> Vec<(usize, Option<usize>)> {
        self.selections
            .iter()
            .map(|s| {
                if s.is_cursor() {
                    (s.head_offset(), None)
                } else {
                    (s.head_offset(), Some(s.anchor_offset()))
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semantic_expansion_grabs_word() {
        let content = String::from("hello world foo");
        let mut sel = Selection::cursor(7); // inside "world"
        sel.expand_semantic(&content, DEFAULT_SEMANTIC_ESCAPE_CHARS);
        assert_eq!(sel.range(), (6, 11));
        assert_eq!(sel.mode, SelectionMode::Semantic);
    }

    #[test]
    fn line_expansion_includes_newline() {
        let content = String::from("first\nsecond\nthird\n");
        let mut sel = Selection::cursor(8); // inside "second"
        sel.expand_to_lines(&content);
        // "second\n" is chars 6..13
        assert_eq!(sel.range(), (6, 13));
        assert_eq!(sel.mode, SelectionMode::Line);
    }

    #[test]
    fn semantic_does_not_cross_brackets() {
        let content = String::from("foo.bar(baz)");
        let mut sel = Selection::cursor(9); // inside "baz"
        sel.expand_semantic(&content, DEFAULT_SEMANTIC_ESCAPE_CHARS);
        // Stops at '(' and ')' — selects "baz".
        assert_eq!(sel.range(), (8, 11));
    }

    #[test]
    fn semantic_keeps_dotted_identifier() {
        let content = String::from("foo.bar baz");
        let mut sel = Selection::cursor(2);
        sel.expand_semantic(&content, DEFAULT_SEMANTIC_ESCAPE_CHARS);
        // Stops at the space after "bar" — selects "foo.bar".
        assert_eq!(sel.range(), (0, 7));
    }
}

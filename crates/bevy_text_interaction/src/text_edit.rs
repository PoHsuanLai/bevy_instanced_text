//! Anchor-based position tracking types

use std::sync::atomic::{AtomicU64, Ordering};

static ANCHOR_ID_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Determines how an anchor behaves when text is inserted exactly at its position.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum AnchorBias {
    #[default]
    Left,
    Right,
}

/// Edit-resilient position: adjusts automatically when text is inserted or
/// deleted around it. Use for cursors, selection bounds, LSP diagnostic marks,
/// or any position that must survive edits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Anchor {
    pub id: u64,
    pub offset: usize,
    pub bias: AnchorBias,
    /// Buffer version when last updated; stale if edits occurred since.
    pub version: u64,
}

impl Anchor {
    pub fn new(offset: usize, bias: AnchorBias) -> Self {
        Self {
            id: ANCHOR_ID_COUNTER.fetch_add(1, Ordering::Relaxed),
            offset,
            bias,
            version: 0,
        }
    }

    pub fn at(offset: usize) -> Self {
        Self::new(offset, AnchorBias::Left)
    }

    pub fn at_right(offset: usize) -> Self {
        Self::new(offset, AnchorBias::Right)
    }
}

impl Default for Anchor {
    fn default() -> Self {
        Self::at(0)
    }
}

impl PartialOrd for Anchor {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Anchor {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.offset
            .cmp(&other.offset)
            .then_with(|| self.bias.cmp(&other.bias))
    }
}

impl PartialOrd for AnchorBias {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for AnchorBias {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // Left bias comes before Right bias at the same position
        match (self, other) {
            (AnchorBias::Left, AnchorBias::Right) => std::cmp::Ordering::Less,
            (AnchorBias::Right, AnchorBias::Left) => std::cmp::Ordering::Greater,
            _ => std::cmp::Ordering::Equal,
        }
    }
}

/// A text edit operation for anchor adjustment.
#[derive(Clone, Debug)]
pub struct TextEdit {
    /// Character offset where the edit begins
    pub start: usize,
    /// Character offset of the old end (> start for deletions)
    pub old_end: usize,
    /// Character offset of the new end (> start for insertions)
    pub new_end: usize,
}

impl TextEdit {
    pub fn insert(position: usize, length: usize) -> Self {
        Self {
            start: position,
            old_end: position,
            new_end: position + length,
        }
    }

    pub fn delete(start: usize, end: usize) -> Self {
        Self {
            start,
            old_end: end,
            new_end: start,
        }
    }

    pub fn replace(start: usize, old_end: usize, new_length: usize) -> Self {
        Self {
            start,
            old_end,
            new_end: start + new_length,
        }
    }

    pub fn delta(&self) -> isize {
        self.new_end as isize - self.old_end as isize
    }

    pub fn is_insertion(&self) -> bool {
        self.start == self.old_end && self.new_end > self.start
    }

    pub fn is_deletion(&self) -> bool {
        self.old_end > self.start && self.new_end == self.start
    }
}

/// Tracks anchors and batch-updates them when text edits occur.
#[derive(Clone, Debug, Default)]
pub struct AnchorSet {
    /// Sorted by offset for efficient range queries
    anchors: Vec<Anchor>,
    pending_edits: Vec<TextEdit>,
    version: u64,
}

impl AnchorSet {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, mut anchor: Anchor) -> u64 {
        anchor.version = self.version;
        let id = anchor.id;

        let pos = self
            .anchors
            .iter()
            .position(|a| a.offset > anchor.offset)
            .unwrap_or(self.anchors.len());
        self.anchors.insert(pos, anchor);

        id
    }

    pub fn anchor_at(&mut self, offset: usize, bias: AnchorBias) -> Anchor {
        let anchor = Anchor::new(offset, bias);
        self.insert(anchor);
        anchor
    }

    pub fn remove(&mut self, id: u64) -> Option<Anchor> {
        if let Some(pos) = self.anchors.iter().position(|a| a.id == id) {
            Some(self.anchors.remove(pos))
        } else {
            None
        }
    }

    pub fn get(&self, id: u64) -> Option<&Anchor> {
        self.anchors.iter().find(|a| a.id == id)
    }

    pub fn get_mut(&mut self, id: u64) -> Option<&mut Anchor> {
        self.anchors.iter_mut().find(|a| a.id == id)
    }

    pub fn resolve(&self, anchor: &Anchor) -> usize {
        let mut offset = anchor.offset;

        for edit in &self.pending_edits {
            offset = Self::adjust_offset(offset, anchor.bias, edit);
        }

        offset
    }

    pub fn record_edit(&mut self, edit: TextEdit) {
        self.pending_edits.push(edit);
        self.version += 1;
    }

    pub fn apply_pending_edits(&mut self) {
        if self.pending_edits.is_empty() {
            return;
        }

        for anchor in &mut self.anchors {
            for edit in &self.pending_edits {
                anchor.offset = Self::adjust_offset(anchor.offset, anchor.bias, edit);
            }
            anchor.version = self.version;
        }

        self.pending_edits.clear();

        self.anchors.sort_by_key(|a| (a.offset, a.bias));
    }

    pub fn adjust_offset(offset: usize, bias: AnchorBias, edit: &TextEdit) -> usize {
        if offset < edit.start {
            offset
        } else if offset > edit.old_end {
            let delta = edit.delta();
            if delta < 0 {
                offset.saturating_sub((-delta) as usize)
            } else {
                offset + delta as usize
            }
        } else if offset == edit.start && edit.is_insertion() {
            match bias {
                AnchorBias::Left => offset,
                AnchorBias::Right => edit.new_end,
            }
        } else {
            // Anchor is within the deleted range; collapse to edit start
            edit.start
        }
    }

    pub fn clear(&mut self) {
        self.anchors.clear();
        self.pending_edits.clear();
    }

    pub fn len(&self) -> usize {
        self.anchors.len()
    }

    pub fn is_empty(&self) -> bool {
        self.anchors.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Anchor> {
        self.anchors.iter()
    }

    pub fn anchors_in_range(&self, start: usize, end: usize) -> impl Iterator<Item = &Anchor> {
        self.anchors
            .iter()
            .filter(move |a| a.offset >= start && a.offset <= end)
    }

    pub fn version(&self) -> u64 {
        self.version
    }
}

//! Shared UI primitives for [`bevy_instanced_text`] views.
//!
//! This crate provides the **content-agnostic** pieces every interactive
//! text view needs: clipboard, multi-cursor selection model, blinking
//! caret renderer, pointer + keyboard observers. It depends only on
//! [`bevy_instanced_text`] and pulls in no rope library.
//!
//! Consumer matrix:
//!
//! - **DAW / HUD / labels** — depend on [`bevy_instanced_text`] alone.
//! - **Terminal** (bevsterm) — depend on this crate; spawn
//!   [`InstancedTextInteractionPlugin::<TextSpan>::default()`] to get
//!   click-to-place selection, drag-select, scroll, Cmd+C copy.
//! - **Editor** (bevscode, via `bevy_instanced_text_editor`) — gets this plus
//!   rope-backed editing, undo, multi-cursor expansion.
//!
//! ## Key components
//!
//! - [`ClipboardResource`] — system clipboard (pluggable via [`ClipboardProvider`]).
//! - [`SelectionState`] — multi-cursor / multi-selection state on an entity.
//! - [`CursorState`] — primary cursor's `usize` char offset.
//! - [`BlinkPhase`] / [`CursorSettings`] / [`TextCursorColor`] / [`TextSelectionColor`]
//!   — caret rendering primitives.
//! - [`ScrollConfig`] / [`InteractionSettings`] — per-view tuning.
//! - [`TextViewDragState`] — drag tracking written by [`on_pointer_press`] / [`on_pointer_drag`].
//!
//! ## Observers
//!
//! [`on_pointer_press`], [`on_pointer_drag`], [`on_pointer_release`],
//! [`on_pointer_scroll`], [`on_focused_keyboard`] are generic over the
//! content type and registered by [`InstancedTextInteractionPlugin<T>`].

pub mod clipboard;
pub mod color;
pub mod cursor;
pub mod focus;
pub mod interaction_states;
pub mod key_repeat;
pub mod plugin;
pub mod selection;
pub mod text_edit;
pub mod text_state;

#[cfg(feature = "arboard")]
pub use clipboard::SystemClipboard;
#[cfg(feature = "clipboard-wasm")]
pub use clipboard::WasmClipboard;
pub use clipboard::{ClipboardProvider, ClipboardResource, NullClipboard};
pub use color::{TextCursorColor, TextSelectionColor};
pub use cursor::{
    caret_overlay, cursor_blink_visible, BlinkPhase, CursorBlinkingMode, CursorSettings,
    CursorStyle, SmoothCaretAnimation, SurroundingLinesStyle,
};
pub use focus::{
    copy_selection, on_focused_keyboard, on_pointer_drag, on_pointer_press, on_pointer_release,
    on_pointer_scroll, screen_to_char_pos, selection_text, InteractionSettings,
};
pub use interaction_states::{
    ScrollConfig, ScrollbarConfig, ScrollbarVisibility, TextViewDragState,
};
pub use key_repeat::{KeyRepeatSettings, KeyRepeatState};
pub use plugin::InstancedTextInteractionPlugin;
pub use selection::{Selection, SelectionCollection, SelectionMode, DEFAULT_SEMANTIC_ESCAPE_CHARS};
pub use text_edit::{Anchor, AnchorBias, AnchorSet, TextEdit};
pub use text_state::{CursorState, SelectionState};

pub mod prelude {
    //! Common types for spawning interactive text views.
    #[cfg(feature = "arboard")]
    pub use crate::SystemClipboard;
    pub use crate::{
        caret_overlay, copy_selection, cursor_blink_visible, screen_to_char_pos, selection_text,
        Anchor, AnchorBias, AnchorSet, BlinkPhase, ClipboardProvider, ClipboardResource,
        CursorSettings, CursorState, CursorStyle, InstancedTextInteractionPlugin,
        InteractionSettings, KeyRepeatSettings, KeyRepeatState, NullClipboard, ScrollConfig,
        Selection, SelectionCollection, SelectionMode, SelectionState, TextCursorColor, TextEdit,
        TextSelectionColor, TextViewDragState, DEFAULT_SEMANTIC_ESCAPE_CHARS,
    };
}

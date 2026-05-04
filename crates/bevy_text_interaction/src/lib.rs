//! # bevy_text_interaction
//!
//! Mouse + keyboard interaction (scroll, click + drag selection, copy) for
//! [`bevy_text_engine`] `TextView` entities. This is the input-side peer to
//! the engine crate's render-side: pair [`TextInteractionPlugin`] with
//! `bevy_text_engine::TextEnginePlugins` to get a fully interactive text
//! view.
//!
//! ## What this crate adds
//!
//! - Per-view components [`TextViewSelectionState`], [`TextViewDragState`],
//!   [`ScrollConfig`].
//! - Three systems registered by [`TextInteractionPlugin`]: hit-tested
//!   mouse-wheel scroll, click + drag selection (with `InputFocus`
//!   handoff), and Cmd/Ctrl+C copy.
//! - Helper `screen_to_char_pos` for hosts that build their own click
//!   handlers (e.g. an editor that needs fold-aware click resolution).
//!
//! Hosts that want interactivity attach the state components to a
//! `TextView` entity (the editor does this via `#[require]`); plain
//! display-only views simply omit them.

pub mod components;
pub mod interaction;
pub mod plugin;
pub mod prelude;

pub use components::{ScrollConfig, TextViewDragState, TextViewSelectionState};
pub use interaction::{
    copy_selection, handle_text_view_copy, handle_text_view_mouse, handle_text_view_scroll,
    screen_to_char_pos,
};
pub use plugin::TextInteractionPlugin;

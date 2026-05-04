//! # bevy_text_interaction
//!
//! Pointer + focused-keyboard interaction (scroll, click + drag selection,
//! copy) for [`bevy_text_engine`] `TextView` entities. Pair
//! [`TextInteractionPlugin`] with `bevy_text_engine::TextEnginePlugins` to
//! get a fully interactive text view.
//!
//! ## Architecture
//!
//! - A custom `bevy_picking` backend in [`picking`] hit-tests the
//!   [`bevy_text_engine::TextViewViewport`] rect of every `TextView` and
//!   produces `PointerHits`.
//! - Observers in [`interaction`] consume `Pointer<Press|Drag|Release|Scroll>`
//!   events that picking has already routed to the right entity.
//! - Cmd/Ctrl+C is handled via a `FocusedInput<KeyboardInput>` observer
//!   driven by `bevy_input_focus::InputDispatchPlugin`.
//!
//! No polling systems, no manual cursor-rect hit tests.
//!
//! ## What this crate adds
//!
//! - Per-view components [`TextViewSelectionState`], [`TextViewDragState`],
//!   [`ScrollConfig`].
//! - Helper [`screen_to_char_pos`] for hosts that build their own click
//!   handlers (e.g. an editor that needs fold-aware click resolution).
//!
//! Hosts that want interactivity attach the state components to a
//! `TextView` entity (the editor does this via `#[require]`); plain
//! display-only views simply omit them.

pub mod components;
pub mod interaction;
pub mod picking;
pub mod plugin;
pub mod prelude;

pub use components::{ScrollConfig, TextViewDragState, TextViewSelectionState};
pub use interaction::{copy_selection, screen_to_char_pos};
pub use plugin::TextInteractionPlugin;

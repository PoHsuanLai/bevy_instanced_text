//! Convenient re-exports for hosts wiring up `TextView` interactivity.
//!
//! Pair with `bevy_text_engine::prelude::*` which supplies the rendering
//! primitives (`TextView`, `TextViewState`, `TextViewViewport`,
//! `FontConfig`, the engine plugin group).

pub use crate::components::{ScrollConfig, TextViewDragState, TextViewSelectionState};
pub use crate::interaction::{copy_selection, screen_to_char_pos};
pub use crate::plugin::TextInteractionPlugin;

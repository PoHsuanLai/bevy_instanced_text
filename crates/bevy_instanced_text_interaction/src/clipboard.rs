//! Pluggable clipboard backend.
//!
//! Copy / cut / paste handlers go through the [`ClipboardProvider`]
//! resource so embedders running headlessly, in CI, on the web, or under
//! a custom protocol (Wayland-only paste, sandboxed multi-window IPC) can
//! plug in their own implementation.
//!
//! With the default `arboard` feature, [`SystemClipboard`] wraps
//! `arboard` and is used as the default `ClipboardResource`. Disable the
//! feature to drop the `arboard` dependency entirely; the resource then
//! defaults to a no-op [`NullClipboard`] until you insert your own:
//!
//! ```rust,no_run
//! use bevy::prelude::*;
//! use bevy_instanced_text_interaction::{ClipboardProvider, ClipboardResource};
//!
//! #[derive(Default)]
//! struct InMemoryClipboard(std::sync::Mutex<String>);
//! impl ClipboardProvider for InMemoryClipboard {
//!     fn get_text(&self) -> Option<String> {
//!         Some(self.0.lock().ok()?.clone())
//!     }
//!     fn set_text(&self, text: &str) {
//!         if let Ok(mut g) = self.0.lock() {
//!             *g = text.to_owned();
//!         }
//!     }
//! }
//!
//! App::new()
//!     .insert_resource(ClipboardResource::new(InMemoryClipboard::default()))
//!     .run();
//! ```

use bevy::prelude::*;

/// Backing implementation for clipboard get / set. Methods take `&self`
/// (interior mutability) so the resource can stay `Res` rather than
/// `ResMut` and not serialize handlers behind a single mutable borrow.
pub trait ClipboardProvider: Send + Sync + 'static {
    /// Read the current clipboard text, or `None` if unavailable
    /// (no clipboard backend, paste blocked, headless, etc.).
    fn get_text(&self) -> Option<String>;

    /// Write text to the clipboard. Failures are swallowed — clipboard
    /// writes are best-effort; a missing backend should never propagate
    /// to the caller.
    fn set_text(&self, text: &str);
}

/// Resource holding the active clipboard backend. Inserted by
/// `InstancedTextInteractionPlugin` with [`SystemClipboard`] as the default
/// (or [`NullClipboard`] when the `arboard` feature is off).
/// Override by inserting a custom one before plugin setup.
#[derive(Resource)]
pub struct ClipboardResource(Box<dyn ClipboardProvider>);

impl ClipboardResource {
    pub fn new<P: ClipboardProvider>(provider: P) -> Self {
        Self(Box::new(provider))
    }

    pub fn get_text(&self) -> Option<String> {
        self.0.get_text()
    }

    pub fn set_text(&self, text: &str) {
        self.0.set_text(text);
    }
}

impl Default for ClipboardResource {
    fn default() -> Self {
        #[cfg(feature = "arboard")]
        {
            Self::new(SystemClipboard)
        }
        #[cfg(all(feature = "clipboard-wasm", not(feature = "arboard")))]
        {
            Self::new(WasmClipboard)
        }
        #[cfg(not(any(feature = "arboard", feature = "clipboard-wasm")))]
        {
            Self::new(NullClipboard)
        }
    }
}

/// No-op clipboard. Returns `None` from `get_text` and discards
/// `set_text`. Used as the default when the `arboard` feature is
/// disabled, or as an explicit choice for tests / sandboxed hosts.
pub struct NullClipboard;

impl ClipboardProvider for NullClipboard {
    fn get_text(&self) -> Option<String> {
        None
    }
    fn set_text(&self, _text: &str) {}
}

/// WASM clipboard backed by `navigator.clipboard`.
///
/// `set_text` fires off an async `writeText` call and returns immediately.
/// `get_text` always returns `None` — the browser clipboard API is
/// async-only and cannot be read synchronously. To support paste on WASM,
/// listen for the browser `paste` event and feed the text directly into
/// the editor state.
#[cfg(feature = "clipboard-wasm")]
pub struct WasmClipboard;

#[cfg(feature = "clipboard-wasm")]
impl ClipboardProvider for WasmClipboard {
    fn get_text(&self) -> Option<String> {
        None
    }

    fn set_text(&self, text: &str) {
        let window = web_sys::window().expect("no window");
        let navigator = window.navigator();
        let clipboard = navigator.clipboard();
        let promise = clipboard.write_text(text);
        drop(wasm_bindgen_futures::JsFuture::from(promise));
    }
}

/// `arboard`-backed clipboard. A fresh `arboard::Clipboard` is created
/// per call: matches the original behavior and avoids holding a platform
/// handle across frames (which `arboard` documents as fragile on X11 /
/// Wayland). Available with the `arboard` feature.
#[cfg(feature = "arboard")]
pub struct SystemClipboard;

#[cfg(feature = "arboard")]
impl ClipboardProvider for SystemClipboard {
    fn get_text(&self) -> Option<String> {
        arboard::Clipboard::new().ok()?.get_text().ok()
    }

    fn set_text(&self, text: &str) {
        if let Ok(mut clipboard) = arboard::Clipboard::new() {
            let _ = clipboard.set_text(text.to_owned());
        }
    }
}

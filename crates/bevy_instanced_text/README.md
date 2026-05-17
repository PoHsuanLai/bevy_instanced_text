# bevy_instanced_text

[![crates.io](https://img.shields.io/crates/v/bevy_instanced_text.svg)](https://crates.io/crates/bevy_instanced_text)
[![docs.rs](https://docs.rs/bevy_instanced_text/badge.svg)](https://docs.rs/bevy_instanced_text)
[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](https://github.com/PoHsuanLai/bevy_instanced_text)
[![Bevy](https://img.shields.io/badge/Bevy-0.18-blue)](https://bevyengine.org)

GPU-instanced text rendering for Bevy. Spawn a `TextBuffer<T>` on a Bevy UI `Node` and the plugin renders it via one instanced draw call per view. No input model, no UI framework coupling.

## Quick start

```rust
use bevy::prelude::*;
use bevy_instanced_text::prelude::*;
use bevy_instanced_text::TextSpan; // disambiguate from `bevy::text::TextSpan`

fn main() {
    App::new()
        .add_plugins(DefaultPlugins)
        .add_plugins(InstancedTextPlugins)
        .add_systems(Startup, |mut commands: Commands| {
            commands.spawn(Camera2d);
            commands.spawn((
                TextBuffer::<TextSpan>::new("hello world"),
                Node {
                    width: Val::Vw(100.0),
                    height: Val::Vh(100.0),
                    ..default()
                },
                TextFont::default(),
            ));
        })
        .run();
}
```

## Key types

| Type | Role |
|---|---|
| `TextBuffer<T>` | The content. Generic over any `T: TextContent` — ship-built impls include `TextSpan` (a `String` wrapper) and `String` itself; downstream crates plug in rope-backed types. Auto-cascades every renderer component a view needs. |
| `TextFont` | Per-entity font handle, size, hinting. Re-exported from `bevy::text`. |
| `MonoFontFaces` | Optional bold/italic/bold-italic faces and font-synthesis policy for one-font-per-style layouts. |
| `LineStyles` | Per-line styled runs (colors, bold, italic, inline backgrounds). Producers write this; the engine reads it. |
| `HiddenLines` | Which buffer lines to skip (e.g. folded regions). |
| `TextBounds` | Optional soft-wrap budget in pixels. |
| `TextUnderlays` / `TextOverlays` | Decoration rectangles (cursors, selections, highlights) written by the host each frame. Mutate via `Changed<T>`. |
| `DisplayLayout` | The renderer's immutable per-frame snapshot — shaped lines, glyph positions, computed line height. Read for hit-testing and overlay placement. |
| `ContentMetrics` | Widest shaped line's pixel width, useful for sizing external scroll UI. |

## Scroll

Scroll state is `bevy::ui::ScrollPosition` — write to it to move the viewport; the engine reads it. Smooth scroll, scrollbars, and stick-to-bottom behavior belong in the host.

## Layout production

The engine's `produce_layouts` system walks every `TextBuffer<T>` each frame, reads optional `HiddenLines` / `LineStyles` / `TextBounds`, shapes the visible window via cosmic-text, soft-wraps if needed, and writes `DisplayLayout`. Hosts producing styled content write `LineStyles` before `LayoutProduceSet`; the engine reads it within the set.

## Hit-testing and overlay placement

Use `DisplayLayout::buffer_to_display`, `x_at_byte`, and `RowMetricsParam` / `BufferAnchorParam` system params to resolve `(line, character)` or `char_index` to node-local pixel coordinates — handle soft wrap and folds correctly.

## Plugins

`InstancedTextPlugins` bundles `GlyphAtlasPlugin` + `InstancedTextRenderPlugin` + `InstancedTextPlugin`. Add constituents individually for fine-grained control. Add one `TextContentPlugin::<T>` per `T: TextContent + Component` you spawn — `InstancedTextPlugin` already registers it for `TextSpan`.

## Bevy compatibility

| `bevy_instanced_text` | Bevy |
|---|---|
| 0.1 | 0.18 |

## License

MIT OR Apache-2.0

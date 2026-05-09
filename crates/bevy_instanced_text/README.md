# bevy_instanced_text

GPU-instanced text rendering for Bevy. Spawn a `TextView`, write a `DisplayLayout`, and the plugin draws it. No input model, no UI framework coupling.

## Quick start

```rust
use bevy::prelude::*;
use bevy_instanced_text::prelude::*;

fn main() {
    App::new()
        .add_plugins((DefaultPlugins, InstancedTextPlugins))
        .add_systems(Startup, setup)
        .run();
}

fn setup(mut commands: Commands) {
    commands.spawn(Camera2d);
    commands.spawn((TextView, FontConfig::from_size(16.0)));
}
```

## Key types

| Type | Role |
|---|---|
| `TextView` | Marker component. `#[require]` cascades `TextBuffer`, `ScrollState`, `DisplayLayout`, `FontConfig`, `TextViewViewport`. |
| `DisplayLayout` | The renderer's input — rows of styled glyphs. Write this from your own producer system or use the helpers below. |
| `FontConfig` | Per-entity font size, line height, char width. Accepts a `Handle<bevy_text::Font>`. |

## Producing layouts

**Static text** — use `trivial_layout` (one row per line) or `BlockList` (mixed heights, soft-wrap, backgrounds, borders).

**Dynamic content** (editors, log viewers) — call `visible_buffer_range(...)` each frame to get the visible window, write styled runs into `LineStyles`, and let the engine's `produce_layouts` system build `DisplayLayout`.

## Querying layout for inline content

Position sprites or UI nodes relative to text using `DisplayLayout::pos_at_byte` and `buffer_to_display` — the engine exposes coordinates; you handle rendering.

## Plugins

`InstancedTextPlugins` bundles `GlyphAtlasPlugin` + `InstancedTextRenderPlugin` + `InstancedTextPlugin`. Add constituents individually if you need fine-grained control.

## License

MIT OR Apache-2.0

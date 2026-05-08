# bevy_text_engine

GPU-accelerated text rendering for Bevy. Provides primitives — glyph atlas, instanced rendering, soft-wrap layout producer, overlays — for building editors, terminals, chat panels, log viewers, and any other text-heavy UI.

This is the rendering layer. It owns no input model, no UI framework choice, no buffer-edit semantics. Just "given styled text + a viewport, draw it on the GPU."

## What's in the box

- **`TextView`** — marker component for a renderable text view. `#[require]` cascades `TextBuffer` (rope + version), `ScrollState` (scroll offsets), `ContentMetrics` (max-width cache), `TextViewViewport` (rect), `DisplayLayout` (rows of glyphs), `FontConfig`, `TextViewOverlays`, and `Pickable` (for `bevy_picking` integration).
- **`FontConfig`** — per-entity font sizing + optional `Handle<bevy_text::Font>`. Carries `size`, `line_height`, `char_width`. Same handle works in `bevy_text::Text2d` and `TextView`.
- **`DisplayLayout`** — the renderer's input. A list of `ShapedLine`s (text + style runs + per-row line height + padding + indent) plus global metrics. Producers write it; the renderer reads it.
- **Layout producer** — `produce_layouts` system queries entities with `HiddenLines` / `LineStyles` / `LayoutWrap` Components and writes `DisplayLayout` automatically. Handles soft-wrap with whitespace-aware breaks, fold-aware visibility (via `HiddenLines`), per-row styling (via `LineStyles`). Producers populate these Components via the `visible_buffer_range` helper so the engine and producers agree on which lines are about to render.
- **Static-content path**: attach a `BlockList(Arc<Vec<Block>>)` Component and the engine's `produce_block_layout` system writes the entity's `DisplayLayout` from the block list. `Block` carries text + runs + per-row line-height + padding + indent + soft-wrap budget + block-level decoration. Headless / one-shot use: `Block::layout(&blocks, BlockLayoutConfig { … })`.
- **`trivial_layout`** — one-row-per-line helper for static text without block structure (test fixtures, simple panels).
- **GPU pipeline** — `GlyphAtlasPlugin` (manages the cosmic-text font system + a 2048×2048 R8 atlas with shelf packing) and `InstancedTextRenderPlugin` (one instanced draw per text view, `GlyphInstance` per glyph).
- **Overlays** — `RectOverlay` rows (cursor caret, selection rectangles, line highlights, find-matches) layered into the same draw call via z-order.

## What's NOT in the box

- No selection model, multi-cursor, undo/redo. (See [`bevy_text_editor`](../bevy_text_editor) for the editable-text widget layer; the editor crate has the IDE-specific extras.)
- No syntax highlighting. The engine takes pre-computed `StyleRun`s. (See [`bevy_tree_sitter`](../bevy_tree_sitter) for tree-sitter integration.)
- No `bevy_ui::Node` integration. `TextView` renders to a world-space transform inside a `TextViewViewport` rect; embedding inside a flexbox tree requires writing the rect from `ComputedNode` yourself.

## Quick start

```rust
use bevy::prelude::*;
use bevy_text_engine::prelude::*;

fn main() {
    App::new()
        .add_plugins((DefaultPlugins, TextEnginePlugins))
        .add_systems(Startup, setup)
        .run();
}

fn setup(mut commands: Commands) {
    commands.spawn(Camera2d);
    commands.spawn((
        TextView,
        FontConfig::from_size(16.0),
        // Provide a DisplayLayout some other way — see below.
    ));
}
```

For static content (no editor, no markdown, just a paragraph of text), populate the `DisplayLayout` with `trivial_layout`:

```rust
use bevy_text_engine::view::snapshot::trivial_layout;

let layout = trivial_layout(
    &[
        ("Hello, world!".to_string(), vec![]),
        ("This is a second line.".to_string(), vec![]),
    ],
    20.0,    // line_height
    8.0,     // char_width
    5.0,     // baseline_offset
    Color::WHITE,
);
commands.entity(my_view).insert(layout);
```

For markdown-style layout with mixed line heights, padding, soft-wrap, and
block-level decoration (background fills + borders for code blocks /
blockquotes / chat-message bubbles):

```rust
use bevy_text_engine::prelude::*;

fn setup(mut commands: Commands) {
    let blocks = vec![
        Block::new("# Heading")
            .with_line_height(28.0)
            .with_padding(12.0, 6.0)
            .with_wrap_chars(0),                          // headings don't wrap
        Block::new("Lorem ipsum dolor sit amet, consectetur adipiscing elit."),
        Block::new("fn main() { println!(\"hi\"); }")
            .with_padding(8.0, 8.0)
            .with_block_background(Color::srgb(0.12, 0.12, 0.14))
            .with_block_corner_radius(4.0),
        Block::new("> a quoted line")
            .with_padding(4.0, 4.0)
            .with_block_border(Color::srgb(0.5, 0.5, 0.5), 1.0),
    ];
    commands.spawn((
        TextView,
        FontConfig::from_size(16.0),
        BlockList::new(blocks),
        LayoutWrap { budget_px: Some(480.0), indent_px: 0.0 },
    ));
}
```

`BlockList(Arc<Vec<Block>>)` is the static-content data Component. The
engine's `produce_block_layout` system reads the blocks each frame (gated
by Arc-identity change-detection — swap in a fresh `BlockList::new(...)`
to update) and writes the entity's `DisplayLayout`.
`with_block_background` paints a filled quad spanning the block's full
vertical extent (padding_top + all wrap rows + padding_bottom), distinct
from per-row `line_bg`. `with_block_border(color, width)` adds a uniform
border drawn from four edge quads. Blocks with no decoration cost zero.

Headless / test path (no ECS world):

```rust
let layout = Block::layout(&blocks, BlockLayoutConfig {
    line_height: 16.0,
    char_width: 8.0,
    baseline_offset: 5.0,
    default_fg: Color::WHITE,
    default_wrap_chars: Some(60),
});
```

For dynamic content (an editor, a streaming log viewer), write your own producer system that calls `visible_buffer_range(...)` for each `TextView` entity, computes styled runs for the visible window, and stores them in `LineStyles::new(by_line, covered)`. The engine reads `Option<&LineStyles>` and `Option<&HiddenLines>` on each layout pass — no traits, no locks. See the `bevy_code_editor` crate for a worked tree-sitter producer.

## Anchoring inline content (images, buttons, gauges)

The engine renders text only — inline images / buttons / mini-charts /
embedded inputs are the host's responsibility. Hosts spawn their own
`bevy_sprite::Sprite` or `bevy_ui::Node` entities and position them by
querying `DisplayLayout`:

```rust
fn position_my_image(
    layouts: Query<&DisplayLayout>,
    mut sprites: Query<(&MyInlineImage, &mut Transform)>,
) {
    let layout = layouts.single().unwrap();
    for (img, mut tf) in &mut sprites {
        // `img.line` is a buffer line; `img.byte` a byte offset into it.
        let Some((display_row, byte_in_row)) = layout.buffer_to_display(img.line, img.byte)
        else { continue };
        let Some(local) = layout.pos_at_byte(display_row, byte_in_row) else { continue };
        // Add the host's viewport origin to translate to world space.
        tf.translation.x = local.x;
        tf.translation.y = -local.y;
    }
}
```

The engine offers no inline-image data type or render path on purpose:
markdown wants click-to-zoom, chat wants async thumbnails, log viewers
want graph mini-charts — one engine API can't serve all three. Instead
the engine exposes `pos_at_byte` and `buffer_to_display`; hosts build
exactly the inline-content widget they need on top of `bevy_sprite` or
`bevy_ui`, both of which already do positioning, hot-reload, multi-camera,
and render-layer routing well.

## Plugin composition

`TextEnginePlugins` is a `PluginGroup` bundling:

- `GlyphAtlasPlugin` — atlas resource bootstrap.
- `InstancedTextRenderPlugin` — instanced draw pipeline.
- `TextEnginePlugin` — view systems (`produce_layouts`, `update_text_views`, `prewarm_atlas_for_layout`, `animate_text_view_scroll`).

Mirror of `bevy::DefaultPlugins`. Hosts that want fine-grained control can add only the constituents they need.

## System sets

- **`TextViewRenderSet`** — the rendering system runs in this set. Downstream systems can `.before/.after(TextViewRenderSet)`.

## Cargo features

The engine has no own feature flags; all behavior is always-on. Bevy features pulled in: `bevy_render`, `bevy_core_pipeline`, `bevy_asset`, `bevy_sprite`, `bevy_color`, `bevy_mesh`, `bevy_camera`, `bevy_log`, `bevy_picking`, `bevy_text` (for the `Font` asset).

## License

MIT OR Apache-2.0

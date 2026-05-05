# bevy_text_interaction

Pointer + focused-keyboard interaction (scroll, drag-select, copy) for [`bevy_text_engine`](../bevy_text_engine) `TextView` entities.

This is the input-side peer to the rendering crate. Pair `TextInteractionPlugin` with `TextEnginePlugins` and you get a fully interactive text view: click to focus, drag to select, scroll wheel scrolls, Cmd/Ctrl+C copies.

## Architecture

The plugin is **observer-driven**, not polling-system-driven:

- A custom `bevy_picking` backend hit-tests the `TextViewViewport` rect of every `TextView` and produces `PointerHits`. Picking order is `1.0` (above default backends), so a text view inside a `bevy_ui` panel gets the click before the panel itself.
- Observers consume `Pointer<Press|Drag|Release|Scroll>` events that picking has already routed to the right entity. No manual cursor-position math.
- Cmd/Ctrl+C copy is handled via a `FocusedInput<KeyboardInput>` observer driven by `bevy_input_focus::InputDispatchPlugin`.

The plugin idempotently adds `bevy_picking::DefaultPickingPlugins` and `bevy_input_focus::InputDispatchPlugin` if the host hasn't already.

## What's in the box

- **`TextInteractionPlugin`** — registers the picking backend, observers, and focused-keyboard dispatch.
- **`TextViewSelectionState`** — Component holding `selection_start` / `selection_end` char offsets. Optional; entities without it can't be selected.
- **`TextViewDragState`** — Component tracking an in-progress drag.
- **`ScrollConfig`** — Component with `speed: f32` (per-line scroll multiplier) and `smooth: bool` (animate vs. snap).
- **`screen_to_char_pos`** — public helper for hosts that build their own click handlers (e.g. an editor that wants fold-aware click resolution).
- **`copy_selection`** — public helper that reads a `TextViewSelectionState` + `TextViewState` and pushes the slice to the system clipboard via `arboard`.

## Quick start

```rust
use bevy::prelude::*;
use bevy_text_engine::prelude::*;
use bevy_text_interaction::TextInteractionPlugin;

fn main() {
    App::new()
        .add_plugins((DefaultPlugins, TextEnginePlugins, TextInteractionPlugin))
        .run();
}
```

To make a text view selectable, attach the state Components:

```rust
use bevy_text_interaction::{ScrollConfig, TextViewDragState, TextViewSelectionState};

commands.spawn((
    TextView,
    FontConfig::from_size(16.0),
    TextViewSelectionState::default(),
    TextViewDragState::default(),
    ScrollConfig::default(),
));
```

The editor's `CodeEditor` `#[require]` cascade attaches all three automatically. Plain `TextView` entities (chat panels, log viewers) omit them and stay non-interactive.

## Composition with bevy_picking

If your app already uses `bevy_picking` for other entities, the text view backend coexists — it only emits hits for entities with `TextView`. To opt a particular text view out (e.g. a non-interactive watermark), add `Pickable::IGNORE`.

## Composition with bevy_input_focus

Click on a text view sets `InputFocus` to that entity. Focused keyboard events route to that entity's observers via `dispatch_focused_input::<KeyboardInput>`. Multi-editor setups: whichever editor was clicked last is focused; typing goes there.

## License

MIT OR Apache-2.0

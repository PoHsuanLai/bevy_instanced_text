# bevy_instanced_text

GPU-instanced text rendering for the [Bevy](https://bevyengine.org) game engine, plus the interaction layer (selection, clipboard, caret, picking) that sits on top.

It is designed to mirror Bevy UI's `Text` API as closely as possible, so moving a label, log panel, or text-heavy widget over is mostly a one-line swap. You keep spawning a `Node`, you keep using Bevy's own `TextFont` / `TextColor` / `TextLayout` components — only the content component changes. The engine then shapes and draws the whole view in **one instanced GPU draw call** instead of one quad per glyph, which is what makes it scale to editor- and terminal-sized content.

| Crate | What it is |
|---|---|
| **[`bevy_instanced_text`](crates/bevy_instanced_text)** | GPU instanced glyph rendering, layout, overlays. Content-agnostic — use it for editors, terminals, chat panels, log viewers, HUDs, labels. |
| **[`bevy_instanced_text_interaction`](crates/bevy_instanced_text_interaction)** | Shared UI primitives for instanced-text views: clipboard, selection model, blinking caret, pointer + keyboard observers. No rope dependency. |

Downstream crates — rope-backed editor primitives, the full code editor, the terminal widget — live in [bevscode](https://github.com/PoHsuanLai/bevscode).

## Migrating from Bevy UI `Text`

The component that holds the string changes from `Text` to `InstancedText` (a generic `InstancedText<T>`; use `InstancedText<TextSpan>` for plain strings). **Everything else stays the same** — you spawn the same `Node`, and you style it with the exact same Bevy components (`TextFont`, `TextColor`, `TextLayout`), because this crate reads Bevy's own components rather than reinventing them.

### A basic label

```diff
  commands.spawn((
-     Text::new("hello world"),
+     InstancedText::from("hello world"),   // InstancedText<TextSpan>
      TextFont::from_font_size(16.0),
      TextColor(Color::WHITE),
      Node { width: Val::Px(400.0), height: Val::Px(40.0), ..default() },
  ));
```

`InstancedText<TextSpan>` carries `From<&str>`/`From<String>`, so for the
plain-string case the content swap is a true one-liner — no turbofish.
`TextFont`, `TextColor`, and `Node` are unchanged because the engine reads
Bevy's own components.

The only addition required at the app level is the plugin and a `Camera2d`:

```diff
  App::new()
      .add_plugins(DefaultPlugins)
+     .add_plugins(InstancedTextPlugins)
      .add_systems(Startup, |mut commands: Commands| {
+         commands.spawn(Camera2d);
          commands.spawn((
-             Text::new("hello world"),
+             InstancedText::from("hello world"),
              TextFont::from_font_size(16.0),
              TextColor(Color::WHITE),
              Node { width: Val::Px(400.0), height: Val::Px(40.0), ..default() },
          ));
      })
      .run();
```

### Side-by-side

**Bevy UI**

```rust
use bevy::prelude::*;

fn setup(mut commands: Commands) {
    commands.spawn((
        Text::new("hello world\nsecond line"),
        TextFont::from_font_size(16.0),
        TextColor(Color::srgb(0.9, 0.9, 0.9)),
        TextLayout::new_with_justify(Justify::Center),
        Node { width: Val::Px(400.0), height: Val::Px(80.0), ..default() },
    ));
}
```

**bevy_instanced_text**

```rust
use bevy::prelude::*;
use bevy_instanced_text::prelude::*;

fn setup(mut commands: Commands) {
    commands.spawn((
        InstancedText::from("hello world\nsecond line"), // ← swap Text → InstancedText
        TextFont::from_font_size(16.0),                 // ← same component
        TextColor(Color::srgb(0.9, 0.9, 0.9)),          // ← same component
        TextLayout::new_with_justify(Justify::Center),  // ← same component
        Node { width: Val::Px(400.0), height: Val::Px(80.0), ..default() },
    ));
}
```

## What maps directly, and what's different

| Concern | Bevy UI `Text` | `bevy_instanced_text` | Same? |
|---|---|---|---|
| The string | `Text(String)` | `InstancedText<TextSpan>` (any `T: TextContent`) | swap component |
| Font, size, line height | `TextFont` + `LineHeight` | `TextFont` + `LineHeight` | ✅ identical |
| Foreground color | `TextColor` | `TextColor` | ✅ identical |
| Background color | `TextBackgroundColor` | `TextBackgroundColor` | ✅ identical |
| Justify | `TextLayout.justify` | `TextLayout.justify` | ✅ identical |
| Line break (`NoWrap`/`WordBoundary`/`AnyCharacter`) | `TextLayout.linebreak` | `TextLayout.linebreak` | ✅ honored |
| Auto-inserted style components | `#[require(...)]` | required components | ✅ spawn the buffer alone |
| Sizing / padding / `width: auto` | `Node` + intrinsic measure | `Node` + intrinsic measure | ✅ equivalent |
| Hit-testing & picking | Bevy UI | Bevy UI | ✅ identical |
| Per-run styling | child `TextSpan` entities | `LineStyles` + `TextFormat` runs | different model |
| Bold / italic faces | font synthesis | `MonoFontFaces` (explicit faces) | different model |
| Draw calls | one quad per glyph | **one instanced call per view** | the whole point |

Wrap width itself comes from the `TextBounds` component (a pixel budget);
`TextLayout.linebreak` then decides how to use it — `NoWrap` disables wrapping
regardless of `TextBounds`, `WordBoundary` breaks at whitespace, `AnyCharacter`
breaks at the exact overflowing glyph.

### The two real differences

**1. Rich / multi-style text uses a run model instead of child entities.**

Bevy UI builds styled text from a tree of child `TextSpan` entities. This crate keeps the whole view in one component and attaches per-line styling as a `LineStyles` map — keyed by line number — of `FormattedSpan` runs, each carrying its text and a byte-range `TextFormat`:

```rust
use std::collections::HashMap;

// Bevy UI: child spans
commands.spawn(Text::new("normal ")).with_children(|p| {
    p.spawn((TextSpan::new("bold blue"), TextColor(Color::srgb(0.4, 0.6, 1.0))));
});

// bevy_instanced_text: per-line runs on one entity
let blue = Color::srgb(0.4, 0.6, 1.0);
let mut by_line = HashMap::new();
by_line.insert(0, vec![
    FormattedSpan { text: "normal ".into(), format: TextFormat::fg(0..0, Color::WHITE), is_virtual: false },
    FormattedSpan { text: "bold blue".into(), format: TextFormat::fg(0..0, blue), is_virtual: false },
]);
commands.spawn((
    InstancedText::from("normal bold blue"),
    LineStyles::new(by_line),   // line 0's runs; engine rebases byte_range
    TextFont::from_font_size(16.0),
));
```

The `format.byte_range` on input is ignored — the engine concatenates each line's run texts and rebases the ranges itself, so you set it to `0..0`. This flat run model is what lets the engine batch an entire syntax-highlighted document or a scrolling terminal into one draw — there are no per-span entities to walk.

**2. You add the plugin and a `Camera2d`.**

Glyphs render as GPU instances through a `Camera2d`. A single `Camera2d` covers a full-window view; split-pane setups give each camera a `Camera::viewport` and tag views with `RenderLayers` (see the [crate docs](crates/bevy_instanced_text/src/lib.rs) for the multi-camera recipe).

## When to reach for this

Use it when text is the workload, not the decoration: code editors, terminals, log viewers, chat transcripts, large data tables, anything that scrolls thousands of glyphs. For a handful of HUD labels, Bevy UI's built-in `Text` is simpler and perfectly fast — this crate earns its keep once glyph counts climb.

## Quick start

```rust
use bevy::prelude::*;
use bevy_instanced_text::prelude::*;

App::new()
    .add_plugins(DefaultPlugins)
    .add_plugins(InstancedTextPlugins)
    .add_systems(Startup, |mut commands: Commands| {
        commands.spawn(Camera2d);
        commands.spawn((
            InstancedText::from("hello world"),
            Node { width: Val::Vw(100.0), height: Val::Vh(100.0), ..default() },
        ));
    })
    .run();
```

`TextFont`, `TextColor`, and `TextLayout` are auto-inserted with defaults when
you spawn the component, so a bare `InstancedText` renders in the default font —
just like spawning a bare `Text`.

Run the standalone demo: `cargo run --example text_view`.

## Status

`0.1.x` — API is unstable, expect churn.

## Bevy compatibility

| `bevy_instanced_text` | Bevy |
|---|---|
| 0.1 | 0.18 |

## License

Dual-licensed under MIT or Apache-2.0 at your option.

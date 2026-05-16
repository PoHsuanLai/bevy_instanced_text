# bevy_instanced_text

GPU-accelerated text rendering primitives for the [Bevy](https://bevy.org) game engine, plus the interaction layer (selection, clipboard, caret, picking) that sits on top.

| Crate | What it is |
|---|---|
| [`bevy_instanced_text`](crates/bevy_instanced_text) | GPU instanced glyph rendering, layout, overlays. Content-agnostic — use it for editors, terminals, chat panels, log viewers, HUDs, labels. |
| [`bevy_instanced_text_interaction`](crates/bevy_instanced_text_interaction) | Shared UI primitives for instanced-text views: clipboard, selection model, blinking caret, pointer + keyboard observers. No rope dependency. |

Downstream crates (editor, terminal, code editor) live in [bevy_code_editor](https://github.com/PoHsuanLai/bevscode).

## Status

`0.1.x` — API is unstable, expect churn.

## License

Dual-licensed under MIT or Apache-2.0 at your option.

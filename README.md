# bevy_instanced_text

GPU-instanced text rendering for the [Bevy](https://bevyengine.org) game engine, plus the interaction layer (selection, clipboard, caret, picking) that sits on top.

| Crate | What it is |
|---|---|
| **[`bevy_instanced_text`](crates/bevy_instanced_text)** | GPU instanced glyph rendering, layout, overlays. Content-agnostic — use it for editors, terminals, chat panels, log viewers, HUDs, labels. |
| **[`bevy_instanced_text_interaction`](crates/bevy_instanced_text_interaction)** | Shared UI primitives for instanced-text views: clipboard, selection model, blinking caret, pointer + keyboard observers. No rope dependency. |

Downstream crates — rope-backed editor primitives, the full code editor, the terminal widget — live in [bevscode](https://github.com/PoHsuanLai/bevscode).

## Status

`0.1.x` — API is unstable, expect churn.

## Bevy compatibility

| `bevy_instanced_text` | Bevy |
|---|---|
| 0.1 | 0.18 |

## License

Dual-licensed under MIT or Apache-2.0 at your option.

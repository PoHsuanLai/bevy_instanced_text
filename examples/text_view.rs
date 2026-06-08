//! Text View Demo — standalone `InstancedTextPlugins` without any editor.
//!
//! Demonstrates that the engine's `InstancedTextPlugins` (GPU + view systems)
//! can render styled text independently, without `CodeEditorPlugin`, cursor,
//! selection, syntax highlighting, or keybindings.
//!
//! `InstancedTextInteractionPlugin::<String>` is added for mouse-wheel
//! scrolling — it routes `Pointer<Scroll>` events to the hovered text view
//! automatically, so no custom system is needed.

use bevy::prelude::*;
use bevy::text::TextFont;
use bevy_instanced_text::prelude::*;
use bevy_instanced_text_interaction::InstancedTextInteractionPlugin;

fn main() {
    let mut app = App::new();
    app.add_plugins(
        DefaultPlugins
            .set(WindowPlugin {
                primary_window: Some(Window {
                    title: "InstancedTextPlugins Demo — No Editor".to_string(),
                    resolution: (800, 600).into(),
                    ..default()
                }),
                ..default()
            })
            .set(bevy::asset::AssetPlugin {
                file_path: "assets".into(),
                ..default()
            }),
    );

    app.add_plugins(InstancedTextPlugins)
        .add_plugins(InstancedTextInteractionPlugin::<String>::default())
        .add_systems(Startup, (setup_camera, setup_text_view))
        .run();
}

fn setup_camera(mut commands: Commands) {
    commands.spawn(Camera2d);
}

fn setup_text_view(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    windows: Query<&Window, With<bevy::window::PrimaryWindow>>,
) {
    let Ok(window) = windows.single() else {
        return;
    };

    // Bevy introduction content from https://bevy.org/learn/quick-start/introduction/
    let h1 = Color::srgb(1.0, 1.0, 1.0);
    let h2 = Color::srgb(0.9, 0.75, 0.4);
    let body = Color::srgb(0.82, 0.82, 0.82);
    let bullet_key = Color::srgb(0.4, 0.8, 1.0);
    let dim = Color::srgb(0.55, 0.55, 0.55);
    let warn = Color::srgb(1.0, 0.75, 0.3);

    let lines = vec![
        colored_line("Introduction", h1),
        plain_line(""),
        colored_line(
            "If you came here to learn how to make 2D/3D games, visualizations,",
            body,
        ),
        colored_line(
            "user interfaces, or other graphical applications with Bevy,",
            body,
        ),
        colored_line("this is the right place.", body),
        plain_line(""),
        plain_line(""),
        colored_line("What's a BEVY?", h2),
        plain_line(""),
        colored_line("A bevy is a group of birds!", body),
        plain_line(""),
        colored_line(
            "Bevy is also described as \"a refreshingly simple data-driven",
            body,
        ),
        colored_line(
            "game engine built in Rust.\" It is free and open-source under",
            body,
        ),
        colored_line("the MIT or Apache 2.0 licenses.", body),
        plain_line(""),
        plain_line(""),
        colored_line("Design Goals", h2),
        plain_line(""),
        colored_line("Bevy aims to be:", body),
        plain_line(""),
        multi_segment_line(vec![
            ("  Capable     ", bullet_key),
            ("— Complete 2D and 3D feature set", body),
        ]),
        multi_segment_line(vec![
            ("  Simple      ", bullet_key),
            (
                "— Accessible for newcomers, flexible for advanced users",
                body,
            ),
        ]),
        multi_segment_line(vec![
            ("  Data Focused", bullet_key),
            ("— Entity Component System (ECS) architecture", body),
        ]),
        multi_segment_line(vec![
            ("  Modular     ", bullet_key),
            ("— Use only the components you need", body),
        ]),
        multi_segment_line(vec![
            ("  Fast        ", bullet_key),
            (
                "— Quick app logic with parallel processing capability",
                body,
            ),
        ]),
        multi_segment_line(vec![
            ("  Productive  ", bullet_key),
            ("— Fast compilation times", body),
        ]),
        plain_line(""),
        plain_line(""),
        colored_line("Development Philosophy", h2),
        plain_line(""),
        colored_line(
            "The engine is \"built in the open by volunteers\" using Rust.",
            body,
        ),
        colored_line(
            "The developers emphasize that games represent millions of hours",
            body,
        ),
        colored_line(
            "of human development effort, yet many developers rely on",
            body,
        ),
        colored_line(
            "closed-source commercial engines that take revenue cuts.",
            body,
        ),
        plain_line(""),
        plain_line(""),
        colored_line("Stability Warning", h2),
        plain_line(""),
        colored_line(
            "Important features remain under development and documentation",
            warn,
        ),
        colored_line(
            "may be limited. Breaking API changes occur approximately once",
            warn,
        ),
        colored_line("every 3 months.", warn),
        plain_line(""),
        colored_line(
            "Migration guides are provided, though migrations are not always",
            body,
        ),
        colored_line("straightforward.", body),
        plain_line(""),
        colored_line(
            "The page recommends Godot Engine for production projects",
            dim,
        ),
        colored_line(
            "requiring stability, noting it offers similar open-source",
            dim,
        ),
        colored_line("benefits with greater feature completeness.", dim),
    ];

    // Build rope text from the styled lines.
    let mut full_text = String::new();
    for (i, (text, _)) in lines.iter().enumerate() {
        full_text.push_str(text);
        if i < lines.len() - 1 {
            full_text.push('\n');
        }
    }

    // Build LineStyles from the per-line runs.
    let mut by_line = std::collections::HashMap::new();
    for (i, (_text, runs)) in lines.iter().enumerate() {
        let row_runs: Vec<FormattedSpan> = runs
            .iter()
            .map(|r| FormattedSpan {
                text: _text.clone(),
                format: r.clone(),
                is_virtual: false,
            })
            .collect();
        by_line.insert(i as u32, row_runs);
    }
    let line_styles = LineStyles::new(by_line);

    commands.spawn((
        InstancedText::<String>::new(full_text.clone()),
        line_styles,
        TextFont::from_font_size(16.0).with_font(asset_server.load("fonts/FiraMono-Regular.ttf")),
        MonoFontFaces::default().with_bold(asset_server.load("fonts/FiraMono-Medium.ttf")),
        // Val::Px so Bevy UI layout resolves size without needing a UI camera.
        Node {
            width: Val::Px(window.width()),
            height: Val::Px(window.height()),
            padding: UiRect::all(Val::Px(16.0)),
            ..default()
        },
    ));
}

fn colored_line(text: &str, color: Color) -> (String, Vec<TextFormat>) {
    (text.to_string(), vec![TextFormat::fg(0..text.len(), color)])
}

fn plain_line(text: &str) -> (String, Vec<TextFormat>) {
    (text.to_string(), vec![])
}

fn multi_segment_line(segments: Vec<(&str, Color)>) -> (String, Vec<TextFormat>) {
    let mut text = String::new();
    let mut runs = Vec::with_capacity(segments.len());
    let mut byte_cursor = 0;
    for (t, c) in segments {
        let len = t.len();
        text.push_str(t);
        runs.push(TextFormat::fg(byte_cursor..byte_cursor + len, c));
        byte_cursor += len;
    }
    (text, runs)
}

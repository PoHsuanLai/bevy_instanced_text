//! Property inspector — instanced text inside a real Bevy UI layout.
//!
//! A three-panel UI: a sidebar listing scene objects, a main properties panel
//! showing key/value rows for the selected object, and a log panel at the bottom.
//! All text panels use `TextBuffer<TextSpan>` inside standard Bevy UI `Node`
//! containers — no editor plugin, no cursor, no input handling from bevscode.
//!
//! Run with:
//!   cargo run --example ui_inspector
//!
//! ## What this exercises
//!
//! - Multiple `TextBuffer<TextSpan>` views in a Bevy UI flex layout
//! - `LineStyles` for per-row coloring (key column vs value column)
//! - `TextOverlays` / `TextUnderlays` for hover highlight and selection band
//! - Per-panel mouse-wheel scrolling via `InstancedTextInteractionPlugin`
//!   (Bevy picking routes `Pointer<Scroll>` to the hovered entity)
//! - Live updates: clicking an object in the sidebar rebuilds the properties panel
//! - A log panel that appends a new line each second

use bevy::prelude::*;
use bevy_instanced_text::prelude::*;
use bevy_instanced_text_interaction::InstancedTextInteractionPlugin;

fn main() {
    App::new()
        .add_plugins(
            DefaultPlugins.set(WindowPlugin {
                primary_window: Some(Window {
                    title: "Property Inspector — instanced text dogfood".into(),
                    resolution: (1024_u32, 768_u32).into(),
                    ..default()
                }),
                ..default()
            }),
        )
        .add_plugins(InstancedTextPlugins)
        // Routes Pointer<Scroll> to the hovered TextBuffer<TextSpan> entity,
        // so per-panel mouse-wheel scrolling works without any custom system.
        .add_plugins(InstancedTextInteractionPlugin::<TextSpan>::default())
        .init_resource::<InspectorState>()
        .add_systems(Startup, setup_camera)
        .add_systems(Startup, setup_ui.after(setup_camera))
        .add_systems(
            Update,
            (rebuild_properties_on_selection, tick_log_panel),
        )
        .run();
}

// ---------------------------------------------------------------------------
// Domain data
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct SceneObject {
    name: &'static str,
    props: &'static [(&'static str, &'static str)],
}

const OBJECTS: &[SceneObject] = &[
    SceneObject {
        name: "Camera",
        props: &[
            ("position",   "Vec3(0.0, 5.0, -10.0)"),
            ("rotation",   "Quat(0.0, 0.0, 0.0, 1.0)"),
            ("fov",        "60°"),
            ("near",       "0.1"),
            ("far",        "1000.0"),
            ("projection", "Perspective"),
        ],
    },
    SceneObject {
        name: "DirectionalLight",
        props: &[
            ("direction",   "Vec3(-0.5, -1.0, -0.3)"),
            ("color",       "Color::WHITE"),
            ("illuminance", "10000.0 lux"),
            ("shadows",     "true"),
            ("cascade_count", "4"),
        ],
    },
    SceneObject {
        name: "Terrain",
        props: &[
            ("mesh",       "terrain_512x512.obj"),
            ("material",   "grass_pbr"),
            ("position",   "Vec3(0.0, 0.0, 0.0)"),
            ("scale",      "Vec3(1.0, 1.0, 1.0)"),
            ("collider",   "Heightmap"),
            ("lod_levels", "3"),
        ],
    },
    SceneObject {
        name: "PlayerSpawn",
        props: &[
            ("position",  "Vec3(12.5, 0.0, -3.0)"),
            ("rotation",  "Quat(0.0, 0.785, 0.0, 0.924)"),
            ("team",      "1"),
            ("active",    "true"),
        ],
    },
    SceneObject {
        name: "AmbientOcclusion",
        props: &[
            ("technique",  "SSAO"),
            ("radius",     "0.5"),
            ("samples",    "8"),
            ("intensity",  "0.8"),
            ("bias",       "0.025"),
        ],
    },
];

// ---------------------------------------------------------------------------
// Resources
// ---------------------------------------------------------------------------

#[derive(Resource, Default)]
struct InspectorState {
    selected: usize,
    last_selected: Option<usize>,
    log_lines: Vec<String>,
    log_timer: f32,
}

// ---------------------------------------------------------------------------
// Component markers — so systems can find the right panel
// ---------------------------------------------------------------------------

#[derive(Component)]
struct SidebarPanel;

#[derive(Component)]
struct PropertiesPanel;

#[derive(Component)]
struct LogPanel;

// ---------------------------------------------------------------------------
// Setup
// ---------------------------------------------------------------------------

fn setup_camera(mut commands: Commands) {
    commands.spawn(Camera2d);
}

fn setup_ui(mut commands: Commands, asset_server: Res<AssetServer>, mut state: ResMut<InspectorState>) {
    // Seed the log with a couple of entries so the panel isn't empty.
    state.log_lines.push("[00:00:00] Inspector started".into());
    state.log_lines.push("[00:00:00] Scene loaded: 5 objects".into());

    let font = asset_server.load("fonts/FiraMono-Regular.ttf");
    let font_size = 13.0;

    // Root flex container — full window, column direction.
    commands
        .spawn(Node {
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            flex_direction: FlexDirection::Row,
            ..default()
        })
        .with_children(|root| {
            // ----------------------------------------------------------------
            // Left sidebar — object list
            // ----------------------------------------------------------------
            root.spawn(Node {
                width: Val::Px(200.0),
                height: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                border: UiRect::right(Val::Px(1.0)),
                ..default()
            })
            .insert(BorderColor::all(Color::srgb(0.25, 0.25, 0.25)))
            .with_children(|sidebar| {
                // Header label — plain Bevy UI text (not instanced) is fine for
                // static chrome. Using instanced text here would work but is overkill.
                sidebar.spawn((
                    Text::new("SCENE OBJECTS"),
                    TextFont::from_font_size(11.0),
                    TextColor(Color::srgb(0.5, 0.5, 0.5)),
                    Node {
                        padding: UiRect::all(Val::Px(8.0)),
                        ..default()
                    },
                ));

                // One instanced text buffer for the full sidebar list.
                // Each object name is a row; the selected row gets an underlay highlight.
                let sidebar_text = build_sidebar_text(0);
                let sidebar_styles = build_sidebar_styles(&sidebar_text, 0);
                let sidebar_underlays = build_sidebar_underlays(0);

                sidebar.spawn((
                    TextBuffer::<TextSpan>::new(sidebar_text),
                    sidebar_styles,
                    sidebar_underlays,
                    TextFont::from_font_size(font_size).with_font(font.clone()),
                    TextColor(Color::srgb(0.82, 0.82, 0.82)),
                    TextBackgroundColor(Color::srgb(0.13, 0.13, 0.13)),
                    Node {
                        width: Val::Percent(100.0),
                        flex_grow: 1.0,
                        padding: UiRect::all(Val::Px(6.0)),
                        overflow: Overflow::clip(),
                        ..default()
                    },
                    SidebarPanel,
                ))
                .observe(on_sidebar_click);
            });

            // ----------------------------------------------------------------
            // Right column — properties panel on top, log on bottom
            // ----------------------------------------------------------------
            root.spawn(Node {
                flex_direction: FlexDirection::Column,
                flex_grow: 1.0,
                height: Val::Percent(100.0),
                ..default()
            })
            .with_children(|right| {
                // Properties header
                right.spawn((
                    Text::new("PROPERTIES"),
                    TextFont::from_font_size(11.0),
                    TextColor(Color::srgb(0.5, 0.5, 0.5)),
                    Node {
                        padding: UiRect::all(Val::Px(8.0)),
                        border: UiRect::bottom(Val::Px(1.0)),
                        ..default()
                    },
                    BorderColor::all(Color::srgb(0.25, 0.25, 0.25)),
                ));

                // Properties text view
                let props_text = build_props_text(&OBJECTS[0]);
                let props_styles = build_props_styles(&OBJECTS[0]);

                right.spawn((
                    TextBuffer::<TextSpan>::new(props_text),
                    props_styles,
                    TextFont::from_font_size(font_size).with_font(font.clone()),
                    TextColor(Color::srgb(0.82, 0.82, 0.82)),
                    TextBackgroundColor(Color::srgb(0.11, 0.11, 0.11)),
                    Node {
                        flex_grow: 1.0,
                        width: Val::Percent(100.0),
                        padding: UiRect::all(Val::Px(10.0)),
                        overflow: Overflow::clip(),
                        ..default()
                    },
                    PropertiesPanel,
                ));

                // Log header
                right.spawn((
                    Text::new("LOG"),
                    TextFont::from_font_size(11.0),
                    TextColor(Color::srgb(0.5, 0.5, 0.5)),
                    Node {
                        padding: UiRect::all(Val::Px(8.0)),
                        border: UiRect::vertical(Val::Px(1.0)),
                        ..default()
                    },
                    BorderColor::all(Color::srgb(0.25, 0.25, 0.25)),
                ));

                // Log text view
                right.spawn((
                    TextBuffer::<TextSpan>::new("[00:00:00] Inspector started\n[00:00:00] Scene loaded: 5 objects"),
                    LineStyles::default(),
                    TextFont::from_font_size(font_size).with_font(font.clone()),
                    TextColor(Color::srgb(0.6, 0.7, 0.6)),
                    TextBackgroundColor(Color::srgb(0.09, 0.11, 0.09)),
                    Node {
                        height: Val::Px(120.0),
                        width: Val::Percent(100.0),
                        padding: UiRect::all(Val::Px(8.0)),
                        overflow: Overflow::clip(),
                        ..default()
                    },
                    LogPanel,
                ));
            });
        });
}

// ---------------------------------------------------------------------------
// Text builders
// ---------------------------------------------------------------------------

fn build_sidebar_text(selected: usize) -> String {
    OBJECTS
        .iter()
        .enumerate()
        .map(|(i, o)| {
            if i == selected {
                format!("▶ {}", o.name)
            } else {
                format!("  {}", o.name)
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn build_sidebar_styles(text: &str, selected: usize) -> LineStyles {
    let accent = Color::srgb(0.4, 0.75, 1.0);
    let normal = Color::srgb(0.75, 0.75, 0.75);
    LineStyles::new(
        text.lines()
            .enumerate()
            .map(|(i, line)| {
                let color = if i == selected { accent } else { normal };
                let len = line.len();
                (
                    i as u32,
                    vec![FormattedSpan {
                        text: line.to_string(),
                        format: TextFormat::fg(0..len, color),
                    }],
                )
            })
            .collect(),
    )
}

fn build_sidebar_underlays(selected: usize) -> TextUnderlays {
    TextUnderlays(vec![RectOverlay {
        display_row: selected as u32,
        x_range: 0.0..f32::MAX,
        color: Color::srgba(0.3, 0.5, 0.8, 0.18),
        z: -1,
        corners: CornerRadii::uniform(3.0),
        vertical: RowVertical::FullLeaded,
    }])
}

fn build_props_text(obj: &SceneObject) -> String {
    let mut s = format!("{}  —  {} properties\n\n", obj.name, obj.props.len());
    for (k, v) in obj.props {
        // Monospace column alignment via fixed-width padding.
        s.push_str(&format!("  {:<20} {}\n", k, v));
    }
    s
}

fn build_props_styles(obj: &SceneObject) -> LineStyles {
    let heading = Color::srgb(1.0, 1.0, 1.0);
    let key_color = Color::srgb(0.55, 0.75, 0.95);
    let val_color = Color::srgb(0.88, 0.82, 0.65);

    let heading_text = format!("{}  —  {} properties", obj.name, obj.props.len());
    let heading_len = heading_text.len();

    let mut map = std::collections::HashMap::from_iter([(
        0u32,
        vec![FormattedSpan {
            text: heading_text,
            format: TextFormat::fg(0..heading_len, heading),
        }],
    )]);

    // Row 1 stays blank. Row 2+ is two non-overlapping spans: a padded key
    // segment, then the value segment. The engine concatenates the texts
    // and rebases each format's byte_range to match.
    for (i, (k, v)) in obj.props.iter().enumerate() {
        let key_part = format!("  {:<20} ", k);
        let val_part = v.to_string();
        map.insert(
            (i + 2) as u32,
            vec![
                FormattedSpan {
                    format: TextFormat::fg(0..key_part.len(), key_color),
                    text: key_part,
                },
                FormattedSpan {
                    format: TextFormat::fg(0..val_part.len(), val_color),
                    text: val_part,
                },
            ],
        );
    }

    LineStyles::new(map)
}

// ---------------------------------------------------------------------------
// Systems
// ---------------------------------------------------------------------------

/// Idiomatic Bevy picking observer: `On<Pointer<Press>>` fires for the entity
/// that was clicked, `pick_row_from_hit` turns the normalized hit position
/// directly into a row index. No window query, no cursor math, no node
/// bounds — Bevy's picking backend already did all of that.
fn on_sidebar_click(
    trigger: On<Pointer<Press>>,
    row_metrics: RowMetricsParam,
    mut state: ResMut<InspectorState>,
) {
    if trigger.event().button != PointerButton::Primary {
        return;
    }
    let entity = trigger.event().entity;
    let Some(metrics) = row_metrics.get(entity) else { return };
    if let Some(row) = metrics.pick_row_from_hit(&trigger.event().hit) {
        if (row as usize) < OBJECTS.len() {
            state.selected = row as usize;
        }
    }
}

/// When the selection changes, rewrite both the sidebar and properties panels.
fn rebuild_properties_on_selection(
    mut state: ResMut<InspectorState>,
    mut sidebar_q: Query<
        (&mut TextBuffer<TextSpan>, &mut LineStyles, &mut TextUnderlays),
        (With<SidebarPanel>, Without<PropertiesPanel>),
    >,
    mut props_q: Query<
        (&mut TextBuffer<TextSpan>, &mut LineStyles),
        (With<PropertiesPanel>, Without<SidebarPanel>),
    >,
) {
    if state.last_selected == Some(state.selected) {
        return;
    }
    let selected = state.selected;
    state.last_selected = Some(selected);

    // Rebuild sidebar.
    if let Ok((mut buf, mut styles, mut underlays)) = sidebar_q.single_mut() {
        let text = build_sidebar_text(selected);
        let new_styles = build_sidebar_styles(&text, selected);
        buf.0 = TextSpan::new(text.clone());
        *styles = new_styles;
        *underlays = build_sidebar_underlays(selected);
    }

    // Rebuild properties.
    if let Ok((mut buf, mut styles)) = props_q.single_mut() {
        let obj = &OBJECTS[selected];
        let text = build_props_text(obj);
        let new_styles = build_props_styles(obj);
        buf.0 = TextSpan::new(text);
        *styles = new_styles;
    }
}

/// Append a timestamped log line every second.
fn tick_log_panel(
    mut state: ResMut<InspectorState>,
    time: Res<Time>,
    mut log_q: Query<
        (
            &mut TextBuffer<TextSpan>,
            &mut bevy::ui::ScrollPosition,
            &DisplayLayout,
            &ComputedNode,
        ),
        With<LogPanel>,
    >,
) {
    state.log_timer += time.delta_secs();
    if state.log_timer < 1.0 {
        return;
    }
    state.log_timer -= 1.0;

    let secs = time.elapsed_secs() as u32;
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    let msg = format!("[{:02}:{:02}:{:02}] Frame {} — selected: {}",
        h, m, s,
        time.elapsed_secs() as u64,
        OBJECTS[state.selected].name,
    );
    state.log_lines.push(msg);

    if let Ok((mut buf, mut scroll, layout, computed)) = log_q.single_mut() {
        buf.0 = TextSpan::new(state.log_lines.join("\n"));
        // Scroll to bottom — pin the last row to the viewport bottom.
        let viewport_h = computed.size().y * computed.inverse_scale_factor();
        scroll.y = layout.scroll_to_bottom_target(viewport_h);
    }
}


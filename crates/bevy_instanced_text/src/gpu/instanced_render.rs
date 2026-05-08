//! GPU instanced text rendering.
//!
//! Architecture: each `TextView` owns a "batch entity" carrying a
//! `GlyphBatchComponent` (per-glyph instance buffer + atlas handle). A
//! `SpecializedRenderPipeline` configures:
//!
//! - bind group 0 = `Mesh2dPipeline`'s view bind group (view-projection)
//! - bind group 1 = atlas texture + sampler
//! - vertex buffer 0 = per-instance glyph attributes (one entry per glyph)
//!
//! The vertex shader expands a unit quad from `vertex_index` per glyph and
//! projects directly through `view.clip_from_world`. Glyph positions are
//! emitted by `render_layout` in world-space pixels — the editor's camera
//! positions itself so the panel's top-left sits at world (0, 0).
//!
//! Note: this pipeline does NOT integrate with `Mesh2dPipeline`'s per-mesh
//! bind group (which would let entities have their own `Transform`). That
//! integration uses `@builtin(instance_index)` to index into a per-entity
//! transform array, which conflicts with our per-glyph instancing — every
//! glyph would index a different entity slot. Skipping the per-mesh
//! transform keeps the engine's instancing model intact at the cost of
//! losing per-entity positioning.

use bevy::{
    core_pipeline::core_2d::{Transparent2d, CORE_2D_DEPTH_FORMAT},
    ecs::{
        query::QueryItem,
        system::{lifetimeless::*, SystemParamItem},
    },
    math::FloatOrd,
    mesh::VertexBufferLayout,
    prelude::*,
    render::{
        extract_component::{ExtractComponent, ExtractComponentPlugin},
        render_asset::RenderAssets,
        render_phase::{
            AddRenderCommand, DrawFunctions, PhaseItem, PhaseItemExtraIndex, RenderCommand,
            RenderCommandResult, SetItemPipeline, TrackedRenderPass, ViewSortedRenderPhases,
        },
        render_resource::{
            binding_types::*, BindGroup, BindGroupEntries, BindGroupLayoutDescriptor,
            BindGroupLayoutEntries, BlendState, Buffer, BufferInitDescriptor, BufferUsages,
            ColorTargetState, ColorWrites, CompareFunction, DepthBiasState, DepthStencilState,
            FragmentState, MultisampleState, PipelineCache, PrimitiveState, PrimitiveTopology,
            RenderPipelineDescriptor, SamplerBindingType, ShaderStages, SpecializedRenderPipeline,
            SpecializedRenderPipelines, StencilFaceState, StencilState, TextureFormat,
            TextureSampleType, VertexAttribute, VertexFormat, VertexState, VertexStepMode,
        },
        renderer::RenderDevice,
        sync_world::MainEntity,
        texture::GpuImage,
        view::{ExtractedView, ViewTarget},
        Render, RenderApp, RenderSystems,
    },
    sprite_render::{Mesh2dPipeline, Mesh2dPipelineKey, SetMesh2dViewBindGroup},
};

use bevy_camera::visibility::RenderLayers;

use crate::view::render::{GlyphBatchComponent, GlyphInstance};

/// Registers the GPU instanced text render pipeline: extracts `GlyphBatchComponent`
/// to the render world and issues one instanced draw call per text view per frame.
pub struct InstancedTextRenderPlugin;

impl Plugin for InstancedTextRenderPlugin {
    fn build(&self, app: &mut App) {
        bevy::asset::embedded_asset!(app, "text.wgsl");

        app.add_plugins(ExtractComponentPlugin::<GlyphBatchComponent>::default());

        let render_app = app.sub_app_mut(RenderApp);

        render_app
            .add_render_command::<Transparent2d, DrawInstancedText>()
            .init_resource::<SpecializedRenderPipelines<InstancedTextPipeline>>()
            .add_systems(
                Render,
                (
                    init_instanced_text_pipeline
                        .run_if(not(resource_exists::<InstancedTextPipeline>)),
                    queue_instanced_text
                        .run_if(resource_exists::<InstancedTextPipeline>)
                        .in_set(RenderSystems::QueueMeshes),
                    prepare_instance_buffers
                        .run_if(resource_exists::<InstancedTextPipeline>)
                        .in_set(RenderSystems::PrepareResources),
                ),
            );
    }
}

impl ExtractComponent for GlyphBatchComponent {
    type QueryData = &'static GlyphBatchComponent;
    type QueryFilter = ();
    type Out = Self;

    fn extract_component(item: QueryItem<'_, '_, Self::QueryData>) -> Option<Self> {
        Some(GlyphBatchComponent {
            instances: item.instances.clone(),
            atlas_texture: item.atlas_texture.clone(),
            render_layer: item.render_layer,
        })
    }
}

#[derive(Component)]
pub struct InstanceBuffer {
    pub buffer: Buffer,
    pub length: usize,
}

#[derive(Component)]
pub struct TextureBindGroup {
    pub bind_group: BindGroup,
}

fn prepare_instance_buffers(
    mut commands: Commands,
    query: Query<(Entity, &GlyphBatchComponent)>,
    render_device: Res<RenderDevice>,
    pipeline: Res<InstancedTextPipeline>,
    pipeline_cache: Res<PipelineCache>,
    gpu_images: Res<RenderAssets<GpuImage>>,
) {
    let atlas_layout =
        pipeline_cache.get_bind_group_layout(&pipeline.atlas_bind_group_layout);

    for (entity, batch) in &query {
        if batch.instances.is_empty() {
            commands.entity(entity).remove::<InstanceBuffer>();
            continue;
        }

        let buffer = render_device.create_buffer_with_data(&BufferInitDescriptor {
            label: Some("glyph_instance_buffer"),
            contents: bytemuck::cast_slice(&batch.instances),
            usage: BufferUsages::VERTEX | BufferUsages::COPY_DST,
        });

        commands.entity(entity).insert(InstanceBuffer {
            buffer,
            length: batch.instances.len(),
        });

        if let Some(gpu_image) = gpu_images.get(&batch.atlas_texture) {
            let bind_group = render_device.create_bind_group(
                "text_atlas_bind_group",
                &atlas_layout,
                &BindGroupEntries::sequential((&gpu_image.texture_view, &gpu_image.sampler)),
            );

            commands
                .entity(entity)
                .insert(TextureBindGroup { bind_group });
        }
    }
}

#[derive(Resource, Clone)]
pub struct InstancedTextPipeline {
    view_layout: BindGroupLayoutDescriptor,
    shader: Handle<Shader>,
    atlas_bind_group_layout: BindGroupLayoutDescriptor,
}

fn init_instanced_text_pipeline(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mesh2d_pipeline: Option<Res<Mesh2dPipeline>>,
) {
    let Some(mesh2d_pipeline) = mesh2d_pipeline else {
        return;
    };

    let atlas_bind_group_layout = BindGroupLayoutDescriptor::new(
        "text_atlas_layout",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::FRAGMENT,
            (
                texture_2d(TextureSampleType::Float { filterable: true }),
                sampler(SamplerBindingType::Filtering),
            ),
        ),
    );

    let shader = bevy::asset::load_embedded_asset!(asset_server.as_ref(), "text.wgsl");

    commands.insert_resource(InstancedTextPipeline {
        view_layout: mesh2d_pipeline.view_layout.clone(),
        shader,
        atlas_bind_group_layout,
    });
}

impl SpecializedRenderPipeline for InstancedTextPipeline {
    type Key = Mesh2dPipelineKey;

    fn specialize(&self, key: Self::Key) -> RenderPipelineDescriptor {
        // Buffer 0: per-glyph instance attributes. Locations 0..=7 match
        // the field order of `GlyphInstance` in `view::render`.
        let instance_layout = VertexBufferLayout {
            array_stride: std::mem::size_of::<GlyphInstance>() as u64,
            step_mode: VertexStepMode::Instance,
            attributes: vec![
                // position (vec2)
                VertexAttribute {
                    format: VertexFormat::Float32x2,
                    offset: 0,
                    shader_location: 0,
                },
                // uv_min (vec2)
                VertexAttribute {
                    format: VertexFormat::Float32x2,
                    offset: 8,
                    shader_location: 1,
                },
                // uv_max (vec2)
                VertexAttribute {
                    format: VertexFormat::Float32x2,
                    offset: 16,
                    shader_location: 2,
                },
                // size (vec2)
                VertexAttribute {
                    format: VertexFormat::Float32x2,
                    offset: 24,
                    shader_location: 3,
                },
                // color (vec4)
                VertexAttribute {
                    format: VertexFormat::Float32x4,
                    offset: 32,
                    shader_location: 4,
                },
                // corner_radii (vec4)
                VertexAttribute {
                    format: VertexFormat::Float32x4,
                    offset: 48,
                    shader_location: 5,
                },
                // z_index (f32)
                VertexAttribute {
                    format: VertexFormat::Float32,
                    offset: 64,
                    shader_location: 6,
                },
                // skew (f32)
                VertexAttribute {
                    format: VertexFormat::Float32,
                    offset: 68,
                    shader_location: 7,
                },
            ],
        };

        let format = if key.contains(Mesh2dPipelineKey::HDR) {
            ViewTarget::TEXTURE_FORMAT_HDR
        } else {
            TextureFormat::bevy_default()
        };

        RenderPipelineDescriptor {
            vertex: VertexState {
                shader: self.shader.clone(),
                shader_defs: vec![],
                entry_point: Some("vertex".into()),
                buffers: vec![instance_layout],
            },
            fragment: Some(FragmentState {
                shader: self.shader.clone(),
                shader_defs: vec![],
                entry_point: Some("fragment".into()),
                targets: vec![Some(ColorTargetState {
                    format,
                    blend: Some(BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: ColorWrites::ALL,
                })],
            }),
            layout: vec![
                self.view_layout.clone(),
                self.atlas_bind_group_layout.clone(),
            ],
            primitive: PrimitiveState {
                topology: PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..default()
            },
            depth_stencil: Some(DepthStencilState {
                format: CORE_2D_DEPTH_FORMAT,
                depth_write_enabled: false,
                depth_compare: CompareFunction::GreaterEqual,
                stencil: StencilState {
                    front: StencilFaceState::IGNORE,
                    back: StencilFaceState::IGNORE,
                    read_mask: 0,
                    write_mask: 0,
                },
                bias: DepthBiasState {
                    constant: 0,
                    slope_scale: 0.0,
                    clamp: 0.0,
                },
            }),
            multisample: MultisampleState {
                count: key.msaa_samples(),
                mask: !0,
                alpha_to_coverage_enabled: false,
            },
            push_constant_ranges: vec![],
            zero_initialize_workgroup_memory: true,
            label: Some("instanced_text_pipeline".into()),
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn queue_instanced_text(
    transparent_2d_draw_functions: Res<DrawFunctions<Transparent2d>>,
    instanced_text_pipeline: Res<InstancedTextPipeline>,
    mut pipelines: ResMut<SpecializedRenderPipelines<InstancedTextPipeline>>,
    pipeline_cache: Res<PipelineCache>,
    batches: Query<(
        Entity,
        &MainEntity,
        Option<&GlobalTransform>,
        &GlyphBatchComponent,
    )>,
    mut transparent_render_phases: ResMut<ViewSortedRenderPhases<Transparent2d>>,
    views: Query<(Entity, &ExtractedView, &Msaa, Option<&RenderLayers>)>,
) {
    let draw_function = transparent_2d_draw_functions
        .read()
        .id::<DrawInstancedText>();

    for (_view_entity, view, msaa, view_layers) in &views {
        let Some(transparent_phase) = transparent_render_phases.get_mut(&view.retained_view_entity)
        else {
            continue;
        };

        let view_render_layers = view_layers.cloned().unwrap_or_default();

        let view_key = Mesh2dPipelineKey::from_msaa_samples(msaa.samples())
            | Mesh2dPipelineKey::from_hdr(view.hdr);

        let pipeline_id =
            pipelines.specialize(&pipeline_cache, &instanced_text_pipeline, view_key);

        for (entity, main_entity, global_transform, batch) in &batches {
            // Filter by render layer.
            if let Some(layer) = batch.render_layer {
                let batch_layers = RenderLayers::layer(layer as usize);
                if !view_render_layers.intersects(&batch_layers) {
                    continue;
                }
            }

            let z = global_transform.map(|t| t.translation().z).unwrap_or(0.0);

            transparent_phase.add(Transparent2d {
                entity: (entity, *main_entity),
                pipeline: pipeline_id,
                draw_function,
                sort_key: FloatOrd(z),
                batch_range: 0..1,
                extra_index: PhaseItemExtraIndex::None,
                extracted_index: usize::MAX,
                indexed: false,
            });
        }
    }
}

type DrawInstancedText = (
    SetItemPipeline,
    SetMesh2dViewBindGroup<0>,
    SetAtlasBindGroup<1>,
    DrawTextInstanced,
);

struct SetAtlasBindGroup<const I: usize>;

impl<P: PhaseItem, const I: usize> RenderCommand<P> for SetAtlasBindGroup<I> {
    type Param = ();
    type ViewQuery = ();
    type ItemQuery = Read<TextureBindGroup>;

    fn render<'w>(
        _item: &P,
        _view: (),
        atlas_bind_group: Option<&'w TextureBindGroup>,
        _param: SystemParamItem<'w, '_, Self::Param>,
        pass: &mut TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        let Some(bg) = atlas_bind_group else {
            return RenderCommandResult::Skip;
        };
        pass.set_bind_group(I, &bg.bind_group, &[]);
        RenderCommandResult::Success
    }
}

struct DrawTextInstanced;

impl<P: PhaseItem> RenderCommand<P> for DrawTextInstanced {
    type Param = ();
    type ViewQuery = ();
    type ItemQuery = Read<InstanceBuffer>;

    fn render<'w>(
        _item: &P,
        _view: (),
        instance_buffer: Option<&'w InstanceBuffer>,
        _param: SystemParamItem<'w, '_, Self::Param>,
        pass: &mut TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        let Some(instance_buffer) = instance_buffer else {
            return RenderCommandResult::Skip;
        };
        if instance_buffer.length == 0 {
            return RenderCommandResult::Skip;
        }

        // The shader expands a unit quad from `vertex_index`, six vertices
        // per glyph via the triangle-list `0,1,2, 3,4,5` winding mapped
        // onto BL,BR,TL, TL,BR,TR.
        pass.set_vertex_buffer(0, instance_buffer.buffer.slice(..));
        pass.draw(0..6, 0..instance_buffer.length as u32);

        RenderCommandResult::Success
    }
}

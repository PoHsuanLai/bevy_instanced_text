//! GPU instanced text rendering through Bevy's UI camera.
//!
//! Bind groups: 0 = Bevy UI view (`SetUiViewBindGroup`), 1 = atlas,
//! 2 = per-batch affine + clip rect. Glyph positions are node-local
//! logical px; the shader composes per-batch affine and
//! `view.clip_from_world` to land on the same screen pixels a Bevy
//! `Text` node would.

use bevy::{
    ecs::{
        entity::EntityHashMap,
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
            binding_types::{sampler, texture_2d, uniform_buffer},
            BindGroup, BindGroupEntries, BindGroupLayoutDescriptor, BindGroupLayoutEntries,
            BlendState, Buffer, BufferInitDescriptor, BufferUsages, ColorTargetState, ColorWrites,
            DynamicUniformBuffer, FragmentState, MultisampleState, PipelineCache, PrimitiveState,
            PrimitiveTopology, RenderPipelineDescriptor, SamplerBindingType, ShaderStages,
            ShaderType, SpecializedRenderPipeline, SpecializedRenderPipelines, TextureFormat,
            TextureSampleType, VertexAttribute, VertexFormat, VertexState, VertexStepMode,
        },
        renderer::{RenderDevice, RenderQueue},
        sync_world::{MainEntity, RenderEntity},
        texture::GpuImage,
        view::{ExtractedView, ViewTarget, ViewUniform},
        Extract, Render, RenderApp, RenderSystems,
    },
    ui_render::{stack_z_offsets, SetUiViewBindGroup, TransparentUi, UiCameraView},
};

use crate::view::render::{BatchTransform, GlyphBatchComponent, GlyphInstance};

/// Registers the GPU instanced text render pipeline.
pub struct InstancedTextRenderPlugin;

impl Plugin for InstancedTextRenderPlugin {
    fn build(&self, app: &mut App) {
        bevy::asset::embedded_asset!(app, "text.wgsl");

        app.add_plugins((
            ExtractComponentPlugin::<GlyphBatchComponent>::default(),
            ExtractComponentPlugin::<BatchTransform>::default(),
        ));

        let render_app = app.sub_app_mut(RenderApp);

        render_app
            .add_render_command::<TransparentUi, DrawInstancedText>()
            .init_resource::<SpecializedRenderPipelines<InstancedTextPipeline>>()
            .init_resource::<BatchTransformUniforms>()
            .add_systems(
                bevy::render::ExtractSchedule,
                extract_target_camera_render_entity,
            )
            .add_systems(
                Render,
                (
                    init_instanced_text_pipeline
                        .run_if(not(resource_exists::<InstancedTextPipeline>)),
                    queue_instanced_text
                        .run_if(resource_exists::<InstancedTextPipeline>)
                        .in_set(RenderSystems::Queue),
                    prepare_batch_transform_uniforms
                        .run_if(resource_exists::<InstancedTextPipeline>)
                        .in_set(RenderSystems::PrepareResources),
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

impl ExtractComponent for BatchTransform {
    type QueryData = &'static BatchTransform;
    type QueryFilter = ();
    type Out = Self;

    fn extract_component(item: QueryItem<'_, '_, Self::QueryData>) -> Option<Self> {
        Some(*item)
    }
}

/// Translates each batch's main-world `target_camera` into the matching
/// render-world entity, stored as [`ResolvedTargetCamera`] on the batch's
/// render-world twin.
fn extract_target_camera_render_entity(
    mut commands: Commands,
    batches: Extract<Query<(&BatchTransform, &RenderEntity)>>,
    cam_render_entities: Extract<Query<&RenderEntity>>,
) {
    for (transform, batch_render) in batches.iter() {
        let resolved = transform
            .target_camera
            .and_then(|cam| cam_render_entities.get(cam).ok().map(|r| r.id()));
        if let Some(target) = resolved {
            commands
                .entity(batch_render.id())
                .insert(ResolvedTargetCamera(target));
        }
    }
}

/// Render-world resolution of `BatchTransform::target_camera`.
#[derive(Component, Clone, Copy)]
pub struct ResolvedTargetCamera(pub Entity);

#[derive(Component)]
pub struct InstanceBuffer {
    pub buffer: Buffer,
    pub length: usize,
}

#[derive(Component)]
pub struct TextureBindGroup {
    pub bind_group: BindGroup,
}

#[derive(Component)]
pub struct BatchUniformBindGroup {
    pub bind_group: BindGroup,
    pub offset: u32,
}

/// GPU layout of `BatchTransform`. `clip_max.x < 0` signals no-clip.
#[derive(Clone, Copy, ShaderType)]
pub struct BatchUniform {
    pub affine_row0: Vec3,
    pub affine_row1: Vec3,
    pub clip_min: Vec2,
    pub clip_max: Vec2,
}

#[derive(Resource, Default)]
pub struct BatchTransformUniforms {
    pub buffer: DynamicUniformBuffer<BatchUniform>,
}

fn prepare_batch_transform_uniforms(
    mut commands: Commands,
    query: Query<(Entity, &BatchTransform)>,
    render_device: Res<RenderDevice>,
    render_queue: Res<RenderQueue>,
    pipeline: Res<InstancedTextPipeline>,
    pipeline_cache: Res<PipelineCache>,
    mut uniforms: ResMut<BatchTransformUniforms>,
) {
    uniforms.buffer.clear();
    let layout = pipeline_cache.get_bind_group_layout(&pipeline.batch_bind_group_layout);

    let mut entries = Vec::new();
    for (entity, transform) in &query {
        let (clip_min, clip_max) = match transform.clip {
            Some(rect) => (rect.min, rect.max),
            None => (Vec2::ZERO, Vec2::new(-1.0, -1.0)),
        };
        let uniform = BatchUniform {
            affine_row0: Vec3::new(
                transform.affine[0],
                transform.affine[1],
                transform.affine[2],
            ),
            affine_row1: Vec3::new(
                transform.affine[3],
                transform.affine[4],
                transform.affine[5],
            ),
            clip_min,
            clip_max,
        };
        let offset = uniforms.buffer.push(&uniform);
        entries.push((entity, offset));
    }
    uniforms.buffer.write_buffer(&render_device, &render_queue);

    if let Some(uniform_binding) = uniforms.buffer.binding() {
        for (entity, offset) in entries {
            let bind_group = render_device.create_bind_group(
                "batch_transform_bind_group",
                &layout,
                &BindGroupEntries::single(uniform_binding.clone()),
            );
            commands
                .entity(entity)
                .insert(BatchUniformBindGroup { bind_group, offset });
        }
    }
}

fn prepare_instance_buffers(
    mut commands: Commands,
    query: Query<(Entity, &GlyphBatchComponent)>,
    render_device: Res<RenderDevice>,
    pipeline: Res<InstancedTextPipeline>,
    pipeline_cache: Res<PipelineCache>,
    gpu_images: Res<RenderAssets<GpuImage>>,
) {
    let atlas_layout = pipeline_cache.get_bind_group_layout(&pipeline.atlas_bind_group_layout);

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
    pub view_layout: BindGroupLayoutDescriptor,
    pub atlas_bind_group_layout: BindGroupLayoutDescriptor,
    pub batch_bind_group_layout: BindGroupLayoutDescriptor,
    pub shader: Handle<Shader>,
}

fn init_instanced_text_pipeline(mut commands: Commands, asset_server: Res<AssetServer>) {
    // Single-binding ViewUniform layout, matching Bevy UI's view layout so
    // `SetUiViewBindGroup<0>` can bind into it directly.
    let view_layout = BindGroupLayoutDescriptor::new(
        "instanced_text_view_layout",
        &BindGroupLayoutEntries::single(
            ShaderStages::VERTEX_FRAGMENT,
            uniform_buffer::<ViewUniform>(true),
        ),
    );

    let atlas_bind_group_layout = BindGroupLayoutDescriptor::new(
        "instanced_text_atlas_layout",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::FRAGMENT,
            (
                texture_2d(TextureSampleType::Float { filterable: true }),
                sampler(SamplerBindingType::Filtering),
            ),
        ),
    );

    let batch_bind_group_layout = BindGroupLayoutDescriptor::new(
        "instanced_text_batch_layout",
        &BindGroupLayoutEntries::single(
            ShaderStages::VERTEX_FRAGMENT,
            uniform_buffer::<BatchUniform>(true),
        ),
    );

    let shader = bevy::asset::load_embedded_asset!(asset_server.as_ref(), "text.wgsl");

    commands.insert_resource(InstancedTextPipeline {
        view_layout,
        atlas_bind_group_layout,
        batch_bind_group_layout,
        shader,
    });
}

#[derive(Clone, Copy, Hash, PartialEq, Eq)]
pub struct InstancedTextPipelineKey {
    pub hdr: bool,
    pub msaa_samples: u32,
}

impl SpecializedRenderPipeline for InstancedTextPipeline {
    type Key = InstancedTextPipelineKey;

    fn specialize(&self, key: Self::Key) -> RenderPipelineDescriptor {
        // Per-glyph instance attributes; locations match `GlyphInstance`'s
        // field order in `view::render`.
        let instance_layout = VertexBufferLayout {
            array_stride: std::mem::size_of::<GlyphInstance>() as u64,
            step_mode: VertexStepMode::Instance,
            attributes: vec![
                VertexAttribute {
                    format: VertexFormat::Float32x2,
                    offset: 0,
                    shader_location: 0,
                },
                VertexAttribute {
                    format: VertexFormat::Float32x2,
                    offset: 8,
                    shader_location: 1,
                },
                VertexAttribute {
                    format: VertexFormat::Float32x2,
                    offset: 16,
                    shader_location: 2,
                },
                VertexAttribute {
                    format: VertexFormat::Float32x2,
                    offset: 24,
                    shader_location: 3,
                },
                VertexAttribute {
                    format: VertexFormat::Float32x4,
                    offset: 32,
                    shader_location: 4,
                },
                VertexAttribute {
                    format: VertexFormat::Float32x4,
                    offset: 48,
                    shader_location: 5,
                },
                VertexAttribute {
                    format: VertexFormat::Float32,
                    offset: 64,
                    shader_location: 6,
                },
                VertexAttribute {
                    format: VertexFormat::Float32,
                    offset: 68,
                    shader_location: 7,
                },
            ],
        };

        let format = if key.hdr {
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
                    blend: Some(BlendState::ALPHA_BLENDING),
                    write_mask: ColorWrites::ALL,
                })],
            }),
            layout: vec![
                self.view_layout.clone(),
                self.atlas_bind_group_layout.clone(),
                self.batch_bind_group_layout.clone(),
            ],
            primitive: PrimitiveState {
                topology: PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..default()
            },
            // UI pass uses no depth attachment (see `UiPassNode::run`).
            depth_stencil: None,
            multisample: MultisampleState {
                count: key.msaa_samples,
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
    transparent_ui_draw_functions: Res<DrawFunctions<TransparentUi>>,
    instanced_text_pipeline: Res<InstancedTextPipeline>,
    mut pipelines: ResMut<SpecializedRenderPipelines<InstancedTextPipeline>>,
    pipeline_cache: Res<PipelineCache>,
    batches: Query<(
        Entity,
        &MainEntity,
        &GlyphBatchComponent,
        &BatchTransform,
        Option<&ResolvedTargetCamera>,
    )>,
    mut transparent_render_phases: ResMut<ViewSortedRenderPhases<TransparentUi>>,
    main_views: Query<(Entity, &UiCameraView)>,
    ui_views: Query<&ExtractedView>,
) {
    let draw_function = transparent_ui_draw_functions
        .read()
        .id::<DrawInstancedText>();

    let ui_view_keys: Vec<bevy::render::view::RetainedViewEntity> =
        transparent_render_phases.keys().copied().collect();

    let mut by_main_camera: EntityHashMap<(bevy::render::view::RetainedViewEntity, bool)> =
        EntityHashMap::default();
    for (main_render_entity, ui_camera_view) in &main_views {
        if let Ok(view) = ui_views.get(ui_camera_view.0) {
            by_main_camera.insert(main_render_entity, (view.retained_view_entity, view.hdr));
        }
    }

    // Archetype-timing fallback for the first few frames before the main
    // camera's `UiCameraView` is visible to our query: target every UI
    // view that has a `TransparentUi` phase. Single-camera apps (the common
    // case) route to the only available view; multi-camera apps overshoot
    // for a frame or two, then converge.
    let fallback_views: Vec<(bevy::render::view::RetainedViewEntity, bool)> =
        if by_main_camera.is_empty() {
            ui_views
                .iter()
                .map(|view| (view.retained_view_entity, view.hdr))
                .filter(|(rve, _)| ui_view_keys.contains(rve))
                .collect()
        } else {
            Vec::new()
        };

    for (entity, main_entity, batch, transform, resolved_cam) in &batches {
        if batch.instances.is_empty() {
            continue;
        }

        let routes: Vec<(bevy::render::view::RetainedViewEntity, bool)> =
            match resolved_cam.and_then(|c| by_main_camera.get(&c.0).copied()) {
                Some(route) => vec![route],
                None => fallback_views.clone(),
            };

        for (retained_view, hdr) in routes {
            let Some(transparent_phase) = transparent_render_phases.get_mut(&retained_view) else {
                continue;
            };

            // UI pass is MSAA=1 (see `UiPassNode::run`).
            let pipeline_id = pipelines.specialize(
                &pipeline_cache,
                &instanced_text_pipeline,
                InstancedTextPipelineKey {
                    hdr,
                    msaa_samples: 1,
                },
            );

            let sort = transform.stack_index as f32 + stack_z_offsets::TEXT;

            transparent_phase.add(TransparentUi {
                entity: (entity, *main_entity),
                pipeline: pipeline_id,
                draw_function,
                sort_key: FloatOrd(sort),
                batch_range: 0..1,
                extra_index: PhaseItemExtraIndex::None,
                index: usize::MAX,
                indexed: false,
            });
        }
    }
}

type DrawInstancedText = (
    SetItemPipeline,
    SetUiViewBindGroup<0>,
    SetAtlasBindGroup<1>,
    SetBatchUniformBindGroup<2>,
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

struct SetBatchUniformBindGroup<const I: usize>;

impl<P: PhaseItem, const I: usize> RenderCommand<P> for SetBatchUniformBindGroup<I> {
    type Param = ();
    type ViewQuery = ();
    type ItemQuery = Read<BatchUniformBindGroup>;

    fn render<'w>(
        _item: &P,
        _view: (),
        batch_bind_group: Option<&'w BatchUniformBindGroup>,
        _param: SystemParamItem<'w, '_, Self::Param>,
        pass: &mut TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        let Some(bg) = batch_bind_group else {
            return RenderCommandResult::Skip;
        };
        pass.set_bind_group(I, &bg.bind_group, &[bg.offset]);
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

        // 6 vertices per glyph — the shader expands a unit quad from
        // `vertex_index`, no mesh asset is bound.
        pass.set_vertex_buffer(0, instance_buffer.buffer.slice(..));
        pass.draw(0..6, 0..instance_buffer.length as u32);

        RenderCommandResult::Success
    }
}

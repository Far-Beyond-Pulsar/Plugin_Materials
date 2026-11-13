//! The materials' shading-cost view: what a lit pixel costs to shade, drawn
//! as a heatmap.
//!
//! Only the materials know their light loop and evaluation cost, so the cost
//! model lives here and nowhere else. The pass measures instead of guessing:
//! it times a test patch (the materials' surface, [`SURFACE_PROBE`], lit by 4,
//! 16 and 32 synthetic lights over a roughness x metallic grid) with GPU
//! timestamps, once, and the view colours each pixel by the measured cost at
//! its roughness, metallic and the light count of its screen tile (the tile
//! counter a lighting model offered, `helio_frame_api::diagnostics`). Nothing
//! is shown where timestamps are unavailable or no lighting model offers a
//! count; nothing stands in for the measurement.
//!
//! It is the view of the overlay mode
//! [`PerfOverlayMode::ShaderComplexity`] and a debug view
//! ([`SHADING_COST_VIEW`]) the editor lists; the overlay draws neither.
//!
//! The readback does not block the frame: the timestamps are resolved and
//! copied in one frame, mapped the next and read when the map completes.

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;

use bytemuck::{Pod, Zeroable};
use helio_core::{
    CachedBindGroup, CommandRecorder, DebugViewDescriptor, DebugViewLegend, DebugViewLegendStop,
    PassContext, PrepareContext, RenderPass, RenderSize, ResourceKey, Result as HelioResult,
    ViewGroup,
};
use helio_frame_api::diagnostics::{PerfOverlayMode, TileCounter, HEATMAP_SNIPPET};

use crate::SURFACE_PROBE;

/// The renderer debug mode of the shading-cost view.
pub const SHADING_COST_VIEW: u32 = 60;

const VIEWS: &[DebugViewDescriptor] = &[DebugViewDescriptor {
    name: "Shading Cost",
    debug_mode: SHADING_COST_VIEW,
    description: "What each lit pixel costs to shade: the materials' measured light loop \
                  at the pixel's roughness, metallic and screen-tile light count.",
}];

const LEGEND: DebugViewLegend = DebugViewLegend {
    title: "Shading cost",
    unit: "relative to the costliest measured surface",
    stops: &[
        DebugViewLegendStop {
            label: "Cheapest",
            color: [0, 0, 255],
        },
        DebugViewLegendStop {
            label: "",
            color: [0, 255, 255],
        },
        DebugViewLegendStop {
            label: "",
            color: [0, 255, 0],
        },
        DebugViewLegendStop {
            label: "",
            color: [255, 255, 0],
        },
        DebugViewLegendStop {
            label: "Costliest",
            color: [255, 0, 0],
        },
    ],
};

// The measured grid: one cell per roughness and metallic, the light counts
// within it ascending (`cost_view.wgsl` interpolates along them).
const ROUGHNESS: [f32; 4] = [0.1, 0.4, 0.7, 1.0];
const METALLIC: [f32; 3] = [0.0, 0.5, 1.0];
const LIGHTS: [u32; 3] = [4, 16, 32];
const SAMPLES: usize = 3;
const CONFIGS: usize = ROUGHNESS.len() * METALLIC.len() * LIGHTS.len();
/// The test patch's size in pixels per side: large enough that its draw
/// outlasts the timestamp resolution.
const PATCH_PX: u32 = 256;

/// One measured configuration, as `cost_view.wgsl` reads it.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Timing {
    roughness: f32,
    metallic: f32,
    num_lights: u32,
    cost_ns: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct PatchParams {
    roughness: f32,
    metallic: f32,
    num_lights: u32,
    _pad: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct CostParams {
    internal_width: u32,
    internal_height: u32,
    display_width: u32,
    display_height: u32,
    tile_px: u32,
    num_tiles_x: u32,
    entry_count: u32,
    lights_per_cell: u32,
    max_cost: f32,
    _pad: [u32; 3],
}

fn configs() -> impl Iterator<Item = (f32, f32, u32)> {
    ROUGHNESS.into_iter().flat_map(|roughness| {
        METALLIC
            .into_iter()
            .flat_map(move |metallic| LIGHTS.into_iter().map(move |n| (roughness, metallic, n)))
    })
}

/// Where the one-off measurement is.
enum Measurement {
    /// Not recorded yet.
    Pending,
    /// Recorded and submitted: the next `prepare` maps the readback.
    Recorded,
    /// Mapping: 0 waiting, 1 mapped, 2 failed.
    Mapping(Arc<AtomicU8>),
    /// The table is uploaded.
    Done { max_cost: f32 },
    /// The device cannot time inside encoders, or the map failed.
    Unavailable,
}

/// Everything the view creates, once it is first shown.
struct Gpu {
    view_pipeline: wgpu::RenderPipeline,
    view_bgl: wgpu::BindGroupLayout,
    view_params: wgpu::Buffer,
    timings: wgpu::Buffer,
    patch_pipeline: wgpu::RenderPipeline,
    patch_bg: wgpu::BindGroup,
    patch_stride: u64,
    patch_target: wgpu::TextureView,
    queries: wgpu::QuerySet,
    resolved: wgpu::Buffer,
    readback: wgpu::Buffer,
    timestamp_period: f32,
}

fn uniform_entry(
    binding: u32,
    stage: wgpu::ShaderStages,
    dynamic: bool,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: stage,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: dynamic,
            min_binding_size: None,
        },
        count: None,
    }
}

fn storage_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only: true },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn texture_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: false },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

fn fullscreen_pipeline(
    device: &wgpu::Device,
    label: &str,
    layout: &wgpu::PipelineLayout,
    module: &wgpu::ShaderModule,
    format: wgpu::TextureFormat,
) -> wgpu::RenderPipeline {
    helio_core::shader::create_render_pipeline(
        device,
        wgpu::RenderPipelineDescriptor {
            label: Some(label),
            layout: Some(layout),
            vertex: wgpu::VertexState {
                module,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        },
    )
}

impl Gpu {
    fn new(device: &wgpu::Device, queue: &wgpu::Queue, target_format: wgpu::TextureFormat) -> Self {
        // The view.
        let view_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Shading Cost BGL"),
            entries: &[
                uniform_entry(0, wgpu::ShaderStages::FRAGMENT, false),
                texture_entry(1),
                texture_entry(2),
                storage_entry(3),
                storage_entry(4),
            ],
        });
        let view_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Shading Cost PL"),
            bind_group_layouts: &[Some(&view_bgl)],
            immediate_size: 0,
        });
        let view_module = helio_core::shader::module_with(
            device,
            "Shading Cost Shader",
            helio_core::include_wgsl!("../shaders/cost_view.wgsl"),
            &[HEATMAP_SNIPPET],
        );
        let view_pipeline = fullscreen_pipeline(
            device,
            "Shading Cost Pipeline",
            &view_layout,
            &view_module,
            target_format,
        );
        let view_params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Shading Cost Params"),
            size: std::mem::size_of::<CostParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let timings = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Shading Cost Timings"),
            size: (CONFIGS * std::mem::size_of::<Timing>()) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // The test patch.
        let patch_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Shading Cost Patch BGL"),
            entries: &[uniform_entry(0, wgpu::ShaderStages::FRAGMENT, true)],
        });
        let patch_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Shading Cost Patch PL"),
            bind_group_layouts: &[Some(&patch_bgl)],
            immediate_size: 0,
        });
        let patch_module = helio_core::shader::module_with(
            device,
            "Shading Cost Patch Shader",
            helio_core::include_wgsl!("../shaders/profile_patch.wgsl"),
            &[SURFACE_PROBE, helio_mats::PBR_EVAL_SNIPPET],
        );
        let patch_pipeline = fullscreen_pipeline(
            device,
            "Shading Cost Patch Pipeline",
            &patch_layout,
            &patch_module,
            wgpu::TextureFormat::Rgba16Float,
        );
        let patch_stride = (device.limits().min_uniform_buffer_offset_alignment as u64)
            .max(std::mem::size_of::<PatchParams>() as u64);
        let patch_params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Shading Cost Patch Params"),
            size: patch_stride * CONFIGS as u64,
            usage: wgpu::BufferUsages::UNIFORM,
            mapped_at_creation: true,
        });
        {
            let stride = patch_stride as usize;
            let mut bytes = vec![0u8; stride * CONFIGS];
            for (i, (roughness, metallic, num_lights)) in configs().enumerate() {
                bytes[i * stride..i * stride + std::mem::size_of::<PatchParams>()].copy_from_slice(
                    bytemuck::bytes_of(&PatchParams {
                        roughness,
                        metallic,
                        num_lights,
                        _pad: 0,
                    }),
                );
            }
            patch_params
                .slice(..)
                .get_mapped_range_mut()
                .expect("a buffer created mapped")
                .copy_from_slice(&bytes);
        }
        patch_params.unmap();
        let patch_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Shading Cost Patch BG"),
            layout: &patch_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &patch_params,
                    offset: 0,
                    size: wgpu::BufferSize::new(std::mem::size_of::<PatchParams>() as u64),
                }),
            }],
        });
        let patch_target = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("Shading Cost Patch Target"),
                size: wgpu::Extent3d {
                    width: PATCH_PX,
                    height: PATCH_PX,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba16Float,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            })
            .create_view(&Default::default());

        let query_count = (CONFIGS * SAMPLES * 2) as u32;
        let bytes = query_count as u64 * 8;
        Self {
            view_pipeline,
            view_bgl,
            view_params,
            timings,
            patch_pipeline,
            patch_bg,
            patch_stride,
            patch_target,
            queries: device.create_query_set(&wgpu::QuerySetDescriptor {
                label: Some("Shading Cost Queries"),
                ty: wgpu::QueryType::Timestamp,
                count: query_count,
            }),
            resolved: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Shading Cost Resolved"),
                size: bytes,
                usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            }),
            readback: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Shading Cost Readback"),
                size: bytes,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            }),
            timestamp_period: queue.get_timestamp_period(),
        }
    }
}

/// The shading-cost view ([`crate::cost`]).
pub struct ShadingCostPass {
    /// The tile counter a lighting model offered; without one the view has
    /// no light counts and is not shown.
    counter: Option<TileCounter>,
    /// The overlay mode names this view.
    overlay_mode: bool,
    /// The renderer's debug mode names this view.
    debug_view: bool,
    target_format: wgpu::TextureFormat,
    internal: [u32; 2],
    output: [u32; 2],
    gpu: Option<Gpu>,
    measurement: Measurement,
    view_bg: CachedBindGroup,
}

impl ShadingCostPass {
    pub fn new(
        counter: Option<TileCounter>,
        mode: PerfOverlayMode,
        target_format: wgpu::TextureFormat,
        size: RenderSize,
    ) -> Self {
        Self {
            counter,
            overlay_mode: mode == PerfOverlayMode::ShaderComplexity,
            debug_view: false,
            target_format,
            internal: size.internal,
            output: size.output,
            gpu: None,
            measurement: Measurement::Pending,
            view_bg: CachedBindGroup::new(),
        }
    }

    fn shown(&self) -> bool {
        self.counter.is_some() && (self.overlay_mode || self.debug_view)
    }

    /// Advances the measurement a step that does not wait on the GPU.
    fn advance(&mut self, ctx: &PrepareContext) {
        let Some(gpu) = self.gpu.as_ref() else { return };
        match &self.measurement {
            Measurement::Pending => {
                let timing = wgpu::Features::TIMESTAMP_QUERY
                    | wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS;
                if !ctx.device.features().contains(timing) {
                    log::warn!("shading cost view: the device cannot time inside encoders");
                    self.measurement = Measurement::Unavailable;
                }
            }
            Measurement::Recorded => {
                let state = Arc::new(AtomicU8::new(0));
                let seen = state.clone();
                gpu.readback
                    .slice(..)
                    .map_async(wgpu::MapMode::Read, move |result| {
                        seen.store(if result.is_ok() { 1 } else { 2 }, Ordering::Release);
                    });
                self.measurement = Measurement::Mapping(state);
            }
            Measurement::Mapping(state) => match state.load(Ordering::Acquire) {
                1 => {
                    let table = {
                        let mapped = gpu
                            .readback
                            .slice(..)
                            .get_mapped_range()
                            .expect("the readback is mapped");
                        Self::table(bytemuck::cast_slice(&mapped), gpu.timestamp_period)
                    };
                    gpu.readback.unmap();
                    ctx.write_buffer(&gpu.timings, 0, bytemuck::cast_slice(&table));
                    let max_cost = table.iter().fold(0.0f32, |m, t| m.max(t.cost_ns));
                    self.measurement = Measurement::Done { max_cost };
                }
                2 => {
                    log::warn!("shading cost view: the timestamp readback failed");
                    self.measurement = Measurement::Unavailable;
                }
                _ => {}
            },
            Measurement::Done { .. } | Measurement::Unavailable => {}
        }
    }

    /// The measured configurations: the median of each one's samples, in ns.
    fn table(ticks: &[u64], period: f32) -> Vec<Timing> {
        configs()
            .enumerate()
            .map(|(i, (roughness, metallic, num_lights))| {
                let mut samples: Vec<u64> = (0..SAMPLES)
                    .map(|s| {
                        let at = (i * SAMPLES + s) * 2;
                        ticks[at + 1].saturating_sub(ticks[at])
                    })
                    .collect();
                samples.sort_unstable();
                Timing {
                    roughness,
                    metallic,
                    num_lights,
                    cost_ns: samples[SAMPLES / 2] as f32 * period,
                }
            })
            .collect()
    }

    /// Times every configuration's test patch, in one submission.
    fn record(&mut self, ctx: &mut PassContext) {
        let Some(gpu) = self.gpu.as_ref() else { return };
        gpu.record_patches(&mut ctx.graphics_cmds());
        self.measurement = Measurement::Recorded;
    }
}

impl Gpu {
    /// Times each configuration's patch `SAMPLES` times, resolves the
    /// timestamps and copies them to the readback.
    fn record_patches(&self, cmds: &mut CommandRecorder<'_>) {
        let gpu = self;
        let mut query = 0u32;
        for config in 0..CONFIGS {
            for _ in 0..SAMPLES {
                cmds.write_timestamp(&gpu.queries, query);
                {
                    let mut pass = cmds.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("Shading Cost Patch"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &gpu.patch_target,
                            resolve_target: None,
                            depth_slice: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        depth_stencil_attachment: None,
                        timestamp_writes: None,
                        occlusion_query_set: None,
                        multiview_mask: None,
                    });
                    pass.set_pipeline(&gpu.patch_pipeline);
                    pass.set_bind_group(
                        0,
                        &gpu.patch_bg,
                        &[(config as u64 * gpu.patch_stride) as u32],
                    );
                    pass.draw(0..3, 0..1);
                }
                cmds.write_timestamp(&gpu.queries, query + 1);
                query += 2;
            }
        }
        cmds.resolve_query_set(&gpu.queries, 0..query, &gpu.resolved, 0);
        cmds.copy_buffer_to_buffer(&gpu.resolved, 0, &gpu.readback, 0, query as u64 * 8);
    }
}

impl RenderPass for ShadingCostPass {
    fn name(&self) -> &'static str {
        "Shading Cost"
    }

    /// A member while the view is shown.
    fn is_member(&mut self, _scene: &mut helio_core::SceneContent<'_>) -> bool {
        self.shown()
    }

    fn render_pass_descriptor<'a>(
        &'a self,
        _target: &'a wgpu::TextureView,
        _depth: &'a wgpu::TextureView,
        _resources: &'a helio_core::ResourceRegistry<'a>,
    ) -> Option<wgpu::RenderPassDescriptor<'a>> {
        None
    }

    fn on_resize(&mut self, _device: &wgpu::Device, size: RenderSize) {
        self.internal = size.internal;
        self.output = size.output;
    }

    fn prepare(&mut self, ctx: &PrepareContext) -> HelioResult<()> {
        if !self.shown() {
            return Ok(());
        }
        let tile_px = self.counter.as_ref().map_or(1, |c| c.tile_px.max(1));
        if self.gpu.is_none() {
            self.gpu = Some(Gpu::new(ctx.device, ctx.queue, self.target_format));
        }
        self.advance(ctx);
        let gpu = self.gpu.as_ref().expect("created above");
        let max_cost = match self.measurement {
            Measurement::Done { max_cost } => max_cost,
            _ => 0.0,
        };
        ctx.write_buffer(
            &gpu.view_params,
            0,
            bytemuck::bytes_of(&CostParams {
                internal_width: self.internal[0],
                internal_height: self.internal[1],
                display_width: self.output[0],
                display_height: self.output[1],
                tile_px,
                num_tiles_x: self.internal[0].div_ceil(tile_px),
                entry_count: CONFIGS as u32,
                lights_per_cell: LIGHTS.len() as u32,
                max_cost: max_cost.max(f32::MIN_POSITIVE),
                _pad: [0; 3],
            }),
        );
        Ok(())
    }

    fn execute(&mut self, ctx: &mut PassContext) -> HelioResult<()> {
        if !self.shown() || self.gpu.is_none() {
            return Ok(());
        }
        if matches!(self.measurement, Measurement::Pending) {
            self.record(ctx);
        }
        if !matches!(self.measurement, Measurement::Done { .. }) {
            return Ok(());
        }
        let (Some(gpu), Some(counter)) = (self.gpu.as_ref(), self.counter.as_ref()) else {
            return Ok(());
        };
        let (Some(gbuffer), Some(tile_counts)) = (
            ctx.registry
                .get::<ViewGroup<'_, 4>>(ResourceKey::new(helio_frame_api::gbuffer::GROUP)),
            ctx.registry
                .get::<&wgpu::Buffer>(ResourceKey::new(counter.resource)),
        ) else {
            return Ok(());
        };
        let bind_group = self.view_bg.get_or_create(
            ctx.device,
            &wgpu::BindGroupDescriptor {
                label: Some("Shading Cost BG"),
                layout: &gpu.view_bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: gpu.view_params.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(gbuffer.views[2]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(ctx.depth),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: tile_counts.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: gpu.timings.as_entire_binding(),
                    },
                ],
            },
        );
        let mut cmds = ctx.graphics_cmds();
        let mut pass = cmds.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Shading Cost View"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: ctx.target,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&gpu.view_pipeline);
        pass.set_bind_group(0, bind_group, &[]);
        pass.draw(0..3, 0..1);
        Ok(())
    }

    fn set_debug_mode(&mut self, mode: u32) {
        self.debug_view = mode == SHADING_COST_VIEW;
    }

    fn debug_views(&self) -> &'static [DebugViewDescriptor] {
        VIEWS
    }

    fn debug_view_legend(&self, debug_mode: u32) -> Option<DebugViewLegend> {
        (debug_mode == SHADING_COST_VIEW).then_some(LEGEND)
    }

    fn debug_view_category(&self, _debug_mode: u32) -> &'static str {
        "Materials"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shaders as the pass compiles them, parsed and validated.
    #[test]
    fn the_shaders_validate_as_compiled() {
        let sources = [
            helio_core::shader::resolve_with(
                include_str!("../shaders/cost_view.wgsl"),
                &[HEATMAP_SNIPPET],
            ),
            helio_core::shader::resolve_with(
                include_str!("../shaders/profile_patch.wgsl"),
                &[SURFACE_PROBE, helio_mats::PBR_EVAL_SNIPPET],
            ),
        ];
        for source in sources {
            let module = naga::front::wgsl::parse_str(&source).expect("the shader parses");
            naga::valid::Validator::new(
                naga::valid::ValidationFlags::all(),
                naga::valid::Capabilities::all(),
            )
            .validate(&module)
            .expect("the shader validates");
        }
    }

    /// Each configuration's cost is the median of its samples, scaled by the
    /// timestamp period.
    #[test]
    fn the_table_is_the_median_of_each_configurations_samples_in_ns() {
        let mut ticks = vec![0u64; CONFIGS * SAMPLES * 2];
        for config in 0..CONFIGS {
            for (sample, duration) in [900u64, 100, 500].into_iter().enumerate() {
                let at = (config * SAMPLES + sample) * 2;
                ticks[at] = 1_000;
                ticks[at + 1] = 1_000 + duration;
            }
        }
        let table = ShadingCostPass::table(&ticks, 2.0);
        assert_eq!(table.len(), CONFIGS);
        assert!(table.iter().all(|t| t.cost_ns == 1000.0));
        assert_eq!(table[0].num_lights, LIGHTS[0]);
        assert_eq!(table[1].num_lights, LIGHTS[1]);
        assert_eq!(table[3].metallic, METALLIC[1]);
    }

    /// The whole measurement on a device: both pipelines compile, the patches
    /// time, and a surface lit by more lights costs more.
    #[test]
    fn the_measurement_runs_and_more_lights_cost_more() {
        pollster::block_on(async {
            let instance = wgpu::Instance::new(helio_core::wgpu_instance_descriptor());
            let adapter = instance
                .request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::HighPerformance,
                    compatible_surface: None,
                    force_fallback_adapter: false,
                    apply_limit_buckets: false,
                })
                .await
                .expect("GPU required for the shading cost test");
            let timing =
                wgpu::Features::TIMESTAMP_QUERY | wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS;
            assert!(
                adapter.features().contains(timing),
                "the adapter must time inside encoders"
            );
            let (device, queue) = adapter
                .request_device(&wgpu::DeviceDescriptor {
                    required_features: timing,
                    required_limits: adapter.limits(),
                    ..Default::default()
                })
                .await
                .unwrap();
            device.on_uncaptured_error(Arc::new(|error| panic!("{error:?}")));

            let gpu = Gpu::new(&device, &queue, wgpu::TextureFormat::Rgba8Unorm);
            let mut encoder = device.create_command_encoder(&Default::default());
            gpu.record_patches(&mut CommandRecorder::from_encoder(&mut encoder));
            queue.submit([encoder.finish()]);
            gpu.readback
                .slice(..)
                .map_async(wgpu::MapMode::Read, |r| r.expect("the readback maps"));
            device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
            let table = {
                let mapped = gpu.readback.slice(..).get_mapped_range().unwrap();
                ShadingCostPass::table(bytemuck::cast_slice(&mapped), gpu.timestamp_period)
            };
            assert!(
                table.iter().all(|t| t.cost_ns > 0.0),
                "{:?}",
                cost_list(&table)
            );
            // Summed over the grid, 32 lights cost more than 4.
            let total = |lights| {
                table
                    .iter()
                    .filter(|t| t.num_lights == lights)
                    .map(|t| t.cost_ns)
                    .sum::<f32>()
            };
            assert!(
                total(LIGHTS[2]) > total(LIGHTS[0]),
                "{:?}",
                cost_list(&table)
            );
        });
    }

    fn cost_list(table: &[Timing]) -> Vec<(f32, f32, u32, f32)> {
        table
            .iter()
            .map(|t| (t.roughness, t.metallic, t.num_lights, t.cost_ns))
            .collect()
    }
}

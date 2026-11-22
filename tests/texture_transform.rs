//! The texture slot UV transform on the GPU: the material evaluator's
//! `material_uv_transform` (`material_row.wgsl`, what every drawer, the
//! visibility-buffer shade and a ray hit's surface sample through) and the
//! virtual-geometry alpha test's `vga_uv` give what
//! `helio_mats::TextureTransform::apply` gives: glTF `KHR_texture_transform`.
//! Run: cargo test -p materials_plugin --test texture_transform
//! Requires a GPU adapter; absence is a failure, never a silent pass.

use helio_mats::{
    MaterialBindingConfig, MaterialBindingMode, MaterialTextureSlot, TextureTransform,
};

const KERNEL: &str = r#"
struct Case {
    offset_scale: vec4<f32>,
    rotation: vec4<f32>,
    // xy: a UV, zw: a UV difference.
    uv: vec4<f32>,
}
@group(0) @binding(0) var<storage, read> cases: array<Case>;
// Per case: (material uv, material linear), (vg alpha uv, vg alpha linear).
@group(0) @binding(1) var<storage, read_write> results: array<vec4<f32>>;

@compute @workgroup_size(1)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let c = cases[id.x];
    results[2u * id.x] = vec4<f32>(
        material_uv_transform(c.offset_scale, c.rotation, c.uv.xy),
        material_uv_transform_linear(c.offset_scale, c.rotation, c.uv.zw),
    );
    var a: VgaAlpha;
    a.offset_scale = c.offset_scale;
    a.rotation = c.rotation;
    results[2u * id.x + 1u] = vec4<f32>(vga_uv(a, c.uv.xy), vga_uv_linear(a, c.uv.zw));
}
"#;

#[test]
fn gpu_uv_transform_matches_khr_texture_transform() {
    let transforms = [
        TextureTransform::IDENTITY,
        // The Poly Haven island tree's branches.
        TextureTransform {
            offset: [0.0, -2.4],
            scale: [15.0, 3.4],
            rotation_radians: 0.0,
        },
        TextureTransform {
            offset: [0.25, 0.5],
            scale: [1.0, 1.0],
            rotation_radians: std::f32::consts::FRAC_PI_2,
        },
        TextureTransform {
            offset: [-0.3, 1.7],
            scale: [2.0, 0.5],
            rotation_radians: -0.7,
        },
    ];
    let uvs = [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [0.37, -0.81]];
    let mut cases: Vec<f32> = Vec::new();
    let mut expected = Vec::new();
    for transform in &transforms {
        let slot = MaterialTextureSlot::new(0, 0, transform);
        for uv in uvs {
            let d = [uv[1] - 0.5, uv[0] * 2.0];
            cases.extend(slot.offset_scale);
            cases.extend(slot.rotation);
            cases.extend([uv[0], uv[1], d[0], d[1]]);
            expected.push((transform.apply(uv), transform.apply_linear(d)));
        }
    }

    let binding = MaterialBindingConfig {
        mode: MaterialBindingMode::Expanded,
        max_textures: 1,
    };
    let source = format!(
        "{}\n{}\n{}\n{KERNEL}",
        include_str!("../api/shaders/material_row.wgsl"),
        helio_mats::vg_alpha::bindings_wgsl(binding, 1, 0),
        helio_mats::vg_alpha::WGSL,
    );

    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
            .expect("GPU adapter required");
    let (device, queue) =
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
            .expect("GPU device");
    use wgpu::util::DeviceExt;
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("texture transform"),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("texture transform"),
        layout: None,
        module: &module,
        entry_point: Some("main"),
        compilation_options: Default::default(),
        cache: None,
    });
    let input = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: None,
        contents: bytemuck::cast_slice(&cases),
        usage: wgpu::BufferUsages::STORAGE,
    });
    let size = (expected.len() * 2 * 16) as u64;
    let output = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: input.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: output.as_entire_binding(),
            },
        ],
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_compute_pass(&Default::default());
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &group, &[]);
        pass.dispatch_workgroups(expected.len() as u32, 1, 1);
    }
    encoder.copy_buffer_to_buffer(&output, 0, &readback, 0, size);
    queue.submit([encoder.finish()]);
    readback
        .slice(..)
        .map_async(wgpu::MapMode::Read, |r| r.unwrap());
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    let mapped = readback
        .slice(..)
        .get_mapped_range()
        .expect("mapped readback");
    let results: Vec<f32> = bytemuck::cast_slice(&mapped).to_vec();

    let close = |a: f32, b: f32| (a - b).abs() <= 1e-4 * (1.0 + b.abs());
    for (case, (uv, linear)) in expected.iter().enumerate() {
        for (path, at) in [
            ("material_uv_transform", 2 * case),
            ("vga_uv", 2 * case + 1),
        ] {
            let got = &results[at * 4..at * 4 + 4];
            assert!(
                close(got[0], uv[0])
                    && close(got[1], uv[1])
                    && close(got[2], linear[0])
                    && close(got[3], linear[1]),
                "{path}, case {case}: got {got:?}, expected {uv:?} {linear:?}"
            );
        }
    }
}

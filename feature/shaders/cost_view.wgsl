//! The materials' shading-cost view: each lit pixel coloured by what the
//! measured test patch cost at its roughness, metallic and the light count of
//! its screen tile, relative to the costliest measured configuration.
//!use helio_prelude
//!use frame_heatmap

struct CostParams {
    internal_width: u32,
    internal_height: u32,
    display_width: u32,
    display_height: u32,
    /// Pixels per side of the tiles `tile_light_counts` counts.
    tile_px: u32,
    num_tiles_x: u32,
    /// Measured configurations: cells of `lights_per_cell` entries (one
    /// roughness and metallic, lights ascending).
    entry_count: u32,
    lights_per_cell: u32,
    max_cost: f32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
}

struct Timing {
    roughness: f32,
    metallic: f32,
    num_lights: u32,
    cost_ns: f32,
}

@group(0) @binding(0) var<uniform> params: CostParams;
@group(0) @binding(1) var gbuffer_orm: texture_2d<f32>;
@group(0) @binding(2) var gbuffer_depth: texture_2d<f32>;
@group(0) @binding(3) var<storage, read> tile_light_counts: array<u32>;
@group(0) @binding(4) var<storage, read> timings: array<Timing>;

struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

/// Fullscreen triangle.
@vertex
fn vs_main(@builtin(vertex_index) vertex_idx: u32) -> VertexOut {
    var out: VertexOut;
    let x = f32((vertex_idx << 1u) & 2u);
    let y = f32(vertex_idx & 2u);
    out.position = vec4<f32>(x * 2.0 - 1.0, y * 2.0 - 1.0, 0.0, 1.0);
    out.uv = vec2<f32>(x, 1.0 - y);
    return out;
}

/// The measured cost at (`roughness`, `metallic`, `num_lights`): the nearest
/// measured cell in roughness and metallic, then linear in the light count
/// between the cell's measurements, continued along the last segment past
/// the largest measured count.
fn measured_cost(roughness: f32, metallic: f32, num_lights: u32) -> f32 {
    let per_cell = params.lights_per_cell;
    let cells = params.entry_count / per_cell;
    var best_cell = 0u;
    var best_distance = 1.0e30;
    for (var c = 0u; c < cells; c++) {
        let entry = timings[c * per_cell];
        let d = abs(entry.roughness - roughness) + abs(entry.metallic - metallic);
        if d < best_distance {
            best_distance = d;
            best_cell = c;
        }
    }

    let base = best_cell * per_cell;
    let lights = f32(num_lights);
    var segment = 0u;
    for (var i = 1u; i + 1u < per_cell; i++) {
        if lights > f32(timings[base + i].num_lights) {
            segment = i;
        }
    }
    let lo = timings[base + segment];
    let hi = timings[base + segment + 1u];
    let span = max(f32(hi.num_lights) - f32(lo.num_lights), 1.0);
    let t = (lights - f32(lo.num_lights)) / span;
    return max(mix(lo.cost_ns, hi.cost_ns, t), 0.0);
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    let scale_bar = heatmap_scale_bar(
        in.uv,
        vec2<f32>(f32(params.display_width), f32(params.display_height)),
    );
    if scale_bar.w > 0.0 {
        return scale_bar;
    }

    let px = vec2<u32>(
        u32(clamp(in.uv.x * f32(params.internal_width), 0.0, f32(params.internal_width - 1u))),
        u32(clamp(in.uv.y * f32(params.internal_height), 0.0, f32(params.internal_height - 1u))),
    );
    // Nothing is shaded where nothing was drawn (the sky).
    if helio_is_sky(textureLoad(gbuffer_depth, px, 0).r) {
        return vec4<f32>(heatmap_color(0.0), 1.0);
    }

    let orm = textureLoad(gbuffer_orm, px, 0);
    let tile = (px.y / params.tile_px) * params.num_tiles_x + px.x / params.tile_px;
    let cost = measured_cost(orm.g, orm.b, tile_light_counts[tile]);
    return vec4<f32>(heatmap_color(cost / params.max_cost), 1.0);
}

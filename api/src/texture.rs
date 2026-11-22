//! A material texture as a host uploads it (an image, its format and how it
//! is sampled) and the transform a material applies to a texture's UVs.
//! The host decodes images into [`TextureUpload`]s; the scene's texture
//! store keeps the GPU textures, and the materials' table binds them.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TextureSamplerDesc {
    pub address_mode_u: wgpu::AddressMode,
    pub address_mode_v: wgpu::AddressMode,
    pub address_mode_w: wgpu::AddressMode,
    pub mag_filter: wgpu::FilterMode,
    pub min_filter: wgpu::FilterMode,
    pub mipmap_filter: wgpu::MipmapFilterMode,
}

impl Default for TextureSamplerDesc {
    fn default() -> Self {
        Self {
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
        }
    }
}

#[derive(Debug, Clone)]
pub struct TextureUpload {
    pub label: Option<String>,
    pub width: u32,
    pub height: u32,
    pub format: wgpu::TextureFormat,
    pub data: Vec<u8>,
    pub sampler: TextureSamplerDesc,
}

impl TextureUpload {
    pub fn rgba8(
        label: impl Into<String>,
        width: u32,
        height: u32,
        srgb: bool,
        data: Vec<u8>,
        sampler: TextureSamplerDesc,
    ) -> Self {
        Self {
            label: Some(label.into()),
            width,
            height,
            format: if srgb {
                wgpu::TextureFormat::Rgba8UnormSrgb
            } else {
                wgpu::TextureFormat::Rgba8Unorm
            },
            data,
            sampler,
        }
    }
}

/// How a material texture slot maps the surface's UVs to the texture's: the
/// convention of glTF's `KHR_texture_transform`, which every importer and
/// shader of the materials follows. The texture is sampled at
///
/// ```text
/// uv' = offset + R(rotation) * (scale * uv)
/// R(θ) (u, v) = (cos θ * u + sin θ * v,  -sin θ * u + cos θ * v)
/// ```
///
/// with the UV origin at the image's top-left (v grows down the image), so a
/// positive rotation turns the UVs counter-clockwise as the image is viewed.
/// That is how the Khronos glTF sample renderer and three.js apply it (the
/// extension README's GLSL sketch writes the matrix transposed). The CPU
/// form is [`Self::apply`]; the shaders' is `material_uv_transform`
/// (`//!use material_row`), which reads a [`MaterialTextureSlot`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextureTransform {
    pub offset: [f32; 2],
    pub scale: [f32; 2],
    pub rotation_radians: f32,
}

impl Default for TextureTransform {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl TextureTransform {
    /// UVs as they are.
    pub const IDENTITY: Self = Self {
        offset: [0.0, 0.0],
        scale: [1.0, 1.0],
        rotation_radians: 0.0,
    };

    /// Where the texture is sampled for surface UV `uv`.
    pub fn apply(&self, uv: [f32; 2]) -> [f32; 2] {
        let [u, v] = self.apply_linear(uv);
        [u + self.offset[0], v + self.offset[1]]
    }

    /// The linear part of [`Self::apply`] (scale, then rotation): how a UV
    /// difference, such as a pixel's UV gradient, maps into the texture.
    pub fn apply_linear(&self, d: [f32; 2]) -> [f32; 2] {
        let (sin, cos) = self.rotation_radians.sin_cos();
        let (u, v) = (d[0] * self.scale[0], d[1] * self.scale[1]);
        [cos * u + sin * v, -sin * u + cos * v]
    }
}

/// One texture slot of a material's [`MaterialTextureData`] row: the texture
/// table slot it samples, its UV set and its UV transform. 48 bytes,
/// `MaterialTextureSlot` in `material_surface.wgsl`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
// A field of a SceneDB component (`host_rows::MaterialTexturesComponent`),
// whose fields SceneDB reflects.
#[cfg_attr(feature = "host-rows", derive(pulsar_reflection::Reflectable))]
pub struct MaterialTextureSlot {
    /// Material texture table slot, or [`crate::GpuMaterial::NO_TEXTURE`].
    pub texture_index: u32,
    /// The UV set (0 or 1). The evaluator reads UV set 0 today
    /// (`MaterialInput` carries one).
    pub uv_channel: u32,
    pub _pad: [u32; 2],
    /// `offset.xy`, `scale.xy` of the slot's [`TextureTransform`].
    pub offset_scale: [f32; 4],
    /// Sine and cosine of its rotation (the shaders take none per sample);
    /// `zw` unused.
    pub rotation: [f32; 4],
}

impl MaterialTextureSlot {
    /// No texture: the shaders use the slot's fallback value.
    pub const NONE: Self = Self {
        texture_index: crate::GpuMaterial::NO_TEXTURE,
        uv_channel: 0,
        _pad: [0; 2],
        offset_scale: [0.0, 0.0, 1.0, 1.0],
        rotation: [0.0, 1.0, 0.0, 0.0],
    };

    /// Texture table slot `texture_index` read from UV set `uv_channel`
    /// through `transform`.
    pub fn new(texture_index: u32, uv_channel: u32, transform: &TextureTransform) -> Self {
        let (sin, cos) = transform.rotation_radians.sin_cos();
        Self {
            texture_index,
            uv_channel,
            _pad: [0; 2],
            offset_scale: [
                transform.offset[0],
                transform.offset[1],
                transform.scale[0],
                transform.scale[1],
            ],
            rotation: [sin, cos, 0.0, 0.0],
        }
    }

    /// The slot's transform.
    pub fn transform(&self) -> TextureTransform {
        TextureTransform {
            offset: [self.offset_scale[0], self.offset_scale[1]],
            scale: [self.offset_scale[2], self.offset_scale[3]],
            rotation_radians: self.rotation[0].atan2(self.rotation[1]),
        }
    }
}

/// A material's texture slots and texture parameters: what the shaders
/// sample its textures through (`MaterialTextureData` in
/// `material_surface.wgsl`, 352 bytes), at the material's own row of
/// [`MATERIAL_TEXTURE_DATA`]. A material row reads it only with
/// [`crate::FLAG_TEXTURE_DATA`]; without it the shaders derive the same
/// thing from the row's `tex_*` slots with identity UVs
/// ([`Self::derived`]).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct MaterialTextureData {
    pub base_color: MaterialTextureSlot,
    pub normal: MaterialTextureSlot,
    pub roughness_metallic: MaterialTextureSlot,
    pub emissive: MaterialTextureSlot,
    pub occlusion: MaterialTextureSlot,
    pub specular_color: MaterialTextureSlot,
    pub specular_weight: MaterialTextureSlot,
    /// x normal scale, y occlusion strength, z alpha cutoff (0: not cut
    /// out); w unused.
    pub params: [f32; 4],
}

// SAFETY: a `#[repr(C)]` bytemuck `Pod` row: no padding bytes, any bit
// pattern valid. Lets an engine's SceneDB component embed it.
unsafe impl pulsar_scenedb::Pod for MaterialTextureSlot {}
// SAFETY: as above.
unsafe impl pulsar_scenedb::Pod for MaterialTextureData {}

/// The buffer the shaders read [`MaterialTextureData`] rows from, one per
/// material at the material's own row of [`crate::MATERIALS`]: a SceneDB
/// component an engine stores under this key
/// (`host_rows::MaterialTexturesComponent`). The materials' feature binds
/// it as the table's `material_textures`.
pub const MATERIAL_TEXTURE_DATA: helio_core::BufferContract<MaterialTextureData> =
    helio_core::BufferContract::new("material_texture_data");

impl MaterialTextureData {
    /// What the shaders read for `material` without
    /// [`crate::FLAG_TEXTURE_DATA`]: its `tex_*` slots with identity UVs,
    /// normal scale and occlusion strength 1, and the default cutoff for a
    /// cut-out material (`material_texture_data` in `material_surface.wgsl`).
    pub fn derived(material: &crate::GpuMaterial) -> Self {
        let slot = |index| MaterialTextureSlot {
            texture_index: index,
            ..MaterialTextureSlot::NONE
        };
        let cutoff = if material.flags & crate::FLAG_ALPHA_TEST != 0 {
            0.5
        } else {
            0.0
        };
        Self {
            base_color: slot(material.tex_base_color),
            normal: slot(material.tex_normal),
            roughness_metallic: slot(material.tex_roughness),
            emissive: slot(material.tex_emissive),
            occlusion: slot(material.tex_occlusion),
            specular_color: MaterialTextureSlot::NONE,
            specular_weight: MaterialTextureSlot::NONE,
            params: [1.0, 1.0, cutoff, 0.0],
        }
    }

    /// `row` drawn with this texture data: [`crate::FLAG_TEXTURE_DATA`] set,
    /// its `tex_*` slots this data's (for readers that take a row's
    /// textures without their transforms), and
    /// [`crate::FLAG_HAS_NORMAL_MAP`] with a normal texture. The row and
    /// this data go to the same material row index.
    pub fn bind(&self, mut row: crate::GpuMaterial) -> crate::GpuMaterial {
        row.flags |= crate::FLAG_TEXTURE_DATA;
        if self.normal.texture_index != crate::GpuMaterial::NO_TEXTURE {
            row.flags |= crate::FLAG_HAS_NORMAL_MAP;
        }
        row.tex_base_color = self.base_color.texture_index;
        row.tex_normal = self.normal.texture_index;
        row.tex_roughness = self.roughness_metallic.texture_index;
        row.tex_emissive = self.emissive.texture_index;
        row.tex_occlusion = self.occlusion.texture_index;
        row
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: [f32; 2], b: [f32; 2]) -> bool {
        (a[0] - b[0]).abs() < 1e-5 && (a[1] - b[1]).abs() < 1e-5
    }

    /// glTF `KHR_texture_transform` as the Khronos sample renderer applies
    /// it: a quarter turn takes the UV +u axis to -v, which, with v growing
    /// down the image, is counter-clockwise as the image is viewed.
    #[test]
    fn rotation_follows_khr_texture_transform() {
        let quarter = TextureTransform {
            rotation_radians: std::f32::consts::FRAC_PI_2,
            ..TextureTransform::IDENTITY
        };
        assert!(close(quarter.apply([1.0, 0.0]), [0.0, -1.0]));
        assert!(close(quarter.apply([0.0, 1.0]), [1.0, 0.0]));
    }

    /// Scale first, then rotation, then offset (T * R * S).
    #[test]
    fn scale_then_rotate_then_offset() {
        let t = TextureTransform {
            offset: [0.0, -2.4],
            scale: [15.0, 3.4],
            rotation_radians: 0.3,
        };
        let uv = [0.25, 0.5];
        let (s, c) = 0.3f32.sin_cos();
        let (u, v) = (0.25 * 15.0, 0.5 * 3.4);
        assert!(close(t.apply(uv), [c * u + s * v, -s * u + c * v - 2.4]));
        assert!(close(
            t.apply_linear(uv),
            [t.apply(uv)[0], t.apply(uv)[1] + 2.4]
        ));
        assert!(close(TextureTransform::IDENTITY.apply(uv), uv));
    }

    /// A slot keeps its transform (the rotation as sine and cosine).
    #[test]
    fn a_slot_round_trips_its_transform() {
        let t = TextureTransform {
            offset: [0.5, -2.4],
            scale: [15.0, 3.4],
            rotation_radians: -1.2,
        };
        let slot = MaterialTextureSlot::new(7, 1, &t);
        assert_eq!(slot.texture_index, 7);
        assert_eq!(slot.uv_channel, 1);
        let back = slot.transform();
        assert_eq!(back.offset, t.offset);
        assert_eq!(back.scale, t.scale);
        assert!((back.rotation_radians - t.rotation_radians).abs() < 1e-6);
        assert_eq!(
            MaterialTextureSlot::new(u32::MAX, 0, &TextureTransform::IDENTITY),
            MaterialTextureSlot::NONE
        );
    }

    /// The row ABI: 48-byte slots, a 352-byte row (88 words), fields where
    /// `material_surface.wgsl` reads them.
    #[test]
    fn the_rows_match_the_wgsl_layout() {
        use std::mem::{offset_of, size_of};
        assert_eq!(size_of::<MaterialTextureSlot>(), 48);
        assert_eq!(offset_of!(MaterialTextureSlot, uv_channel), 4);
        assert_eq!(offset_of!(MaterialTextureSlot, offset_scale), 16);
        assert_eq!(offset_of!(MaterialTextureSlot, rotation), 32);
        assert_eq!(size_of::<MaterialTextureData>(), 352);
        assert_eq!(
            size_of::<MaterialTextureData>(),
            crate::MATERIAL_TEXTURE_DATA_WORDS * 4
        );
        assert_eq!(offset_of!(MaterialTextureData, normal), 48);
        assert_eq!(offset_of!(MaterialTextureData, specular_weight), 288);
        assert_eq!(offset_of!(MaterialTextureData, params), 336);
    }
}

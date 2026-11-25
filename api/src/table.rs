//! The material texture table ([`MaterialTextureTable`]) and the stand-in a
//! drawer binds when no materials plugin publishes one ([`MaterialStandIn`]).
//!
//! A table holds one view per slot (a white texture where a slot is empty),
//! the one material sampler every slot is read through and the materials'
//! texture data rows ([`crate::MATERIAL_TEXTURE_DATA`], the scene's; one
//! zero row without them); its
//! [`MaterialTextureBindings`] are what every drawer binds
//! (`MaterialBindingConfig::append_bind_group_entries`). The materials'
//! feature (`helio-pass-materials`) keeps one in step with the scene's
//! texture store and publishes it as [`crate::MATERIAL_TEXTURES`].

use crate::{MaterialBindingConfig, MaterialRow, MaterialSampler, MaterialTextureBindings};

/// Words of one `MaterialTextureData` row (seven 48-byte slots and a 16-byte
/// parameter vector, `material_surface.wgsl`).
pub const MATERIAL_TEXTURE_DATA_WORDS: usize =
    std::mem::size_of::<crate::MaterialTextureData>() / 4;

/// A material texture table: one view per slot, its sampler and metadata.
pub struct MaterialTextureTable {
    _fallback_texture: wgpu::Texture,
    // Boxed (as the slots' views are, in `slots`' heap buffer): its
    // address never changes when the table moves.
    fallback_view: Box<wgpu::TextureView>,
    /// The one sampler every slot is read through (`scene_sampler`).
    sampler: wgpu::Sampler,
    /// One zero `MaterialTextureData` row: what the table binds where the
    /// scene keeps no texture data rows (no material reads it: only a row
    /// with `FLAG_TEXTURE_DATA` does, and such a row has its data).
    empty_texture_data: wgpu::Buffer,
    /// The scene's texture data rows and their buffer's epoch
    /// ([`Self::set_texture_data`]).
    texture_data: Option<(wgpu::Buffer, u64)>,
    /// One entry per slot; never resized, so the views' addresses are
    /// stable until an entry is replaced (then `view_refs` is rebuilt).
    slots: Vec<Option<(wgpu::Texture, wgpu::TextureView)>>,
    /// The table's views (a slot's or the fallback), pointing into `self`.
    view_refs: Vec<&'static wgpu::TextureView>,
    /// Whether [`Self::bindings`] hands out only the live prefix of the views
    /// (the device binds arrays partially: `helio_core::texture_table_binds_partially`).
    partial: bool,
    /// Slots from the first that may hold a texture: one past the highest slot
    /// the scene's texture store has handed out or that was ever set. Never
    /// shrinks (a row still naming a released slot reads the fallback, as it
    /// always did); at least 1.
    live: usize,
    version: u64,
}

impl MaterialTextureTable {
    /// An empty table as large as `device` binds ([`MaterialBindingConfig`]),
    /// read with `sampler` (the default sampler without).
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        sampler: Option<&MaterialSampler>,
    ) -> Self {
        use wgpu::util::DeviceExt;
        let binding = MaterialBindingConfig::for_device(device);
        let fallback_texture = device.create_texture_with_data(
            queue,
            &wgpu::TextureDescriptor {
                label: Some("Default Material Texture"),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            &[255, 255, 255, 255],
        );
        let fallback_view =
            Box::new(fallback_texture.create_view(&wgpu::TextureViewDescriptor::default()));
        let sampler = device.create_sampler(&sampler.map(|s| s.0.clone()).unwrap_or(
            wgpu::SamplerDescriptor {
                label: Some("Default Material Sampler"),
                ..Default::default()
            },
        ));
        let empty_texture_data = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Material Texture Data (none)"),
            contents: bytemuck::bytes_of(
                &<crate::MaterialTextureData as bytemuck::Zeroable>::zeroed(),
            ),
            usage: wgpu::BufferUsages::STORAGE,
        });
        let mut table = Self {
            _fallback_texture: fallback_texture,
            fallback_view,
            sampler,
            empty_texture_data,
            texture_data: None,
            slots: vec![None; binding.max_textures],
            view_refs: Vec::with_capacity(binding.max_textures),
            partial: helio_core::texture_table_binds_partially(device.features()),
            live: 1,
            version: 0,
        };
        table.refresh_refs();
        table
    }

    /// Slots the table holds.
    pub fn capacity(&self) -> usize {
        self.slots.len()
    }

    /// Puts `texture` (`None`: the white fallback) in `slot`. Returns whether
    /// the slot changed (the bindings' version then moves); an error for a
    /// texture the table cannot bind (not a filterable, single-layer,
    /// single-sample 2D texture) or a slot past the table.
    pub fn set(
        &mut self,
        device_features: wgpu::Features,
        slot: usize,
        texture: Option<&wgpu::Texture>,
    ) -> Result<bool, String> {
        let Some(cached) = self.slots.get_mut(slot) else {
            return Err(format!(
                "material texture slot {slot} is past the table's {} slots",
                self.slots.len()
            ));
        };
        if cached.as_ref().map(|(texture, _)| texture) == texture {
            return Ok(false);
        }
        if let Some(texture) = texture {
            if (slot as u32) < helio_core::TEXTURE_TABLE_RESERVED_SLOTS {
                return Err(format!(
                    "material texture slot {slot} is reserved for the white fallback; the texture store must be created with `TextureStore::with_reserved_slots(.., helio_core::TEXTURE_TABLE_RESERVED_SLOTS)`"
                ));
            }
            if texture.dimension() != wgpu::TextureDimension::D2
                || texture.depth_or_array_layers() != 1
                || texture.sample_count() != 1
                || !texture
                    .usage()
                    .contains(wgpu::TextureUsages::TEXTURE_BINDING)
                || texture.format().sample_type(None, Some(device_features))
                    != Some(wgpu::TextureSampleType::Float { filterable: true })
            {
                return Err(format!(
                    "material texture slot {slot} requires a filterable, single-layer, single-sample 2D texture"
                ));
            }
            *cached = Some((texture.clone(), texture.create_view(&Default::default())));
            self.live = self.live.max(slot + 1);
        } else {
            *cached = None;
        }
        self.version = self.version.wrapping_add(1);
        self.refresh_refs();
        Ok(true)
    }

    /// Binds `rows` (the scene's [`crate::MATERIAL_TEXTURE_DATA`] buffer and
    /// its epoch, which moves when the buffer is reallocated; `None`: the
    /// scene keeps none) as the materials' texture data. Returns whether the
    /// binding changed (the bindings' version then moves). New contents of
    /// the same buffer change nothing: drawers read it every frame.
    pub fn set_texture_data(&mut self, rows: Option<(&wgpu::Buffer, u64)>) -> bool {
        let same = match (&self.texture_data, rows) {
            (None, None) => true,
            (Some((buffer, epoch)), Some((new, new_epoch))) => buffer == new && *epoch == new_epoch,
            _ => false,
        };
        if same {
            return false;
        }
        self.texture_data = rows.map(|(buffer, epoch)| (buffer.clone(), epoch));
        self.version = self.version.wrapping_add(1);
        true
    }

    /// Tells the table the texture store has handed out slots below `count`,
    /// so none past it is in use: where a device binds arrays partially the
    /// table binds that many views, not its whole capacity. Only grows;
    /// returns whether it did (the bindings' version then moves).
    pub fn set_live_slots(&mut self, count: usize) -> bool {
        let count = count.clamp(1, self.slots.len());
        if count <= self.live {
            return false;
        }
        self.live = count;
        self.version = self.version.wrapping_add(1);
        true
    }

    /// The table as drawers bind it: every slot, or where the device binds
    /// arrays partially the first [`Self::set_live_slots`] ones (a drawer's
    /// layout and shader keep the full capacity).
    pub fn bindings(&self) -> MaterialTextureBindings<'_> {
        let bound = if self.partial {
            self.live
        } else {
            self.view_refs.len()
        };
        MaterialTextureBindings {
            material_textures: self
                .texture_data
                .as_ref()
                .map_or(&self.empty_texture_data, |(buffer, _)| buffer),
            texture_views: &self.view_refs[..bound],
            sampler: &self.sampler,
            version: self.version,
        }
    }

    /// Points `view_refs` at this table's current views.
    fn refresh_refs(&mut self) {
        // SAFETY: the views are owned by `self` on the heap (the box,
        // `slots`' buffer), so they stay where they are when the
        // table moves, and live as long as it does; `slots` is never resized
        // and an entry replaced in `set` calls this again. The `'static`
        // references never leave `self` but as borrows of it (`bindings`).
        let fallback: &'static wgpu::TextureView =
            unsafe { &*(&*self.fallback_view as *const wgpu::TextureView) };
        self.view_refs.clear();
        for entry in &self.slots {
            self.view_refs.push(match entry {
                Some((_, view)) => unsafe { &*(view as *const wgpu::TextureView) },
                None => fallback,
            });
        }
    }
}

/// What a drawer binds where no materials plugin publishes the material
/// table: the grey stand-in material (`MaterialRow::STAND_IN`, which the
/// shaders' `material_at` also returns for a row the table lacks) and a
/// texture table of white fallbacks. A kind's drawer keeps one and binds
/// it when `MATERIAL_TEXTURES` is not in the frame or no `materials` rows
/// are; without the materials plugin every surface draws default grey PBR.
pub struct MaterialStandIn {
    table: MaterialTextureTable,
    rows: wgpu::Buffer,
}

impl MaterialStandIn {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        use wgpu::util::DeviceExt;
        let rows = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Stand-in material row"),
            contents: bytemuck::bytes_of(&MaterialRow::STAND_IN),
            usage: wgpu::BufferUsages::STORAGE,
        });
        Self {
            table: MaterialTextureTable::new(device, queue, None),
            rows,
        }
    }

    /// The stand-in texture table (white fallbacks). Its version is
    /// `u64::MAX`, which a published table never reaches, so a drawer
    /// caching bind groups by version rebuilds when it switches.
    pub fn textures(&self) -> MaterialTextureBindings<'_> {
        MaterialTextureBindings {
            version: u64::MAX,
            ..self.table.bindings()
        }
    }

    /// One stand-in material row (the `materials` table's stand-in).
    pub fn rows(&self) -> &wgpu::Buffer {
        &self.rows
    }
}

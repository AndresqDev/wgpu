use alloc::{string::String, vec::Vec};
use core::{
    borrow::Borrow,
    sync::atomic::{AtomicBool, Ordering},
};

use windows::Win32::{
    Foundation,
    Graphics::{Direct3D11, Dxgi},
};

/// Placeholder for every GPU resource type that is not implemented yet.
///
/// Buffers, textures, texture views, samplers and bind group objects have
/// concrete wrappers below. This type remains as the placeholder used by
/// render/compute pipelines, query sets and acceleration structures until
/// those milestones land.
#[derive(Debug)]
pub struct Resource;

/// Describes how a buffer can be accessed from the CPU.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BufferMapMode {
    /// A GPU-only buffer created with `D3D11_USAGE_DEFAULT`.
    GpuOnly,
    /// A `D3D11_USAGE_DYNAMIC` buffer that is written with `WRITE_DISCARD`.
    Dynamic,
    /// A `D3D11_USAGE_STAGING` buffer used for uploads and read-back.
    Staging,
}

/// A D3D11 buffer.
#[derive(Debug)]
pub struct Buffer {
    raw: Direct3D11::ID3D11Buffer,
    size: wgt::BufferAddress,
    usage: wgt::BufferUses,
    map_mode: BufferMapMode,
    cpu_readable: bool,
    cpu_writable: bool,
    mapped: AtomicBool,
}

impl Buffer {
    pub(crate) fn new(
        raw: Direct3D11::ID3D11Buffer,
        size: wgt::BufferAddress,
        usage: wgt::BufferUses,
        map_mode: BufferMapMode,
        cpu_readable: bool,
        cpu_writable: bool,
    ) -> Self {
        Self {
            raw,
            size,
            usage,
            map_mode,
            cpu_readable,
            cpu_writable,
            mapped: AtomicBool::new(false),
        }
    }

    /// Size requested by the caller, excluding any D3D11 alignment padding.
    pub fn size(&self) -> wgt::BufferAddress {
        self.size
    }

    pub fn usage(&self) -> wgt::BufferUses {
        self.usage
    }

    pub(crate) fn map_mode(&self) -> BufferMapMode {
        self.map_mode
    }

    pub(crate) fn cpu_readable(&self) -> bool {
        self.cpu_readable
    }

    pub(crate) fn cpu_writable(&self) -> bool {
        self.cpu_writable
    }

    pub(crate) fn set_mapped(&self) -> bool {
        !self.mapped.swap(true, Ordering::AcqRel)
    }

    pub(crate) fn set_unmapped(&self) -> bool {
        self.mapped.swap(false, Ordering::AcqRel)
    }

    pub(crate) fn raw(&self) -> &Direct3D11::ID3D11Buffer {
        &self.raw
    }

    pub(crate) fn clone_for_binding(&self) -> Self {
        Self {
            raw: self.raw.clone(),
            size: self.size,
            usage: self.usage,
            map_mode: self.map_mode,
            cpu_readable: self.cpu_readable,
            cpu_writable: self.cpu_writable,
            mapped: AtomicBool::new(false),
        }
    }
}

impl Borrow<dyn crate::DynBuffer> for Buffer {
    fn borrow(&self) -> &dyn crate::DynBuffer {
        self
    }
}

/// A two-dimensional D3D11 texture (including array textures).
#[derive(Clone, Debug)]
pub struct Texture {
    raw: Direct3D11::ID3D11Texture2D,
    format: wgt::TextureFormat,
    extent: wgt::Extent3d,
    mip_level_count: u32,
    array_layer_count: u32,
    sample_count: u32,
    usage: wgt::TextureUses,
}

impl Texture {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        raw: Direct3D11::ID3D11Texture2D,
        format: wgt::TextureFormat,
        extent: wgt::Extent3d,
        mip_level_count: u32,
        array_layer_count: u32,
        sample_count: u32,
        usage: wgt::TextureUses,
    ) -> Self {
        Self {
            raw,
            format,
            extent,
            mip_level_count,
            array_layer_count,
            sample_count,
            usage,
        }
    }

    pub fn format(&self) -> wgt::TextureFormat {
        self.format
    }

    pub fn extent(&self) -> wgt::Extent3d {
        self.extent
    }

    pub fn mip_level_count(&self) -> u32 {
        self.mip_level_count
    }

    pub fn array_layer_count(&self) -> u32 {
        self.array_layer_count
    }

    pub fn sample_count(&self) -> u32 {
        self.sample_count
    }

    pub fn usage(&self) -> wgt::TextureUses {
        self.usage
    }

    /// Computes the D3D11 subresource index for a 2D array texture.
    pub(crate) fn subresource_index(&self, mip_level: u32, array_layer: u32) -> u32 {
        mip_level + array_layer * self.mip_level_count
    }

    pub(crate) fn raw(&self) -> &Direct3D11::ID3D11Texture2D {
        &self.raw
    }
}

impl Borrow<dyn crate::DynTexture> for Texture {
    fn borrow(&self) -> &dyn crate::DynTexture {
        self
    }
}

/// A view over a texture.
///
/// A single descriptor can ask for more than one kind of view, so the wrapper
/// keeps an optional SRV, RTV and DSV. Swap-chain views only have an RTV; that
/// is still the common case used by the window example.
#[derive(Clone, Debug)]
pub struct TextureView {
    srv: Option<Direct3D11::ID3D11ShaderResourceView>,
    rtv: Option<Direct3D11::ID3D11RenderTargetView>,
    dsv: Option<Direct3D11::ID3D11DepthStencilView>,
    format: Dxgi::Common::DXGI_FORMAT,
}

impl TextureView {
    pub(crate) fn from_parts(
        srv: Option<Direct3D11::ID3D11ShaderResourceView>,
        rtv: Option<Direct3D11::ID3D11RenderTargetView>,
        dsv: Option<Direct3D11::ID3D11DepthStencilView>,
        format: Dxgi::Common::DXGI_FORMAT,
    ) -> Self {
        Self {
            srv,
            rtv,
            dsv,
            format,
        }
    }

    /// Creates an RTV for a 2D texture with the given view format.
    ///
    /// `format` may be `None` to use the texture's own format.
    pub(crate) fn create_render_target(
        device: &Direct3D11::ID3D11Device,
        texture: &Direct3D11::ID3D11Texture2D,
        format: Option<Dxgi::Common::DXGI_FORMAT>,
    ) -> windows::core::Result<Self> {
        let mut texture_desc = Direct3D11::D3D11_TEXTURE2D_DESC::default();
        unsafe { texture.GetDesc(&mut texture_desc) };
        let view_format = format.unwrap_or(texture_desc.Format);

        let desc = Direct3D11::D3D11_RENDER_TARGET_VIEW_DESC {
            Format: view_format,
            ViewDimension: Direct3D11::D3D11_RTV_DIMENSION_TEXTURE2D,
            Anonymous: Direct3D11::D3D11_RENDER_TARGET_VIEW_DESC_0 {
                Texture2D: Direct3D11::D3D11_TEX2D_RTV { MipSlice: 0 },
            },
        };

        let mut raw: Option<Direct3D11::ID3D11RenderTargetView> = None;
        unsafe { device.CreateRenderTargetView(texture, Some(&desc), Some(&mut raw)) }?;

        raw.map(|rtv| Self::from_parts(None, Some(rtv), None, view_format))
            .ok_or_else(|| windows::core::Error::from_hresult(Foundation::E_UNEXPECTED))
    }

    /// Returns the underlying D3D11 render target view.
    ///
    /// # Panics
    ///
    /// Panics if this view was not created with `COLOR_TARGET` usage. Render
    /// pass clears are the only caller and always use render-target views.
    pub fn raw(&self) -> &Direct3D11::ID3D11RenderTargetView {
        self.rtv
            .as_ref()
            .expect("D3D11 texture view is not a render target view")
    }

    pub fn raw_srv(&self) -> Option<&Direct3D11::ID3D11ShaderResourceView> {
        self.srv.as_ref()
    }

    pub fn raw_rtv(&self) -> Option<&Direct3D11::ID3D11RenderTargetView> {
        self.rtv.as_ref()
    }

    pub fn raw_dsv(&self) -> Option<&Direct3D11::ID3D11DepthStencilView> {
        self.dsv.as_ref()
    }

    pub fn format(&self) -> Dxgi::Common::DXGI_FORMAT {
        self.format
    }
}

/// A D3D11 sampler state.
#[derive(Clone, Debug)]
pub struct Sampler {
    raw: Direct3D11::ID3D11SamplerState,
}

impl Sampler {
    pub(crate) fn new(raw: Direct3D11::ID3D11SamplerState) -> Self {
        Self { raw }
    }

    #[allow(dead_code)]
    pub(crate) fn raw(&self) -> &Direct3D11::ID3D11SamplerState {
        &self.raw
    }
}

/// A normalized entry in a [`BindGroupLayout`].
#[derive(Clone, Copy, Debug)]
pub struct BindGroupLayoutEntry {
    pub binding: u32,
    pub visibility: wgt::ShaderStages,
    pub ty: wgt::BindingType,
    pub count: u32,
}

/// CPU-side description of a bind group layout.
///
/// D3D11 has no descriptor sets, so this only records the normalized entries
/// and the bindings that consume dynamic offsets. Registers are assigned when
/// a [`PipelineLayout`] combines one or more groups.
#[derive(Clone, Debug, Default)]
pub struct BindGroupLayout {
    entries: Vec<BindGroupLayoutEntry>,
    dynamic_offsets: Vec<u32>,
}

impl BindGroupLayout {
    pub(crate) fn new(entries: &[wgt::BindGroupLayoutEntry]) -> Result<Self, crate::DeviceError> {
        let mut normalized = Vec::with_capacity(entries.len());
        let mut dynamic_offsets = Vec::new();
        let mut previous = None;

        for entry in entries {
            if previous.is_some_and(|binding| binding >= entry.binding) {
                log::error!("D3D11 bind group layout entries must be sorted by binding");
                return Err(crate::DeviceError::Unexpected);
            }
            previous = Some(entry.binding);

            match entry.ty {
                wgt::BindingType::AccelerationStructure { .. }
                | wgt::BindingType::ExternalTexture => {
                    log::error!("D3D11 does not support this binding type: {:?}", entry.ty);
                    return Err(crate::DeviceError::Unexpected);
                }
                _ => {}
            }

            if entry.ty.has_dynamic_offset() {
                dynamic_offsets.push(entry.binding);
            }

            normalized.push(BindGroupLayoutEntry {
                binding: entry.binding,
                visibility: entry.visibility,
                ty: entry.ty,
                count: entry.count.map_or(1, core::num::NonZeroU32::get),
            });
        }

        Ok(Self {
            entries: normalized,
            dynamic_offsets,
        })
    }

    pub fn entries(&self) -> &[BindGroupLayoutEntry] {
        &self.entries
    }

    /// Binding indices that consume a dynamic offset, in layout order.
    pub fn dynamic_offsets(&self) -> &[u32] {
        &self.dynamic_offsets
    }

    pub fn entry(&self, binding: u32) -> Option<&BindGroupLayoutEntry> {
        self.entries.iter().find(|entry| entry.binding == binding)
    }
}

/// CPU-side pipeline layout.
///
/// D3D11 does not have root signatures. Registers are flattened across all
/// bind groups into independent `b`, `t`, `u` and `s` counter spaces, with
/// register space always set to zero.
#[derive(Debug)]
pub struct PipelineLayout {
    bind_group_layouts: Vec<Option<BindGroupLayout>>,
    immediate_size: u32,
    naga_options: naga::back::hlsl::Options,
}

impl PipelineLayout {
    pub(crate) fn new(
        bind_group_layouts: Vec<Option<BindGroupLayout>>,
        immediate_size: u32,
    ) -> Result<Self, crate::DeviceError> {
        if bind_group_layouts.len() > crate::MAX_BIND_GROUPS {
            log::error!(
                "D3D11 pipeline layout has {} bind groups, at most {} are supported",
                bind_group_layouts.len(),
                crate::MAX_BIND_GROUPS
            );
            return Err(crate::DeviceError::Unexpected);
        }
        if immediate_size % 4 != 0 || immediate_size > 65_536 {
            log::error!(
                "D3D11 immediate data must be a multiple of 4 bytes and at most 65536 bytes"
            );
            return Err(crate::DeviceError::Unexpected);
        }

        use naga::back::hlsl;

        let mut binding_map = hlsl::BindingMap::default();
        let mut next_b = 0u32;
        let mut next_t = 0u32;
        let mut next_u = 0u32;
        let mut next_s = 0u32;

        for (group, layout) in bind_group_layouts.iter().enumerate() {
            let Some(layout) = layout else {
                continue;
            };

            for entry in &layout.entries {
                let register = match entry.ty {
                    wgt::BindingType::Buffer {
                        ty: wgt::BufferBindingType::Uniform,
                        ..
                    } => {
                        let register = next_b;
                        next_b += entry.count;
                        register
                    }
                    wgt::BindingType::Buffer {
                        ty: wgt::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset,
                        ..
                    } => {
                        if has_dynamic_offset {
                            log::error!(
                                "D3D11 dynamic storage buffer offsets are not implemented yet"
                            );
                            return Err(crate::DeviceError::Unexpected);
                        }
                        let register = next_t;
                        next_t += entry.count;
                        register
                    }
                    wgt::BindingType::Buffer {
                        ty: wgt::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset,
                        ..
                    } => {
                        if has_dynamic_offset {
                            log::error!(
                                "D3D11 dynamic storage buffer offsets are not implemented yet"
                            );
                            return Err(crate::DeviceError::Unexpected);
                        }
                        let register = next_u;
                        next_u += entry.count;
                        register
                    }
                    wgt::BindingType::Texture { .. } => {
                        let register = next_t;
                        next_t += entry.count;
                        register
                    }
                    wgt::BindingType::StorageTexture { .. } => {
                        let register = next_u;
                        next_u += entry.count;
                        register
                    }
                    wgt::BindingType::Sampler(_) => {
                        if entry.count > 1 {
                            log::error!(
                                "D3D11 direct sampler binding mode does not support sampler arrays"
                            );
                            return Err(crate::DeviceError::Unexpected);
                        }
                        let register = next_s;
                        next_s += 1;
                        register
                    }
                    wgt::BindingType::AccelerationStructure { .. }
                    | wgt::BindingType::ExternalTexture => {
                        log::error!("D3D11 does not support this binding type: {:?}", entry.ty);
                        return Err(crate::DeviceError::Unexpected);
                    }
                };

                binding_map.insert(
                    naga::ResourceBinding {
                        group: group as u32,
                        binding: entry.binding,
                    },
                    hlsl::BindTarget {
                        space: 0,
                        register,
                        binding_array_size: (entry.count > 1).then_some(entry.count),
                        dynamic_storage_buffer_offsets_index: None,
                        restrict_indexing: entry.count > 1,
                    },
                );
            }
        }

        let immediates_target = (immediate_size != 0).then_some(hlsl::BindTarget {
            space: 0,
            register: next_b,
            binding_array_size: None,
            dynamic_storage_buffer_offsets_index: None,
            restrict_indexing: false,
        });

        let naga_options = hlsl::Options {
            shader_model: hlsl::ShaderModel::V5_0,
            binding_map,
            fake_missing_bindings: false,
            special_constants_binding: None,
            immediates_target,
            sampler_binding_mode: hlsl::SamplerBindingMode::Direct,
            ..Default::default()
        };

        Ok(Self {
            bind_group_layouts,
            immediate_size,
            naga_options,
        })
    }

    pub fn bind_group_layouts(&self) -> &[Option<BindGroupLayout>] {
        &self.bind_group_layouts
    }

    pub fn immediate_size(&self) -> u32 {
        self.immediate_size
    }

    pub fn naga_options(&self) -> &naga::back::hlsl::Options {
        &self.naga_options
    }

    pub fn bind_target(&self, group: u32, binding: u32) -> Option<naga::back::hlsl::BindTarget> {
        self.naga_options
            .binding_map
            .get(&naga::ResourceBinding { group, binding })
            .copied()
    }
}

#[derive(Debug)]
pub struct BoundBuffer {
    pub binding: u32,
    pub buffer: Buffer,
    pub offset: u64,
    pub size: Option<u64>,
    pub dynamic_offset: bool,
}

#[derive(Debug)]
pub struct BoundSampler {
    pub binding: u32,
    pub sampler: Sampler,
}

#[derive(Debug)]
pub struct BoundTexture {
    pub binding: u32,
    pub view: TextureView,
}

/// CPU-side bind group.
///
/// The group keeps the resources alive and records where each binding lives.
/// Binding the group to the immediate context is intentionally left to the
/// render/compute pipeline milestone.
#[derive(Debug)]
pub struct BindGroup {
    label: Option<String>,
    layout: BindGroupLayout,
    buffers: Vec<BoundBuffer>,
    samplers: Vec<BoundSampler>,
    textures: Vec<BoundTexture>,
    dynamic_offsets: Vec<u32>,
}

impl BindGroup {
    pub(crate) fn new(
        desc: &crate::BindGroupDescriptor<
            '_,
            BindGroupLayout,
            Buffer,
            Sampler,
            TextureView,
            Resource,
        >,
    ) -> Result<Self, crate::DeviceError> {
        if desc.entries.len() != desc.layout.entries.len() {
            log::error!("D3D11 bind group entries do not match the layout");
            return Err(crate::DeviceError::Unexpected);
        }

        let mut buffers = Vec::new();
        let mut samplers = Vec::new();
        let mut textures = Vec::new();

        for (entry, layout) in desc.entries.iter().zip(&desc.layout.entries) {
            if entry.binding != layout.binding || entry.count == 0 || entry.count > layout.count {
                log::error!(
                    "D3D11 bind group entry {} does not match the layout entry {}",
                    entry.binding,
                    layout.binding
                );
                return Err(crate::DeviceError::Unexpected);
            }

            let start = entry.resource_index as usize;
            let end = start
                .checked_add(entry.count as usize)
                .ok_or(crate::DeviceError::Unexpected)?;

            match layout.ty {
                wgt::BindingType::Buffer {
                    ty,
                    has_dynamic_offset,
                    ..
                } => {
                    let bindings = desc
                        .buffers
                        .get(start..end)
                        .ok_or(crate::DeviceError::Unexpected)?;
                    for binding in bindings {
                        let buffer_size = binding.buffer.size();
                        if binding.offset > buffer_size {
                            log::error!("D3D11 buffer binding offset is outside the buffer");
                            return Err(crate::DeviceError::Unexpected);
                        }
                        let size = binding
                            .size
                            .map_or(buffer_size - binding.offset, |size| size.get());
                        let end = binding
                            .offset
                            .checked_add(size)
                            .ok_or(crate::DeviceError::Unexpected)?;
                        if end > buffer_size {
                            log::error!("D3D11 buffer binding range is outside the buffer");
                            return Err(crate::DeviceError::Unexpected);
                        }

                        if ty == wgt::BufferBindingType::Uniform {
                            if binding.offset % 16 != 0
                                || size == 0
                                || size % 16 != 0
                                || size > 65_536
                            {
                                log::error!(
                                    "D3D11 uniform buffer bindings must be 16-byte aligned and at most 65536 bytes"
                                );
                                return Err(crate::DeviceError::Unexpected);
                            }
                        }

                        buffers.push(BoundBuffer {
                            binding: layout.binding,
                            buffer: binding.buffer.clone_for_binding(),
                            offset: binding.offset,
                            size: binding.size.map(|size| size.get()),
                            dynamic_offset: has_dynamic_offset,
                        });
                    }
                }
                wgt::BindingType::Sampler(_) => {
                    let bindings = desc
                        .samplers
                        .get(start..end)
                        .ok_or(crate::DeviceError::Unexpected)?;
                    for sampler in bindings {
                        samplers.push(BoundSampler {
                            binding: layout.binding,
                            sampler: (*sampler).clone(),
                        });
                    }
                }
                wgt::BindingType::Texture { .. } => {
                    let bindings = desc
                        .textures
                        .get(start..end)
                        .ok_or(crate::DeviceError::Unexpected)?;
                    for texture in bindings {
                        if !texture.usage.contains(wgt::TextureUses::RESOURCE) {
                            log::error!("D3D11 sampled texture bindings require RESOURCE usage");
                            return Err(crate::DeviceError::Unexpected);
                        }
                        textures.push(BoundTexture {
                            binding: layout.binding,
                            view: texture.view.clone(),
                        });
                    }
                }
                wgt::BindingType::StorageTexture { .. } => {
                    log::error!("D3D11 storage texture bindings are not implemented yet");
                    return Err(crate::DeviceError::Unexpected);
                }
                wgt::BindingType::AccelerationStructure { .. }
                | wgt::BindingType::ExternalTexture => {
                    log::error!("D3D11 does not support this binding type: {:?}", layout.ty);
                    return Err(crate::DeviceError::Unexpected);
                }
            }
        }

        Ok(Self {
            label: desc.label.map(String::from),
            layout: desc.layout.clone(),
            buffers,
            samplers,
            textures,
            dynamic_offsets: desc.layout.dynamic_offsets.clone(),
        })
    }

    pub fn label(&self) -> Option<&str> {
        self.label.as_deref()
    }

    pub fn layout(&self) -> &BindGroupLayout {
        &self.layout
    }

    pub fn buffers(&self) -> &[BoundBuffer] {
        &self.buffers
    }

    pub fn samplers(&self) -> &[BoundSampler] {
        &self.samplers
    }

    pub fn textures(&self) -> &[BoundTexture] {
        &self.textures
    }

    /// Binding indices that consume a dynamic offset, in layout order.
    pub fn dynamic_offsets(&self) -> &[u32] {
        &self.dynamic_offsets
    }
}

/// A swap-chain image acquired from [`crate::Surface::acquire_texture`].
///
/// Back-buffer textures are owned by DXGI, so this wrapper keeps both a
/// `Texture` identity for the current back buffer and the RTV that describes
/// it.
#[derive(Debug)]
pub struct SurfaceTexture {
    texture: Texture,
    view: TextureView,
}

impl SurfaceTexture {
    pub(crate) fn new(texture: Texture, view: TextureView) -> Self {
        Self { texture, view }
    }

    /// Returns the render target view for the acquired back buffer.
    pub fn view(&self) -> &TextureView {
        &self.view
    }
}

impl crate::DynTexture for SurfaceTexture {}

impl Borrow<Texture> for SurfaceTexture {
    fn borrow(&self) -> &Texture {
        &self.texture
    }
}

impl Borrow<dyn crate::DynTexture> for SurfaceTexture {
    fn borrow(&self) -> &dyn crate::DynTexture {
        &self.texture
    }
}

/// A CPU-visible fence value.
///
/// The real implementation will pair this with an `ID3D11Query`
/// (`D3D11_QUERY_EVENT`) so that [`crate::Device::wait`] can block on GPU
/// completion. For now the value is only updated by [`crate::Queue::submit`],
/// which is CPU-synchronous in this skeleton.
#[derive(Debug, Default)]
pub struct Fence {
    value: crate::AtomicFenceValue,
}

impl Fence {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn signal(&self, value: u64) {
        self.value.fetch_max(value, Ordering::Release);
    }

    pub(crate) fn value(&self) -> u64 {
        self.value.load(Ordering::Acquire)
    }
}

/// Dummy pipeline cache.
///
/// D3D11 has no API-level pipeline cache object; compiled shader blobs will
/// eventually be cached by `wgpu-core` or on disk.
#[derive(Debug, Default)]
pub struct PipelineCache;

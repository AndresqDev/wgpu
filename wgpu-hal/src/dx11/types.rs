use alloc::{format, string::String, vec::Vec};
use core::{
    borrow::Borrow,
    sync::atomic::{AtomicBool, Ordering},
};

use windows::Win32::{
    Foundation,
    Graphics::{Direct3D, Direct3D11, Dxgi},
};

/// D3D11 constant-buffer slots available per shader stage (`b0`..`b13`).
const D3D11_CONSTANT_BUFFER_SLOTS: u32 =
    Direct3D11::D3D11_COMMONSHADER_CONSTANT_BUFFER_API_SLOT_COUNT;
/// D3D11 shader-resource slots available per shader stage (`t0`..`t127`).
const D3D11_SRV_SLOTS: u32 = Direct3D11::D3D11_COMMONSHADER_INPUT_RESOURCE_SLOT_COUNT;
/// D3D11 sampler slots available per shader stage (`s0`..`s15`).
const D3D11_SAMPLER_SLOTS: u32 = Direct3D11::D3D11_COMMONSHADER_SAMPLER_SLOT_COUNT;

/// Binding counters for one shader stage, used to enforce the advertised
/// per-stage limits while constructing bind group and pipeline layouts.
#[derive(Clone, Copy, Debug, Default)]
struct StageBindingCounts {
    uniform_buffers: u32,
    sampled_textures: u32,
    samplers: u32,
}

/// Checks the per-stage binding counters against the advertised limits.
fn validate_stage_counts(
    counts: StageBindingCounts,
    limits: &wgt::Limits,
    stage: &str,
    context: &str,
) -> Result<(), crate::DeviceError> {
    if counts.uniform_buffers > limits.max_uniform_buffers_per_shader_stage {
        log::error!(
            "D3D11 {context} requests {} uniform buffers in the {stage} stage, at most {} are supported",
            counts.uniform_buffers,
            limits.max_uniform_buffers_per_shader_stage
        );
        return Err(crate::DeviceError::Unexpected);
    }
    if counts.sampled_textures > limits.max_sampled_textures_per_shader_stage {
        log::error!(
            "D3D11 {context} requests {} sampled textures in the {stage} stage, at most {} are supported",
            counts.sampled_textures,
            limits.max_sampled_textures_per_shader_stage
        );
        return Err(crate::DeviceError::Unexpected);
    }
    if counts.samplers > limits.max_samplers_per_shader_stage {
        log::error!(
            "D3D11 {context} requests {} samplers in the {stage} stage, at most {} are supported",
            counts.samplers,
            limits.max_samplers_per_shader_stage
        );
        return Err(crate::DeviceError::Unexpected);
    }
    Ok(())
}

/// Placeholder for every GPU resource type that is not implemented yet.
///
/// Buffers, textures, texture views, samplers, bind group objects and render
/// pipelines have concrete wrappers below. This type remains as the
/// placeholder used by compute pipelines, query sets and acceleration
/// structures until those milestones land.
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
#[derive(Clone, Copy, Debug, PartialEq)]
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
    limits: wgt::Limits,
}

impl BindGroupLayout {
    pub(crate) fn new(
        entries: &[wgt::BindGroupLayoutEntry],
        limits: &wgt::Limits,
    ) -> Result<Self, crate::DeviceError> {
        let mut normalized = Vec::with_capacity(entries.len());
        let mut dynamic_offsets = Vec::new();
        let mut previous = None;
        let mut vertex_counts = StageBindingCounts::default();
        let mut fragment_counts = StageBindingCounts::default();

        for entry in entries {
            if previous.is_some_and(|binding| binding >= entry.binding) {
                log::error!("D3D11 bind group layout entries must be sorted by binding");
                return Err(crate::DeviceError::Unexpected);
            }
            previous = Some(entry.binding);

            if entry.binding >= limits.max_bindings_per_bind_group {
                log::error!(
                    "D3D11 bind group layout binding {} is outside the advertised limit of {} bindings per group",
                    entry.binding,
                    limits.max_bindings_per_bind_group
                );
                return Err(crate::DeviceError::Unexpected);
            }

            // Compute shaders are not advertised: only vertex and fragment
            // bindings can ever be used.
            let allowed_visibility = wgt::ShaderStages::VERTEX | wgt::ShaderStages::FRAGMENT;
            if entry.visibility.is_empty() || !allowed_visibility.contains(entry.visibility) {
                log::error!(
                    "D3D11 bind group layout binding {} has visibility {:?}; only VERTEX and FRAGMENT are supported",
                    entry.binding,
                    entry.visibility
                );
                return Err(crate::DeviceError::Unexpected);
            }

            // `max_binding_array_*` is zero in the advertised contract:
            // D3D11 registers are flattened into single slots, so no kind of
            // binding array can be represented.
            let count = entry.count.map_or(1, core::num::NonZeroU32::get);
            if count > 1 {
                log::error!(
                    "D3D11 does not support binding arrays (binding {} has count {}); all `max_binding_array_*` limits are zero",
                    entry.binding,
                    count
                );
                return Err(crate::DeviceError::Unexpected);
            }

            match entry.ty {
                // No storage binding may be accepted and then silently dropped.
                // D3D11 without compute/writable-storage support has no way to
                // bind storage buffers or storage textures.
                wgt::BindingType::Buffer {
                    ty: wgt::BufferBindingType::Storage { .. },
                    has_dynamic_offset,
                    ..
                } => {
                    if has_dynamic_offset {
                        log::error!(
                            "D3D11 does not support dynamic storage buffer offsets (binding {})",
                            entry.binding
                        );
                    } else {
                        log::error!(
                            "D3D11 does not support storage buffers (binding {})",
                            entry.binding
                        );
                    }
                    return Err(crate::DeviceError::Unexpected);
                }
                wgt::BindingType::StorageTexture { .. } => {
                    log::error!(
                        "D3D11 does not support storage textures (binding {})",
                        entry.binding
                    );
                    return Err(crate::DeviceError::Unexpected);
                }
                wgt::BindingType::AccelerationStructure { .. }
                | wgt::BindingType::ExternalTexture => {
                    log::error!("D3D11 does not support this binding type: {:?}", entry.ty);
                    return Err(crate::DeviceError::Unexpected);
                }
                wgt::BindingType::Texture {
                    multisampled,
                    view_dimension,
                    ..
                } => {
                    if multisampled {
                        log::error!(
                            "D3D11 does not support multisampled textures (binding {})",
                            entry.binding
                        );
                        return Err(crate::DeviceError::Unexpected);
                    }
                    if matches!(
                        view_dimension,
                        wgt::TextureViewDimension::D1 | wgt::TextureViewDimension::D3
                    ) {
                        log::error!(
                            "D3D11 only supports D2, D2Array, Cube and CubeArray texture views (binding {} requests {view_dimension:?})",
                            entry.binding
                        );
                        return Err(crate::DeviceError::Unexpected);
                    }
                }
                wgt::BindingType::Buffer {
                    ty: wgt::BufferBindingType::Uniform,
                    min_binding_size: Some(min_binding_size),
                    ..
                } => {
                    if min_binding_size.get() > limits.max_uniform_buffer_binding_size {
                        log::error!(
                            "D3D11 uniform binding {} requires {} bytes, more than the advertised maximum of {}",
                            entry.binding,
                            min_binding_size.get(),
                            limits.max_uniform_buffer_binding_size
                        );
                        return Err(crate::DeviceError::Unexpected);
                    }
                }
                _ => {}
            }

            match entry.ty {
                wgt::BindingType::Buffer {
                    ty: wgt::BufferBindingType::Uniform,
                    ..
                } => {
                    if entry.visibility.contains(wgt::ShaderStages::VERTEX) {
                        vertex_counts.uniform_buffers += 1;
                    }
                    if entry.visibility.contains(wgt::ShaderStages::FRAGMENT) {
                        fragment_counts.uniform_buffers += 1;
                    }
                }
                wgt::BindingType::Texture { .. } => {
                    if entry.visibility.contains(wgt::ShaderStages::VERTEX) {
                        vertex_counts.sampled_textures += 1;
                    }
                    if entry.visibility.contains(wgt::ShaderStages::FRAGMENT) {
                        fragment_counts.sampled_textures += 1;
                    }
                }
                wgt::BindingType::Sampler(_) => {
                    if entry.visibility.contains(wgt::ShaderStages::VERTEX) {
                        vertex_counts.samplers += 1;
                    }
                    if entry.visibility.contains(wgt::ShaderStages::FRAGMENT) {
                        fragment_counts.samplers += 1;
                    }
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
                count,
            });
        }

        validate_stage_counts(vertex_counts, limits, "vertex", "bind group layout")?;
        validate_stage_counts(fragment_counts, limits, "fragment", "bind group layout")?;

        Ok(Self {
            entries: normalized,
            dynamic_offsets,
            limits: limits.clone(),
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

    /// Limits captured when the layout was created, used to validate bind
    /// groups without changing the public `Device::create_bind_group` API.
    pub(crate) fn limits(&self) -> &wgt::Limits {
        &self.limits
    }

    /// Whether two layouts describe the same bindings. The limit snapshot is
    /// deliberately excluded: two groups from the same device carry the same
    /// limits, and only the bindings determine register compatibility.
    pub(crate) fn same_layout(&self, other: &Self) -> bool {
        self.entries == other.entries && self.dynamic_offsets == other.dynamic_offsets
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
    limits: wgt::Limits,
    naga_options: naga::back::hlsl::Options,
}

impl PipelineLayout {
    pub(crate) fn new(
        bind_group_layouts: Vec<Option<BindGroupLayout>>,
        immediate_size: u32,
        limits: wgt::Limits,
        context1_available: bool,
    ) -> Result<Self, crate::DeviceError> {
        if bind_group_layouts.len() > limits.max_bind_groups as usize {
            log::error!(
                "D3D11 pipeline layout has {} bind groups, at most {} are supported",
                bind_group_layouts.len(),
                limits.max_bind_groups
            );
            return Err(crate::DeviceError::Unexpected);
        }
        if immediate_size > limits.max_immediate_size {
            // `max_immediate_size` is advertised as 0, so no accepted pipeline
            // layout may request immediates; otherwise `set_immediates` would be
            // reachable through a layout that cannot honour it.
            log::error!(
                "D3D11 does not support immediates, but the pipeline layout requests {immediate_size} bytes (maximum {})",
                limits.max_immediate_size
            );
            return Err(crate::DeviceError::Unexpected);
        }

        use naga::back::hlsl;

        let mut binding_map = hlsl::BindingMap::default();
        let mut next_b = 0u32;
        let mut next_t = 0u32;
        let mut next_s = 0u32;
        let mut vertex_counts = StageBindingCounts::default();
        let mut fragment_counts = StageBindingCounts::default();
        let mut dynamic_uniform_buffers = 0u32;

        // D3D11 registers are global per shader stage, not per bind group.
        // Naga/HLSL declares every module-level global in every entry point, so
        // registers must be unique across the entire pipeline layout; otherwise
        // two groups could claim the same `b`/`t`/`s` slot and the compiled
        // shader would silently alias them.
        for (group, layout) in bind_group_layouts.iter().enumerate() {
            let Some(layout) = layout else {
                continue;
            };

            for entry in &layout.entries {
                // Binding arrays are rejected by `BindGroupLayout::new`; keep a
                // defensive check here instead of reserving unreachable slots.
                if entry.count > 1 {
                    log::error!(
                        "D3D11 pipeline layout binding (group {group}, binding {}) has count {}; binding arrays are not supported",
                        entry.binding,
                        entry.count
                    );
                    return Err(crate::DeviceError::Unexpected);
                }

                let register = match entry.ty {
                    wgt::BindingType::Buffer {
                        ty: wgt::BufferBindingType::Uniform,
                        has_dynamic_offset,
                        ..
                    } => {
                        if has_dynamic_offset {
                            dynamic_uniform_buffers += 1;
                        }
                        if next_b >= D3D11_CONSTANT_BUFFER_SLOTS {
                            log::error!(
                                "D3D11 pipeline layout needs constant-buffer register b{next_b}, but only {D3D11_CONSTANT_BUFFER_SLOTS} slots exist per stage (group {group}, binding {})",
                                entry.binding
                            );
                            return Err(crate::DeviceError::Unexpected);
                        }
                        let register = next_b;
                        next_b += 1;
                        register
                    }
                    wgt::BindingType::Texture { .. } => {
                        if next_t >= D3D11_SRV_SLOTS {
                            log::error!(
                                "D3D11 pipeline layout needs shader resource register t{next_t}, but only {D3D11_SRV_SLOTS} slots exist per stage (group {group}, binding {})",
                                entry.binding
                            );
                            return Err(crate::DeviceError::Unexpected);
                        }
                        let register = next_t;
                        next_t += 1;
                        register
                    }
                    wgt::BindingType::Sampler(_) => {
                        if next_s >= D3D11_SAMPLER_SLOTS {
                            log::error!(
                                "D3D11 pipeline layout needs sampler register s{next_s}, but only {D3D11_SAMPLER_SLOTS} slots exist per stage (group {group}, binding {})",
                                entry.binding
                            );
                            return Err(crate::DeviceError::Unexpected);
                        }
                        let register = next_s;
                        next_s += 1;
                        register
                    }
                    wgt::BindingType::Buffer {
                        ty: wgt::BufferBindingType::Storage { .. },
                        ..
                    } => {
                        log::error!(
                            "D3D11 does not support storage buffers in a pipeline layout (group {group}, binding {})",
                            entry.binding
                        );
                        return Err(crate::DeviceError::Unexpected);
                    }
                    wgt::BindingType::StorageTexture { .. } => {
                        log::error!(
                            "D3D11 does not support storage textures in a pipeline layout (group {group}, binding {})",
                            entry.binding
                        );
                        return Err(crate::DeviceError::Unexpected);
                    }
                    wgt::BindingType::AccelerationStructure { .. }
                    | wgt::BindingType::ExternalTexture => {
                        log::error!(
                            "D3D11 does not support this binding type in a pipeline layout: {:?}",
                            entry.ty
                        );
                        return Err(crate::DeviceError::Unexpected);
                    }
                };

                match entry.ty {
                    wgt::BindingType::Buffer {
                        ty: wgt::BufferBindingType::Uniform,
                        ..
                    } => {
                        if entry.visibility.contains(wgt::ShaderStages::VERTEX) {
                            vertex_counts.uniform_buffers += 1;
                        }
                        if entry.visibility.contains(wgt::ShaderStages::FRAGMENT) {
                            fragment_counts.uniform_buffers += 1;
                        }
                    }
                    wgt::BindingType::Texture { .. } => {
                        if entry.visibility.contains(wgt::ShaderStages::VERTEX) {
                            vertex_counts.sampled_textures += 1;
                        }
                        if entry.visibility.contains(wgt::ShaderStages::FRAGMENT) {
                            fragment_counts.sampled_textures += 1;
                        }
                    }
                    wgt::BindingType::Sampler(_) => {
                        if entry.visibility.contains(wgt::ShaderStages::VERTEX) {
                            vertex_counts.samplers += 1;
                        }
                        if entry.visibility.contains(wgt::ShaderStages::FRAGMENT) {
                            fragment_counts.samplers += 1;
                        }
                    }
                    _ => {}
                }

                binding_map.insert(
                    naga::ResourceBinding {
                        group: group as u32,
                        binding: entry.binding,
                    },
                    hlsl::BindTarget {
                        space: 0,
                        register,
                        binding_array_size: None,
                        dynamic_storage_buffer_offsets_index: None,
                        restrict_indexing: false,
                    },
                );
            }
        }

        validate_stage_counts(vertex_counts, &limits, "vertex", "pipeline layout")?;
        validate_stage_counts(fragment_counts, &limits, "fragment", "pipeline layout")?;

        if dynamic_uniform_buffers > limits.max_dynamic_uniform_buffers_per_pipeline_layout {
            log::error!(
                "D3D11 pipeline layout has {dynamic_uniform_buffers} dynamic uniform buffers, at most {} are supported",
                limits.max_dynamic_uniform_buffers_per_pipeline_layout
            );
            return Err(crate::DeviceError::Unexpected);
        }
        if dynamic_uniform_buffers > 0 && !context1_available {
            // `ID3D11DeviceContext1` is required to express a non-zero
            // constant-buffer window. Degrading to the plain offset-0 path
            // would make shaders read the wrong constants.
            log::error!(
                "D3D11 pipeline layout has {dynamic_uniform_buffers} dynamic uniform buffers, but ID3D11DeviceContext1 is unavailable"
            );
            return Err(crate::DeviceError::Unexpected);
        }

        let naga_options = hlsl::Options {
            shader_model: hlsl::ShaderModel::V5_0,
            binding_map,
            fake_missing_bindings: false,
            special_constants_binding: None,
            immediates_target: None,
            sampler_binding_mode: hlsl::SamplerBindingMode::Direct,
            ..Default::default()
        };

        Ok(Self {
            bind_group_layouts,
            immediate_size,
            limits,
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

    /// Validates that `group` can be bound at `index` with `dynamic_offsets`.
    ///
    /// This checks the layout identity, the dynamic-offset count and order, the
    /// exact set of resources in the group, D3D11 register/slot ranges, uniform
    /// buffer ranges and the `ID3D11DeviceContext1` constant-buffer window
    /// constraints. It is public so the descriptor tests can exercise the same
    /// path `CommandEncoder::set_bind_group` uses.
    pub fn validate_bind_group(
        &self,
        index: u32,
        group: &BindGroup,
        dynamic_offsets: &[wgt::DynamicOffset],
        context1_available: bool,
    ) -> Result<(), String> {
        let group_layout = self
            .bind_group_layouts
            .get(index as usize)
            .and_then(Option::as_ref)
            .ok_or_else(|| format!("bind group {index} is not present in the pipeline layout"))?;

        if !group_layout.same_layout(group.layout()) {
            return Err(format!(
                "bind group {index} does not match the bind group layout at that index"
            ));
        }

        let mut expected_dynamic_offsets = Vec::new();
        for entry in group_layout.entries() {
            if entry.ty.has_dynamic_offset() {
                expected_dynamic_offsets.push(entry.binding);
            }
        }
        if group_layout.dynamic_offsets() != expected_dynamic_offsets.as_slice() {
            return Err(format!(
                "bind group {index} layout has an inconsistent dynamic offset order"
            ));
        }
        if dynamic_offsets.len() != expected_dynamic_offsets.len() {
            return Err(format!(
                "bind group {index} expects {} dynamic offsets, got {}",
                expected_dynamic_offsets.len(),
                dynamic_offsets.len()
            ));
        }

        let resource_count =
            group.buffers().len() + group.samplers().len() + group.textures().len();
        if resource_count != group_layout.entries().len() {
            return Err(format!(
                "bind group {index} provides {resource_count} resources for {} layout entries",
                group_layout.entries().len()
            ));
        }

        for entry in group_layout.entries() {
            match entry.ty {
                wgt::BindingType::Buffer {
                    ty: wgt::BufferBindingType::Uniform,
                    has_dynamic_offset,
                    ..
                } => {
                    let buffer = group
                        .buffers()
                        .iter()
                        .find(|buffer| buffer.binding == entry.binding)
                        .ok_or_else(|| {
                            format!(
                                "bind group {index} binding {} is missing its uniform buffer",
                                entry.binding
                            )
                        })?;
                    if buffer.dynamic_offset != has_dynamic_offset {
                        return Err(format!(
                            "bind group {index} binding {} dynamic-offset state does not match the layout",
                            entry.binding
                        ));
                    }

                    let target = self.bind_target(index, entry.binding).ok_or_else(|| {
                        format!(
                            "bind group {index} binding {} has no flattened register",
                            entry.binding
                        )
                    })?;
                    if target.space != 0 {
                        return Err(format!(
                            "bind group {index} binding {} maps to register space {}, expected 0",
                            entry.binding, target.space
                        ));
                    }
                    if target.register >= D3D11_CONSTANT_BUFFER_SLOTS {
                        return Err(format!(
                            "bind group {index} binding {} maps to b{}, outside the {} constant-buffer slots",
                            entry.binding, target.register, D3D11_CONSTANT_BUFFER_SLOTS
                        ));
                    }

                    let dynamic_offset = if has_dynamic_offset {
                        let position = expected_dynamic_offsets
                            .iter()
                            .position(|binding| *binding == entry.binding)
                            .ok_or_else(|| {
                                format!(
                                    "bind group {index} binding {} has no dynamic offset slot",
                                    entry.binding
                                )
                            })?;
                        u64::from(dynamic_offsets[position])
                    } else {
                        0
                    };

                    let buffer_size = buffer.buffer.size();
                    let base_offset = buffer.offset;
                    if base_offset > buffer_size {
                        return Err(format!(
                            "bind group {index} binding {} base offset {base_offset} is outside the buffer of {buffer_size} bytes",
                            entry.binding
                        ));
                    }
                    let size = buffer
                        .size
                        .unwrap_or_else(|| buffer_size.saturating_sub(base_offset));
                    if base_offset % 16 != 0 {
                        return Err(format!(
                            "bind group {index} binding {} base offset {base_offset} is not 16-byte aligned",
                            entry.binding
                        ));
                    }
                    if size == 0 || size % 16 != 0 {
                        return Err(format!(
                            "bind group {index} binding {} size {size} is not a non-zero multiple of 16 bytes",
                            entry.binding
                        ));
                    }
                    if size > self.limits.max_uniform_buffer_binding_size {
                        return Err(format!(
                            "bind group {index} binding {} size {size} exceeds max_uniform_buffer_binding_size {}",
                            entry.binding, self.limits.max_uniform_buffer_binding_size
                        ));
                    }
                    if has_dynamic_offset {
                        let alignment =
                            u64::from(self.limits.min_uniform_buffer_offset_alignment.max(1));
                        if base_offset % alignment != 0 {
                            return Err(format!(
                                "bind group {index} binding {} base offset {base_offset} is not aligned to min_uniform_buffer_offset_alignment {alignment}",
                                entry.binding
                            ));
                        }
                    }

                    let total_offset =
                        base_offset.checked_add(dynamic_offset).ok_or_else(|| {
                            format!(
                                "bind group {index} binding {} offset overflowed",
                                entry.binding
                            )
                        })?;
                    if total_offset % 16 != 0 {
                        return Err(format!(
                            "bind group {index} binding {} total offset {total_offset} is not 16-byte aligned",
                            entry.binding
                        ));
                    }
                    let end = total_offset.checked_add(size).ok_or_else(|| {
                        format!(
                            "bind group {index} binding {} range overflowed",
                            entry.binding
                        )
                    })?;
                    if end > buffer_size {
                        return Err(format!(
                            "bind group {index} binding {} range {total_offset}..{end} is outside the buffer of {buffer_size} bytes",
                            entry.binding
                        ));
                    }

                    if total_offset != 0 {
                        if !context1_available {
                            return Err(format!(
                                "bind group {index} binding {} needs offset {total_offset}, but ID3D11DeviceContext1 is unavailable",
                                entry.binding
                            ));
                        }
                        if total_offset % 256 != 0 {
                            return Err(format!(
                                "bind group {index} binding {} total offset {total_offset} is not a multiple of 256 bytes",
                                entry.binding
                            ));
                        }
                        let first_constant = total_offset / 16;
                        let num_constants = size / 16;
                        if first_constant.saturating_add(num_constants) > 4096 {
                            return Err(format!(
                                "bind group {index} binding {} constant window {first_constant}..{} exceeds 4096 constants",
                                entry.binding,
                                first_constant.saturating_add(num_constants)
                            ));
                        }
                    }
                }
                wgt::BindingType::Texture { .. } => {
                    let texture = group
                        .textures()
                        .iter()
                        .find(|texture| texture.binding == entry.binding)
                        .ok_or_else(|| {
                            format!(
                                "bind group {index} binding {} is missing its sampled texture",
                                entry.binding
                            )
                        })?;
                    let target = self.bind_target(index, entry.binding).ok_or_else(|| {
                        format!(
                            "bind group {index} binding {} has no flattened register",
                            entry.binding
                        )
                    })?;
                    if target.space != 0 {
                        return Err(format!(
                            "bind group {index} binding {} maps to register space {}, expected 0",
                            entry.binding, target.space
                        ));
                    }
                    if target.register >= D3D11_SRV_SLOTS {
                        return Err(format!(
                            "bind group {index} binding {} maps to t{}, outside the {} shader resource slots",
                            entry.binding, target.register, D3D11_SRV_SLOTS
                        ));
                    }
                    if texture.view.raw_srv().is_none() {
                        return Err(format!(
                            "bind group {index} binding {} has no shader resource view",
                            entry.binding
                        ));
                    }
                }
                wgt::BindingType::Sampler(_) => {
                    group
                        .samplers()
                        .iter()
                        .find(|sampler| sampler.binding == entry.binding)
                        .ok_or_else(|| {
                            format!(
                                "bind group {index} binding {} is missing its sampler",
                                entry.binding
                            )
                        })?;
                    let target = self.bind_target(index, entry.binding).ok_or_else(|| {
                        format!(
                            "bind group {index} binding {} has no flattened register",
                            entry.binding
                        )
                    })?;
                    if target.space != 0 {
                        return Err(format!(
                            "bind group {index} binding {} maps to register space {}, expected 0",
                            entry.binding, target.space
                        ));
                    }
                    if target.register >= D3D11_SAMPLER_SLOTS {
                        return Err(format!(
                            "bind group {index} binding {} maps to s{}, outside the {} sampler slots",
                            entry.binding, target.register, D3D11_SAMPLER_SLOTS
                        ));
                    }
                }
                _ => {
                    return Err(format!(
                        "bind group {index} binding {} has an unsupported binding type {:?}",
                        entry.binding, entry.ty
                    ));
                }
            }
        }

        Ok(())
    }
}

/// A D3D11 render pipeline.
///
/// D3D11 has no monolithic pipeline state object. This wrapper keeps the shader
/// objects, input layout and fixed-function state objects that
/// [`crate::Device::create_render_pipeline`] created. Binding the pipeline
/// applies them to the immediate context.
#[derive(Debug)]
pub struct RenderPipeline {
    label: Option<String>,
    vertex_shader: Direct3D11::ID3D11VertexShader,
    pixel_shader: Option<Direct3D11::ID3D11PixelShader>,
    input_layout: Option<Direct3D11::ID3D11InputLayout>,
    rasterizer_state: Direct3D11::ID3D11RasterizerState,
    depth_stencil_state: Direct3D11::ID3D11DepthStencilState,
    blend_state: Direct3D11::ID3D11BlendState,
    topology: Direct3D::D3D_PRIMITIVE_TOPOLOGY,
    vertex_strides: Vec<Option<u32>>,
    color_formats: Vec<Option<Dxgi::Common::DXGI_FORMAT>>,
    depth_format: Option<Dxgi::Common::DXGI_FORMAT>,
    sample_count: u32,
    sample_mask: u32,
}

impl RenderPipeline {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        label: Option<String>,
        vertex_shader: Direct3D11::ID3D11VertexShader,
        pixel_shader: Option<Direct3D11::ID3D11PixelShader>,
        input_layout: Option<Direct3D11::ID3D11InputLayout>,
        rasterizer_state: Direct3D11::ID3D11RasterizerState,
        depth_stencil_state: Direct3D11::ID3D11DepthStencilState,
        blend_state: Direct3D11::ID3D11BlendState,
        topology: Direct3D::D3D_PRIMITIVE_TOPOLOGY,
        vertex_strides: Vec<Option<u32>>,
        color_formats: Vec<Option<Dxgi::Common::DXGI_FORMAT>>,
        depth_format: Option<Dxgi::Common::DXGI_FORMAT>,
        sample_count: u32,
        sample_mask: u32,
    ) -> Self {
        Self {
            label,
            vertex_shader,
            pixel_shader,
            input_layout,
            rasterizer_state,
            depth_stencil_state,
            blend_state,
            topology,
            vertex_strides,
            color_formats,
            depth_format,
            sample_count,
            sample_mask,
        }
    }

    pub fn label(&self) -> Option<&str> {
        self.label.as_deref()
    }

    pub(crate) fn raw_vertex_shader(&self) -> &Direct3D11::ID3D11VertexShader {
        &self.vertex_shader
    }

    pub(crate) fn raw_pixel_shader(&self) -> Option<&Direct3D11::ID3D11PixelShader> {
        self.pixel_shader.as_ref()
    }

    pub(crate) fn raw_input_layout(&self) -> Option<&Direct3D11::ID3D11InputLayout> {
        self.input_layout.as_ref()
    }

    pub(crate) fn raw_rasterizer_state(&self) -> &Direct3D11::ID3D11RasterizerState {
        &self.rasterizer_state
    }

    pub(crate) fn raw_depth_stencil_state(&self) -> &Direct3D11::ID3D11DepthStencilState {
        &self.depth_stencil_state
    }

    pub(crate) fn raw_blend_state(&self) -> &Direct3D11::ID3D11BlendState {
        &self.blend_state
    }

    pub fn topology(&self) -> Direct3D::D3D_PRIMITIVE_TOPOLOGY {
        self.topology
    }

    pub fn vertex_strides(&self) -> &[Option<u32>] {
        &self.vertex_strides
    }

    pub fn color_formats(&self) -> &[Option<Dxgi::Common::DXGI_FORMAT>] {
        &self.color_formats
    }

    pub fn depth_format(&self) -> Option<Dxgi::Common::DXGI_FORMAT> {
        self.depth_format
    }

    pub fn sample_count(&self) -> u32 {
        self.sample_count
    }

    pub fn sample_mask(&self) -> u32 {
        self.sample_mask
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
/// [`crate::CommandEncoder::set_bind_group`] maps these bindings to the D3D11
/// registers assigned by the pipeline layout.
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

        let limits = desc.layout.limits();

        let mut buffers = Vec::new();
        let mut samplers = Vec::new();
        let mut textures = Vec::new();

        for (entry, layout) in desc.entries.iter().zip(&desc.layout.entries) {
            if entry.binding != layout.binding || entry.count != 1 || entry.count != layout.count {
                log::error!(
                    "D3D11 bind group entry {} (count {}) does not match the layout entry {} (count {})",
                    entry.binding,
                    entry.count,
                    layout.binding,
                    layout.count
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
                    min_binding_size,
                } => {
                    if ty != wgt::BufferBindingType::Uniform {
                        log::error!(
                            "D3D11 storage buffer bindings are not implemented yet (binding {})",
                            layout.binding
                        );
                        return Err(crate::DeviceError::Unexpected);
                    }

                    let bindings = desc
                        .buffers
                        .get(start..end)
                        .ok_or(crate::DeviceError::Unexpected)?;
                    for binding in bindings {
                        if !binding.buffer.usage().contains(wgt::BufferUses::UNIFORM) {
                            log::error!(
                                "D3D11 uniform buffer binding {} requires UNIFORM usage",
                                layout.binding
                            );
                            return Err(crate::DeviceError::Unexpected);
                        }

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

                        if binding.offset % 16 != 0 || size == 0 || size % 16 != 0 {
                            log::error!(
                                "D3D11 uniform buffer bindings must have a 16-byte aligned offset and a non-zero 16-byte aligned size"
                            );
                            return Err(crate::DeviceError::Unexpected);
                        }
                        if size > limits.max_uniform_buffer_binding_size {
                            log::error!(
                                "D3D11 uniform buffer binding size {size} exceeds max_uniform_buffer_binding_size {}",
                                limits.max_uniform_buffer_binding_size
                            );
                            return Err(crate::DeviceError::Unexpected);
                        }
                        if let Some(min_binding_size) = min_binding_size {
                            if size < min_binding_size.get() {
                                log::error!(
                                    "D3D11 uniform buffer binding size {size} is smaller than min_binding_size {}",
                                    min_binding_size.get()
                                );
                                return Err(crate::DeviceError::Unexpected);
                            }
                        }
                        if has_dynamic_offset {
                            let alignment =
                                u64::from(limits.min_uniform_buffer_offset_alignment.max(1));
                            if binding.offset % alignment != 0 {
                                log::error!(
                                    "D3D11 dynamic uniform buffer base offset {} is not aligned to {alignment}",
                                    binding.offset
                                );
                                return Err(crate::DeviceError::Unexpected);
                            }
                        }

                        buffers.push(BoundBuffer {
                            binding: layout.binding,
                            buffer: binding.buffer.clone_for_binding(),
                            offset: binding.offset,
                            // Record the effective size even when the caller
                            // omitted it, so bind-time validation always has an
                            // explicit range.
                            size: Some(size),
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

/// Dummy pipeline cache.
///
/// D3D11 has no API-level pipeline cache object; compiled shader blobs will
/// eventually be cached by `wgpu-core` or on disk.
#[derive(Debug, Default)]
pub struct PipelineCache;

#[cfg(test)]
mod tests {
    use super::*;

    fn test_limits() -> wgt::Limits {
        // These tests do not open a device; use the contract limits for the
        // 11_0 feature level instead.
        crate::dx11::conv::limits_for_feature_level(
            windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_11_0,
        )
    }

    fn uniform_entry(
        binding: u32,
        visibility: wgt::ShaderStages,
        has_dynamic_offset: bool,
    ) -> wgt::BindGroupLayoutEntry {
        wgt::BindGroupLayoutEntry {
            binding,
            visibility,
            ty: wgt::BindingType::Buffer {
                ty: wgt::BufferBindingType::Uniform,
                has_dynamic_offset,
                min_binding_size: None,
            },
            count: None,
        }
    }

    fn texture_entry(binding: u32, visibility: wgt::ShaderStages) -> wgt::BindGroupLayoutEntry {
        wgt::BindGroupLayoutEntry {
            binding,
            visibility,
            ty: wgt::BindingType::Texture {
                sample_type: wgt::TextureSampleType::Float { filterable: true },
                view_dimension: wgt::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        }
    }

    fn sampler_entry(binding: u32, visibility: wgt::ShaderStages) -> wgt::BindGroupLayoutEntry {
        wgt::BindGroupLayoutEntry {
            binding,
            visibility,
            ty: wgt::BindingType::Sampler(wgt::SamplerBindingType::Filtering),
            count: None,
        }
    }

    #[test]
    fn pipeline_layout_rejects_immediates_and_accepts_none() {
        let limits = test_limits();
        assert!(PipelineLayout::new(Vec::new(), 0, limits.clone(), false).is_ok());
        assert!(PipelineLayout::new(Vec::new(), 4, limits, false).is_err());
    }

    #[test]
    fn pipeline_layout_requires_context1_for_dynamic_uniforms() {
        let limits = test_limits();

        let dynamic_layout = BindGroupLayout::new(
            &[uniform_entry(0, wgt::ShaderStages::VERTEX, true)],
            &limits,
        )
        .expect("a dynamic uniform buffer is valid on its own");
        assert!(
            PipelineLayout::new(vec![Some(dynamic_layout.clone())], 0, limits.clone(), false,)
                .is_err(),
            "a dynamic uniform buffer must be rejected without ID3D11DeviceContext1"
        );
        assert!(
            PipelineLayout::new(vec![Some(dynamic_layout)], 0, limits.clone(), true).is_ok(),
            "a dynamic uniform buffer must be accepted with ID3D11DeviceContext1"
        );

        let static_layout = BindGroupLayout::new(
            &[uniform_entry(0, wgt::ShaderStages::VERTEX, false)],
            &limits,
        )
        .expect("a static uniform buffer is valid");
        assert!(
            PipelineLayout::new(vec![Some(static_layout)], 0, limits, false).is_ok(),
            "static uniform buffers must not require ID3D11DeviceContext1"
        );
    }

    #[test]
    fn bind_group_layout_enforces_per_stage_binding_limits() {
        let limits = test_limits();

        let seven_uniforms: Vec<_> = (0..7)
            .map(|binding| uniform_entry(binding, wgt::ShaderStages::VERTEX, false))
            .collect();
        assert!(BindGroupLayout::new(&seven_uniforms, &limits).is_ok());
        let eight_uniforms: Vec<_> = (0..8)
            .map(|binding| uniform_entry(binding, wgt::ShaderStages::VERTEX, false))
            .collect();
        assert!(BindGroupLayout::new(&eight_uniforms, &limits).is_err());

        // An entry visible to both stages counts against both stage limits.
        let seven_both: Vec<_> = (0..7)
            .map(|binding| uniform_entry(binding, wgt::ShaderStages::VERTEX_FRAGMENT, false))
            .collect();
        assert!(BindGroupLayout::new(&seven_both, &limits).is_ok());
        let eight_both: Vec<_> = (0..8)
            .map(|binding| uniform_entry(binding, wgt::ShaderStages::VERTEX_FRAGMENT, false))
            .collect();
        assert!(BindGroupLayout::new(&eight_both, &limits).is_err());

        let sixteen_textures: Vec<_> = (0..16)
            .map(|binding| texture_entry(binding, wgt::ShaderStages::FRAGMENT))
            .collect();
        assert!(BindGroupLayout::new(&sixteen_textures, &limits).is_ok());
        let seventeen_textures: Vec<_> = (0..17)
            .map(|binding| texture_entry(binding, wgt::ShaderStages::FRAGMENT))
            .collect();
        assert!(BindGroupLayout::new(&seventeen_textures, &limits).is_err());

        let eight_samplers: Vec<_> = (0..8)
            .map(|binding| sampler_entry(binding, wgt::ShaderStages::VERTEX))
            .collect();
        assert!(BindGroupLayout::new(&eight_samplers, &limits).is_ok());
        let nine_samplers: Vec<_> = (0..9)
            .map(|binding| sampler_entry(binding, wgt::ShaderStages::VERTEX))
            .collect();
        assert!(BindGroupLayout::new(&nine_samplers, &limits).is_err());
    }
}

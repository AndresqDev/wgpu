//! D3D11 device implementation.
//!
//! The device owns the `ID3D11Device` created by [`crate::Adapter::open`].
//! Buffers, 2D textures, texture views, samplers, bind groups, shader modules
//! and render pipelines are created here. Compute pipelines, query sets and
//! acceleration structures remain explicit stubs for later milestones.

use alloc::{
    ffi::CString,
    string::{String, ToString as _},
    vec::Vec,
};
use core::{fmt, ptr::NonNull};

use windows::{
    core::{Interface as _, PCSTR},
    Win32::Graphics::{Direct3D, Direct3D11, Dxgi},
};

use super::{
    debug::DebugLayer, Api, BindGroup, BindGroupLayout, Buffer, BufferMapMode, CommandBuffer,
    DebugMessage, Fence, PipelineCache, PipelineLayout, RenderPipeline, Resource, Sampler,
    ShaderModule, Texture, TextureView,
};

/// Rounds `value` up to a multiple of `alignment`, which must be a power of two.
fn align_to(value: u64, alignment: u64) -> u64 {
    value.div_ceil(alignment) * alignment
}

fn unexpected(message: &str) -> crate::DeviceError {
    log::error!("{message}");
    crate::DeviceError::Unexpected
}

fn map_buffer_bind_flags(usage: wgt::BufferUses) -> u32 {
    let mut bind_flags = 0u32;
    if usage.contains(wgt::BufferUses::VERTEX) {
        bind_flags |= Direct3D11::D3D11_BIND_VERTEX_BUFFER.0 as u32;
    }
    if usage.contains(wgt::BufferUses::INDEX) {
        bind_flags |= Direct3D11::D3D11_BIND_INDEX_BUFFER.0 as u32;
    }
    if usage.contains(wgt::BufferUses::UNIFORM) {
        bind_flags |= Direct3D11::D3D11_BIND_CONSTANT_BUFFER.0 as u32;
    }
    bind_flags
}

fn map_texture_bind_flags(usage: wgt::TextureUses) -> u32 {
    let mut bind_flags = 0u32;
    if usage.contains(wgt::TextureUses::RESOURCE) {
        bind_flags |= Direct3D11::D3D11_BIND_SHADER_RESOURCE.0 as u32;
    }
    if usage.contains(wgt::TextureUses::COLOR_TARGET) {
        bind_flags |= Direct3D11::D3D11_BIND_RENDER_TARGET.0 as u32;
    }
    if usage
        .intersects(wgt::TextureUses::DEPTH_STENCIL_READ | wgt::TextureUses::DEPTH_STENCIL_WRITE)
    {
        bind_flags |= Direct3D11::D3D11_BIND_DEPTH_STENCIL.0 as u32;
    }
    bind_flags
}

/// Returns whether a view format is compatible with the texture's base format.
fn texture_formats_compatible(texture: wgt::TextureFormat, view: wgt::TextureFormat) -> bool {
    if texture == view {
        return true;
    }

    matches!(
        (texture, view),
        (
            wgt::TextureFormat::Rgba8Unorm,
            wgt::TextureFormat::Rgba8UnormSrgb
        ) | (
            wgt::TextureFormat::Rgba8UnormSrgb,
            wgt::TextureFormat::Rgba8Unorm
        ) | (
            wgt::TextureFormat::Bgra8Unorm,
            wgt::TextureFormat::Bgra8UnormSrgb
        ) | (
            wgt::TextureFormat::Bgra8UnormSrgb,
            wgt::TextureFormat::Bgra8Unorm
        )
    )
}

/// An open D3D11 device.
pub struct Device {
    raw_device: Direct3D11::ID3D11Device,
    raw_context: Direct3D11::ID3D11DeviceContext,
    info: wgt::AdapterInfo,
    feature_level: Direct3D::D3D_FEATURE_LEVEL,
    limits: wgt::Limits,
    /// `Some` only when the device was created with
    /// `D3D11_CREATE_DEVICE_DEBUG` and exposes `ID3D11InfoQueue`.
    debug_layer: Option<DebugLayer>,
}

impl fmt::Debug for Device {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Device")
            .field("adapter", &self.info.name)
            .field("feature_level", &self.feature_level.0)
            .finish_non_exhaustive()
    }
}

impl Device {
    pub(crate) fn new(
        raw_device: Direct3D11::ID3D11Device,
        raw_context: Direct3D11::ID3D11DeviceContext,
        info: wgt::AdapterInfo,
        feature_level: Direct3D::D3D_FEATURE_LEVEL,
        limits: wgt::Limits,
    ) -> Self {
        // Probe unconditionally: only devices created with
        // `D3D11_CREATE_DEVICE_DEBUG` expose `ID3D11InfoQueue`, so this is
        // `None` on the normal path and costs a single interface query.
        let debug_layer = DebugLayer::new(&raw_device);

        Self {
            raw_device,
            raw_context,
            info,
            feature_level,
            limits,
            debug_layer,
        }
    }

    /// Returns the underlying D3D11 device.
    pub fn raw_device(&self) -> &Direct3D11::ID3D11Device {
        &self.raw_device
    }

    /// Returns the immediate context owned by this device.
    pub fn raw_context(&self) -> &Direct3D11::ID3D11DeviceContext {
        &self.raw_context
    }

    /// Returns the feature level used to create this device.
    pub fn feature_level(&self) -> Direct3D::D3D_FEATURE_LEVEL {
        self.feature_level
    }

    /// Returns the limits advertised by the adapter.
    pub fn limits(&self) -> &wgt::Limits {
        &self.limits
    }

    /// Returns whether this device exposes the D3D11 debug layer.
    pub fn debug_layer_available(&self) -> bool {
        self.debug_layer.is_some()
    }

    /// Drains and returns the messages stored by the D3D11 debug layer.
    ///
    /// Every message is also logged at the level implied by its severity. A
    /// device created without the debug layer has no message queue, so this
    /// returns an empty vector. The returned messages are drained exactly once:
    /// calling this again returns only messages emitted since the previous
    /// call.
    pub fn take_debug_messages(&self) -> Vec<DebugMessage> {
        let Some(debug_layer) = &self.debug_layer else {
            return Vec::new();
        };

        let messages = debug_layer.take_messages();
        for message in &messages {
            message.log();
        }
        messages
    }

    /// Returns whether `GetDeviceRemovedReason` reports that the device was
    /// removed, reset or hung.
    pub fn is_device_lost(&self) -> bool {
        // SAFETY: the device interface is valid for the lifetime of `self`.
        unsafe { self.raw_device.GetDeviceRemovedReason() }.is_err()
    }

    /// Returns the formatted device removal reason, or `None` for a healthy
    /// device.
    pub fn device_removed_reason(&self) -> Option<String> {
        // SAFETY: the device interface is valid for the lifetime of `self`.
        match unsafe { self.raw_device.GetDeviceRemovedReason() } {
            Ok(()) => None,
            Err(error) => Some(alloc::format!("{error}")),
        }
    }

    /// Checks `GetDeviceRemovedReason` and reports a removed device as
    /// [`crate::DeviceError::Lost`].
    ///
    /// This is the single classification point used by fence polling, waits
    /// and buffer mapping, so a removal discovered through an unrelated call is
    /// reported consistently as `Lost` rather than `Unexpected`.
    pub fn check_device_state(&self) -> Result<(), crate::DeviceError> {
        match self.device_removed_reason() {
            Some(reason) => {
                log::error!("the D3D11 device was removed: {reason}");
                Err(crate::DeviceError::Lost)
            }
            None => Ok(()),
        }
    }
}

/// Returns an error for an unsupported render pipeline feature.
fn pipeline_error(message: impl Into<alloc::string::String>) -> crate::PipelineError {
    let message = message.into();
    log::error!("{message}");
    crate::PipelineError::Linkage(wgt::ShaderStages::VERTEX_FRAGMENT, message)
}

/// Builds a D3D11 input layout from the WebGPU vertex buffer descriptions.
///
/// Naga's HLSL backend names vertex inputs `LOC0`, `LOC1`, ... so the input
/// layout uses semantic name `LOC` with the shader location as the semantic
/// index. Direct HLSL shader modules must follow the same convention.
fn create_input_layout(
    device: &Direct3D11::ID3D11Device,
    vertex_buffers: &[Option<crate::VertexBufferLayout<'_>>],
    vertex_bytecode: &[u8],
) -> Result<(Option<Direct3D11::ID3D11InputLayout>, Vec<Option<u32>>), crate::PipelineError> {
    if vertex_buffers.len() > Direct3D11::D3D11_IA_VERTEX_INPUT_RESOURCE_SLOT_COUNT as usize {
        return Err(pipeline_error(alloc::format!(
            "D3D11 render pipelines support at most {} vertex buffer slots, got {}",
            Direct3D11::D3D11_IA_VERTEX_INPUT_RESOURCE_SLOT_COUNT,
            vertex_buffers.len()
        )));
    }

    let semantic = CString::new("LOC")
        .map_err(|_| pipeline_error("failed to create the D3D11 input semantic string"))?;
    let mut elements: Vec<Direct3D11::D3D11_INPUT_ELEMENT_DESC> = Vec::new();
    let mut strides: Vec<Option<u32>> = Vec::new();
    strides.resize(vertex_buffers.len(), None);
    let mut locations: Vec<u32> = Vec::new();

    for (slot, layout) in vertex_buffers.iter().enumerate() {
        let Some(layout) = layout else {
            continue;
        };

        let stride = u32::try_from(layout.array_stride).map_err(|_| {
            pipeline_error(alloc::format!(
                "vertex buffer slot {slot} stride {} does not fit in u32",
                layout.array_stride
            ))
        })?;
        strides[slot] = Some(stride);

        let (input_slot_class, step_rate) = match layout.step_mode {
            wgt::VertexStepMode::Vertex => (Direct3D11::D3D11_INPUT_PER_VERTEX_DATA, 0),
            wgt::VertexStepMode::Instance => (Direct3D11::D3D11_INPUT_PER_INSTANCE_DATA, 1),
        };

        for attribute in layout.attributes {
            if locations.contains(&attribute.shader_location) {
                return Err(pipeline_error(alloc::format!(
                    "vertex shader location {} is declared more than once",
                    attribute.shader_location
                )));
            }
            locations.push(attribute.shader_location);

            let format = super::conv::map_vertex_format(attribute.format).ok_or_else(|| {
                pipeline_error(alloc::format!(
                    "vertex format {:?} is not supported by the D3D11 backend",
                    attribute.format
                ))
            })?;
            let offset = u32::try_from(attribute.offset).map_err(|_| {
                pipeline_error(alloc::format!(
                    "vertex attribute offset {} does not fit in u32",
                    attribute.offset
                ))
            })?;

            if layout.array_stride != 0
                && attribute.offset.saturating_add(attribute.format.size()) > layout.array_stride
            {
                return Err(pipeline_error(alloc::format!(
                    "vertex attribute at location {} extends past the stride of slot {slot}",
                    attribute.shader_location
                )));
            }

            elements.push(Direct3D11::D3D11_INPUT_ELEMENT_DESC {
                SemanticName: PCSTR(semantic.as_ptr().cast()),
                SemanticIndex: attribute.shader_location,
                Format: format,
                InputSlot: slot as u32,
                AlignedByteOffset: offset,
                InputSlotClass: input_slot_class,
                InstanceDataStepRate: step_rate,
            });
        }
    }

    if elements.is_empty() {
        return Ok((None, strides));
    }

    let mut raw: Option<Direct3D11::ID3D11InputLayout> = None;
    unsafe { device.CreateInputLayout(&elements, vertex_bytecode, Some(&mut raw)) }.map_err(
        |e| {
            pipeline_error(alloc::format!(
                "ID3D11Device::CreateInputLayout failed for {} elements: {e}",
                elements.len()
            ))
        },
    )?;

    let raw = raw.ok_or_else(|| {
        pipeline_error("ID3D11Device::CreateInputLayout succeeded without returning a layout")
    })?;

    Ok((Some(raw), strides))
}

/// Creates the rasterizer state for a render pipeline.
fn create_rasterizer_state(
    device: &Direct3D11::ID3D11Device,
    primitive: &wgt::PrimitiveState,
    depth_bias: wgt::DepthBiasState,
) -> Result<Direct3D11::ID3D11RasterizerState, crate::PipelineError> {
    if primitive.conservative {
        return Err(pipeline_error(
            "D3D11 conservative rasterization is not implemented yet",
        ));
    }

    let fill_mode = match primitive.polygon_mode {
        wgt::PolygonMode::Fill => Direct3D11::D3D11_FILL_SOLID,
        wgt::PolygonMode::Line => Direct3D11::D3D11_FILL_WIREFRAME,
        wgt::PolygonMode::Point => {
            return Err(pipeline_error(
                "D3D11 does not support point polygon fill mode; draw a point topology instead",
            ));
        }
    };
    let cull_mode = match primitive.cull_mode {
        None => Direct3D11::D3D11_CULL_NONE,
        Some(wgt::Face::Front) => Direct3D11::D3D11_CULL_FRONT,
        Some(wgt::Face::Back) => Direct3D11::D3D11_CULL_BACK,
    };

    let desc = Direct3D11::D3D11_RASTERIZER_DESC {
        FillMode: fill_mode,
        CullMode: cull_mode,
        FrontCounterClockwise: matches!(primitive.front_face, wgt::FrontFace::Ccw).into(),
        DepthBias: depth_bias.constant,
        DepthBiasClamp: depth_bias.clamp,
        SlopeScaledDepthBias: depth_bias.slope_scale,
        DepthClipEnable: (!primitive.unclipped_depth).into(),
        // Scissor rectangles are part of the WebGPU render pass state.
        ScissorEnable: true.into(),
        MultisampleEnable: false.into(),
        AntialiasedLineEnable: false.into(),
    };

    let mut raw: Option<Direct3D11::ID3D11RasterizerState> = None;
    unsafe { device.CreateRasterizerState(&desc, Some(&mut raw)) }.map_err(|e| {
        pipeline_error(alloc::format!(
            "ID3D11Device::CreateRasterizerState failed: {e}"
        ))
    })?;
    raw.ok_or_else(|| {
        pipeline_error("ID3D11Device::CreateRasterizerState returned no state object")
    })
}

fn default_stencil_face() -> Direct3D11::D3D11_DEPTH_STENCILOP_DESC {
    Direct3D11::D3D11_DEPTH_STENCILOP_DESC {
        StencilFailOp: Direct3D11::D3D11_STENCIL_OP_KEEP,
        StencilDepthFailOp: Direct3D11::D3D11_STENCIL_OP_KEEP,
        StencilPassOp: Direct3D11::D3D11_STENCIL_OP_KEEP,
        StencilFunc: Direct3D11::D3D11_COMPARISON_ALWAYS,
    }
}

/// Creates the depth/stencil state for a render pipeline.
///
/// Pipelines without a depth/stencil state still need a state object so that
/// binding them disables depth and stencil tests explicitly.
fn create_depth_stencil_state(
    device: &Direct3D11::ID3D11Device,
    depth_stencil: Option<&wgt::DepthStencilState>,
) -> Result<Direct3D11::ID3D11DepthStencilState, crate::PipelineError> {
    let desc = match depth_stencil {
        Some(depth_stencil) => {
            if !depth_stencil.format.has_depth_aspect()
                && !depth_stencil.format.has_stencil_aspect()
            {
                return Err(pipeline_error(alloc::format!(
                    "depth/stencil state format {:?} has neither a depth nor a stencil aspect",
                    depth_stencil.format
                )));
            }
            if depth_stencil.stencil.is_enabled() && !depth_stencil.format.has_stencil_aspect() {
                return Err(pipeline_error(alloc::format!(
                    "stencil state is enabled but format {:?} has no stencil aspect",
                    depth_stencil.format
                )));
            }

            Direct3D11::D3D11_DEPTH_STENCIL_DESC {
                DepthEnable: depth_stencil.is_depth_enabled().into(),
                DepthWriteMask: if depth_stencil.depth_write_enabled.unwrap_or(false) {
                    Direct3D11::D3D11_DEPTH_WRITE_MASK_ALL
                } else {
                    Direct3D11::D3D11_DEPTH_WRITE_MASK_ZERO
                },
                DepthFunc: super::conv::map_comparison(
                    depth_stencil
                        .depth_compare
                        .unwrap_or(wgt::CompareFunction::Always),
                ),
                StencilEnable: depth_stencil.stencil.is_enabled().into(),
                StencilReadMask: depth_stencil.stencil.read_mask as u8,
                StencilWriteMask: depth_stencil.stencil.write_mask as u8,
                FrontFace: super::conv::map_stencil_face(&depth_stencil.stencil.front),
                BackFace: super::conv::map_stencil_face(&depth_stencil.stencil.back),
            }
        }
        None => Direct3D11::D3D11_DEPTH_STENCIL_DESC {
            DepthEnable: false.into(),
            DepthWriteMask: Direct3D11::D3D11_DEPTH_WRITE_MASK_ZERO,
            DepthFunc: Direct3D11::D3D11_COMPARISON_ALWAYS,
            StencilEnable: false.into(),
            StencilReadMask: 0,
            StencilWriteMask: 0,
            FrontFace: default_stencil_face(),
            BackFace: default_stencil_face(),
        },
    };

    let mut raw: Option<Direct3D11::ID3D11DepthStencilState> = None;
    unsafe { device.CreateDepthStencilState(&desc, Some(&mut raw)) }.map_err(|e| {
        pipeline_error(alloc::format!(
            "ID3D11Device::CreateDepthStencilState failed: {e}"
        ))
    })?;
    raw.ok_or_else(|| {
        pipeline_error("ID3D11Device::CreateDepthStencilState returned no state object")
    })
}

/// Creates the blend state for a render pipeline.
fn create_blend_state(
    device: &Direct3D11::ID3D11Device,
    color_targets: &[Option<wgt::ColorTargetState>],
) -> Result<Direct3D11::ID3D11BlendState, crate::PipelineError> {
    if color_targets.len() > Direct3D11::D3D11_SIMULTANEOUS_RENDER_TARGET_COUNT as usize {
        return Err(pipeline_error(alloc::format!(
            "D3D11 render pipelines support at most {} color targets, got {}",
            Direct3D11::D3D11_SIMULTANEOUS_RENDER_TARGET_COUNT,
            color_targets.len()
        )));
    }

    let disabled = Direct3D11::D3D11_RENDER_TARGET_BLEND_DESC {
        BlendEnable: false.into(),
        SrcBlend: Direct3D11::D3D11_BLEND_ONE,
        DestBlend: Direct3D11::D3D11_BLEND_ZERO,
        BlendOp: Direct3D11::D3D11_BLEND_OP_ADD,
        SrcBlendAlpha: Direct3D11::D3D11_BLEND_ONE,
        DestBlendAlpha: Direct3D11::D3D11_BLEND_ZERO,
        BlendOpAlpha: Direct3D11::D3D11_BLEND_OP_ADD,
        RenderTargetWriteMask: 0,
    };
    let mut desc = Direct3D11::D3D11_BLEND_DESC {
        AlphaToCoverageEnable: false.into(),
        IndependentBlendEnable: true.into(),
        RenderTarget: [disabled; Direct3D11::D3D11_SIMULTANEOUS_RENDER_TARGET_COUNT as usize],
    };

    for (index, target) in color_targets.iter().enumerate() {
        let Some(target) = target else {
            continue;
        };

        let raw = &mut desc.RenderTarget[index];
        raw.RenderTargetWriteMask = (target.write_mask.bits() & 0xF) as u8;

        if let Some(blend) = &target.blend {
            raw.BlendEnable = true.into();
            raw.BlendOp = super::conv::map_blend_operation(blend.color.operation);
            raw.SrcBlend = super::conv::map_blend_factor(blend.color.src_factor, false)
                .ok_or_else(|| {
                    pipeline_error("D3D11 dual-source blending is not implemented yet")
                })?;
            raw.DestBlend = super::conv::map_blend_factor(blend.color.dst_factor, false)
                .ok_or_else(|| {
                    pipeline_error("D3D11 dual-source blending is not implemented yet")
                })?;
            raw.BlendOpAlpha = super::conv::map_blend_operation(blend.alpha.operation);
            raw.SrcBlendAlpha = super::conv::map_blend_factor(blend.alpha.src_factor, true)
                .ok_or_else(|| {
                    pipeline_error("D3D11 dual-source blending is not implemented yet")
                })?;
            raw.DestBlendAlpha = super::conv::map_blend_factor(blend.alpha.dst_factor, true)
                .ok_or_else(|| {
                    pipeline_error("D3D11 dual-source blending is not implemented yet")
                })?;
        }
    }

    let mut raw: Option<Direct3D11::ID3D11BlendState> = None;
    unsafe { device.CreateBlendState(&desc, Some(&mut raw)) }.map_err(|e| {
        pipeline_error(alloc::format!("ID3D11Device::CreateBlendState failed: {e}"))
    })?;
    raw.ok_or_else(|| pipeline_error("ID3D11Device::CreateBlendState returned no state object"))
}

impl crate::Device for Device {
    type A = Api;

    unsafe fn create_buffer(
        &self,
        desc: &crate::BufferDescriptor,
    ) -> Result<Buffer, crate::DeviceError> {
        let usage = desc.usage;

        // Storage buffers are not implemented: there is no UAV binding and no
        // compute/writable-storage path. Rejecting here keeps the creation
        // path from advertising a bind flag it can never honour.
        if usage
            .intersects(wgt::BufferUses::STORAGE_READ_ONLY | wgt::BufferUses::STORAGE_READ_WRITE)
        {
            return Err(unexpected("D3D11 storage buffers are not implemented yet"));
        }

        // D3D11 cannot create a buffer that is both CPU accessible and an
        // indirect-arguments buffer: `D3D11_RESOURCE_MISC_DRAWINDIRECT_ARGS`
        // is only valid on non-staging, non-dynamic resources.
        if usage.contains(wgt::BufferUses::INDIRECT)
            && usage.intersects(wgt::BufferUses::MAP_READ | wgt::BufferUses::MAP_WRITE)
        {
            return Err(unexpected(
                "D3D11 indirect buffers cannot be mapped on the CPU",
            ));
        }

        let bind_flags = map_buffer_bind_flags(usage);

        // D3D11 requires a constant buffer to be separate from resources that
        // are bound to other pipeline stages. wgpu-core normally creates
        // uniform buffers with only COPY_DST in addition to UNIFORM, so reject
        // the uncommon mixed case explicitly instead of silently dropping one
        // of the bindings.
        let constant_buffer = Direct3D11::D3D11_BIND_CONSTANT_BUFFER.0 as u32;
        if bind_flags & constant_buffer != 0 && bind_flags != constant_buffer {
            return Err(unexpected(
                "D3D11 does not support combining UNIFORM with other buffer bind usages yet",
            ));
        }

        if desc.size > u32::MAX as u64 {
            return Err(unexpected("D3D11 buffers cannot exceed u32::MAX bytes"));
        }

        let (map_mode, d3d_usage, cpu_access_flags) = if usage.contains(wgt::BufferUses::MAP_READ) {
            if bind_flags != 0 {
                return Err(unexpected(
                    "D3D11 read-back buffers must not be bound to a pipeline stage",
                ));
            }
            let mut access = Direct3D11::D3D11_CPU_ACCESS_READ.0;
            if usage.contains(wgt::BufferUses::MAP_WRITE) {
                access |= Direct3D11::D3D11_CPU_ACCESS_WRITE.0;
            }
            (
                BufferMapMode::Staging,
                Direct3D11::D3D11_USAGE_STAGING,
                access as u32,
            )
        } else if usage.contains(wgt::BufferUses::MAP_WRITE) {
            if bind_flags == 0 {
                (
                    BufferMapMode::Staging,
                    Direct3D11::D3D11_USAGE_STAGING,
                    (Direct3D11::D3D11_CPU_ACCESS_READ.0 | Direct3D11::D3D11_CPU_ACCESS_WRITE.0)
                        as u32,
                )
            } else {
                (
                    BufferMapMode::Dynamic,
                    Direct3D11::D3D11_USAGE_DYNAMIC,
                    Direct3D11::D3D11_CPU_ACCESS_WRITE.0 as u32,
                )
            }
        } else {
            (BufferMapMode::GpuOnly, Direct3D11::D3D11_USAGE_DEFAULT, 0)
        };

        let alignment = if usage.contains(wgt::BufferUses::UNIFORM) {
            16
        } else {
            4
        };
        let byte_width = align_to(desc.size.max(1), alignment);
        if byte_width > u32::MAX as u64 {
            return Err(unexpected(
                "D3D11 buffers cannot exceed u32::MAX bytes after alignment",
            ));
        }
        if usage.contains(wgt::BufferUses::UNIFORM) && byte_width > 65_536 {
            return Err(unexpected(
                "D3D11 constant buffers cannot exceed 65536 bytes",
            ));
        }

        let mut misc_flags = 0u32;
        if usage.contains(wgt::BufferUses::INDIRECT) {
            misc_flags |= Direct3D11::D3D11_RESOURCE_MISC_DRAWINDIRECT_ARGS.0 as u32;
        }

        let buffer_desc = Direct3D11::D3D11_BUFFER_DESC {
            ByteWidth: byte_width as u32,
            Usage: d3d_usage,
            BindFlags: bind_flags,
            CPUAccessFlags: cpu_access_flags,
            MiscFlags: misc_flags,
            StructureByteStride: 0,
        };

        // This version of `wgpu-hal` does not carry initial buffer data in
        // `BufferDescriptor`, so `D3D11_SUBRESOURCE_DATA` is not used here.
        let mut raw: Option<Direct3D11::ID3D11Buffer> = None;
        unsafe {
            self.raw_device
                .CreateBuffer(&buffer_desc, None, Some(&mut raw))
        }
        .map_err(|e| {
            log::error!(
                "ID3D11Device::CreateBuffer failed (size {byte_width}, usage {usage:?}): {e}"
            );
            if self.is_device_lost() {
                crate::DeviceError::Lost
            } else {
                crate::DeviceError::Unexpected
            }
        })?;

        let raw = raw.ok_or_else(|| {
            unexpected("ID3D11Device::CreateBuffer succeeded without returning a buffer")
        })?;

        log::debug!(
            "created D3D11 buffer: {} bytes (aligned to {byte_width}), usage {usage:?}",
            desc.size
        );

        Ok(Buffer::new(
            raw,
            desc.size,
            usage,
            map_mode,
            usage.contains(wgt::BufferUses::MAP_READ),
            usage.contains(wgt::BufferUses::MAP_WRITE),
        ))
    }

    unsafe fn destroy_buffer(&self, buffer: Buffer) {
        drop(buffer);
    }

    unsafe fn add_raw_buffer(&self, _buffer: &Buffer) {}

    unsafe fn map_buffer(
        &self,
        buffer: &Buffer,
        range: crate::MemoryRange,
    ) -> Result<crate::BufferMapping, crate::DeviceError> {
        if range.start >= range.end || range.end > buffer.size() {
            return Err(unexpected(
                "D3D11 buffer mapping range is outside the buffer",
            ));
        }

        let map_type = match buffer.map_mode() {
            BufferMapMode::Dynamic => Direct3D11::D3D11_MAP_WRITE_DISCARD,
            BufferMapMode::Staging => match (buffer.cpu_readable(), buffer.cpu_writable()) {
                (true, true) => Direct3D11::D3D11_MAP_READ_WRITE,
                (true, false) => Direct3D11::D3D11_MAP_READ,
                (false, true) => Direct3D11::D3D11_MAP_WRITE,
                (false, false) => {
                    return Err(unexpected("D3D11 staging buffer has no CPU access flags"));
                }
            },
            BufferMapMode::GpuOnly => {
                return Err(unexpected(
                    "buffer was not created with MAP_READ or MAP_WRITE usage",
                ));
            }
        };

        if !buffer.set_mapped() {
            return Err(unexpected("D3D11 buffer is already mapped"));
        }

        let mut mapped = Direct3D11::D3D11_MAPPED_SUBRESOURCE::default();
        if let Err(e) = unsafe {
            self.raw_context
                .Map(buffer.raw(), 0, map_type, 0, Some(&mut mapped))
        } {
            buffer.set_unmapped();
            log::error!("ID3D11DeviceContext::Map failed: {e}");
            // A failed map can be the first symptom of a removed device, which
            // must be reported as `Lost` rather than an unexpected failure.
            self.check_device_state()?;
            return Err(crate::DeviceError::Unexpected);
        }

        let base = mapped.pData as *mut u8;
        if base.is_null() {
            buffer.set_unmapped();
            unsafe { self.raw_context.Unmap(buffer.raw(), 0) };
            return Err(unexpected(
                "ID3D11DeviceContext::Map returned a null pointer",
            ));
        }

        let ptr = unsafe { NonNull::new_unchecked(base.add(range.start as usize)) };
        Ok(crate::BufferMapping {
            ptr,
            // D3D11 only guarantees visibility after `Unmap`; wgpu-core
            // always flushes and unmaps before submitting work.
            is_coherent: false,
        })
    }

    unsafe fn unmap_buffer(&self, buffer: &Buffer) {
        if buffer.set_unmapped() {
            unsafe { self.raw_context.Unmap(buffer.raw(), 0) };
        } else {
            log::warn!("unmap_buffer called for a D3D11 buffer that was not mapped");
        }
    }

    unsafe fn flush_mapped_ranges<I>(&self, _buffer: &Buffer, _ranges: I) {
        // D3D11 makes staging/dynamic writes visible on `Unmap`; wgpu-core
        // calls this immediately before unmapping, so no extra work is needed.
    }

    unsafe fn invalidate_mapped_ranges<I>(&self, _buffer: &Buffer, _ranges: I) {
        // Reads from a staging buffer already observe the completed copy that
        // made the buffer GPU-dirty. See `flush_mapped_ranges`.
    }

    unsafe fn create_texture(
        &self,
        desc: &crate::TextureDescriptor,
    ) -> Result<Texture, crate::DeviceError> {
        if desc.dimension != wgt::TextureDimension::D2 {
            return Err(unexpected(
                "D3D11 resource creation currently supports 2D textures only",
            ));
        }
        if desc.sample_count != 1 {
            return Err(unexpected(
                "D3D11 texture creation currently supports sample_count == 1 only",
            ));
        }
        if desc.usage.intersects(
            wgt::TextureUses::STORAGE_READ_ONLY
                | wgt::TextureUses::STORAGE_WRITE_ONLY
                | wgt::TextureUses::STORAGE_READ_WRITE
                | wgt::TextureUses::STORAGE_ATOMIC,
        ) {
            return Err(unexpected(
                "D3D11 storage textures/UAVs are not implemented yet",
            ));
        }
        if desc.size.width == 0 || desc.size.height == 0 {
            return Err(unexpected("D3D11 texture dimensions must be non-zero"));
        }
        if desc.mip_level_count == 0 {
            return Err(unexpected("D3D11 texture mip level count must be non-zero"));
        }
        if desc.size.depth_or_array_layers == 0 {
            return Err(unexpected(
                "D3D11 texture array layer count must be non-zero",
            ));
        }

        // A 2D texture can only have mip levels down to 1x1, so the chain is
        // bounded by the largest dimension. D3D11 would reject the extra
        // levels, but reporting the reason here is clearer.
        let max_dimension = desc.size.width.max(desc.size.height);
        let max_mip_levels = u32::BITS - max_dimension.leading_zeros();
        if desc.mip_level_count > max_mip_levels {
            return Err(unexpected(
                "D3D11 texture mip level count exceeds the size of the mip chain",
            ));
        }

        for view_format in &desc.view_formats {
            if !texture_formats_compatible(desc.format, *view_format) {
                let message = alloc::format!(
                    "D3D11 texture view format {view_format:?} is not compatible with {:?}",
                    desc.format
                );
                return Err(unexpected(&message));
            }
        }

        let format = super::conv::map_texture_format_resource(desc.format)
            .ok_or_else(|| unexpected("texture format is not supported by the D3D11 backend"))?;

        let bind_flags = map_texture_bind_flags(desc.usage);

        // wgpu-core does not request host-visible textures. Treat the coherent
        // memory hint as a request for a staging texture when no pipeline bind
        // is needed; this also gives the copy helpers a way to create staging
        // textures from the public HAL surface.
        let staging = desc
            .memory_flags
            .contains(crate::MemoryFlags::PREFER_COHERENT)
            && bind_flags == 0;
        let (d3d_usage, cpu_access_flags) = if staging {
            (
                Direct3D11::D3D11_USAGE_STAGING,
                (Direct3D11::D3D11_CPU_ACCESS_READ.0 | Direct3D11::D3D11_CPU_ACCESS_WRITE.0) as u32,
            )
        } else {
            (Direct3D11::D3D11_USAGE_DEFAULT, 0)
        };

        let texture_desc = Direct3D11::D3D11_TEXTURE2D_DESC {
            Width: desc.size.width,
            Height: desc.size.height,
            MipLevels: desc.mip_level_count,
            ArraySize: desc.size.depth_or_array_layers,
            Format: format,
            SampleDesc: Dxgi::Common::DXGI_SAMPLE_DESC {
                Count: desc.sample_count,
                Quality: 0,
            },
            Usage: d3d_usage,
            BindFlags: bind_flags,
            CPUAccessFlags: cpu_access_flags,
            // The cube misc flag is what lets `create_texture_view` build a
            // `TEXTURECUBE`/`TEXTURECUBEARRAY` SRV from this resource. Staging
            // textures cannot have bind flags or a cube view, and D3D11 rejects
            // resource misc flags on them.
            MiscFlags: if desc.is_cube_compatible() && !staging {
                Direct3D11::D3D11_RESOURCE_MISC_TEXTURECUBE.0 as u32
            } else {
                0
            },
        };

        // `TextureDescriptor` does not expose initial texture data in this
        // version of wgpu-hal. Uploads go through `copy_buffer_to_texture`.
        let mut raw: Option<Direct3D11::ID3D11Texture2D> = None;
        unsafe {
            self.raw_device
                .CreateTexture2D(&texture_desc, None, Some(&mut raw))
        }
        .map_err(|e| {
            log::error!(
                "ID3D11Device::CreateTexture2D failed ({}x{}, {:?}, {} mips, {} layers): {e}",
                desc.size.width,
                desc.size.height,
                desc.format,
                desc.mip_level_count,
                desc.size.depth_or_array_layers
            );
            if self.is_device_lost() {
                crate::DeviceError::Lost
            } else {
                crate::DeviceError::Unexpected
            }
        })?;

        let raw = raw.ok_or_else(|| {
            unexpected("ID3D11Device::CreateTexture2D succeeded without returning a texture")
        })?;

        log::debug!(
            "created D3D11 texture: {}x{}x{} {:?}, {} mips, usage {:?}",
            desc.size.width,
            desc.size.height,
            desc.size.depth_or_array_layers,
            desc.format,
            desc.mip_level_count,
            desc.usage
        );

        Ok(Texture::new(
            raw,
            desc.format,
            desc.size,
            desc.mip_level_count,
            desc.size.depth_or_array_layers,
            desc.sample_count,
            desc.usage,
        ))
    }

    unsafe fn destroy_texture(&self, texture: Texture) {
        drop(texture);
    }

    unsafe fn add_raw_texture(&self, _texture: &Texture) {}

    unsafe fn create_texture_view(
        &self,
        texture: &Texture,
        desc: &crate::TextureViewDescriptor,
    ) -> Result<TextureView, crate::DeviceError> {
        use Direct3D11 as d3d;

        if matches!(
            desc.dimension,
            wgt::TextureViewDimension::D1 | wgt::TextureViewDimension::D3
        ) {
            return Err(unexpected("D3D11 texture views support 2D dimensions only"));
        }

        let is_cube = matches!(
            desc.dimension,
            wgt::TextureViewDimension::Cube | wgt::TextureViewDimension::CubeArray
        );
        // D3D11 has no cube render target or cube depth-stencil views, so a
        // cube resource can only be exposed to shaders through a cube SRV.
        if is_cube
            && desc.usage.intersects(
                wgt::TextureUses::COLOR_TARGET
                    | wgt::TextureUses::DEPTH_STENCIL_READ
                    | wgt::TextureUses::DEPTH_STENCIL_WRITE,
            )
        {
            return Err(unexpected(
                "D3D11 does not support cube color targets or cube depth-stencil views",
            ));
        }

        if !texture_formats_compatible(texture.format(), desc.format) {
            return Err(unexpected(
                "D3D11 texture views must use a compatible format; arbitrary view formats are not supported",
            ));
        }
        if desc.usage.intersects(
            wgt::TextureUses::STORAGE_READ_ONLY
                | wgt::TextureUses::STORAGE_WRITE_ONLY
                | wgt::TextureUses::STORAGE_READ_WRITE
                | wgt::TextureUses::STORAGE_ATOMIC,
        ) {
            return Err(unexpected(
                "D3D11 UAV texture views are not implemented yet",
            ));
        }

        let base_mip = desc.range.base_mip_level;
        let mip_count = desc
            .range
            .mip_level_count
            .unwrap_or_else(|| texture.mip_level_count().saturating_sub(base_mip));
        let base_layer = desc.range.base_array_layer;
        let layer_count = desc
            .range
            .array_layer_count
            .unwrap_or_else(|| texture.array_layer_count().saturating_sub(base_layer));

        if mip_count == 0
            || layer_count == 0
            || base_mip + mip_count > texture.mip_level_count()
            || base_layer + layer_count > texture.array_layer_count()
        {
            return Err(unexpected(
                "D3D11 texture view subresource range is outside the texture",
            ));
        }

        let aspects = crate::FormatAspects::new(texture.format(), desc.range.aspect);
        if aspects.is_empty() {
            return Err(unexpected(
                "D3D11 texture view aspect is not present in the texture format",
            ));
        }

        match desc.dimension {
            // A plain 2D view is a single layer. Multi-layer views must ask for
            // `D2Array` explicitly.
            wgt::TextureViewDimension::D2 if layer_count != 1 => {
                return Err(unexpected(
                    "D3D11 2D texture views must cover exactly one array layer",
                ));
            }
            wgt::TextureViewDimension::Cube => {
                if texture.extent().width != texture.extent().height {
                    return Err(unexpected("D3D11 cube views require a square 2D texture"));
                }
                if base_layer != 0 || layer_count != 6 {
                    return Err(unexpected(
                        "D3D11 cube views require exactly six layers starting at layer zero",
                    ));
                }
            }
            wgt::TextureViewDimension::CubeArray => {
                if texture.extent().width != texture.extent().height {
                    return Err(unexpected(
                        "D3D11 cube-array views require a square 2D texture",
                    ));
                }
                if !base_layer.is_multiple_of(6) || !layer_count.is_multiple_of(6) {
                    return Err(unexpected(
                        "D3D11 cube-array views require six-layer aligned ranges",
                    ));
                }
            }
            _ => {}
        }

        let is_array = texture.array_layer_count() > 1 || layer_count > 1;
        let mut srv = None;
        let mut rtv = None;
        let mut dsv = None;

        if desc.usage.contains(wgt::TextureUses::RESOURCE) {
            // Use the view format so that sRGB views of a linear texture (and
            // vice versa) sample with the requested encoding.
            let format = super::conv::map_srv_format(desc.format, desc.range.aspect)
                .ok_or_else(|| unexpected("D3D11 SRV format is not supported"))?;

            let mut raw_desc = d3d::D3D11_SHADER_RESOURCE_VIEW_DESC::default();
            raw_desc.Format = format;
            match desc.dimension {
                wgt::TextureViewDimension::D2 if !is_array => {
                    raw_desc.ViewDimension = Direct3D::D3D_SRV_DIMENSION_TEXTURE2D;
                    raw_desc.Anonymous.Texture2D = d3d::D3D11_TEX2D_SRV {
                        MostDetailedMip: base_mip,
                        MipLevels: mip_count,
                    };
                }
                wgt::TextureViewDimension::D2 | wgt::TextureViewDimension::D2Array => {
                    raw_desc.ViewDimension = Direct3D::D3D_SRV_DIMENSION_TEXTURE2DARRAY;
                    raw_desc.Anonymous.Texture2DArray = d3d::D3D11_TEX2D_ARRAY_SRV {
                        MostDetailedMip: base_mip,
                        MipLevels: mip_count,
                        FirstArraySlice: base_layer,
                        ArraySize: layer_count,
                    };
                }
                wgt::TextureViewDimension::Cube => {
                    raw_desc.ViewDimension = Direct3D::D3D_SRV_DIMENSION_TEXTURECUBE;
                    raw_desc.Anonymous.TextureCube = d3d::D3D11_TEXCUBE_SRV {
                        MostDetailedMip: base_mip,
                        MipLevels: mip_count,
                    };
                }
                wgt::TextureViewDimension::CubeArray => {
                    raw_desc.ViewDimension = Direct3D::D3D_SRV_DIMENSION_TEXTURECUBEARRAY;
                    raw_desc.Anonymous.TextureCubeArray = d3d::D3D11_TEXCUBE_ARRAY_SRV {
                        MostDetailedMip: base_mip,
                        MipLevels: mip_count,
                        First2DArrayFace: base_layer,
                        NumCubes: layer_count / 6,
                    };
                }
                wgt::TextureViewDimension::D1 | wgt::TextureViewDimension::D3 => {
                    return Err(unexpected(
                        "D3D11 SRV views currently support 2D dimensions only",
                    ));
                }
            }

            let mut raw = None;
            unsafe {
                self.raw_device.CreateShaderResourceView(
                    texture.raw(),
                    Some(&raw_desc),
                    Some(&mut raw),
                )
            }
            .map_err(|e| {
                log::error!("ID3D11Device::CreateShaderResourceView failed: {e}");
                if self.is_device_lost() {
                    crate::DeviceError::Lost
                } else {
                    crate::DeviceError::Unexpected
                }
            })?;
            srv = raw;
        }

        // Cube and cube-array views only ever create an SRV; the descriptor
        // checks above already rejected target usages for them.
        if !is_cube && desc.usage.contains(wgt::TextureUses::COLOR_TARGET) {
            if !texture.format().has_color_aspect() {
                return Err(unexpected(
                    "cannot create a color render target for a depth/stencil texture",
                ));
            }
            let format = super::conv::map_texture_format(desc.format)
                .ok_or_else(|| unexpected("D3D11 RTV format is not supported"))?;

            let mut raw_desc = d3d::D3D11_RENDER_TARGET_VIEW_DESC::default();
            raw_desc.Format = format;
            if is_array {
                raw_desc.ViewDimension = d3d::D3D11_RTV_DIMENSION_TEXTURE2DARRAY;
                raw_desc.Anonymous.Texture2DArray = d3d::D3D11_TEX2D_ARRAY_RTV {
                    MipSlice: base_mip,
                    FirstArraySlice: base_layer,
                    ArraySize: layer_count,
                };
            } else {
                raw_desc.ViewDimension = d3d::D3D11_RTV_DIMENSION_TEXTURE2D;
                raw_desc.Anonymous.Texture2D = d3d::D3D11_TEX2D_RTV { MipSlice: base_mip };
            }

            let mut raw = None;
            unsafe {
                self.raw_device.CreateRenderTargetView(
                    texture.raw(),
                    Some(&raw_desc),
                    Some(&mut raw),
                )
            }
            .map_err(|e| {
                log::error!("ID3D11Device::CreateRenderTargetView failed: {e}");
                if self.is_device_lost() {
                    crate::DeviceError::Lost
                } else {
                    crate::DeviceError::Unexpected
                }
            })?;
            rtv = raw;
        }

        if !is_cube
            && desc.usage.intersects(
                wgt::TextureUses::DEPTH_STENCIL_READ | wgt::TextureUses::DEPTH_STENCIL_WRITE,
            )
        {
            if !texture.format().has_depth_aspect() && !texture.format().has_stencil_aspect() {
                return Err(unexpected(
                    "cannot create a depth-stencil view for a color texture",
                ));
            }
            let format = super::conv::map_dsv_format(desc.format, desc.range.aspect)
                .ok_or_else(|| unexpected("D3D11 DSV format is not supported"))?;

            let mut raw_desc = d3d::D3D11_DEPTH_STENCIL_VIEW_DESC::default();
            raw_desc.Format = format;
            raw_desc.Flags = 0;
            // Sampling depth/stencil through an SRV does not require the DSV to
            // be writable. Marks the read-only aspects so the driver can keep
            // them out of the write path; depth-stencil writes need no flags.
            if !desc.usage.contains(wgt::TextureUses::DEPTH_STENCIL_WRITE) {
                if aspects.contains(crate::FormatAspects::DEPTH) {
                    raw_desc.Flags |= Direct3D11::D3D11_DSV_READ_ONLY_DEPTH.0 as u32;
                }
                if aspects.contains(crate::FormatAspects::STENCIL) {
                    raw_desc.Flags |= Direct3D11::D3D11_DSV_READ_ONLY_STENCIL.0 as u32;
                }
            }
            if is_array {
                raw_desc.ViewDimension = d3d::D3D11_DSV_DIMENSION_TEXTURE2DARRAY;
                raw_desc.Anonymous.Texture2DArray = d3d::D3D11_TEX2D_ARRAY_DSV {
                    MipSlice: base_mip,
                    FirstArraySlice: base_layer,
                    ArraySize: layer_count,
                };
            } else {
                raw_desc.ViewDimension = d3d::D3D11_DSV_DIMENSION_TEXTURE2D;
                raw_desc.Anonymous.Texture2D = d3d::D3D11_TEX2D_DSV { MipSlice: base_mip };
            }

            let mut raw = None;
            unsafe {
                self.raw_device.CreateDepthStencilView(
                    texture.raw(),
                    Some(&raw_desc),
                    Some(&mut raw),
                )
            }
            .map_err(|e| {
                log::error!("ID3D11Device::CreateDepthStencilView failed: {e}");
                if self.is_device_lost() {
                    crate::DeviceError::Lost
                } else {
                    crate::DeviceError::Unexpected
                }
            })?;
            dsv = raw;
        }

        if srv.is_none() && rtv.is_none() && dsv.is_none() {
            return Err(unexpected(
                "texture view descriptor requested no supported view usage",
            ));
        }

        let view_format = super::conv::map_texture_format(desc.format)
            .ok_or_else(|| unexpected("D3D11 texture view format is not supported"))?;
        Ok(TextureView::from_parts(srv, rtv, dsv, view_format))
    }

    unsafe fn destroy_texture_view(&self, view: TextureView) {
        drop(view);
    }

    unsafe fn create_sampler(
        &self,
        desc: &crate::SamplerDescriptor,
    ) -> Result<Sampler, crate::DeviceError> {
        // Anisotropic filtering is not advertised as a feature, but the
        // descriptor is still validated against the D3D11 range so an invalid
        // clamp cannot be forwarded to the driver.
        if !(1..=16).contains(&desc.anisotropy_clamp) {
            return Err(unexpected(
                "D3D11 sampler anisotropy_clamp must be between 1 and 16",
            ));
        }

        let sampler_desc = Direct3D11::D3D11_SAMPLER_DESC {
            Filter: super::conv::map_filter(
                desc.mag_filter,
                desc.min_filter,
                desc.mipmap_filter,
                desc.compare,
                desc.anisotropy_clamp,
            ),
            AddressU: super::conv::map_address_mode(desc.address_modes[0]),
            AddressV: super::conv::map_address_mode(desc.address_modes[1]),
            AddressW: super::conv::map_address_mode(desc.address_modes[2]),
            MipLODBias: 0.0,
            MaxAnisotropy: u32::from(desc.anisotropy_clamp),
            ComparisonFunc: desc
                .compare
                .map(super::conv::map_comparison)
                .unwrap_or(Direct3D11::D3D11_COMPARISON_NEVER),
            BorderColor: desc
                .border_color
                .map(super::conv::map_border_color)
                .unwrap_or([0.0; 4]),
            MinLOD: desc.lod_clamp.start,
            MaxLOD: desc.lod_clamp.end,
        };

        let mut raw: Option<Direct3D11::ID3D11SamplerState> = None;
        unsafe {
            self.raw_device
                .CreateSamplerState(&sampler_desc, Some(&mut raw))
        }
        .map_err(|e| {
            log::error!("ID3D11Device::CreateSamplerState failed: {e}");
            crate::DeviceError::Unexpected
        })?;

        raw.map(Sampler::new)
            .ok_or_else(|| unexpected("CreateSamplerState returned no sampler"))
    }

    unsafe fn destroy_sampler(&self, sampler: Sampler) {
        drop(sampler);
    }

    unsafe fn create_command_encoder(
        &self,
        _desc: &crate::CommandEncoderDescriptor<super::Queue>,
    ) -> Result<CommandBuffer, crate::DeviceError> {
        Ok(CommandBuffer::new(
            self.raw_context.clone(),
            self.raw_device.clone(),
        ))
    }

    unsafe fn create_bind_group_layout(
        &self,
        desc: &crate::BindGroupLayoutDescriptor,
    ) -> Result<BindGroupLayout, crate::DeviceError> {
        BindGroupLayout::new(desc.entries, &self.limits)
    }

    unsafe fn destroy_bind_group_layout(&self, bg_layout: BindGroupLayout) {
        drop(bg_layout);
    }

    unsafe fn create_pipeline_layout(
        &self,
        desc: &crate::PipelineLayoutDescriptor<BindGroupLayout>,
    ) -> Result<PipelineLayout, crate::DeviceError> {
        let bind_group_layouts = desc
            .bind_group_layouts
            .iter()
            .map(|layout| match layout {
                Some(layout) => Some((*layout).clone()),
                None => None,
            })
            .collect();
        PipelineLayout::new(
            bind_group_layouts,
            desc.immediate_size,
            self.limits.clone(),
            self.raw_context
                .cast::<Direct3D11::ID3D11DeviceContext1>()
                .is_ok(),
        )
    }

    unsafe fn destroy_pipeline_layout(&self, pipeline_layout: PipelineLayout) {
        drop(pipeline_layout);
    }

    unsafe fn create_bind_group(
        &self,
        desc: &crate::BindGroupDescriptor<
            '_,
            BindGroupLayout,
            Buffer,
            Sampler,
            TextureView,
            Resource,
        >,
    ) -> Result<BindGroup, crate::DeviceError> {
        BindGroup::new(desc)
    }

    unsafe fn destroy_bind_group(&self, group: BindGroup) {
        drop(group);
    }

    unsafe fn create_shader_module(
        &self,
        desc: &crate::ShaderModuleDescriptor,
        shader: crate::ShaderInput,
    ) -> Result<ShaderModule, crate::ShaderError> {
        ShaderModule::new(desc, shader)
    }

    unsafe fn destroy_shader_module(&self, module: ShaderModule) {
        drop(module);
    }

    unsafe fn create_render_pipeline(
        &self,
        desc: &crate::RenderPipelineDescriptor<PipelineLayout, ShaderModule, PipelineCache>,
    ) -> Result<RenderPipeline, crate::PipelineError> {
        let (vertex_buffers, vertex_stage) = match &desc.vertex_processor {
            crate::VertexProcessor::Standard {
                vertex_buffers,
                vertex_stage,
            } => (*vertex_buffers, vertex_stage),
            crate::VertexProcessor::Mesh { .. } => {
                return Err(pipeline_error(
                    "D3D11 mesh shader pipelines are not implemented",
                ));
            }
        };

        if desc.multiview_mask.is_some() {
            return Err(pipeline_error(
                "D3D11 multiview render pipelines are not implemented",
            ));
        }
        if desc.multisample.count != 1 {
            return Err(pipeline_error(alloc::format!(
                "D3D11 currently supports render pipelines with sample_count == 1 only, got {}",
                desc.multisample.count
            )));
        }
        if desc.multisample.mask & 1 == 0 {
            return Err(pipeline_error(
                "D3D11 render pipeline sample mask disables the only sample",
            ));
        }
        if desc.multisample.alpha_to_coverage_enabled {
            return Err(pipeline_error(
                "D3D11 alpha-to-coverage is not implemented yet",
            ));
        }
        if desc.primitive.strip_index_format.is_some() {
            return Err(pipeline_error(
                "D3D11 indexed strip primitive restart is not implemented",
            ));
        }
        if desc.color_targets.len() > Direct3D11::D3D11_SIMULTANEOUS_RENDER_TARGET_COUNT as usize {
            return Err(pipeline_error(alloc::format!(
                "D3D11 render pipelines support at most {} color targets, got {}",
                Direct3D11::D3D11_SIMULTANEOUS_RENDER_TARGET_COUNT,
                desc.color_targets.len()
            )));
        }

        let vertex = vertex_stage
            .module
            .compile(
                vertex_stage.entry_point,
                naga::ShaderStage::Vertex,
                desc.layout,
                vertex_stage.constants,
            )
            .map_err(|e| crate::PipelineError::Linkage(wgt::ShaderStages::VERTEX, e.to_string()))?;

        let fragment = match desc.fragment_stage.as_ref() {
            Some(stage) => Some(
                stage
                    .module
                    .compile(
                        stage.entry_point,
                        naga::ShaderStage::Fragment,
                        desc.layout,
                        stage.constants,
                    )
                    .map_err(|e| {
                        crate::PipelineError::Linkage(wgt::ShaderStages::FRAGMENT, e.to_string())
                    })?,
            ),
            None => None,
        };

        let mut raw_vertex_shader: Option<Direct3D11::ID3D11VertexShader> = None;
        unsafe {
            self.raw_device
                .CreateVertexShader(&vertex.bytecode, None, Some(&mut raw_vertex_shader))
        }
        .map_err(|e| {
            log::error!("ID3D11Device::CreateVertexShader failed: {e}");
            crate::PipelineError::Device(crate::DeviceError::Unexpected)
        })?;
        let vertex_shader = raw_vertex_shader.ok_or_else(|| {
            pipeline_error("ID3D11Device::CreateVertexShader returned no shader object")
        })?;

        let pixel_shader = match &fragment {
            Some(fragment) => {
                let mut raw_pixel_shader: Option<Direct3D11::ID3D11PixelShader> = None;
                unsafe {
                    self.raw_device.CreatePixelShader(
                        &fragment.bytecode,
                        None,
                        Some(&mut raw_pixel_shader),
                    )
                }
                .map_err(|e| {
                    log::error!("ID3D11Device::CreatePixelShader failed: {e}");
                    crate::PipelineError::Device(crate::DeviceError::Unexpected)
                })?;
                Some(raw_pixel_shader.ok_or_else(|| {
                    pipeline_error("ID3D11Device::CreatePixelShader returned no shader object")
                })?)
            }
            None => None,
        };

        let (input_layout, vertex_strides) =
            create_input_layout(&self.raw_device, vertex_buffers, &vertex.bytecode)?;

        let mut color_formats: Vec<Option<Dxgi::Common::DXGI_FORMAT>> =
            Vec::with_capacity(desc.color_targets.len());
        for target in desc.color_targets {
            match target {
                Some(target) => {
                    if !target.format.has_color_aspect() {
                        return Err(pipeline_error(alloc::format!(
                            "color target format {:?} has no color aspect",
                            target.format
                        )));
                    }
                    let format =
                        super::conv::map_texture_format(target.format).ok_or_else(|| {
                            pipeline_error(alloc::format!(
                                "color target format {:?} is not supported by the D3D11 backend",
                                target.format
                            ))
                        })?;
                    color_formats.push(Some(format));
                }
                None => color_formats.push(None),
            }
        }

        let depth_format = match desc.depth_stencil.as_ref() {
            Some(depth_stencil) => {
                if !depth_stencil.format.has_depth_aspect()
                    && !depth_stencil.format.has_stencil_aspect()
                {
                    return Err(pipeline_error(alloc::format!(
                        "depth/stencil format {:?} has neither a depth nor a stencil aspect",
                        depth_stencil.format
                    )));
                }
                if depth_stencil.stencil.is_enabled() && !depth_stencil.format.has_stencil_aspect()
                {
                    return Err(pipeline_error(alloc::format!(
                        "stencil state is enabled but depth/stencil format {:?} has no stencil aspect",
                        depth_stencil.format
                    )));
                }
                Some(
                    super::conv::map_texture_format(depth_stencil.format).ok_or_else(|| {
                        pipeline_error(alloc::format!(
                            "depth/stencil format {:?} is not supported by the D3D11 backend",
                            depth_stencil.format
                        ))
                    })?,
                )
            }
            None => None,
        };

        let topology = super::conv::map_primitive_topology(desc.primitive.topology);
        let depth_bias = desc
            .depth_stencil
            .as_ref()
            .map_or(wgt::DepthBiasState::default(), |state| state.bias);
        let rasterizer_state =
            create_rasterizer_state(&self.raw_device, &desc.primitive, depth_bias)?;
        let depth_stencil_state =
            create_depth_stencil_state(&self.raw_device, desc.depth_stencil.as_ref())?;
        let blend_state = create_blend_state(&self.raw_device, desc.color_targets)?;

        let pipeline = RenderPipeline::new(
            desc.label.map(alloc::string::String::from),
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
            desc.multisample.count,
            (desc.multisample.mask & u32::MAX as u64) as u32,
        );

        log::debug!(
            "created D3D11 render pipeline {:?} with topology {:?}",
            desc.label,
            desc.primitive.topology
        );

        Ok(pipeline)
    }

    unsafe fn destroy_render_pipeline(&self, pipeline: RenderPipeline) {
        drop(pipeline);
    }

    unsafe fn create_compute_pipeline(
        &self,
        _desc: &crate::ComputePipelineDescriptor<PipelineLayout, ShaderModule, PipelineCache>,
    ) -> Result<Resource, crate::PipelineError> {
        log::error!("D3D11 compute pipelines are not implemented");
        Err(crate::PipelineError::Linkage(
            wgt::ShaderStages::COMPUTE,
            "D3D11 compute pipelines are not implemented".into(),
        ))
    }

    unsafe fn destroy_compute_pipeline(&self, _pipeline: Resource) {}

    unsafe fn create_ray_tracing_pipeline(
        &self,
        _desc: &crate::RayTracingPipelineDescriptor<PipelineLayout, ShaderModule, PipelineCache>,
    ) -> Result<Resource, crate::PipelineError> {
        log::error!("D3D11 does not support ray tracing pipelines");
        Err(crate::PipelineError::Linkage(
            wgt::ShaderStages::VERTEX_FRAGMENT,
            "D3D11 does not support ray tracing pipelines".into(),
        ))
    }

    unsafe fn destroy_ray_tracing_pipeline(&self, _pipeline: Resource) {}

    unsafe fn get_raytracing_pipeline_group_data(
        &self,
        _pipeline: &Resource,
        _groups: core::ops::Range<u32>,
    ) -> Result<Vec<u8>, crate::DeviceError> {
        Err(unexpected("D3D11 does not support ray tracing"))
    }

    unsafe fn create_pipeline_cache(
        &self,
        _desc: &crate::PipelineCacheDescriptor<'_>,
    ) -> Result<PipelineCache, crate::PipelineCacheError> {
        Ok(PipelineCache)
    }

    unsafe fn destroy_pipeline_cache(&self, _cache: PipelineCache) {}

    unsafe fn create_query_set(
        &self,
        _desc: &wgt::QuerySetDescriptor<crate::Label>,
    ) -> Result<Resource, crate::DeviceError> {
        Err(unexpected("D3D11 query set creation is not implemented"))
    }

    unsafe fn destroy_query_set(&self, _set: Resource) {}

    unsafe fn create_fence(&self) -> Result<Fence, crate::DeviceError> {
        Fence::new(&self.raw_device)
    }

    unsafe fn destroy_fence(&self, _fence: Fence) {
        // Dropping the fence releases its pool of `ID3D11Query` objects. No
        // explicit D3D11 call is needed, and the queries must not be used
        // afterwards.
    }

    unsafe fn get_fence_value(
        &self,
        fence: &Fence,
    ) -> Result<crate::FenceValue, crate::DeviceError> {
        // wgpu-core treats this as the "currently finished submission index",
        // so report the highest value the GPU has actually completed, never
        // the highest value merely submitted.
        //
        // Fast path: nothing is in flight, so no GPU poll is needed.
        if fence.pending() <= fence.completed() {
            return Ok(fence.completed());
        }

        // A live-looking fence can still belong to a removed device; check
        // before polling so the caller observes `Lost` instead of a value that
        // will never advance.
        self.check_device_state()?;

        // Poll every armed pool slot once without blocking. `Poll`-style
        // callers rely on repeated `get_fence_value` calls to observe progress
        // without waiting.
        fence.get_completed_value(&self.raw_context, &self.raw_device)
    }

    unsafe fn wait(
        &self,
        fence: &Fence,
        value: crate::FenceValue,
        timeout: Option<core::time::Duration>,
    ) -> Result<bool, crate::DeviceError> {
        // Fast path: the GPU already reached this value.
        if fence.completed() >= value {
            return Ok(true);
        }

        // The fence may already be past this value on paper while the device
        // is gone. Report the removal as `Lost` before deciding whether the
        // value was ever submitted.
        self.check_device_state()?;

        // The value was never submitted on this fence, so waiting cannot make
        // progress. The HAL contract says callers must not do this; report
        // "not reached" instead of blocking until the timeout expires.
        if fence.pending() < value {
            log::warn!(
                "waiting for D3D11 fence value {value}, which was never submitted \
                 (pending value is {})",
                fence.pending()
            );
            return Ok(false);
        }

        // Poll the fence's armed `D3D11_QUERY_EVENT` pool slots. Only a marker
        // the GPU has actually reached reports completion, so `completed_value`
        // is never advanced on a CPU-only guess. The wait loop does not hold
        // the slot mutex while sleeping, and a timeout leaves the markers armed
        // for the next wait or submission.
        fence.wait(&self.raw_context, &self.raw_device, value, timeout)
    }

    unsafe fn start_graphics_debugger_capture(&self) -> bool {
        false
    }

    unsafe fn stop_graphics_debugger_capture(&self) {}

    unsafe fn create_acceleration_structure(
        &self,
        _desc: &crate::AccelerationStructureDescriptor,
    ) -> Result<Resource, crate::DeviceError> {
        Err(unexpected("D3D11 does not support acceleration structures"))
    }

    unsafe fn get_acceleration_structure_build_sizes<'a>(
        &self,
        _desc: &crate::GetAccelerationStructureBuildSizesDescriptor<'a, Buffer>,
    ) -> crate::AccelerationStructureBuildSizes {
        Default::default()
    }

    unsafe fn get_acceleration_structure_device_address(
        &self,
        _acceleration_structure: &Resource,
    ) -> wgt::BufferAddress {
        0
    }

    unsafe fn destroy_acceleration_structure(&self, _acceleration_structure: Resource) {}

    fn tlas_instance_to_bytes(&self, _instance: crate::TlasInstance) -> Vec<u8> {
        Vec::new()
    }

    fn get_internal_counters(&self) -> wgt::HalCounters {
        Default::default()
    }

    fn check_if_oom(&self) -> Result<(), crate::DeviceError> {
        Ok(())
    }
}

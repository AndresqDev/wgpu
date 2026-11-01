//! D3D11 device implementation.
//!
//! The device owns the `ID3D11Device` created by [`crate::Adapter::open`].
//! Buffers, 2D textures, texture views and samplers are created here; bind
//! groups, shaders, pipelines and query sets remain explicit stubs for later
//! milestones.

use alloc::vec::Vec;
use core::{fmt, ptr::NonNull};

use windows::Win32::Graphics::{Direct3D, Direct3D11, Dxgi};

use super::{
    Api, Buffer, BufferMapMode, CommandBuffer, Fence, PipelineCache, Resource, Sampler,
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
    if usage.intersects(wgt::BufferUses::STORAGE_READ_ONLY | wgt::BufferUses::STORAGE_READ_WRITE) {
        bind_flags |= Direct3D11::D3D11_BIND_SHADER_RESOURCE.0 as u32;
        bind_flags |= Direct3D11::D3D11_BIND_UNORDERED_ACCESS.0 as u32;
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
    if usage.intersects(
        wgt::TextureUses::STORAGE_READ_ONLY
            | wgt::TextureUses::STORAGE_WRITE_ONLY
            | wgt::TextureUses::STORAGE_READ_WRITE
            | wgt::TextureUses::STORAGE_ATOMIC,
    ) {
        bind_flags |= Direct3D11::D3D11_BIND_UNORDERED_ACCESS.0 as u32;
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
        Self {
            raw_device,
            raw_context,
            info,
            feature_level,
            limits,
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
}

impl crate::Device for Device {
    type A = Api;

    unsafe fn create_buffer(
        &self,
        desc: &crate::BufferDescriptor,
    ) -> Result<Buffer, crate::DeviceError> {
        let usage = desc.usage;
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
        if usage
            .intersects(wgt::BufferUses::STORAGE_READ_ONLY | wgt::BufferUses::STORAGE_READ_WRITE)
        {
            misc_flags |= Direct3D11::D3D11_RESOURCE_MISC_BUFFER_ALLOW_RAW_VIEWS.0 as u32;
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
            crate::DeviceError::Unexpected
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

        let format = super::conv::map_texture_format(desc.format)
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
            MiscFlags: 0,
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
            crate::DeviceError::Unexpected
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

        let is_array = texture.array_layer_count() > 1 || layer_count > 1;
        let mut srv = None;
        let mut rtv = None;
        let mut dsv = None;

        if desc.usage.contains(wgt::TextureUses::RESOURCE) {
            let format = super::conv::map_srv_format(texture.format(), desc.range.aspect)
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
                    if base_layer != 0 || !layer_count.is_multiple_of(6) {
                        return Err(unexpected(
                            "D3D11 cube SRVs need six layers starting at layer zero",
                        ));
                    }
                    raw_desc.ViewDimension = Direct3D::D3D_SRV_DIMENSION_TEXTURECUBE;
                    raw_desc.Anonymous.TextureCube = d3d::D3D11_TEXCUBE_SRV {
                        MostDetailedMip: base_mip,
                        MipLevels: mip_count,
                    };
                }
                wgt::TextureViewDimension::CubeArray => {
                    if !layer_count.is_multiple_of(6) {
                        return Err(unexpected(
                            "D3D11 cube-array SRVs need a multiple of six layers",
                        ));
                    }
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
                crate::DeviceError::Unexpected
            })?;
            srv = raw;
        }

        if desc.usage.contains(wgt::TextureUses::COLOR_TARGET) {
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
                crate::DeviceError::Unexpected
            })?;
            rtv = raw;
        }

        if desc.usage.intersects(
            wgt::TextureUses::DEPTH_STENCIL_READ | wgt::TextureUses::DEPTH_STENCIL_WRITE,
        ) {
            if !texture.format().has_depth_aspect() && !texture.format().has_stencil_aspect() {
                return Err(unexpected(
                    "cannot create a depth-stencil view for a color texture",
                ));
            }
            let format = super::conv::map_dsv_format(texture.format(), desc.range.aspect)
                .ok_or_else(|| unexpected("D3D11 DSV format is not supported"))?;

            let mut raw_desc = d3d::D3D11_DEPTH_STENCIL_VIEW_DESC::default();
            raw_desc.Format = format;
            raw_desc.Flags = 0;
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
                crate::DeviceError::Unexpected
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
            MaxAnisotropy: u32::from(desc.anisotropy_clamp.max(1)),
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
        _desc: &crate::BindGroupLayoutDescriptor,
    ) -> Result<Resource, crate::DeviceError> {
        unimplemented!("D3D11 bind group layout creation is not implemented yet")
    }

    unsafe fn destroy_bind_group_layout(&self, _bg_layout: Resource) {}

    unsafe fn create_pipeline_layout(
        &self,
        _desc: &crate::PipelineLayoutDescriptor<Resource>,
    ) -> Result<Resource, crate::DeviceError> {
        unimplemented!("D3D11 pipeline layout creation is not implemented yet")
    }

    unsafe fn destroy_pipeline_layout(&self, _pipeline_layout: Resource) {}

    unsafe fn create_bind_group(
        &self,
        _desc: &crate::BindGroupDescriptor<'_, Resource, Buffer, Sampler, TextureView, Resource>,
    ) -> Result<Resource, crate::DeviceError> {
        unimplemented!("D3D11 bind group creation is not implemented yet")
    }

    unsafe fn destroy_bind_group(&self, _group: Resource) {}

    unsafe fn create_shader_module(
        &self,
        _desc: &crate::ShaderModuleDescriptor,
        _shader: crate::ShaderInput,
    ) -> Result<ShaderModule, crate::ShaderError> {
        unimplemented!("D3D11 shader compilation is not implemented yet")
    }

    unsafe fn destroy_shader_module(&self, _module: ShaderModule) {}

    unsafe fn create_render_pipeline(
        &self,
        _desc: &crate::RenderPipelineDescriptor<Resource, ShaderModule, PipelineCache>,
    ) -> Result<Resource, crate::PipelineError> {
        unimplemented!("D3D11 render pipeline creation is not implemented yet")
    }

    unsafe fn destroy_render_pipeline(&self, _pipeline: Resource) {}

    unsafe fn create_compute_pipeline(
        &self,
        _desc: &crate::ComputePipelineDescriptor<Resource, ShaderModule, PipelineCache>,
    ) -> Result<Resource, crate::PipelineError> {
        unimplemented!("D3D11 compute pipeline creation is not implemented yet")
    }

    unsafe fn destroy_compute_pipeline(&self, _pipeline: Resource) {}

    unsafe fn create_ray_tracing_pipeline(
        &self,
        _desc: &crate::RayTracingPipelineDescriptor<Resource, ShaderModule, PipelineCache>,
    ) -> Result<Resource, crate::PipelineError> {
        unimplemented!("D3D11 does not support ray tracing")
    }

    unsafe fn destroy_ray_tracing_pipeline(&self, _pipeline: Resource) {}

    unsafe fn get_raytracing_pipeline_group_data(
        &self,
        _pipeline: &Resource,
        _groups: core::ops::Range<u32>,
    ) -> Result<Vec<u8>, crate::DeviceError> {
        unimplemented!("D3D11 does not support ray tracing")
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
        unimplemented!("D3D11 query set creation is not implemented yet")
    }

    unsafe fn destroy_query_set(&self, _set: Resource) {}

    unsafe fn create_fence(&self) -> Result<Fence, crate::DeviceError> {
        Ok(Fence::new())
    }

    unsafe fn destroy_fence(&self, _fence: Fence) {}

    unsafe fn get_fence_value(
        &self,
        fence: &Fence,
    ) -> Result<crate::FenceValue, crate::DeviceError> {
        Ok(fence.value())
    }

    unsafe fn wait(
        &self,
        fence: &Fence,
        value: crate::FenceValue,
        _timeout: Option<core::time::Duration>,
    ) -> Result<bool, crate::DeviceError> {
        // Queue submissions in this skeleton are CPU-synchronous and signal the
        // fence immediately. A real implementation will poll an
        // `ID3D11Query` of type `D3D11_QUERY_EVENT` here.
        Ok(fence.value() >= value)
    }

    unsafe fn start_graphics_debugger_capture(&self) -> bool {
        false
    }

    unsafe fn stop_graphics_debugger_capture(&self) {}

    unsafe fn create_acceleration_structure(
        &self,
        _desc: &crate::AccelerationStructureDescriptor,
    ) -> Result<Resource, crate::DeviceError> {
        unimplemented!("D3D11 does not support acceleration structures")
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

//! Minimal D3D11 command encoder.
//!
//! D3D11 records commands on an `ID3D11DeviceContext`. This milestone does not
//! create deferred contexts yet: the encoder keeps a clone of the device's
//! immediate context and executes supported commands (buffer/texture copies and
//! render-pass clears) as soon as they are recorded. [`crate::Queue::submit`]
//! then flushes that context.
//!
//! This means the clear becomes visible before the command buffer is submitted,
//! which is fine for the clear/present milestone but not the final design.
//! Deferred contexts and ordered command-list execution are the next step.

use alloc::vec::Vec;
use core::{ffi::c_void, ops::Range, ptr};

use windows::Win32::{
    Foundation,
    Graphics::{Direct3D11, Dxgi},
};

use super::{
    Api, BindGroup, Buffer, BufferMapMode, PipelineLayout, Resource, Texture, TextureView,
};

/// D3D11 requires staging buffer sizes to be a multiple of four bytes.
fn align_to(value: u64, alignment: u64) -> u64 {
    value.div_ceil(alignment) * alignment
}

fn make_box(origin: &wgt::Origin3d, size: &crate::CopyExtent) -> Option<Direct3D11::D3D11_BOX> {
    let right = origin.x.checked_add(size.width)?;
    let bottom = origin.y.checked_add(size.height)?;
    let back = origin.z.checked_add(size.depth)?;
    Some(Direct3D11::D3D11_BOX {
        left: origin.x,
        top: origin.y,
        front: origin.z,
        right,
        bottom,
        back,
    })
}

fn buffer_box(offset: u64, size: u64) -> Option<Direct3D11::D3D11_BOX> {
    let left = u32::try_from(offset).ok()?;
    let right = u32::try_from(offset.checked_add(size)?).ok()?;
    Some(Direct3D11::D3D11_BOX {
        left,
        top: 0,
        front: 0,
        right,
        bottom: 1,
        back: 1,
    })
}

fn create_staging_buffer(
    device: &Direct3D11::ID3D11Device,
    size: u64,
    initial_data: Option<&[u8]>,
) -> windows::core::Result<Direct3D11::ID3D11Buffer> {
    let byte_width = align_to(size.max(1), 4);
    let byte_width = u32::try_from(byte_width)
        .map_err(|_| windows::core::Error::from_hresult(Foundation::E_OUTOFMEMORY))?;

    let desc = Direct3D11::D3D11_BUFFER_DESC {
        ByteWidth: byte_width,
        Usage: Direct3D11::D3D11_USAGE_STAGING,
        BindFlags: 0,
        CPUAccessFlags: (Direct3D11::D3D11_CPU_ACCESS_READ.0 | Direct3D11::D3D11_CPU_ACCESS_WRITE.0)
            as u32,
        MiscFlags: 0,
        StructureByteStride: 0,
    };

    let initial = initial_data.map(|data| Direct3D11::D3D11_SUBRESOURCE_DATA {
        pSysMem: data.as_ptr().cast::<c_void>(),
        SysMemPitch: 0,
        SysMemSlicePitch: 0,
    });

    let mut raw = None;
    unsafe {
        device.CreateBuffer(
            &desc,
            initial
                .as_ref()
                .map(|initial| initial as *const Direct3D11::D3D11_SUBRESOURCE_DATA),
            Some(&mut raw),
        )
    }?;
    raw.ok_or_else(|| windows::core::Error::from_hresult(Foundation::E_UNEXPECTED))
}

fn create_staging_texture(
    device: &Direct3D11::ID3D11Device,
    format: Dxgi::Common::DXGI_FORMAT,
    width: u32,
    height: u32,
) -> windows::core::Result<Direct3D11::ID3D11Texture2D> {
    let desc = Direct3D11::D3D11_TEXTURE2D_DESC {
        Width: width,
        Height: height,
        MipLevels: 1,
        ArraySize: 1,
        Format: format,
        SampleDesc: Dxgi::Common::DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: Direct3D11::D3D11_USAGE_STAGING,
        BindFlags: 0,
        CPUAccessFlags: Direct3D11::D3D11_CPU_ACCESS_READ.0 as u32,
        MiscFlags: 0,
    };

    let mut raw = None;
    unsafe { device.CreateTexture2D(&desc, None, Some(&mut raw)) }?;
    raw.ok_or_else(|| windows::core::Error::from_hresult(Foundation::E_UNEXPECTED))
}

fn bytes_per_row(
    format: wgt::TextureFormat,
    aspect: wgt::TextureAspect,
    width: u32,
    layout: &wgt::TexelCopyBufferLayout,
) -> Option<(u32, u32, u32)> {
    let block_size = format.block_copy_size(Some(aspect))?;
    let (block_width, block_height) = format.block_dimensions();
    let row_bytes = (width / block_width) * block_size;
    let pitch = layout.bytes_per_row.unwrap_or(row_bytes);
    if pitch < row_bytes {
        return None;
    }
    Some((pitch, row_bytes, block_height))
}

/// Command buffer type, which also acts as the command encoder.
///
/// Keeping a single type for both roles follows the structure of the noop
/// backend and avoids maintaining two state machines before command recording
/// is implemented.
#[derive(Debug)]
pub struct CommandBuffer {
    context: Direct3D11::ID3D11DeviceContext,
    device: Direct3D11::ID3D11Device,
}

impl CommandBuffer {
    pub(crate) fn new(
        context: Direct3D11::ID3D11DeviceContext,
        device: Direct3D11::ID3D11Device,
    ) -> Self {
        Self { context, device }
    }
}

impl crate::CommandEncoder for CommandBuffer {
    type A = Api;

    unsafe fn begin_encoding(&mut self, _label: crate::Label) -> Result<(), crate::DeviceError> {
        Ok(())
    }

    unsafe fn discard_encoding(&mut self) {}

    unsafe fn end_encoding(&mut self) -> Result<CommandBuffer, crate::DeviceError> {
        // The supported commands have already been executed on the immediate
        // context, so the returned buffer only has to carry the context forward
        // for `Queue::submit` (which currently just flushes).
        Ok(CommandBuffer::new(
            self.context.clone(),
            self.device.clone(),
        ))
    }

    unsafe fn reset_all<I>(&mut self, _command_buffers: I) {}

    unsafe fn transition_buffers<'a, T>(&mut self, _barriers: T)
    where
        T: Iterator<Item = crate::BufferBarrier<'a, Buffer>>,
    {
    }

    unsafe fn transition_textures<'a, T>(&mut self, _barriers: T)
    where
        T: Iterator<Item = crate::TextureBarrier<'a, Texture>>,
    {
    }

    unsafe fn clear_buffer(&mut self, buffer: &Buffer, range: crate::MemoryRange) {
        if range.start >= range.end || range.end > buffer.size() {
            log::error!("clear_buffer range {:?} is outside the buffer", range);
            return;
        }

        let size = range.end - range.start;
        match buffer.map_mode() {
            BufferMapMode::Staging if buffer.cpu_writable() => {
                let mut mapped = Direct3D11::D3D11_MAPPED_SUBRESOURCE::default();
                let result = unsafe {
                    self.context.Map(
                        buffer.raw(),
                        0,
                        Direct3D11::D3D11_MAP_WRITE,
                        0,
                        Some(&mut mapped),
                    )
                };
                if let Err(e) = result {
                    log::error!("failed to map a D3D11 staging buffer for clearing: {e}");
                    return;
                }
                unsafe {
                    ptr::write_bytes(
                        (mapped.pData as *mut u8).add(range.start as usize),
                        0,
                        size as usize,
                    );
                    self.context.Unmap(buffer.raw(), 0);
                }
            }
            BufferMapMode::Dynamic if buffer.cpu_writable() => {
                // `WRITE_DISCARD` invalidates the whole dynamic buffer, so this
                // has to clear more than the requested range. Documented
                // limitation until a UAV/clear path exists.
                log::debug!(
                    "clearing a D3D11 dynamic buffer; the entire buffer is discarded and zeroed"
                );
                let mut mapped = Direct3D11::D3D11_MAPPED_SUBRESOURCE::default();
                let result = unsafe {
                    self.context.Map(
                        buffer.raw(),
                        0,
                        Direct3D11::D3D11_MAP_WRITE_DISCARD,
                        0,
                        Some(&mut mapped),
                    )
                };
                if let Err(e) = result {
                    log::error!("failed to map a D3D11 dynamic buffer for clearing: {e}");
                    return;
                }
                unsafe {
                    ptr::write_bytes(mapped.pData as *mut u8, 0, buffer.size() as usize);
                    self.context.Unmap(buffer.raw(), 0);
                }
            }
            _ => {
                let mut zeros = Vec::<u8>::new();
                zeros.resize(align_to(size, 4) as usize, 0);
                let staging = match create_staging_buffer(&self.device, size, Some(&zeros[..])) {
                    Ok(staging) => staging,
                    Err(e) => {
                        log::error!("failed to create a staging buffer for clear_buffer: {e}");
                        return;
                    }
                };
                let Some(src_box) = buffer_box(0, size) else {
                    log::error!("clear_buffer range does not fit in a D3D11 box");
                    return;
                };
                unsafe {
                    self.context.CopySubresourceRegion(
                        buffer.raw(),
                        0,
                        range.start as u32,
                        0,
                        0,
                        &staging,
                        0,
                        Some(&src_box),
                    );
                }
            }
        }
    }

    unsafe fn copy_buffer_to_buffer<T>(&mut self, src: &Buffer, dst: &Buffer, regions: T)
    where
        T: Iterator<Item = crate::BufferCopy>,
    {
        for copy in regions {
            let size = copy.size.get();
            if size == 0 {
                continue;
            }
            if copy.src_offset + size > src.size() || copy.dst_offset + size > dst.size() {
                log::error!(
                    "buffer copy {:?} -> {:?} is outside the source or destination buffer",
                    copy.src_offset..copy.src_offset + size,
                    copy.dst_offset..copy.dst_offset + size,
                );
                continue;
            }
            if dst.map_mode() == BufferMapMode::Dynamic {
                log::error!("D3D11 cannot use a dynamic buffer as a copy destination");
                continue;
            }

            if copy.src_offset == 0
                && copy.dst_offset == 0
                && size == src.size()
                && size == dst.size()
                && src.map_mode() != BufferMapMode::Dynamic
            {
                unsafe { self.context.CopyResource(dst.raw(), src.raw()) };
                continue;
            }

            let Some(src_box) = buffer_box(copy.src_offset, size) else {
                log::error!("buffer copy does not fit in a D3D11 box");
                continue;
            };
            let Some(dst_x) = u32::try_from(copy.dst_offset).ok() else {
                log::error!("buffer copy destination offset exceeds u32::MAX");
                continue;
            };

            unsafe {
                self.context.CopySubresourceRegion(
                    dst.raw(),
                    0,
                    dst_x,
                    0,
                    0,
                    src.raw(),
                    0,
                    Some(&src_box),
                )
            };
        }
    }

    #[cfg(webgl)]
    unsafe fn copy_external_image_to_texture<T>(
        &mut self,
        _src: &wgt::CopyExternalImageSourceInfo,
        _dst: &Texture,
        _dst_premultiplication: bool,
        _regions: T,
    ) where
        T: Iterator<Item = crate::TextureCopy>,
    {
    }

    unsafe fn copy_texture_to_texture<T>(
        &mut self,
        src: &Texture,
        _src_usage: wgt::TextureUses,
        dst: &Texture,
        regions: T,
    ) where
        T: Iterator<Item = crate::TextureCopy>,
    {
        if src.format() != dst.format() {
            log::error!(
                "D3D11 texture copies require matching formats ({:?} -> {:?})",
                src.format(),
                dst.format()
            );
            return;
        }

        let format = src.format();
        let full_aspects = crate::FormatAspects::from(format);

        for copy in regions {
            if !copy.src_base.aspect.is_one()
                || !copy.dst_base.aspect.is_one()
                || copy.src_base.aspect != full_aspects
                || copy.dst_base.aspect != full_aspects
            {
                log::error!(
                    "D3D11 texture copies cannot select or combine depth/stencil aspects yet"
                );
                continue;
            }
            if copy.src_base.origin.z != 0 || copy.dst_base.origin.z != 0 || copy.size.depth != 1 {
                log::error!("D3D11 texture copies currently support single 2D slices only");
                continue;
            }

            let Some(src_box) = make_box(&copy.src_base.origin, &copy.size) else {
                log::error!("texture copy source box overflowed");
                continue;
            };

            unsafe {
                self.context.CopySubresourceRegion(
                    dst.raw(),
                    dst.subresource_index(copy.dst_base.mip_level, copy.dst_base.array_layer),
                    copy.dst_base.origin.x,
                    copy.dst_base.origin.y,
                    0,
                    src.raw(),
                    src.subresource_index(copy.src_base.mip_level, copy.src_base.array_layer),
                    Some(&src_box),
                )
            };
        }
    }

    unsafe fn copy_buffer_to_texture<T>(&mut self, src: &Buffer, dst: &Texture, regions: T)
    where
        T: Iterator<Item = crate::BufferTextureCopy>,
    {
        let format = dst.format();
        let full_aspects = crate::FormatAspects::from(format);

        for copy in regions {
            let aspect = copy.texture_base.aspect;
            if !aspect.is_one() || aspect != full_aspects {
                log::error!(
                    "D3D11 buffer-to-texture copies cannot select or combine depth/stencil aspects yet"
                );
                continue;
            }
            if copy.texture_base.origin.z != 0 || copy.size.depth != 1 {
                log::error!(
                    "D3D11 buffer-to-texture copies currently support single 2D slices only"
                );
                continue;
            }

            let Some((pitch, row_bytes, block_height)) =
                bytes_per_row(format, aspect.map(), copy.size.width, &copy.buffer_layout)
            else {
                log::error!("invalid D3D11 buffer-to-texture row layout");
                continue;
            };
            let rows = copy.size.height.div_ceil(block_height);
            if row_bytes == 0 || rows == 0 {
                continue;
            }
            let data_size = u64::from(pitch) * u64::from(rows);
            if copy.buffer_layout.offset + data_size > src.size() {
                log::error!("buffer-to-texture copy reads past the end of the source buffer");
                continue;
            }

            let staging = match create_staging_buffer(&self.device, data_size, None) {
                Ok(staging) => staging,
                Err(e) => {
                    log::error!("failed to create a staging buffer for texture upload: {e}");
                    continue;
                }
            };

            let Some(src_box) = buffer_box(copy.buffer_layout.offset, data_size) else {
                log::error!("buffer-to-texture source range does not fit in a D3D11 box");
                continue;
            };
            unsafe {
                self.context.CopySubresourceRegion(
                    &staging,
                    0,
                    0,
                    0,
                    0,
                    src.raw(),
                    0,
                    Some(&src_box),
                )
            };

            let mut mapped = Direct3D11::D3D11_MAPPED_SUBRESOURCE::default();
            if let Err(e) = unsafe {
                self.context.Map(
                    &staging,
                    0,
                    Direct3D11::D3D11_MAP_READ,
                    0,
                    Some(&mut mapped),
                )
            } {
                log::error!("failed to map the staging buffer for texture upload: {e}");
                continue;
            }

            let Some(dst_box) = make_box(&copy.texture_base.origin, &copy.size) else {
                unsafe { self.context.Unmap(&staging, 0) };
                log::error!("buffer-to-texture destination box overflowed");
                continue;
            };

            unsafe {
                self.context.UpdateSubresource(
                    dst.raw(),
                    dst.subresource_index(
                        copy.texture_base.mip_level,
                        copy.texture_base.array_layer,
                    ),
                    Some(&dst_box),
                    mapped.pData as *const c_void,
                    pitch,
                    pitch * rows,
                );
                self.context.Unmap(&staging, 0);
            }
        }
    }

    unsafe fn copy_texture_to_buffer<T>(
        &mut self,
        src: &Texture,
        _src_usage: wgt::TextureUses,
        dst: &Buffer,
        regions: T,
    ) where
        T: Iterator<Item = crate::BufferTextureCopy>,
    {
        let format = src.format();
        let full_aspects = crate::FormatAspects::from(format);
        let dxgi_format = match super::conv::map_texture_format(format) {
            Some(format) => format,
            None => {
                log::error!("texture format is not supported for D3D11 read-back");
                return;
            }
        };

        for copy in regions {
            let aspect = copy.texture_base.aspect;
            if !aspect.is_one() || aspect != full_aspects {
                log::error!(
                    "D3D11 texture-to-buffer copies cannot select or combine depth/stencil aspects yet"
                );
                continue;
            }
            if copy.texture_base.origin.z != 0 || copy.size.depth != 1 {
                log::error!(
                    "D3D11 texture-to-buffer copies currently support single 2D slices only"
                );
                continue;
            }

            let Some((dst_pitch, row_bytes, block_height)) =
                bytes_per_row(format, aspect.map(), copy.size.width, &copy.buffer_layout)
            else {
                log::error!("invalid D3D11 texture-to-buffer row layout");
                continue;
            };
            let rows = copy.size.height.div_ceil(block_height);
            if row_bytes == 0 || rows == 0 {
                continue;
            }
            let data_size = u64::from(dst_pitch) * u64::from(rows);
            if copy.buffer_layout.offset + data_size > dst.size() {
                log::error!("texture-to-buffer copy writes past the end of the destination buffer");
                continue;
            }
            if dst.map_mode() == BufferMapMode::Dynamic {
                log::error!("D3D11 cannot use a dynamic buffer as a copy destination");
                continue;
            }

            let staging_texture = match create_staging_texture(
                &self.device,
                dxgi_format,
                copy.size.width,
                copy.size.height,
            ) {
                Ok(texture) => texture,
                Err(e) => {
                    log::error!("failed to create a staging texture for read-back: {e}");
                    continue;
                }
            };

            let Some(src_box) = make_box(&copy.texture_base.origin, &copy.size) else {
                log::error!("texture-to-buffer source box overflowed");
                continue;
            };
            unsafe {
                self.context.CopySubresourceRegion(
                    &staging_texture,
                    0,
                    0,
                    0,
                    0,
                    src.raw(),
                    src.subresource_index(
                        copy.texture_base.mip_level,
                        copy.texture_base.array_layer,
                    ),
                    Some(&src_box),
                )
            };

            let mut mapped = Direct3D11::D3D11_MAPPED_SUBRESOURCE::default();
            if let Err(e) = unsafe {
                self.context.Map(
                    &staging_texture,
                    0,
                    Direct3D11::D3D11_MAP_READ,
                    0,
                    Some(&mut mapped),
                )
            } {
                log::error!("failed to map the staging texture for read-back: {e}");
                continue;
            }

            let mut data = Vec::<u8>::new();
            // D3D11 copies `ByteWidth` bytes from the initial-data pointer, and
            // staging buffers are rounded up to four bytes. Keep the CPU buffer
            // at least that large even when the visible texture rows are not.
            data.resize(align_to(data_size, 4) as usize, 0);
            for row in 0..rows as usize {
                let src_row =
                    unsafe { (mapped.pData as *const u8).add(row * mapped.RowPitch as usize) };
                let dst_row = unsafe { data.as_mut_ptr().add(row * dst_pitch as usize) };
                unsafe { ptr::copy_nonoverlapping(src_row, dst_row, row_bytes as usize) };
            }
            unsafe { self.context.Unmap(&staging_texture, 0) };

            let staging_buffer =
                match create_staging_buffer(&self.device, data_size, Some(data.as_slice())) {
                    Ok(buffer) => buffer,
                    Err(e) => {
                        log::error!("failed to create a staging buffer for texture read-back: {e}");
                        continue;
                    }
                };

            let Some(dst_offset) = u32::try_from(copy.buffer_layout.offset).ok() else {
                log::error!("texture-to-buffer destination offset exceeds u32::MAX");
                continue;
            };
            let Some(src_box) = buffer_box(0, data_size) else {
                log::error!("texture-to-buffer staging range does not fit in a D3D11 box");
                continue;
            };
            unsafe {
                self.context.CopySubresourceRegion(
                    dst.raw(),
                    0,
                    dst_offset,
                    0,
                    0,
                    &staging_buffer,
                    0,
                    Some(&src_box),
                )
            };
        }
    }

    unsafe fn copy_acceleration_structure_to_acceleration_structure(
        &mut self,
        _src: &Resource,
        _dst: &Resource,
        _copy: wgt::AccelerationStructureCopy,
    ) {
    }

    unsafe fn begin_query(&mut self, _set: &Resource, _index: u32) {}
    unsafe fn end_query(&mut self, _set: &Resource, _index: u32) {}
    unsafe fn write_timestamp(&mut self, _set: &Resource, _index: u32) {}
    unsafe fn read_acceleration_structure_compact_size(
        &mut self,
        _acceleration_structure: &Resource,
        _buf: &Buffer,
    ) {
    }
    unsafe fn reset_queries(&mut self, _set: &Resource, _range: Range<u32>) {}
    unsafe fn copy_query_results(
        &mut self,
        _set: &Resource,
        _range: Range<u32>,
        _buffer: &Buffer,
        _offset: wgt::BufferAddress,
        _stride: wgt::BufferSize,
    ) {
    }

    // render

    unsafe fn begin_render_pass(
        &mut self,
        desc: &crate::RenderPassDescriptor<Resource, TextureView>,
    ) -> Result<(), crate::DeviceError> {
        for attachment in desc.color_attachments.iter().flatten() {
            // D3D11 clears do not require the render target to be bound, so the
            // pass can stay a no-op apart from the clear. `LOAD`/`STORE` and
            // actual draw operations are not implemented yet.
            if attachment.ops.contains(crate::AttachmentOps::LOAD_CLEAR) {
                let color = attachment.clear_value;
                let color = [
                    color.r as f32,
                    color.g as f32,
                    color.b as f32,
                    color.a as f32,
                ];
                unsafe {
                    self.context
                        .ClearRenderTargetView(attachment.target.view.raw(), &color)
                };
            }
        }

        Ok(())
    }
    unsafe fn end_render_pass(&mut self) {}

    unsafe fn set_bind_group(
        &mut self,
        _layout: &PipelineLayout,
        _index: u32,
        _group: &BindGroup,
        _dynamic_offsets: &[wgt::DynamicOffset],
    ) {
        // Bind groups are CPU-side descriptions until render pipelines exist.
    }

    unsafe fn set_immediates(
        &mut self,
        _layout: &PipelineLayout,
        _offset_bytes: u32,
        _data: &[u32],
    ) {
    }

    unsafe fn insert_debug_marker(&mut self, _label: &str) {}
    unsafe fn begin_debug_marker(&mut self, _group_label: &str) {}
    unsafe fn end_debug_marker(&mut self) {}

    unsafe fn set_render_pipeline(&mut self, _pipeline: &Resource) {}

    unsafe fn set_index_buffer<'a>(
        &mut self,
        _binding: crate::BufferBinding<'a, Buffer>,
        _format: wgt::IndexFormat,
    ) {
    }

    unsafe fn set_vertex_buffer<'a>(
        &mut self,
        _index: u32,
        _binding: crate::BufferBinding<'a, Buffer>,
    ) {
    }

    unsafe fn set_viewport(&mut self, _rect: &crate::Rect<f32>, _depth_range: Range<f32>) {}
    unsafe fn set_scissor_rect(&mut self, _rect: &crate::Rect<u32>) {}
    unsafe fn set_stencil_reference(&mut self, _value: u32) {}
    unsafe fn set_blend_constants(&mut self, _color: &[f32; 4]) {}

    unsafe fn draw(
        &mut self,
        _first_vertex: u32,
        _vertex_count: u32,
        _first_instance: u32,
        _instance_count: u32,
    ) {
    }

    unsafe fn draw_indexed(
        &mut self,
        _first_index: u32,
        _index_count: u32,
        _base_vertex: i32,
        _first_instance: u32,
        _instance_count: u32,
    ) {
    }

    unsafe fn draw_mesh_tasks(
        &mut self,
        _group_count_x: u32,
        _group_count_y: u32,
        _group_count_z: u32,
    ) {
    }

    unsafe fn draw_indirect(
        &mut self,
        _buffer: &Buffer,
        _offset: wgt::BufferAddress,
        _draw_count: u32,
    ) {
    }

    unsafe fn draw_indexed_indirect(
        &mut self,
        _buffer: &Buffer,
        _offset: wgt::BufferAddress,
        _draw_count: u32,
    ) {
    }

    unsafe fn draw_mesh_tasks_indirect(
        &mut self,
        _buffer: &Buffer,
        _offset: wgt::BufferAddress,
        _draw_count: u32,
    ) {
    }

    unsafe fn draw_indirect_count(
        &mut self,
        _buffer: &Buffer,
        _offset: wgt::BufferAddress,
        _count_buffer: &Buffer,
        _count_offset: wgt::BufferAddress,
        _max_count: u32,
    ) {
    }

    unsafe fn draw_indexed_indirect_count(
        &mut self,
        _buffer: &Buffer,
        _offset: wgt::BufferAddress,
        _count_buffer: &Buffer,
        _count_offset: wgt::BufferAddress,
        _max_count: u32,
    ) {
    }

    unsafe fn draw_mesh_tasks_indirect_count(
        &mut self,
        _buffer: &Buffer,
        _offset: wgt::BufferAddress,
        _count_buffer: &Buffer,
        _count_offset: wgt::BufferAddress,
        _max_count: u32,
    ) {
    }

    // compute

    unsafe fn begin_compute_pass(&mut self, _desc: &crate::ComputePassDescriptor<Resource>) {}
    unsafe fn end_compute_pass(&mut self) {}

    unsafe fn set_compute_pipeline(&mut self, _pipeline: &Resource) {}

    unsafe fn dispatch_workgroups(&mut self, _count: [u32; 3]) {}
    unsafe fn dispatch_workgroups_indirect(
        &mut self,
        _buffer: &Buffer,
        _offset: wgt::BufferAddress,
    ) {
    }

    unsafe fn build_acceleration_structures<'a, T>(
        &mut self,
        _descriptor_count: u32,
        _descriptors: T,
    ) where
        Api: 'a,
        T: IntoIterator<Item = crate::BuildAccelerationStructureDescriptor<'a, Buffer, Resource>>,
    {
    }

    unsafe fn place_acceleration_structure_barrier(
        &mut self,
        _barriers: crate::AccelerationStructureBarrier,
    ) {
    }

    unsafe fn set_acceleration_structure_dependencies(
        _command_buffers: &[&CommandBuffer],
        _dependencies: &[&Resource],
    ) {
    }

    unsafe fn begin_ray_tracing_pass(&mut self, _desc: &crate::RayTracingPassDescriptor) {}

    unsafe fn end_ray_tracing_pass(&mut self) {}

    unsafe fn set_ray_tracing_pipeline(&mut self, _pipeline: &Resource) {}

    unsafe fn trace_rays(
        &mut self,
        _count: [u32; 3],
        _ray_generation_group_data: crate::PipelineGroupData<Buffer>,
        _miss_group_data: crate::PipelineGroupData<Buffer>,
        _intersection_group_data: crate::PipelineGroupData<Buffer>,
    ) {
    }
}

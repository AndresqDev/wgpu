//! Minimal D3D11 command encoder.
//!
//! D3D11 records commands on an `ID3D11DeviceContext`. This milestone does not
//! create deferred contexts yet: the encoder keeps a clone of the device's
//! immediate context and executes supported commands immediately as they are
//! recorded. [`crate::Queue::submit`] then flushes that context.
//!
//! Render pipelines, render-pass attachments, bind groups, vertex/index
//! buffers and draw calls are now forwarded to the immediate context. Dynamic
//! uniform offsets use `ID3D11DeviceContext1::*SetConstantBuffers1`; when that
//! interface is unavailable, or when the requested offset cannot be expressed
//! with D3D11.1's 256-byte-aligned constant-buffer windows, the binding is
//! rejected explicitly instead of being silently mis-bound.
//!
//! Push constants are not implemented: `set_immediates` records an explicit
//! encoder error so `end_encoding` rejects the command buffer instead of
//! submitting a draw that silently reads zeros.

use alloc::{borrow::ToOwned, format, string::String, vec::Vec};
use core::{
    ffi::c_void,
    ops::{Deref, Range},
    ptr,
    sync::atomic::{AtomicUsize, Ordering},
};

use parking_lot::lock_api::RawMutex as _;
use windows::{
    core::{Interface as _, BOOL},
    Win32::{
        Foundation,
        Graphics::{Direct3D11, Dxgi},
    },
};

use super::{
    Api, BindGroup, Buffer, BufferMapMode, PipelineLayout, RenderPipeline, Resource, Texture,
    TextureView,
};

/// Serializes access to the immediate D3D11 context.
///
/// Commands execute while they are recorded, so the lock is held for the
/// complete command-buffer encoding. Nesting is allowed because wgpu-core
/// records internal uploads on the same thread while a caller encoder is open.
static CONTEXT_LOCK: parking_lot::RawMutex = parking_lot::RawMutex::INIT;
static CONTEXT_OWNER: AtomicUsize = AtomicUsize::new(0);
static CONTEXT_DEPTH: AtomicUsize = AtomicUsize::new(0);

thread_local! {
    static CONTEXT_THREAD_KEY: u8 = 0;
}

fn context_thread_id() -> usize {
    CONTEXT_THREAD_KEY.with(|key| key as *const u8 as usize)
}

#[derive(Debug)]
pub(crate) struct ContextGuard;

impl ContextGuard {
    pub(crate) fn acquire() -> Self {
        let id = context_thread_id();
        if CONTEXT_OWNER.load(Ordering::Acquire) == id {
            CONTEXT_DEPTH.fetch_add(1, Ordering::AcqRel);
        } else {
            CONTEXT_LOCK.lock();
            CONTEXT_OWNER.store(id, Ordering::Release);
            CONTEXT_DEPTH.store(1, Ordering::Release);
        }
        Self
    }
}

impl Drop for ContextGuard {
    fn drop(&mut self) {
        if CONTEXT_DEPTH.fetch_sub(1, Ordering::AcqRel) == 1 {
            CONTEXT_OWNER.store(0, Ordering::Release);
            unsafe { CONTEXT_LOCK.unlock() };
        }
    }
}

pub(crate) fn lock_context() -> ContextGuard {
    ContextGuard::acquire()
}

struct ContextRef<'a> {
    _guard: ContextGuard,
    raw: &'a Direct3D11::ID3D11DeviceContext,
}

impl Deref for ContextRef<'_> {
    type Target = Direct3D11::ID3D11DeviceContext;

    fn deref(&self) -> &Self::Target {
        self.raw
    }
}

struct Context1Ref<'a> {
    _guard: ContextGuard,
    raw: &'a Direct3D11::ID3D11DeviceContext1,
}

impl Deref for Context1Ref<'_> {
    type Target = Direct3D11::ID3D11DeviceContext1;

    fn deref(&self) -> &Self::Target {
        self.raw
    }
}

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
    cpu_access_flags: u32,
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
        CPUAccessFlags: cpu_access_flags,
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

/// Pipeline state that the encoder needs while recording draw commands.
#[derive(Clone, Debug)]
struct PipelineState {
    vertex_strides: Vec<Option<u32>>,
}

/// Attachment formats captured by `begin_render_pass` for pipeline checks.
#[derive(Clone, Debug)]
struct RenderPassState {
    color_formats: Vec<Option<Dxgi::Common::DXGI_FORMAT>>,
    depth_format: Option<Dxgi::Common::DXGI_FORMAT>,
    sample_count: u32,
}

/// Removes trailing empty color attachments, which D3D11 omits from the
/// render target array and pipelines may leave out.
fn color_formats_trimmed(
    formats: &[Option<Dxgi::Common::DXGI_FORMAT>],
) -> &[Option<Dxgi::Common::DXGI_FORMAT>] {
    let mut len = formats.len();
    while len > 0 && formats[len - 1].is_none() {
        len -= 1;
    }
    &formats[..len]
}

impl RenderPassState {
    /// Checks that `pipeline` can be used with this render pass without
    /// creating an invalid D3D11 pipeline combination.
    fn validate_pipeline(&self, pipeline: &RenderPipeline) -> Result<(), String> {
        if self.sample_count != pipeline.sample_count() {
            return Err(format!(
                "sample count mismatch: render pass has {}, pipeline has {}",
                self.sample_count,
                pipeline.sample_count()
            ));
        }
        if color_formats_trimmed(&self.color_formats)
            != color_formats_trimmed(pipeline.color_formats())
        {
            return Err(format!(
                "color formats mismatch: render pass has {:?}, pipeline has {:?}",
                self.color_formats,
                pipeline.color_formats()
            ));
        }
        if self.depth_format != pipeline.depth_format() {
            return Err(format!(
                "depth format mismatch: render pass has {:?}, pipeline has {:?}",
                self.depth_format,
                pipeline.depth_format()
            ));
        }
        Ok(())
    }
}

/// Command buffer type, which also acts as the command encoder.
///
/// Each encoder records into a deferred D3D11 context; the queue executes the
/// resulting command lists in submission order.
#[derive(Debug)]
pub struct CommandBuffer {
    raw_context: Direct3D11::ID3D11DeviceContext,
    raw_context1: Option<Direct3D11::ID3D11DeviceContext1>,
    raw_immediate: Direct3D11::ID3D11DeviceContext,
    command_list: Option<Direct3D11::ID3D11CommandList>,
    device: Direct3D11::ID3D11Device,
    current_pipeline: Option<PipelineState>,
    current_blend_state: Option<Direct3D11::ID3D11BlendState>,
    current_depth_stencil_state: Option<Direct3D11::ID3D11DepthStencilState>,
    blend_constants: [f32; 4],
    sample_mask: u32,
    stencil_reference: u32,
    render_pass: Option<RenderPassState>,
    /// Whether `IASetIndexBuffer` has been called for the current render pass.
    index_buffer_set: bool,
    /// First explicit encoding error. `end_encoding` reports it instead of
    /// producing a command buffer that would submit partial state.
    error: Option<String>,
}

impl CommandBuffer {
    pub(crate) fn new(
        context: Direct3D11::ID3D11DeviceContext,
        immediate: Direct3D11::ID3D11DeviceContext,
        device: Direct3D11::ID3D11Device,
        command_list: Option<Direct3D11::ID3D11CommandList>,
    ) -> Self {
        let raw_context1 = context.cast::<Direct3D11::ID3D11DeviceContext1>().ok();
        Self {
            raw_context: context,
            raw_context1,
            raw_immediate: immediate,
            command_list,
            device,
            current_pipeline: None,
            current_blend_state: None,
            current_depth_stencil_state: None,
            blend_constants: [0.0; 4],
            sample_mask: u32::MAX,
            stencil_reference: 0,
            render_pass: None,
            index_buffer_set: false,
            error: None,
        }
    }

    fn context(&self) -> ContextRef<'_> {
        ContextRef {
            _guard: lock_context(),
            raw: &self.raw_context,
        }
    }

    fn context1(&self) -> Option<Context1Ref<'_>> {
        let guard = lock_context();
        self.raw_context1
            .as_ref()
            .map(move |raw| Context1Ref { _guard: guard, raw })
    }

    fn immediate(&self) -> ContextRef<'_> {
        ContextRef {
            _guard: lock_context(),
            raw: &self.raw_immediate,
        }
    }

    pub(crate) fn command_list(&self) -> Option<&Direct3D11::ID3D11CommandList> {
        self.command_list.as_ref()
    }

    /// Records the first explicit encoding error so [`crate::CommandEncoder::end_encoding`]
    /// rejects the command buffer instead of submitting partial state.
    fn encoder_error(&mut self, message: String) {
        log::error!("D3D11 command encoding: {message}");
        if self.error.is_none() {
            self.error = Some(message);
        }
    }
}

impl crate::CommandEncoder for CommandBuffer {
    type A = Api;

    unsafe fn begin_encoding(&mut self, _label: crate::Label) -> Result<(), crate::DeviceError> {
        self.error = None;
        self.render_pass = None;
        self.current_pipeline = None;
        self.index_buffer_set = false;
        Ok(())
    }

    unsafe fn discard_encoding(&mut self) {
        self.error = None;
        self.render_pass = None;
        self.current_pipeline = None;
        self.index_buffer_set = false;
    }

    unsafe fn end_encoding(&mut self) -> Result<CommandBuffer, crate::DeviceError> {
        if let Some(error) = &self.error {
            log::error!("D3D11 command encoding failed: {error}");
            return Err(crate::DeviceError::Unexpected);
        }
        let mut list = None;
        self.raw_context
            .FinishCommandList(false, Some(&mut list))
            .map_err(|error| {
                log::error!("ID3D11DeviceContext::FinishCommandList failed: {error}");
                crate::DeviceError::Unexpected
            })?;
        let list = list.ok_or_else(|| {
            log::error!("FinishCommandList succeeded without returning a command list");
            crate::DeviceError::Unexpected
        })?;
        Ok(CommandBuffer::new(
            self.raw_context.clone(),
            self.raw_immediate.clone(),
            self.device.clone(),
            Some(list),
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
        // D3D11 copies and clears work in 4-byte units; the backend advertises
        // that alignment, so reject any range that could not be expressed as a
        // subresource box.
        if range.start % 4 != 0 || size % 4 != 0 {
            log::error!(
                "D3D11 buffer clears require a 4-byte aligned range, got {:?}",
                range
            );
            return;
        }
        match buffer.map_mode() {
            BufferMapMode::Staging if buffer.cpu_writable() => {
                let mut mapped = Direct3D11::D3D11_MAPPED_SUBRESOURCE::default();
                let result = unsafe {
                    self.immediate().Map(
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
                    self.immediate().Unmap(buffer.raw(), 0);
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
                    self.immediate().Map(
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
                    self.immediate().Unmap(buffer.raw(), 0);
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
                    self.context().CopySubresourceRegion(
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
            // D3D11 expresses buffer copies through a subresource box, whose
            // coordinates are 32-bit byte offsets, and requires 4-byte
            // multiples. Reject unaligned copies instead of rounding them.
            if copy.src_offset % 4 != 0 || copy.dst_offset % 4 != 0 || size % 4 != 0 {
                log::error!(
                    "D3D11 buffer copies require 4-byte aligned offsets and size, got {} -> {} with size {size}",
                    copy.src_offset,
                    copy.dst_offset
                );
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
                unsafe { self.context().CopyResource(dst.raw(), src.raw()) };
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
                self.context().CopySubresourceRegion(
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
        if src.dimension() != wgt::TextureDimension::D2
            || dst.dimension() != wgt::TextureDimension::D2
        {
            log::debug!("D3D11 volume texture copies are not implemented; skipping");
            return;
        }
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
                self.context().CopySubresourceRegion(
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
        if dst.dimension() != wgt::TextureDimension::D2 {
            log::debug!("D3D11 non-2D texture uploads are not implemented; skipping placeholder");
            return;
        }
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

            // Validate the destination before issuing a D3D11 copy. An
            // out-of-range subresource or box is undefined in D3D11 and can
            // remove the device rather than returning an error.
            let dst_extent = dst.extent();
            let within_destination = copy
                .texture_base
                .origin
                .x
                .checked_add(copy.size.width)
                .is_some_and(|right| right <= dst_extent.width)
                && copy
                    .texture_base
                    .origin
                    .y
                    .checked_add(copy.size.height)
                    .is_some_and(|bottom| bottom <= dst_extent.height);
            if copy.size.width == 0
                || copy.size.height == 0
                || copy.texture_base.mip_level >= dst.mip_level_count()
                || copy.texture_base.array_layer >= dst.array_layer_count()
                || !within_destination
            {
                log::error!(
                    "invalid D3D11 buffer-to-texture destination: format={format:?}, mip={}, layer={} (of {}), origin=({}, {}), size={}x{}, extent={}x{}",
                    copy.texture_base.mip_level,
                    copy.texture_base.array_layer,
                    dst.array_layer_count(),
                    copy.texture_base.origin.x,
                    copy.texture_base.origin.y,
                    copy.size.width,
                    copy.size.height,
                    dst_extent.width,
                    dst_extent.height
                );
                continue;
            }

            // Queue uploads are backed by a CPU-readable staging buffer. Copy
            // directly from that mapping into a DEFAULT colour texture with
            // UpdateSubresource. This avoids an unnecessary GPU buffer copy
            // and the staging-texture round trip for the common upload path.
            // R8 coverage pages are used by the demand-filled font atlas. Keep
            // them on the staging-texture path; UpdateSubresource with a
            // sub-rectangle on single-channel array pages is not reliable on
            // every D3D11 driver.
            let direct_upload =
                !dst.format().has_depth_aspect() && dst.format() != wgt::TextureFormat::R8Unorm;
            if direct_upload && src.cpu_readable() && !src.set_mapped() {
                log::error!(
                    "D3D11 buffer-to-texture source is already mapped; refusing an invalid copy"
                );
                continue;
            }
            if direct_upload && src.cpu_readable() {
                let map_raw = src.staging().unwrap_or_else(|| src.raw());
                let mut mapped_src = Direct3D11::D3D11_MAPPED_SUBRESOURCE::default();
                let mapped = unsafe {
                    self.immediate().Map(
                        map_raw,
                        0,
                        Direct3D11::D3D11_MAP_READ,
                        0,
                        Some(&mut mapped_src),
                    )
                };
                if mapped.is_ok() && !mapped_src.pData.is_null() {
                    if let Some(dst_box) = make_box(&copy.texture_base.origin, &copy.size) {
                        let source = unsafe {
                            (mapped_src.pData as *const u8).add(copy.buffer_layout.offset as usize)
                        };
                        unsafe {
                            self.immediate().UpdateSubresource(
                                dst.raw(),
                                dst.subresource_index(
                                    copy.texture_base.mip_level,
                                    copy.texture_base.array_layer,
                                ),
                                Some(&dst_box as *const Direct3D11::D3D11_BOX),
                                source.cast(),
                                pitch,
                                pitch.saturating_mul(rows),
                            );
                            self.immediate().Unmap(map_raw, 0);
                        }
                        src.set_unmapped();
                        continue;
                    }
                } else if let Err(error) = mapped {
                    log::warn!("failed to map the CPU upload buffer for texture upload: {error}");
                }
                unsafe { self.immediate().Unmap(map_raw, 0) };
                src.set_unmapped();
            }

            // The staging-buffer fallback addresses rows in 4-byte units. The
            // direct UpdateSubresource path above supports arbitrary R8 pitches.
            if copy.buffer_layout.offset % 4 != 0
                || copy
                    .buffer_layout
                    .bytes_per_row
                    .is_some_and(|bytes_per_row| bytes_per_row % 4 != 0)
            {
                log::error!(
                    "D3D11 staging buffer-to-texture copies require 4-byte aligned offsets and row pitches"
                );
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
                // Uploads run before the frame's command lists are executed.
                // Use the immediate context so the CPU mapping below observes the copy.
                self.immediate().CopySubresourceRegion(
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

            let dxgi_format = match super::conv::map_texture_format_resource(format) {
                Some(format) => format,
                None => {
                    log::error!("texture format is not supported for D3D11 upload");
                    continue;
                }
            };

            // `UpdateSubresource` cannot target depth-stencil resources, so
            // stage the rows in a writable staging texture and copy from there.
            let staging_texture = match create_staging_texture(
                &self.device,
                dxgi_format,
                copy.size.width,
                copy.size.height,
                Direct3D11::D3D11_CPU_ACCESS_WRITE.0 as u32,
            ) {
                Ok(texture) => texture,
                Err(e) => {
                    log::error!("failed to create a staging texture for texture upload: {e}");
                    continue;
                }
            };

            let mut mapped_src = Direct3D11::D3D11_MAPPED_SUBRESOURCE::default();
            if let Err(e) = unsafe {
                self.immediate().Map(
                    &staging,
                    0,
                    Direct3D11::D3D11_MAP_READ,
                    0,
                    Some(&mut mapped_src),
                )
            } {
                log::error!("failed to map the staging buffer for texture upload: {e}");
                continue;
            }

            let mut mapped_dst = Direct3D11::D3D11_MAPPED_SUBRESOURCE::default();
            if let Err(e) = unsafe {
                self.immediate().Map(
                    &staging_texture,
                    0,
                    Direct3D11::D3D11_MAP_WRITE,
                    0,
                    Some(&mut mapped_dst),
                )
            } {
                unsafe { self.immediate().Unmap(&staging, 0) };
                log::error!("failed to map the staging texture for texture upload: {e}");
                continue;
            }

            for row in 0..rows as usize {
                let src_row = unsafe { (mapped_src.pData as *const u8).add(row * pitch as usize) };
                let dst_row = unsafe {
                    (mapped_dst.pData as *mut u8).add(row * mapped_dst.RowPitch as usize)
                };
                unsafe { ptr::copy_nonoverlapping(src_row, dst_row, row_bytes as usize) };
            }

            unsafe {
                self.immediate().Unmap(&staging_texture, 0);
                self.immediate().Unmap(&staging, 0);

                let full_depth_subresource = dst.format().has_depth_aspect()
                    && dst.mip_level_count() == 1
                    && dst.array_layer_count() == 1
                    && copy.texture_base.mip_level == 0
                    && copy.texture_base.array_layer == 0
                    && copy.texture_base.origin.x == 0
                    && copy.texture_base.origin.y == 0
                    && copy.texture_base.origin.z == 0
                    && copy.size.width == dst.extent().width
                    && copy.size.height == dst.extent().height
                    && copy.size.depth == 1;

                if full_depth_subresource {
                    // CopySubresourceRegion on a depth-stencil resource must
                    // copy the whole subresource. CopyResource is the
                    // whole-resource form and avoids the depth box restriction.
                    self.context().CopyResource(dst.raw(), &staging_texture);
                } else {
                    self.immediate().CopySubresourceRegion(
                        dst.raw(),
                        dst.subresource_index(
                            copy.texture_base.mip_level,
                            copy.texture_base.array_layer,
                        ),
                        copy.texture_base.origin.x,
                        copy.texture_base.origin.y,
                        0,
                        &staging_texture,
                        0,
                        None,
                    );
                }
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
        if src.dimension() != wgt::TextureDimension::D2 {
            log::debug!("D3D11 non-2D texture readback is not implemented; skipping");
            return;
        }
        let format = src.format();
        let full_aspects = crate::FormatAspects::from(format);
        let dxgi_format = match super::conv::map_texture_format_resource(format) {
            Some(format) => format,
            None => {
                log::error!("texture format is not supported for D3D11 read-back");
                return;
            }
        };

        for copy in regions {
            if copy.buffer_layout.offset % 4 != 0
                || copy
                    .buffer_layout
                    .bytes_per_row
                    .is_some_and(|bytes_per_row| bytes_per_row % 4 != 0)
            {
                log::error!(
                    "D3D11 texture-to-buffer copies require 4-byte aligned offsets and row pitches"
                );
                continue;
            }

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
                Direct3D11::D3D11_CPU_ACCESS_READ.0 as u32,
            ) {
                Ok(texture) => texture,
                Err(e) => {
                    log::error!("failed to create a staging texture for read-back: {e}");
                    continue;
                }
            };

            let full_depth_subresource = src.format().has_depth_aspect()
                && src.mip_level_count() == 1
                && src.array_layer_count() == 1
                && copy.texture_base.mip_level == 0
                && copy.texture_base.array_layer == 0
                && copy.texture_base.origin.x == 0
                && copy.texture_base.origin.y == 0
                && copy.texture_base.origin.z == 0
                && copy.size.width == src.extent().width
                && copy.size.height == src.extent().height
                && copy.size.depth == 1;

            if full_depth_subresource {
                unsafe { self.immediate().CopyResource(&staging_texture, src.raw()) };
            } else {
                let Some(src_box) = make_box(&copy.texture_base.origin, &copy.size) else {
                    log::error!("texture-to-buffer source box overflowed");
                    continue;
                };
                unsafe {
                    self.immediate().CopySubresourceRegion(
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
            }

            let mut mapped = Direct3D11::D3D11_MAPPED_SUBRESOURCE::default();
            if let Err(e) = unsafe {
                self.immediate().Map(
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
            unsafe { self.immediate().Unmap(&staging_texture, 0) };

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
                self.context().CopySubresourceRegion(
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
        self.encoder_error("D3D11 acceleration structure copies are not implemented".to_owned());
    }

    unsafe fn begin_query(&mut self, _set: &Resource, _index: u32) {
        self.encoder_error("D3D11 begin_query is not implemented".to_owned());
    }
    unsafe fn end_query(&mut self, _set: &Resource, _index: u32) {
        self.encoder_error("D3D11 end_query is not implemented".to_owned());
    }
    unsafe fn write_timestamp(&mut self, _set: &Resource, _index: u32) {
        self.encoder_error("D3D11 write_timestamp is not implemented".to_owned());
    }
    unsafe fn read_acceleration_structure_compact_size(
        &mut self,
        _acceleration_structure: &Resource,
        _buf: &Buffer,
    ) {
        self.encoder_error(
            "D3D11 acceleration structure compact size queries are not implemented".to_owned(),
        );
    }
    unsafe fn reset_queries(&mut self, _set: &Resource, _range: Range<u32>) {
        self.encoder_error("D3D11 reset_queries is not implemented".to_owned());
    }
    unsafe fn copy_query_results(
        &mut self,
        _set: &Resource,
        _range: Range<u32>,
        _buffer: &Buffer,
        _offset: wgt::BufferAddress,
        _stride: wgt::BufferSize,
    ) {
        self.encoder_error("D3D11 copy_query_results is not implemented".to_owned());
    }

    // render

    unsafe fn begin_render_pass(
        &mut self,
        desc: &crate::RenderPassDescriptor<Resource, TextureView>,
    ) -> Result<(), crate::DeviceError> {
        if self.render_pass.is_some() {
            log::error!("a D3D11 render pass is already active");
            return Err(crate::DeviceError::Unexpected);
        }
        if desc.sample_count != 1 {
            log::error!(
                "D3D11 render passes currently support sample_count == 1 only, got {}",
                desc.sample_count
            );
            return Err(crate::DeviceError::Unexpected);
        }
        if desc.multiview_mask.is_some() {
            log::error!("D3D11 multiview render passes are not implemented");
            return Err(crate::DeviceError::Unexpected);
        }
        if desc.color_attachments.len()
            > Direct3D11::D3D11_SIMULTANEOUS_RENDER_TARGET_COUNT as usize
        {
            log::error!(
                "D3D11 render passes support at most {} color attachments, got {}",
                Direct3D11::D3D11_SIMULTANEOUS_RENDER_TARGET_COUNT,
                desc.color_attachments.len()
            );
            return Err(crate::DeviceError::Unexpected);
        }

        let mut render_targets: Vec<Option<Direct3D11::ID3D11RenderTargetView>> = Vec::new();
        let mut color_formats: Vec<Option<Dxgi::Common::DXGI_FORMAT>> =
            Vec::with_capacity(desc.color_attachments.len());

        for attachment in desc.color_attachments {
            let Some(attachment) = attachment else {
                render_targets.push(None);
                color_formats.push(None);
                continue;
            };

            if attachment.depth_slice.is_some() {
                log::error!("D3D11 3D render target slices are not implemented");
                return Err(crate::DeviceError::Unexpected);
            }
            if attachment.resolve_target.is_some() {
                log::error!("D3D11 multisampled resolve targets are not implemented");
                return Err(crate::DeviceError::Unexpected);
            }
            if !attachment
                .target
                .usage
                .contains(wgt::TextureUses::COLOR_TARGET)
            {
                log::error!(
                    "D3D11 color attachment usage {:?} is not COLOR_TARGET",
                    attachment.target.usage
                );
                return Err(crate::DeviceError::Unexpected);
            }

            let Some(rtv) = attachment.target.view.raw_rtv() else {
                log::error!("D3D11 color attachment view has no render target view");
                return Err(crate::DeviceError::Unexpected);
            };
            render_targets.push(Some(rtv.clone()));
            color_formats.push(Some(attachment.target.view.format()));
        }

        // D3D11 accepts NULL entries in the render target array, but leaving
        // trailing NULLs out keeps the call and debug output smaller.
        while matches!(render_targets.last(), Some(None)) {
            render_targets.pop();
        }

        let depth_stencil = desc.depth_stencil_attachment.as_ref();
        let depth_format = match depth_stencil {
            Some(depth_stencil) => {
                if !depth_stencil.target.usage.intersects(
                    wgt::TextureUses::DEPTH_STENCIL_READ | wgt::TextureUses::DEPTH_STENCIL_WRITE,
                ) {
                    log::error!(
                        "D3D11 depth attachment usage {:?} is not a depth/stencil usage",
                        depth_stencil.target.usage
                    );
                    return Err(crate::DeviceError::Unexpected);
                }
                if depth_stencil.target.view.raw_dsv().is_none() {
                    log::error!("D3D11 depth attachment view has no depth stencil view");
                    return Err(crate::DeviceError::Unexpected);
                }
                Some(depth_stencil.target.view.format())
            }
            None => None,
        };

        if render_targets.is_empty() && depth_stencil.is_none() {
            log::error!("D3D11 render pass has no attachments");
            return Err(crate::DeviceError::Unexpected);
        }

        let depth_stencil_view = depth_stencil
            .and_then(|attachment| attachment.target.view.raw_dsv())
            .cloned();

        unsafe {
            self.context().OMSetRenderTargets(
                if render_targets.is_empty() {
                    None
                } else {
                    Some(render_targets.as_slice())
                },
                depth_stencil_view.as_ref(),
            );
        }

        for attachment in desc.color_attachments.iter().flatten() {
            if attachment.ops.contains(crate::AttachmentOps::LOAD_CLEAR) {
                let color = attachment.clear_value;
                let color = [
                    color.r as f32,
                    color.g as f32,
                    color.b as f32,
                    color.a as f32,
                ];
                if let Some(rtv) = attachment.target.view.raw_rtv() {
                    unsafe { self.context().ClearRenderTargetView(rtv, &color) };
                }
            }
        }

        if let Some(depth_stencil) = depth_stencil {
            let mut clear_flags = 0u32;
            if depth_stencil
                .depth_ops
                .contains(crate::AttachmentOps::LOAD_CLEAR)
            {
                clear_flags |= Direct3D11::D3D11_CLEAR_DEPTH.0;
            }
            if depth_stencil
                .stencil_ops
                .contains(crate::AttachmentOps::LOAD_CLEAR)
            {
                clear_flags |= Direct3D11::D3D11_CLEAR_STENCIL.0;
            }

            if clear_flags != 0 {
                if let Some(dsv) = depth_stencil.target.view.raw_dsv() {
                    unsafe {
                        self.context().ClearDepthStencilView(
                            dsv,
                            clear_flags,
                            depth_stencil.clear_value.0,
                            depth_stencil.clear_value.1 as u8,
                        )
                    };
                }
            }
        }

        let viewport = Direct3D11::D3D11_VIEWPORT {
            TopLeftX: 0.0,
            TopLeftY: 0.0,
            Width: desc.extent.width as f32,
            Height: desc.extent.height as f32,
            MinDepth: 0.0,
            MaxDepth: 1.0,
        };
        let scissor = Foundation::RECT {
            left: 0,
            top: 0,
            right: i32::try_from(desc.extent.width).unwrap_or(i32::MAX),
            bottom: i32::try_from(desc.extent.height).unwrap_or(i32::MAX),
        };
        unsafe {
            self.context().RSSetViewports(Some(&[viewport]));
            self.context().RSSetScissorRects(Some(&[scissor]));
        }

        self.render_pass = Some(RenderPassState {
            color_formats,
            depth_format,
            sample_count: desc.sample_count,
        });
        // Pipeline and index-buffer state belong to the previous pass.
        self.current_pipeline = None;
        self.index_buffer_set = false;

        Ok(())
    }

    unsafe fn end_render_pass(&mut self) {
        unsafe {
            self.context().OMSetRenderTargets(
                None::<&[Option<Direct3D11::ID3D11RenderTargetView>]>,
                None::<&Direct3D11::ID3D11DepthStencilView>,
            );
        }
        self.render_pass = None;
        self.current_pipeline = None;
        self.index_buffer_set = false;
    }

    unsafe fn set_bind_group(
        &mut self,
        layout: &PipelineLayout,
        index: u32,
        group: &BindGroup,
        dynamic_offsets: &[wgt::DynamicOffset],
    ) {
        // Validate the whole group before touching the D3D11 context. A failed
        // group must leave no partial binding state behind.
        if let Err(error) =
            layout.validate_bind_group(index, group, dynamic_offsets, self.raw_context1.is_some())
        {
            self.encoder_error(format!("set_bind_group group {index}: {error}"));
            return;
        }

        let Some(group_layout) = layout
            .bind_group_layouts()
            .get(index as usize)
            .and_then(|layout| layout.as_ref())
        else {
            self.encoder_error(format!(
                "set_bind_group called for missing bind group {index}"
            ));
            return;
        };

        for bound in group.buffers() {
            let Some(entry) = group_layout.entry(bound.binding) else {
                self.encoder_error(format!(
                    "bind group {index} has no layout entry for binding {}",
                    bound.binding
                ));
                return;
            };
            match entry.ty {
                wgt::BindingType::Buffer {
                    ty: wgt::BufferBindingType::Uniform,
                    has_dynamic_offset,
                    ..
                } => {
                    let Some(target) = layout.bind_target(index, bound.binding) else {
                        self.encoder_error(format!(
                            "pipeline layout has no register for group {index} binding {}",
                            bound.binding
                        ));
                        return;
                    };
                    let slot = target.register;
                    if slot >= Direct3D11::D3D11_COMMONSHADER_CONSTANT_BUFFER_API_SLOT_COUNT {
                        self.encoder_error(format!(
                            "constant buffer register {slot} is outside the supported range"
                        ));
                        return;
                    }

                    let dynamic_offset = if has_dynamic_offset {
                        let Some(position) = group_layout
                            .dynamic_offsets()
                            .iter()
                            .position(|binding| *binding == bound.binding)
                        else {
                            self.encoder_error(format!(
                                "dynamic binding {} has no dynamic offset slot",
                                bound.binding
                            ));
                            return;
                        };
                        let Some(offset) = dynamic_offsets.get(position) else {
                            self.encoder_error(format!(
                                "dynamic binding {} is missing its dynamic offset",
                                bound.binding
                            ));
                            return;
                        };
                        u64::from(*offset)
                    } else {
                        0
                    };

                    let Some(offset) = bound.offset.checked_add(dynamic_offset) else {
                        self.encoder_error("uniform buffer offset overflowed".to_owned());
                        return;
                    };
                    let size = bound
                        .size
                        .unwrap_or_else(|| bound.buffer.size().saturating_sub(bound.offset));
                    let Some(end) = offset.checked_add(size) else {
                        self.encoder_error("uniform buffer range overflowed".to_owned());
                        return;
                    };
                    if size == 0 || end > bound.buffer.size() {
                        self.encoder_error(format!(
                            "uniform buffer range {offset}..{end} is outside the buffer of {} bytes",
                            bound.buffer.size()
                        ));
                        return;
                    }
                    if offset % 16 != 0 || size % 16 != 0 {
                        self.encoder_error(format!(
                            "uniform buffer ranges must be 16-byte aligned, got offset {offset} and size {size}"
                        ));
                        return;
                    }

                    let buffers = [Some(bound.buffer.raw().clone())];
                    if offset == 0 {
                        if entry.visibility.contains(wgt::ShaderStages::VERTEX) {
                            unsafe { self.context().VSSetConstantBuffers(slot, Some(&buffers)) };
                        }
                        if entry.visibility.contains(wgt::ShaderStages::FRAGMENT) {
                            unsafe { self.context().PSSetConstantBuffers(slot, Some(&buffers)) };
                        }
                        continue;
                    }

                    let Some(context1) = self.context1() else {
                        self.encoder_error(format!(
                            "constant buffer binding {} has offset {offset}, but ID3D11DeviceContext1 is unavailable",
                            bound.binding
                        ));
                        return;
                    };
                    // D3D11.1 expresses constant buffer windows in 16-constant
                    // (256-byte) units, so the total offset must be 256-byte aligned.
                    // The window length is expressed in 16-byte constants, so the
                    // binding size only has to be a multiple of 16, not 256.
                    if offset % 256 != 0 {
                        self.encoder_error(format!(
                            "constant buffer total offset must be 256-byte aligned, got {offset}"
                        ));
                        return;
                    }
                    let first_constant = match u32::try_from(offset / 16) {
                        Ok(value) => value,
                        Err(_) => {
                            self.encoder_error(format!(
                                "constant buffer offset {offset} does not fit in u32"
                            ));
                            return;
                        }
                    };
                    let num_constants = match u32::try_from(size / 16) {
                        Ok(value) => value,
                        Err(_) => {
                            self.encoder_error(format!(
                                "constant buffer size {size} does not fit in u32"
                            ));
                            return;
                        }
                    };
                    let end_constant = first_constant.saturating_add(num_constants);
                    if end_constant > 4096 {
                        self.encoder_error(format!(
                            "constant buffer window {first_constant}..{end_constant} exceeds 4096 constants"
                        ));
                        return;
                    }

                    let first_constants = [first_constant];
                    let num_constants = [num_constants];
                    if entry.visibility.contains(wgt::ShaderStages::VERTEX) {
                        unsafe {
                            context1.VSSetConstantBuffers1(
                                slot,
                                1,
                                Some(buffers.as_ptr()),
                                Some(first_constants.as_ptr()),
                                Some(num_constants.as_ptr()),
                            )
                        };
                    }
                    if entry.visibility.contains(wgt::ShaderStages::FRAGMENT) {
                        unsafe {
                            context1.PSSetConstantBuffers1(
                                slot,
                                1,
                                Some(buffers.as_ptr()),
                                Some(first_constants.as_ptr()),
                                Some(num_constants.as_ptr()),
                            )
                        };
                    }
                }
                wgt::BindingType::Buffer {
                    ty: wgt::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    ..
                } => {
                    if bound.dynamic_offset {
                        self.encoder_error(format!(
                            "storage buffer binding {} must not use a dynamic offset",
                            bound.binding
                        ));
                        return;
                    }
                    let Some(target) = layout.bind_target(index, bound.binding) else {
                        self.encoder_error(format!(
                            "pipeline layout has no register for group {index} binding {}",
                            bound.binding
                        ));
                        return;
                    };
                    let slot = target.register;
                    if slot >= Direct3D11::D3D11_COMMONSHADER_INPUT_RESOURCE_SLOT_COUNT {
                        self.encoder_error(format!(
                            "shader resource register {slot} is outside the supported range"
                        ));
                        return;
                    }
                    let Some(srv) = bound.srv.as_ref() else {
                        self.encoder_error(format!(
                            "storage buffer binding {} has no shader resource view",
                            bound.binding
                        ));
                        return;
                    };
                    // The binding offset is baked into the raw SRV, so there
                    // is no dynamic offset to apply before binding it.
                    let views = [Some(srv.clone())];
                    if entry.visibility.contains(wgt::ShaderStages::VERTEX) {
                        unsafe { self.context().VSSetShaderResources(slot, Some(&views)) };
                    }
                    if entry.visibility.contains(wgt::ShaderStages::FRAGMENT) {
                        unsafe { self.context().PSSetShaderResources(slot, Some(&views)) };
                    }
                }
                wgt::BindingType::Buffer {
                    ty: wgt::BufferBindingType::Storage { read_only: false },
                    ..
                } => {
                    self.encoder_error(format!(
                        "binding {} is a read-write/UAV storage buffer, which is not implemented",
                        bound.binding
                    ));
                    return;
                }
                wgt::BindingType::Buffer {
                    ty: wgt::BufferBindingType::Storage { .. },
                    ..
                } => {
                    self.encoder_error(format!(
                        "binding {} uses dynamic storage buffer offsets, which are not supported",
                        bound.binding
                    ));
                    return;
                }
                _ => {
                    self.encoder_error(format!(
                        "binding {} is not a uniform or read-only storage buffer in the bind group layout",
                        bound.binding
                    ));
                    return;
                }
            }
        }

        for bound in group.textures() {
            let Some(entry) = group_layout.entry(bound.binding) else {
                self.encoder_error(format!(
                    "bind group {index} has no layout entry for binding {}",
                    bound.binding
                ));
                return;
            };
            if !matches!(entry.ty, wgt::BindingType::Texture { .. }) {
                self.encoder_error(format!(
                    "binding {} is not a texture binding in the bind group layout",
                    bound.binding
                ));
                return;
            }
            let Some(target) = layout.bind_target(index, bound.binding) else {
                self.encoder_error(format!(
                    "pipeline layout has no register for group {index} binding {}",
                    bound.binding
                ));
                return;
            };
            let slot = target.register;
            if slot >= Direct3D11::D3D11_COMMONSHADER_INPUT_RESOURCE_SLOT_COUNT {
                self.encoder_error(format!(
                    "shader resource register {slot} is outside the supported range"
                ));
                return;
            }
            let Some(srv) = bound.view.raw_srv() else {
                self.encoder_error(format!(
                    "texture binding {} has no shader resource view",
                    bound.binding
                ));
                return;
            };
            let views = [Some(srv.clone())];
            if entry.visibility.contains(wgt::ShaderStages::VERTEX) {
                unsafe { self.context().VSSetShaderResources(slot, Some(&views)) };
            }
            if entry.visibility.contains(wgt::ShaderStages::FRAGMENT) {
                unsafe { self.context().PSSetShaderResources(slot, Some(&views)) };
            }
        }

        for bound in group.samplers() {
            let Some(entry) = group_layout.entry(bound.binding) else {
                self.encoder_error(format!(
                    "bind group {index} has no layout entry for binding {}",
                    bound.binding
                ));
                return;
            };
            if !matches!(entry.ty, wgt::BindingType::Sampler(_)) {
                self.encoder_error(format!(
                    "binding {} is not a sampler binding in the bind group layout",
                    bound.binding
                ));
                return;
            }
            let Some(target) = layout.bind_target(index, bound.binding) else {
                self.encoder_error(format!(
                    "pipeline layout has no register for group {index} binding {}",
                    bound.binding
                ));
                return;
            };
            let slot = target.register;
            if slot >= Direct3D11::D3D11_COMMONSHADER_SAMPLER_SLOT_COUNT {
                self.encoder_error(format!(
                    "sampler register {slot} is outside the supported range"
                ));
                return;
            }
            let samplers = [Some(bound.sampler.raw().clone())];
            if entry.visibility.contains(wgt::ShaderStages::VERTEX) {
                unsafe { self.context().VSSetSamplers(slot, Some(&samplers)) };
            }
            if entry.visibility.contains(wgt::ShaderStages::FRAGMENT) {
                unsafe { self.context().PSSetSamplers(slot, Some(&samplers)) };
            }
        }
    }

    unsafe fn set_immediates(&mut self, _layout: &PipelineLayout, offset_bytes: u32, data: &[u32]) {
        // Push constants are not advertised (`max_immediate_size` is zero) and
        // no layout should make this path reachable. If it is called anyway,
        // fail the encoder instead of silently leaving zeros in the shader.
        self.encoder_error(format!(
            "push constants are not supported (offset {offset_bytes}, {} words)",
            data.len()
        ));
    }

    unsafe fn insert_debug_marker(&mut self, _label: &str) {}
    unsafe fn begin_debug_marker(&mut self, _group_label: &str) {}
    unsafe fn end_debug_marker(&mut self) {}

    unsafe fn set_render_pipeline(&mut self, pipeline: &RenderPipeline) {
        if self.render_pass.is_none() {
            self.encoder_error("set_render_pipeline called outside a render pass".to_owned());
            return;
        }
        let validation = self
            .render_pass
            .as_ref()
            .map(|render_pass| render_pass.validate_pipeline(pipeline))
            .unwrap_or(Ok(()));
        if let Err(message) = validation {
            self.encoder_error(format!(
                "render pipeline is incompatible with the active render pass: {message}"
            ));
            self.current_pipeline = None;
            return;
        }

        self.current_blend_state = Some(pipeline.raw_blend_state().clone());
        self.current_depth_stencil_state = Some(pipeline.raw_depth_stencil_state().clone());
        self.sample_mask = pipeline.sample_mask();

        unsafe {
            self.context()
                .VSSetShader(pipeline.raw_vertex_shader(), None);
            self.context()
                .PSSetShader(pipeline.raw_pixel_shader(), None);
            self.context().IASetInputLayout(pipeline.raw_input_layout());
            self.context().IASetPrimitiveTopology(pipeline.topology());
            self.context().RSSetState(pipeline.raw_rasterizer_state());
            self.context()
                .OMSetDepthStencilState(pipeline.raw_depth_stencil_state(), self.stencil_reference);
            self.context().OMSetBlendState(
                self.current_blend_state.as_ref(),
                Some(&self.blend_constants),
                self.sample_mask,
            );
        }

        self.current_pipeline = Some(PipelineState {
            vertex_strides: pipeline.vertex_strides().to_vec(),
        });
    }

    unsafe fn set_index_buffer<'a>(
        &mut self,
        binding: crate::BufferBinding<'a, Buffer>,
        format: wgt::IndexFormat,
    ) {
        if !binding.buffer.usage().contains(wgt::BufferUses::INDEX) {
            self.encoder_error("D3D11 index buffer was not created with INDEX usage".to_owned());
            return;
        }
        let Ok(offset) = u32::try_from(binding.offset) else {
            self.encoder_error("D3D11 index buffer offset does not fit in u32".to_owned());
            return;
        };
        let format = super::conv::map_index_format(format);
        unsafe {
            self.context()
                .IASetIndexBuffer(binding.buffer.raw(), format, offset)
        };
        self.index_buffer_set = true;
    }

    unsafe fn set_vertex_buffer<'a>(
        &mut self,
        index: u32,
        binding: crate::BufferBinding<'a, Buffer>,
    ) {
        if index >= Direct3D11::D3D11_IA_VERTEX_INPUT_RESOURCE_SLOT_COUNT {
            self.encoder_error(format!(
                "D3D11 vertex buffer slot {index} is outside the supported range"
            ));
            return;
        }
        if !binding.buffer.usage().contains(wgt::BufferUses::VERTEX) {
            self.encoder_error("D3D11 vertex buffer was not created with VERTEX usage".to_owned());
            return;
        }
        if self.current_pipeline.is_none() {
            self.encoder_error(
                "D3D11 set_vertex_buffer called before set_render_pipeline".to_owned(),
            );
            return;
        }
        let stride = self.current_pipeline.as_ref().and_then(|pipeline| {
            pipeline
                .vertex_strides
                .get(index as usize)
                .copied()
                .flatten()
        });
        let Some(stride) = stride else {
            self.encoder_error(format!(
                "D3D11 render pipeline does not declare vertex buffer slot {index}"
            ));
            return;
        };
        let Ok(offset) = u32::try_from(binding.offset) else {
            self.encoder_error("D3D11 vertex buffer offset does not fit in u32".to_owned());
            return;
        };

        let buffers = [Some(binding.buffer.raw().clone())];
        let strides = [stride];
        let offsets = [offset];
        unsafe {
            self.context().IASetVertexBuffers(
                index,
                1,
                Some(buffers.as_ptr()),
                Some(strides.as_ptr()),
                Some(offsets.as_ptr()),
            )
        };
    }

    unsafe fn set_viewport(&mut self, rect: &crate::Rect<f32>, depth_range: Range<f32>) {
        if !(rect.w > 0.0) || !(rect.h > 0.0) {
            self.encoder_error("D3D11 viewport width and height must be positive".to_owned());
            return;
        }
        if !(0.0..=1.0).contains(&depth_range.start)
            || !(0.0..=1.0).contains(&depth_range.end)
            || depth_range.start > depth_range.end
        {
            self.encoder_error("D3D11 viewport depth range must be within 0..=1".to_owned());
            return;
        }
        let viewport = Direct3D11::D3D11_VIEWPORT {
            TopLeftX: rect.x,
            TopLeftY: rect.y,
            Width: rect.w,
            Height: rect.h,
            MinDepth: depth_range.start,
            MaxDepth: depth_range.end,
        };
        unsafe { self.context().RSSetViewports(Some(&[viewport])) };
    }

    unsafe fn set_scissor_rect(&mut self, rect: &crate::Rect<u32>) {
        let Some(right) = rect.x.checked_add(rect.w) else {
            self.encoder_error("D3D11 scissor rectangle coordinates overflow u32".to_owned());
            return;
        };
        let Some(bottom) = rect.y.checked_add(rect.h) else {
            self.encoder_error("D3D11 scissor rectangle coordinates overflow u32".to_owned());
            return;
        };
        let Ok(left) = i32::try_from(rect.x) else {
            self.encoder_error("D3D11 scissor rectangle coordinates do not fit in i32".to_owned());
            return;
        };
        let Ok(top) = i32::try_from(rect.y) else {
            self.encoder_error("D3D11 scissor rectangle coordinates do not fit in i32".to_owned());
            return;
        };
        let Ok(right) = i32::try_from(right) else {
            self.encoder_error("D3D11 scissor rectangle coordinates do not fit in i32".to_owned());
            return;
        };
        let Ok(bottom) = i32::try_from(bottom) else {
            self.encoder_error("D3D11 scissor rectangle coordinates do not fit in i32".to_owned());
            return;
        };
        let scissor = Foundation::RECT {
            left,
            top,
            right,
            bottom,
        };
        unsafe { self.context().RSSetScissorRects(Some(&[scissor])) };
    }

    unsafe fn set_stencil_reference(&mut self, value: u32) {
        self.stencil_reference = value;
        if let Some(state) = self.current_depth_stencil_state.as_ref() {
            unsafe { self.context().OMSetDepthStencilState(state, value) };
        }
    }

    unsafe fn set_blend_constants(&mut self, color: &[f32; 4]) {
        self.blend_constants = *color;
        if let Some(state) = self.current_blend_state.as_ref() {
            unsafe {
                self.context()
                    .OMSetBlendState(state, Some(&self.blend_constants), self.sample_mask)
            };
        }
    }

    unsafe fn draw(
        &mut self,
        first_vertex: u32,
        vertex_count: u32,
        first_instance: u32,
        instance_count: u32,
    ) {
        if self.render_pass.is_none() {
            self.encoder_error("D3D11 draw called outside a render pass".to_owned());
            return;
        }
        if self.current_pipeline.is_none() {
            self.encoder_error("D3D11 draw called before set_render_pipeline".to_owned());
            return;
        }
        if vertex_count == 0 || instance_count == 0 {
            return;
        }
        unsafe {
            self.context()
                .DrawInstanced(vertex_count, instance_count, first_vertex, first_instance)
        };
    }

    unsafe fn draw_indexed(
        &mut self,
        first_index: u32,
        index_count: u32,
        base_vertex: i32,
        first_instance: u32,
        instance_count: u32,
    ) {
        if self.render_pass.is_none() {
            self.encoder_error("D3D11 indexed draw called outside a render pass".to_owned());
            return;
        }
        if self.current_pipeline.is_none() {
            self.encoder_error("D3D11 indexed draw called before set_render_pipeline".to_owned());
            return;
        }
        if index_count == 0 || instance_count == 0 {
            return;
        }
        if !self.index_buffer_set {
            self.encoder_error("D3D11 indexed draw called before set_index_buffer".to_owned());
            return;
        }
        unsafe {
            self.context().DrawIndexedInstanced(
                index_count,
                instance_count,
                first_index,
                base_vertex,
                first_instance,
            )
        };
    }

    unsafe fn draw_mesh_tasks(
        &mut self,
        _group_count_x: u32,
        _group_count_y: u32,
        _group_count_z: u32,
    ) {
        self.encoder_error("D3D11 mesh shader draws are not implemented".to_owned());
    }

    unsafe fn draw_indirect(
        &mut self,
        buffer: &Buffer,
        offset: wgt::BufferAddress,
        draw_count: u32,
    ) {
        if self.render_pass.is_none() || self.current_pipeline.is_none() {
            self.encoder_error(
                "D3D11 draw_indirect called without a render pass and pipeline".to_owned(),
            );
            return;
        }
        if !buffer.usage().contains(wgt::BufferUses::INDIRECT) {
            self.encoder_error(
                "D3D11 indirect draw buffer was not created with INDIRECT usage".to_owned(),
            );
            return;
        }
        if offset % 4 != 0 {
            self.encoder_error("D3D11 indirect draw offset must be 4-byte aligned".to_owned());
            return;
        }
        let Some(total) = u64::from(draw_count).checked_mul(16) else {
            self.encoder_error("D3D11 indirect draw argument size overflowed".to_owned());
            return;
        };
        if offset
            .checked_add(total)
            .is_none_or(|end| end > buffer.size())
        {
            self.encoder_error("D3D11 indirect draw arguments are outside the buffer".to_owned());
            return;
        }
        let Ok(offset) = u32::try_from(offset) else {
            self.encoder_error("D3D11 indirect draw offset does not fit in u32".to_owned());
            return;
        };

        for draw in 0..draw_count {
            let Some(relative_offset) = draw.checked_mul(16) else {
                self.encoder_error("D3D11 indirect draw offset overflowed".to_owned());
                return;
            };
            let Some(draw_offset) = offset.checked_add(relative_offset) else {
                self.encoder_error("D3D11 indirect draw offset overflowed".to_owned());
                return;
            };
            unsafe {
                self.context()
                    .DrawInstancedIndirect(buffer.raw(), draw_offset)
            };
        }
    }

    unsafe fn draw_indexed_indirect(
        &mut self,
        buffer: &Buffer,
        offset: wgt::BufferAddress,
        draw_count: u32,
    ) {
        if self.render_pass.is_none() || self.current_pipeline.is_none() {
            self.encoder_error(
                "D3D11 draw_indexed_indirect called without a render pass and pipeline".to_owned(),
            );
            return;
        }
        if !self.index_buffer_set {
            self.encoder_error(
                "D3D11 indexed indirect draw called before set_index_buffer".to_owned(),
            );
            return;
        }
        if !buffer.usage().contains(wgt::BufferUses::INDIRECT) {
            self.encoder_error(
                "D3D11 indirect draw buffer was not created with INDIRECT usage".to_owned(),
            );
            return;
        }
        if offset % 4 != 0 {
            self.encoder_error("D3D11 indirect draw offset must be 4-byte aligned".to_owned());
            return;
        }
        let Some(total) = u64::from(draw_count).checked_mul(20) else {
            self.encoder_error("D3D11 indexed indirect draw argument size overflowed".to_owned());
            return;
        };
        if offset
            .checked_add(total)
            .is_none_or(|end| end > buffer.size())
        {
            self.encoder_error(
                "D3D11 indexed indirect draw arguments are outside the buffer".to_owned(),
            );
            return;
        }
        let Ok(offset) = u32::try_from(offset) else {
            self.encoder_error("D3D11 indexed indirect draw offset does not fit in u32".to_owned());
            return;
        };

        for draw in 0..draw_count {
            let Some(relative_offset) = draw.checked_mul(20) else {
                self.encoder_error("D3D11 indexed indirect draw offset overflowed".to_owned());
                return;
            };
            let Some(draw_offset) = offset.checked_add(relative_offset) else {
                self.encoder_error("D3D11 indexed indirect draw offset overflowed".to_owned());
                return;
            };
            unsafe {
                self.context()
                    .DrawIndexedInstancedIndirect(buffer.raw(), draw_offset)
            };
        }
    }

    unsafe fn draw_mesh_tasks_indirect(
        &mut self,
        _buffer: &Buffer,
        _offset: wgt::BufferAddress,
        _draw_count: u32,
    ) {
        self.encoder_error("D3D11 mesh shader indirect draws are not implemented".to_owned());
    }

    unsafe fn draw_indirect_count(
        &mut self,
        _buffer: &Buffer,
        _offset: wgt::BufferAddress,
        _count_buffer: &Buffer,
        _count_offset: wgt::BufferAddress,
        _max_count: u32,
    ) {
        self.encoder_error("D3D11 draw_indirect_count is not implemented".to_owned());
    }

    unsafe fn draw_indexed_indirect_count(
        &mut self,
        _buffer: &Buffer,
        _offset: wgt::BufferAddress,
        _count_buffer: &Buffer,
        _count_offset: wgt::BufferAddress,
        _max_count: u32,
    ) {
        self.encoder_error("D3D11 draw_indexed_indirect_count is not implemented".to_owned());
    }

    unsafe fn draw_mesh_tasks_indirect_count(
        &mut self,
        _buffer: &Buffer,
        _offset: wgt::BufferAddress,
        _count_buffer: &Buffer,
        _count_offset: wgt::BufferAddress,
        _max_count: u32,
    ) {
        self.encoder_error("D3D11 mesh shader indirect count draws are not implemented".to_owned());
    }

    // compute

    unsafe fn begin_compute_pass(&mut self, _desc: &crate::ComputePassDescriptor<Resource>) {
        self.encoder_error("D3D11 compute passes are not implemented".to_owned());
    }
    unsafe fn end_compute_pass(&mut self) {
        self.encoder_error("D3D11 compute passes are not implemented".to_owned());
    }

    unsafe fn set_compute_pipeline(&mut self, _pipeline: &Resource) {
        self.encoder_error("D3D11 compute pipelines are not implemented".to_owned());
    }

    unsafe fn dispatch_workgroups(&mut self, _count: [u32; 3]) {
        self.encoder_error("D3D11 compute dispatches are not implemented".to_owned());
    }
    unsafe fn dispatch_workgroups_indirect(
        &mut self,
        _buffer: &Buffer,
        _offset: wgt::BufferAddress,
    ) {
        self.encoder_error("D3D11 compute indirect dispatches are not implemented".to_owned());
    }

    unsafe fn build_acceleration_structures<'a, T>(
        &mut self,
        _descriptor_count: u32,
        _descriptors: T,
    ) where
        Api: 'a,
        T: IntoIterator<Item = crate::BuildAccelerationStructureDescriptor<'a, Buffer, Resource>>,
    {
        self.encoder_error("D3D11 acceleration structure builds are not implemented".to_owned());
    }

    unsafe fn place_acceleration_structure_barrier(
        &mut self,
        _barriers: crate::AccelerationStructureBarrier,
    ) {
        self.encoder_error("D3D11 acceleration structure barriers are not implemented".to_owned());
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

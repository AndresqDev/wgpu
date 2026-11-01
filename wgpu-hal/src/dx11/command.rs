//! Minimal D3D11 command encoder.
//!
//! D3D11 records commands on an `ID3D11DeviceContext`. This milestone does not
//! create deferred contexts yet: the encoder keeps a clone of the device's
//! immediate context and executes supported commands (currently only render-pass
//! clears) as soon as they are recorded. [`crate::Queue::submit`] then flushes
//! that context.
//!
//! This means the clear becomes visible before the command buffer is submitted,
//! which is fine for the clear/present milestone but not the final design.
//! Deferred contexts and ordered command-list execution are the next step.

use core::ops::Range;

use windows::Win32::Graphics::Direct3D11;

use super::{Api, Resource, TextureView};

/// Command buffer type, which also acts as the command encoder.
///
/// Keeping a single type for both roles follows the structure of the noop
/// backend and avoids maintaining two state machines before command recording
/// is implemented.
#[derive(Debug)]
pub struct CommandBuffer {
    context: Direct3D11::ID3D11DeviceContext,
}

impl CommandBuffer {
    pub(crate) fn new(context: Direct3D11::ID3D11DeviceContext) -> Self {
        Self { context }
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
        Ok(CommandBuffer::new(self.context.clone()))
    }

    unsafe fn reset_all<I>(&mut self, _command_buffers: I) {}

    unsafe fn transition_buffers<'a, T>(&mut self, _barriers: T)
    where
        T: Iterator<Item = crate::BufferBarrier<'a, Resource>>,
    {
    }

    unsafe fn transition_textures<'a, T>(&mut self, _barriers: T)
    where
        T: Iterator<Item = crate::TextureBarrier<'a, Resource>>,
    {
    }

    unsafe fn clear_buffer(&mut self, _buffer: &Resource, _range: crate::MemoryRange) {}

    unsafe fn copy_buffer_to_buffer<T>(&mut self, _src: &Resource, _dst: &Resource, _regions: T)
    where
        T: Iterator<Item = crate::BufferCopy>,
    {
    }

    #[cfg(webgl)]
    unsafe fn copy_external_image_to_texture<T>(
        &mut self,
        _src: &wgt::CopyExternalImageSourceInfo,
        _dst: &Resource,
        _dst_premultiplication: bool,
        _regions: T,
    ) where
        T: Iterator<Item = crate::TextureCopy>,
    {
    }

    unsafe fn copy_texture_to_texture<T>(
        &mut self,
        _src: &Resource,
        _src_usage: wgt::TextureUses,
        _dst: &Resource,
        _regions: T,
    ) where
        T: Iterator<Item = crate::TextureCopy>,
    {
    }

    unsafe fn copy_buffer_to_texture<T>(&mut self, _src: &Resource, _dst: &Resource, _regions: T)
    where
        T: Iterator<Item = crate::BufferTextureCopy>,
    {
    }

    unsafe fn copy_texture_to_buffer<T>(
        &mut self,
        _src: &Resource,
        _src_usage: wgt::TextureUses,
        _dst: &Resource,
        _regions: T,
    ) where
        T: Iterator<Item = crate::BufferTextureCopy>,
    {
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
        _buf: &Resource,
    ) {
    }
    unsafe fn reset_queries(&mut self, _set: &Resource, _range: Range<u32>) {}
    unsafe fn copy_query_results(
        &mut self,
        _set: &Resource,
        _range: Range<u32>,
        _buffer: &Resource,
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
        _layout: &Resource,
        _index: u32,
        _group: &Resource,
        _dynamic_offsets: &[wgt::DynamicOffset],
    ) {
    }

    unsafe fn set_immediates(&mut self, _layout: &Resource, _offset_bytes: u32, _data: &[u32]) {}

    unsafe fn insert_debug_marker(&mut self, _label: &str) {}
    unsafe fn begin_debug_marker(&mut self, _group_label: &str) {}
    unsafe fn end_debug_marker(&mut self) {}

    unsafe fn set_render_pipeline(&mut self, _pipeline: &Resource) {}

    unsafe fn set_index_buffer<'a>(
        &mut self,
        _binding: crate::BufferBinding<'a, Resource>,
        _format: wgt::IndexFormat,
    ) {
    }

    unsafe fn set_vertex_buffer<'a>(
        &mut self,
        _index: u32,
        _binding: crate::BufferBinding<'a, Resource>,
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
        _buffer: &Resource,
        _offset: wgt::BufferAddress,
        _draw_count: u32,
    ) {
    }

    unsafe fn draw_indexed_indirect(
        &mut self,
        _buffer: &Resource,
        _offset: wgt::BufferAddress,
        _draw_count: u32,
    ) {
    }

    unsafe fn draw_mesh_tasks_indirect(
        &mut self,
        _buffer: &Resource,
        _offset: wgt::BufferAddress,
        _draw_count: u32,
    ) {
    }

    unsafe fn draw_indirect_count(
        &mut self,
        _buffer: &Resource,
        _offset: wgt::BufferAddress,
        _count_buffer: &Resource,
        _count_offset: wgt::BufferAddress,
        _max_count: u32,
    ) {
    }

    unsafe fn draw_indexed_indirect_count(
        &mut self,
        _buffer: &Resource,
        _offset: wgt::BufferAddress,
        _count_buffer: &Resource,
        _count_offset: wgt::BufferAddress,
        _max_count: u32,
    ) {
    }

    unsafe fn draw_mesh_tasks_indirect_count(
        &mut self,
        _buffer: &Resource,
        _offset: wgt::BufferAddress,
        _count_buffer: &Resource,
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
        _buffer: &Resource,
        _offset: wgt::BufferAddress,
    ) {
    }

    unsafe fn build_acceleration_structures<'a, T>(
        &mut self,
        _descriptor_count: u32,
        _descriptors: T,
    ) where
        Api: 'a,
        T: IntoIterator<Item = crate::BuildAccelerationStructureDescriptor<'a, Resource, Resource>>,
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
        _ray_generation_group_data: crate::PipelineGroupData<Resource>,
        _miss_group_data: crate::PipelineGroupData<Resource>,
        _intersection_group_data: crate::PipelineGroupData<Resource>,
    ) {
    }
}

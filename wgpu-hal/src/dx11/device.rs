//! D3D11 device stubs.
//!
//! The device owns the `ID3D11Device` created by [`crate::Adapter::open`].
//! Resource creation and shader compilation are not implemented yet; the
//! adapter/device smoke test only needs a successfully created device, so those
//! entry points are explicitly marked with `unimplemented!`.

use alloc::vec::Vec;
use core::fmt;

use windows::Win32::Graphics::{Direct3D, Direct3D11};

use super::{Api, CommandBuffer, Fence, PipelineCache, Resource, ShaderModule};

/// An open D3D11 device.
pub struct Device {
    raw_device: Direct3D11::ID3D11Device,
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
        info: wgt::AdapterInfo,
        feature_level: Direct3D::D3D_FEATURE_LEVEL,
        limits: wgt::Limits,
    ) -> Self {
        Self {
            raw_device,
            info,
            feature_level,
            limits,
        }
    }

    /// Returns the underlying D3D11 device.
    pub fn raw_device(&self) -> &Direct3D11::ID3D11Device {
        &self.raw_device
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
        _desc: &crate::BufferDescriptor,
    ) -> Result<Resource, crate::DeviceError> {
        unimplemented!("D3D11 buffer creation is not implemented yet")
    }

    unsafe fn destroy_buffer(&self, _buffer: Resource) {}
    unsafe fn add_raw_buffer(&self, _buffer: &Resource) {}

    unsafe fn map_buffer(
        &self,
        _buffer: &Resource,
        _range: crate::MemoryRange,
    ) -> Result<crate::BufferMapping, crate::DeviceError> {
        unimplemented!("D3D11 buffer mapping is not implemented yet")
    }

    unsafe fn unmap_buffer(&self, _buffer: &Resource) {}
    unsafe fn flush_mapped_ranges<I>(&self, _buffer: &Resource, _ranges: I) {}
    unsafe fn invalidate_mapped_ranges<I>(&self, _buffer: &Resource, _ranges: I) {}

    unsafe fn create_texture(
        &self,
        _desc: &crate::TextureDescriptor,
    ) -> Result<Resource, crate::DeviceError> {
        unimplemented!("D3D11 texture creation is not implemented yet")
    }

    unsafe fn destroy_texture(&self, _texture: Resource) {}
    unsafe fn add_raw_texture(&self, _texture: &Resource) {}

    unsafe fn create_texture_view(
        &self,
        _texture: &Resource,
        _desc: &crate::TextureViewDescriptor,
    ) -> Result<Resource, crate::DeviceError> {
        unimplemented!("D3D11 texture view creation is not implemented yet")
    }

    unsafe fn destroy_texture_view(&self, _view: Resource) {}

    unsafe fn create_sampler(
        &self,
        _desc: &crate::SamplerDescriptor,
    ) -> Result<Resource, crate::DeviceError> {
        unimplemented!("D3D11 sampler creation is not implemented yet")
    }

    unsafe fn destroy_sampler(&self, _sampler: Resource) {}

    unsafe fn create_command_encoder(
        &self,
        _desc: &crate::CommandEncoderDescriptor<super::Queue>,
    ) -> Result<CommandBuffer, crate::DeviceError> {
        Ok(CommandBuffer::new())
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
        _desc: &crate::BindGroupDescriptor<Resource, Resource, Resource, Resource, Resource>,
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
        _desc: &crate::GetAccelerationStructureBuildSizesDescriptor<'a, Resource>,
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

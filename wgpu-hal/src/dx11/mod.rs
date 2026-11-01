/*!
# Direct3D 11 API internals.

This is an experimental, deliberately small backend skeleton. It implements
adapter enumeration with DXGI and device creation with `D3D11CreateDevice`, and
provides compilable stubs for the rest of the `wgpu-hal` API.

## Current state

- [`Instance::init`](crate::Instance::init) creates an `IDXGIFactory1`.
- [`Instance::enumerate_adapters`](crate::Instance::enumerate_adapters)
  enumerates `IDXGIAdapter1`s and validates each one by creating an
  `ID3D11Device` at feature level 11.0 or 10.1.
- [`Adapter::open`](crate::Adapter::open) creates a fresh `ID3D11Device` and
  returns [`Device`] / [`Queue`] wrappers around the device and its immediate
  context.
- Buffers, textures, samplers, bind groups, pipelines, shaders and surfaces are
  stubs. Their creation methods call `unimplemented!`, and command recording is
  a no-op. They are not used by the `raw-dx11` smoke test.

## Next steps

The immediate next milestone is to make command encoding and submission real:
create deferred `ID3D11DeviceContext`s (or record on the immediate context),
store deferred command lists in [`CommandBuffer`], and execute/flush them from
[`Queue::submit`](crate::Queue::submit). After that, buffers and textures can be
implemented, followed by FXC HLSL compilation and swap-chain presentation.
*/

#![allow(unused_variables)]

mod adapter;
mod command;
mod conv;
mod device;
mod instance;
mod queue;
mod shader;
mod surface;
mod types;

pub use adapter::Adapter;
pub use command::CommandBuffer;
pub use device::Device;
pub use instance::Instance;
pub use queue::Queue;
pub use shader::ShaderModule;
pub use surface::Surface;
pub use types::{Fence, PipelineCache, Resource};

/// The D3D11 backend marker type.
#[derive(Clone, Debug)]
pub struct Api;

impl crate::Api for Api {
    const VARIANT: wgt::Backend = wgt::Backend::Dx11;

    type Instance = Instance;
    type Surface = Surface;
    type Adapter = Adapter;
    type Device = Device;

    type Queue = Queue;
    type CommandEncoder = CommandBuffer;
    type CommandBuffer = CommandBuffer;

    type Buffer = Resource;
    type Texture = Resource;
    type SurfaceTexture = Resource;
    type TextureView = Resource;
    type Sampler = Resource;
    type QuerySet = Resource;
    type Fence = Fence;

    type BindGroupLayout = Resource;
    type BindGroup = Resource;
    type PipelineLayout = Resource;
    type ShaderModule = ShaderModule;
    type RenderPipeline = Resource;
    type ComputePipeline = Resource;
    type RayTracingPipeline = Resource;
    type PipelineCache = PipelineCache;

    type AccelerationStructure = Resource;
}

crate::impl_dyn_resource!(
    Adapter,
    CommandBuffer,
    Device,
    Fence,
    Instance,
    PipelineCache,
    Queue,
    Resource,
    ShaderModule,
    Surface
);

impl crate::DynAccelerationStructure for Resource {}
impl crate::DynBindGroup for Resource {}
impl crate::DynBindGroupLayout for Resource {}
impl crate::DynBuffer for Resource {}
impl crate::DynCommandBuffer for CommandBuffer {}
impl crate::DynComputePipeline for Resource {}
impl crate::DynFence for Fence {}
impl crate::DynPipelineCache for PipelineCache {}
impl crate::DynPipelineLayout for Resource {}
impl crate::DynQuerySet for Resource {}
impl crate::DynRenderPipeline for Resource {}
impl crate::DynRayTracingPipeline for Resource {}
impl crate::DynSampler for Resource {}
impl crate::DynShaderModule for ShaderModule {}
impl crate::DynSurfaceTexture for Resource {}
impl crate::DynTexture for Resource {}
impl crate::DynTextureView for Resource {}

impl core::borrow::Borrow<dyn crate::DynTexture> for Resource {
    fn borrow(&self) -> &dyn crate::DynTexture {
        self
    }
}

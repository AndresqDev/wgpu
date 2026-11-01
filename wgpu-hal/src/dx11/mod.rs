/*!
# Direct3D 11 API internals.

This is an experimental, deliberately small backend skeleton. It implements
adapter enumeration with DXGI and device creation with `D3D11CreateDevice`, and
provides compilable stubs for the rest of the `wgpu-hal` API.

## Current state

- [`Instance::init`](crate::Instance::init) creates an `IDXGIFactory2`.
- [`Instance::enumerate_adapters`](crate::Instance::enumerate_adapters)
  enumerates `IDXGIAdapter1`s and validates each one by creating an
  `ID3D11Device` at feature level 11.0 or 10.1.
- [`Instance::create_surface`](crate::Instance::create_surface) creates a
  windowed [`Surface`] for a Win32 `HWND`.
- [`Adapter::open`](crate::Adapter::open) creates a fresh `ID3D11Device` and
  returns [`Device`] / [`Queue`] wrappers around the device and its immediate
  context.
- [`Surface::configure`](crate::Surface::configure) creates a
  `DXGI_SWAP_EFFECT_FLIP_DISCARD` swap chain with two BGRA8 back buffers and a
  render-target view for each buffer.
- [`CommandBuffer::begin_render_pass`](crate::CommandEncoder::begin_render_pass)
  clears color attachments immediately on the immediate context. The
  `raw-dx11-window` example uses this to clear and present frames.
- [`Device::create_buffer`](crate::Device::create_buffer),
  [`Device::create_texture`](crate::Device::create_texture),
  [`Device::create_texture_view`](crate::Device::create_texture_view) and
  [`Device::create_sampler`](crate::Device::create_sampler) create real D3D11
  resources. Copy commands upload data from staging buffers; the
  `raw-dx11-resources` example exercises this path.
- Bind groups, pipelines and shaders are still stubs. Their creation methods
  call `unimplemented!`.

## Next steps

The immediate next milestone is to make command encoding and submission
ordered: create deferred `ID3D11DeviceContext`s (or record into command lists)
and execute them from [`Queue::submit`](crate::Queue::submit), instead of
clearing during recording. Bind groups, shaders and pipelines can then follow.
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
pub(crate) use types::BufferMapMode;
pub use types::{
    Buffer, Fence, PipelineCache, Resource, Sampler, SurfaceTexture, Texture, TextureView,
};

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

    type Buffer = Buffer;
    type Texture = Texture;
    type SurfaceTexture = SurfaceTexture;
    type TextureView = TextureView;
    type Sampler = Sampler;
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
    Buffer,
    CommandBuffer,
    Device,
    Fence,
    Instance,
    PipelineCache,
    Queue,
    Resource,
    Sampler,
    ShaderModule,
    Surface,
    SurfaceTexture,
    Texture,
    TextureView
);

impl crate::DynAccelerationStructure for Resource {}
impl crate::DynBindGroup for Resource {}
impl crate::DynBindGroupLayout for Resource {}
impl crate::DynBuffer for Buffer {}
impl crate::DynCommandBuffer for CommandBuffer {}
impl crate::DynComputePipeline for Resource {}
impl crate::DynFence for Fence {}
impl crate::DynPipelineCache for PipelineCache {}
impl crate::DynPipelineLayout for Resource {}
impl crate::DynQuerySet for Resource {}
impl crate::DynRenderPipeline for Resource {}
impl crate::DynRayTracingPipeline for Resource {}
impl crate::DynSampler for Sampler {}
impl crate::DynShaderModule for ShaderModule {}
impl crate::DynSurfaceTexture for SurfaceTexture {}
impl crate::DynTexture for Texture {}
impl crate::DynTextureView for TextureView {}

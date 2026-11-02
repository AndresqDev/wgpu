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
- [`InstanceFlags::DEBUG`](wgt::InstanceFlags::DEBUG) and
  [`InstanceFlags::VALIDATION`](wgt::InstanceFlags::VALIDATION) request
  `D3D11_CREATE_DEVICE_DEBUG`. When the D3D11 debug layer
  (`d3d11sdklayers.dll`) is installed, [`Adapter::debug_layer_available`]
  reports it, devices expose [`Device::take_debug_messages`], and queued
  messages are drained and logged after `submit`, `present` and idle waits.
  When the layer is missing, device creation retries without the debug flag
  instead of rejecting the adapter, so normal operation is unaffected.
- [`Instance::create_surface`](crate::Instance::create_surface) creates a
  windowed [`Surface`] for a Win32 `HWND`.
- [`Adapter::open`](crate::Adapter::open) creates a fresh `ID3D11Device` and
  returns [`Device`] / [`Queue`] wrappers around the device and its immediate
  context.
- [`Surface::configure`](crate::Surface::configure) creates a
  `DXGI_SWAP_EFFECT_FLIP_DISCARD` swap chain with two BGRA8 back buffers and a
  render-target view for each buffer. Reconfiguring (for example on a window
  resize) releases the previous swap chain and its views before creating the
  replacement chain; the `raw-dx11-resize` example verifies that the acquired
  back-buffer extent follows the requested configuration.
- [`CommandBuffer::begin_render_pass`](crate::CommandEncoder::begin_render_pass)
  binds real render-target and depth-stencil views with `OMSetRenderTargets` and
  applies the requested clears. Render pipelines and draw commands are executed
  on the immediate context.
- [`Device::create_buffer`](crate::Device::create_buffer),
  [`Device::create_texture`](crate::Device::create_texture),
  [`Device::create_texture_view`](crate::Device::create_texture_view) and
  [`Device::create_sampler`](crate::Device::create_sampler) create real D3D11
  resources. Copy commands upload data from staging buffers; the
  `raw-dx11-resources` example exercises this path.
- [`Device::create_bind_group_layout`](crate::Device::create_bind_group_layout),
  [`Device::create_pipeline_layout`](crate::Device::create_pipeline_layout) and
  [`Device::create_bind_group`](crate::Device::create_bind_group) create CPU-side
  bind group objects. Registers are flattened into separate `b`/`t`/`u`/`s`
  counters in the pipeline layout; [`CommandEncoder::set_bind_group`](crate::CommandEncoder::set_bind_group)
  binds their uniform buffers, read-only storage buffers (as raw buffer SRVs),
  textures and samplers.
- [`Device::create_shader_module`](crate::Device::create_shader_module) stores
  Naga IR or HLSL source. [`compile_shader`] translates Naga IR to HLSL with
  direct sampler registers and compiles it with FXC for Shader Model 5.0. The
  `raw-dx11-shader` example exercises this path.
- Read-only storage buffers are implemented at feature level 11_0: Naga emits
  `ByteAddressBuffer` for `var<storage, read>`, which the backend binds through
  a raw buffer SRV in the shared `t` register space. The binding offset is
  baked into the SRV, so static offsets work while dynamic storage offsets and
  read-write/UAV storage do not. Storage textures and compute dispatches remain
  unimplemented.
- [`Device::create_render_pipeline`](crate::Device::create_render_pipeline)
  creates the vertex/pixel shader objects, input layout, rasterizer state,
  depth-stencil state and blend state. The `raw-dx11-triangle` example draws one
  triangle offscreen and verifies the result by readback.
- Compute pipelines, dispatches, queries, push constants and deferred command
  lists are still stubs. There is no MSAA, multiview, mesh-shader, compute,
  UAV/read-write storage, storage-texture or dynamic-storage-offset support.
  Non-zero uniform buffer offsets require
  `ID3D11DeviceContext1` and must describe 256-byte-aligned constant-buffer
  windows; otherwise the binding is rejected with an explicit error log.
  Vertex input layouts use Naga's `LOC<location>` semantics.
- [`Queue::wait_for_idle`](crate::Queue::wait_for_idle) performs a real
  GPU-completion wait: the queue owns an `ID3D11Query` of type
  `D3D11_QUERY_EVENT`, `wait_for_idle` ends it and flushes before polling
  `ID3D11DeviceContext::GetData` on that marker with a bounded five-second
  timeout.
- [`Device::wait`](crate::Device::wait) waits for real GPU completion. Each
  [`Fence`] owns a pool of eight `D3D11_QUERY_EVENT` markers. `submit` arms a
  free slot before flushing and records the value as pending. When all slots
  are still in flight, `submit` waits briefly for the oldest marker and, if it
  does not complete, rotates that slot to a fresh query instead of blocking the
  next submission. `wait` polls the armed slots and advances the completed
  value only to values whose marker the GPU actually reached.
  [`Device::get_fence_value`](crate::Device::get_fence_value) reports the
  highest GPU-completed value, not the highest submitted one. Device removal is
  classified as `DeviceError::Lost` / `SurfaceError::Lost` from submit, present
  and the wait paths. [`Device::is_device_lost`], [`Device::check_device_state`]
  and [`Device::device_removed_reason`] expose the same check to callers, and
  resource creation and mapping turn a removal discovered through
  `GetDeviceRemovedReason` into `DeviceError::Lost` instead of an unexpected
  error.
- `maximum_frame_latency` is applied through
  `IDXGISwapChain2::SetMaximumFrameLatency` when available. `PresentMode::Immediate`
  is intentionally not advertised until the present/back-buffer path has a
  dedicated validation matrix.

## Next steps

The next milestone is compute pipelines and dispatches, followed by deferred
contexts so command recording and submission can be separated. Current command
encoding still executes immediately on the immediate context and
[`Queue::submit`](crate::Queue::submit) only flushes it.
*/

#![allow(unused_variables)]

mod adapter;
mod command;
mod conv;
mod debug;
mod device;
mod instance;
mod query;
mod queue;
mod shader;
mod surface;
mod types;

pub use adapter::Adapter;
pub use command::CommandBuffer;
pub use debug::{DebugMessage, DebugSeverity};
pub use device::Device;
pub use instance::Instance;
pub use query::Fence;
pub use queue::Queue;
pub use shader::{compile_shader, CompiledShader, ShaderEntryPoint, ShaderModule};
pub use surface::Surface;
pub(crate) use types::BufferMapMode;
pub use types::{
    BindGroup, BindGroupLayout, BindGroupLayoutEntry, BoundBuffer, BoundSampler, BoundTexture,
    Buffer, PipelineCache, PipelineLayout, RenderPipeline, Resource, Sampler, SurfaceTexture,
    Texture, TextureView,
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

    type BindGroupLayout = BindGroupLayout;
    type BindGroup = BindGroup;
    type PipelineLayout = PipelineLayout;
    type ShaderModule = ShaderModule;
    type RenderPipeline = RenderPipeline;
    type ComputePipeline = Resource;
    type RayTracingPipeline = Resource;
    type PipelineCache = PipelineCache;

    type AccelerationStructure = Resource;
}

crate::impl_dyn_resource!(
    Adapter,
    BindGroup,
    BindGroupLayout,
    Buffer,
    CommandBuffer,
    Device,
    Fence,
    Instance,
    PipelineCache,
    PipelineLayout,
    Queue,
    RenderPipeline,
    Resource,
    Sampler,
    ShaderModule,
    Surface,
    SurfaceTexture,
    Texture,
    TextureView
);

impl crate::DynAccelerationStructure for Resource {}
impl crate::DynBindGroup for BindGroup {}
impl crate::DynBindGroupLayout for BindGroupLayout {}
impl crate::DynBuffer for Buffer {}
impl crate::DynCommandBuffer for CommandBuffer {}
impl crate::DynComputePipeline for Resource {}
impl crate::DynFence for Fence {}
impl crate::DynPipelineCache for PipelineCache {}
impl crate::DynPipelineLayout for PipelineLayout {}
impl crate::DynQuerySet for Resource {}
impl crate::DynRenderPipeline for RenderPipeline {}
impl crate::DynRayTracingPipeline for Resource {}
impl crate::DynSampler for Sampler {}
impl crate::DynShaderModule for ShaderModule {}
impl crate::DynSurfaceTexture for SurfaceTexture {}
impl crate::DynTexture for Texture {}
impl crate::DynTextureView for TextureView {}

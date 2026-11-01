use core::{
    borrow::Borrow,
    sync::atomic::{AtomicBool, Ordering},
};

use windows::Win32::{
    Foundation,
    Graphics::{Direct3D11, Dxgi},
};

/// Placeholder for every GPU resource type that is not implemented yet.
///
/// Buffers, textures, texture views and samplers have concrete wrappers below.
/// This type remains as the placeholder used by bind groups, layouts, pipelines,
/// query sets and acceleration structures until those milestones land.
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
#[derive(Debug)]
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

/// A CPU-visible fence value.
///
/// The real implementation will pair this with an `ID3D11Query`
/// (`D3D11_QUERY_EVENT`) so that [`crate::Device::wait`] can block on GPU
/// completion. For now the value is only updated by [`crate::Queue::submit`],
/// which is CPU-synchronous in this skeleton.
#[derive(Debug, Default)]
pub struct Fence {
    value: crate::AtomicFenceValue,
}

impl Fence {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn signal(&self, value: u64) {
        self.value.fetch_max(value, Ordering::Release);
    }

    pub(crate) fn value(&self) -> u64 {
        self.value.load(Ordering::Acquire)
    }
}

/// Dummy pipeline cache.
///
/// D3D11 has no API-level pipeline cache object; compiled shader blobs will
/// eventually be cached by `wgpu-core` or on disk.
#[derive(Debug, Default)]
pub struct PipelineCache;

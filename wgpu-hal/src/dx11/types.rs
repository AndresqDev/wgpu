use core::{borrow::Borrow, sync::atomic::Ordering};

use windows::Win32::{
    Foundation,
    Graphics::{Direct3D11, Dxgi},
};

/// Placeholder for every GPU resource type that is not implemented yet.
///
/// The skeleton deliberately reuses one zero-sized type for buffers, textures,
/// samplers, bind groups, layouts, pipelines and query sets. Once the
/// corresponding D3D11 objects are implemented, each associated type in
/// [`super::Api`] should be replaced by its own concrete wrapper.
#[derive(Debug)]
pub struct Resource;

/// A render-target view.
///
/// The only texture view the backend can create today is the render target for
/// a swap-chain back buffer. Generic texture creation will later grow this
/// wrapper to also hold shader resource and unordered access views.
#[derive(Clone, Debug)]
pub struct TextureView {
    raw: Direct3D11::ID3D11RenderTargetView,
}

impl TextureView {
    pub(crate) fn new(raw: Direct3D11::ID3D11RenderTargetView) -> Self {
        Self { raw }
    }

    /// Creates an RTV for a 2D texture with the given view format.
    ///
    /// `format` may be `None` to use the texture's own format.
    pub(crate) fn create_render_target(
        device: &Direct3D11::ID3D11Device,
        texture: &Direct3D11::ID3D11Texture2D,
        format: Option<Dxgi::Common::DXGI_FORMAT>,
    ) -> windows::core::Result<Self> {
        let desc = format.map(|format| Direct3D11::D3D11_RENDER_TARGET_VIEW_DESC {
            Format: format,
            ViewDimension: Direct3D11::D3D11_RTV_DIMENSION_TEXTURE2D,
            Anonymous: Direct3D11::D3D11_RENDER_TARGET_VIEW_DESC_0 {
                Texture2D: Direct3D11::D3D11_TEX2D_RTV { MipSlice: 0 },
            },
        });

        let mut raw: Option<Direct3D11::ID3D11RenderTargetView> = None;
        unsafe {
            device.CreateRenderTargetView(
                texture,
                desc.as_ref().map(|desc| desc as *const _),
                Some(&mut raw),
            )
        }?;

        raw.map(Self::new)
            .ok_or_else(|| windows::core::Error::from_hresult(Foundation::E_UNEXPECTED))
    }

    /// Returns the underlying D3D11 render target view.
    pub fn raw(&self) -> &Direct3D11::ID3D11RenderTargetView {
        &self.raw
    }
}

/// A swap-chain image acquired from [`crate::Surface::acquire_texture`].
///
/// Back-buffer textures do not have a real `Api::Texture` wrapper yet, so this
/// type carries the zero-sized [`Resource`] marker required by the
/// `Borrow<dyn DynTexture>` bound, together with the RTV that describes the
/// currently acquired back buffer.
#[derive(Debug)]
pub struct SurfaceTexture {
    resource: Resource,
    view: TextureView,
}

impl SurfaceTexture {
    pub(crate) fn new(view: TextureView) -> Self {
        Self {
            resource: Resource,
            view,
        }
    }

    /// Returns the render target view for the acquired back buffer.
    pub fn view(&self) -> &TextureView {
        &self.view
    }
}

impl Borrow<dyn crate::DynTexture> for SurfaceTexture {
    fn borrow(&self) -> &dyn crate::DynTexture {
        &self.resource
    }
}

impl Borrow<Resource> for SurfaceTexture {
    fn borrow(&self) -> &Resource {
        &self.resource
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

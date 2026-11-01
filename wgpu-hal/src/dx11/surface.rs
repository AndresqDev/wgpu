//! Windowed DXGI swap-chain surface.
//!
//! The surface owns an `IDXGISwapChain1` created for a single `HWND` and one
//! render-target view per back buffer. Flip-model swap chains rotate their back
//! buffers, so keeping a view for each buffer lets [`crate::Surface::acquire_texture`]
//! hand out the buffer that DXGI is actually expecting next.
//!
//! Window resizing is not handled yet: [`crate::Surface::configure`] recreates
//! the swap chain, but there is no automatic `ResizeBuffers` path.

use alloc::vec::Vec;
use core::{fmt, time::Duration};

use parking_lot::Mutex;
use windows::{
    core::Interface as _,
    Win32::{
        Foundation,
        Graphics::{Direct3D11, Dxgi},
    },
};

use super::{Api, Device, Fence, SurfaceTexture, Texture, TextureView};

/// Number of back buffers requested from DXGI.
const BUFFER_COUNT: u32 = 2;

/// DXGI flip-model swap chains are normally created in a non-sRGB format and
/// presented through an sRGB render-target view, so that shader output is
/// encoded by the hardware on write.
const SWAPCHAIN_FORMAT: Dxgi::Common::DXGI_FORMAT = Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;

/// A swap-chain-backed surface.
pub struct Surface {
    factory: Dxgi::IDXGIFactory2,
    hwnd: Foundation::HWND,
    state: Mutex<SurfaceState>,
}

// `HWND` and the DXGI interfaces are raw COM/OS handles with no thread
// affinity of their own. Mutable surface state is protected by `state`, and the
// HAL surface contract requires external synchronization for configure,
// acquire, present and unconfigure.
unsafe impl Send for Surface {}
unsafe impl Sync for Surface {}

impl fmt::Debug for Surface {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let configured = self.state.lock().swapchain.is_some();
        f.debug_struct("Surface")
            .field("hwnd", &self.hwnd)
            .field("configured", &configured)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Default)]
struct SurfaceState {
    swapchain: Option<SwapChain>,
}

#[derive(Debug)]
struct SwapChain {
    /// One view per back buffer. Dropped before the swap chain itself so that
    /// no back-buffer references survive when the swap chain is released.
    views: Vec<TextureView>,
    /// Identity wrappers for the back-buffer resources. They are handed out to
    /// `wgpu-core` through `SurfaceTexture`, which requires `Borrow<Texture>`.
    textures: Vec<Texture>,
    raw: Dxgi::IDXGISwapChain1,
    /// Used to find the current back-buffer index. Available on Windows 10 and
    /// later; without it the surface conservatively uses buffer 0.
    raw3: Option<Dxgi::IDXGISwapChain3>,
    present_interval: u32,
}

impl Surface {
    pub(crate) fn new(factory: Dxgi::IDXGIFactory2, hwnd: Foundation::HWND) -> Self {
        Self {
            factory,
            hwnd,
            state: Mutex::new(SurfaceState::default()),
        }
    }

    /// Presents the swap-chain image associated with `texture`.
    pub(crate) fn present_texture(
        &self,
        _texture: &SurfaceTexture,
    ) -> Result<(), crate::SurfaceError> {
        let state = self.state.lock();
        let swapchain = state
            .swapchain
            .as_ref()
            .ok_or(crate::SurfaceError::Other("surface is not configured"))?;

        let hr = unsafe {
            swapchain
                .raw
                .Present(swapchain.present_interval, Dxgi::DXGI_PRESENT(0))
        };

        if hr == Foundation::DXGI_STATUS_OCCLUDED {
            // An occluded window is not an error; rendering continues and the
            // next successful present will display the accumulated result.
            log::debug!("D3D11 swap-chain present reported DXGI_STATUS_OCCLUDED");
            return Ok(());
        }

        if hr.is_err() {
            log::warn!(
                "IDXGISwapChain1::Present failed: {}",
                windows::core::Error::from_hresult(hr)
            );
            return Err(
                if hr == Dxgi::DXGI_ERROR_DEVICE_REMOVED || hr == Dxgi::DXGI_ERROR_DEVICE_RESET {
                    crate::SurfaceError::Lost
                } else {
                    crate::SurfaceError::Other("D3D11 swap-chain present failed")
                },
            );
        }

        Ok(())
    }
}

fn create_swapchain(
    factory: &Dxgi::IDXGIFactory2,
    device: &Device,
    hwnd: Foundation::HWND,
    desc: &Dxgi::DXGI_SWAP_CHAIN_DESC1,
    srgb_view: bool,
    present_interval: u32,
) -> Result<SwapChain, crate::SurfaceError> {
    profiling::scope!("IDXGIFactory2::CreateSwapChainForHwnd");
    let raw =
        unsafe { factory.CreateSwapChainForHwnd(device.raw_device(), hwnd, desc, None, None) }
            .map_err(|e| {
                log::error!("failed to create the D3D11 swap chain: {e}");
                crate::SurfaceError::Other("D3D11 swap-chain creation failed")
            })?;

    // `IDXGISwapChain3::GetCurrentBackBufferIndex` is the only reliable way
    // to know which flip-model buffer is the current one. It is available
    // on Windows 10+, which all supported D3D11 targets have in practice.
    let raw3 = raw.cast::<Dxgi::IDXGISwapChain3>().ok();
    if raw3.is_none() {
        log::warn!("IDXGISwapChain3 is unavailable; falling back to back-buffer 0 for acquisition");
    }

    let mut views = Vec::with_capacity(desc.BufferCount as usize);
    let mut textures = Vec::with_capacity(desc.BufferCount as usize);
    for index in 0..desc.BufferCount {
        let texture: Direct3D11::ID3D11Texture2D =
            unsafe { raw.GetBuffer(index) }.map_err(|e| {
                log::error!("failed to get swap-chain buffer {index}: {e}");
                crate::SurfaceError::Other("failed to access the D3D11 swap-chain buffer")
            })?;

        let view = if srgb_view {
            match TextureView::create_render_target(
                device.raw_device(),
                &texture,
                Some(Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM_SRGB),
            ) {
                Ok(view) => view,
                Err(e) => {
                    log::warn!(
                        "sRGB render-target view for swap-chain buffer {index} failed ({e}); \
                             falling back to the non-sRGB format"
                    );
                    TextureView::create_render_target(device.raw_device(), &texture, None).map_err(
                        |e| {
                            log::error!(
                                "failed to create a render-target view for swap-chain buffer \
                                     {index}: {e}"
                            );
                            crate::SurfaceError::Other(
                                "failed to create the D3D11 swap-chain render target",
                            )
                        },
                    )?
                }
            }
        } else {
            TextureView::create_render_target(device.raw_device(), &texture, None).map_err(|e| {
                log::error!(
                    "failed to create a render-target view for swap-chain buffer {index}: \
                             {e}"
                );
                crate::SurfaceError::Other("failed to create the D3D11 swap-chain render target")
            })?
        };

        views.push(view);
        textures.push(Texture::new(
            texture,
            if srgb_view {
                wgt::TextureFormat::Bgra8UnormSrgb
            } else {
                wgt::TextureFormat::Bgra8Unorm
            },
            wgt::Extent3d {
                width: desc.Width,
                height: desc.Height,
                depth_or_array_layers: 1,
            },
            1,
            1,
            1,
            wgt::TextureUses::COLOR_TARGET | wgt::TextureUses::PRESENT,
        ));
    }

    Ok(SwapChain {
        raw,
        raw3,
        views,
        textures,
        present_interval,
    })
}

impl crate::Surface for Surface {
    type A = Api;

    unsafe fn configure(
        &self,
        device: &Device,
        config: &crate::SurfaceConfiguration,
    ) -> Result<(), crate::SurfaceError> {
        if config.extent.width == 0 || config.extent.height == 0 {
            return Err(crate::SurfaceError::Other(
                "D3D11 surface extent must be non-zero",
            ));
        }
        if !config.usage.contains(wgt::TextureUses::COLOR_TARGET) {
            return Err(crate::SurfaceError::Other(
                "D3D11 surfaces only support COLOR_TARGET usage",
            ));
        }

        let srgb_view = match config.format {
            wgt::TextureFormat::Bgra8UnormSrgb => true,
            wgt::TextureFormat::Bgra8Unorm => false,
            _ => {
                return Err(crate::SurfaceError::Other(
                    "D3D11 surfaces only support Bgra8Unorm and Bgra8UnormSrgb",
                ));
            }
        };

        if !matches!(
            config.color_space,
            wgt::SurfaceColorSpace::Auto | wgt::SurfaceColorSpace::Srgb
        ) {
            return Err(crate::SurfaceError::Other(
                "D3D11 surfaces only support the sRGB color space",
            ));
        }

        let alpha_mode = match config.composite_alpha_mode {
            wgt::CompositeAlphaMode::Auto
            | wgt::CompositeAlphaMode::Inherit
            | wgt::CompositeAlphaMode::Opaque => Dxgi::Common::DXGI_ALPHA_MODE_IGNORE,
            _ => {
                return Err(crate::SurfaceError::Other(
                    "D3D11 surfaces only support Opaque composite alpha",
                ));
            }
        };

        let present_interval = match config.present_mode {
            wgt::PresentMode::Fifo | wgt::PresentMode::AutoVsync => 1,
            wgt::PresentMode::Immediate | wgt::PresentMode::AutoNoVsync => 0,
            _ => {
                return Err(crate::SurfaceError::Other(
                    "D3D11 surfaces only support Fifo and Immediate present modes",
                ));
            }
        };

        // The safety contract guarantees that no acquired textures are alive,
        // so the old swap chain (and its views) can be released first. This
        // also covers reconfiguration and window resize.
        let old_swapchain = self.state.lock().swapchain.take();
        drop(old_swapchain);

        let mut desc = Dxgi::DXGI_SWAP_CHAIN_DESC1 {
            Width: config.extent.width,
            Height: config.extent.height,
            Format: SWAPCHAIN_FORMAT,
            Stereo: false.into(),
            SampleDesc: Dxgi::Common::DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            BufferUsage: Dxgi::DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: BUFFER_COUNT,
            Scaling: Dxgi::DXGI_SCALING_STRETCH,
            SwapEffect: Dxgi::DXGI_SWAP_EFFECT_FLIP_DISCARD,
            AlphaMode: alpha_mode,
            Flags: 0,
        };

        let swapchain = match create_swapchain(
            &self.factory,
            device,
            self.hwnd,
            &desc,
            srgb_view,
            present_interval,
        ) {
            Ok(swapchain) => swapchain,
            Err(first_error) => {
                // Microsoft's WARP/basic render driver can report a flip-model
                // swap chain but fail to expose its second buffer. Fall back to
                // the legacy discard model with one buffer; the native Intel
                // driver normally takes the flip path above.
                log::warn!(
                    "flip-model D3D11 swap chain is unusable ({first_error:?}); \
                     retrying with DXGI_SWAP_EFFECT_DISCARD"
                );
                desc.SwapEffect = Dxgi::DXGI_SWAP_EFFECT_DISCARD;
                desc.BufferCount = 1;
                create_swapchain(
                    &self.factory,
                    device,
                    self.hwnd,
                    &desc,
                    srgb_view,
                    present_interval,
                )
                .map_err(|e| {
                    log::error!("failed to create a compatible D3D11 swap chain: {e:?}");
                    e
                })?
            }
        };

        let mut state = self.state.lock();
        state.swapchain = Some(swapchain);

        log::debug!(
            "configured D3D11 surface: {}x{}, B8G8R8A8_UNORM{}",
            config.extent.width,
            config.extent.height,
            if srgb_view {
                " (requested sRGB render-target views)"
            } else {
                ""
            }
        );

        Ok(())
    }

    unsafe fn unconfigure(&self, _device: &Device) {
        let mut state = self.state.lock();
        state.swapchain = None;
    }

    unsafe fn acquire_texture(
        &self,
        _timeout: Option<Duration>,
        _fence: &Fence,
    ) -> Result<crate::AcquiredSurfaceTexture<Api>, crate::SurfaceError> {
        let state = self.state.lock();
        let swapchain = state
            .swapchain
            .as_ref()
            .ok_or(crate::SurfaceError::Other("surface is not configured"))?;

        let index = swapchain
            .raw3
            .as_ref()
            .map(|raw3| unsafe { raw3.GetCurrentBackBufferIndex() } as usize)
            .unwrap_or(0)
            % swapchain.views.len();

        Ok(crate::AcquiredSurfaceTexture {
            texture: SurfaceTexture::new(
                swapchain.textures[index].clone(),
                swapchain.views[index].clone(),
            ),
            suboptimal: false,
        })
    }

    unsafe fn discard_texture(&self, _texture: SurfaceTexture) {}
}

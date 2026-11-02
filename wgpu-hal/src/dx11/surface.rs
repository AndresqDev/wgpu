//! Windowed DXGI swap-chain surface.
//!
//! The surface owns an `IDXGISwapChain1` created for a single `HWND`.
//! D3D11 exposes only buffer 0 of a flip-model swap chain to the application;
//! DXGI rotates the remaining buffers internally. The surface therefore keeps
//! one render-target view for flip-discard/discard and the full set only for
//! flip-sequential chains.
//!
//! Reconfiguration (including a window resize) is handled by releasing the
//! previous swap chain and creating a new one at the requested extent instead
//! of calling `ResizeBuffers`: the HAL contract guarantees that no acquired
//! textures are alive during [`crate::Surface::configure`], so no back-buffer
//! reference can outlive the old chain. The `raw-dx11-resize` example exercises
//! this path. `maximum_frame_latency` is applied through
//! `IDXGISwapChain2::SetMaximumFrameLatency` when that interface is available;
//! otherwise DXGI keeps its default frame-latency queue.

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
    ///
    /// The field order below is load-bearing: `views` and `textures` must be
    /// declared before `raw`, because Rust drops struct fields in declaration
    /// order. [`SwapChain`] also implements `Drop` to clear both vectors
    /// explicitly, so the invariant holds even if this order is changed.
    views: Vec<TextureView>,
    /// Identity wrappers for the back-buffer resources. They are handed out to
    /// `wgpu-core` through `SurfaceTexture`, which requires `Borrow<Texture>`.
    textures: Vec<Texture>,
    raw: Dxgi::IDXGISwapChain1,
    /// Used to find the current back-buffer index. Available on Windows 10 and
    /// later; without it the surface conservatively uses buffer 0.
    raw3: Option<Dxgi::IDXGISwapChain3>,
    swap_effect: Dxgi::DXGI_SWAP_EFFECT,
    present_interval: u32,
}

impl Drop for SwapChain {
    fn drop(&mut self) {
        // Release the per-buffer views and texture identities before the raw
        // swap chain goes away. `Drop::drop` runs before the fields are
        // dropped, so clearing here releases every back-buffer reference while
        // DXGI still owns the chain; the raw interface itself is released
        // afterwards in declaration order.
        self.views.clear();
        self.textures.clear();
        log::trace!(
            "releasing a D3D11 swap chain ({} present interval)",
            self.present_interval
        );
    }
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
                // A removed or reset device invalidates the swap chain as a
                // whole; report it as `Lost` so the caller can recreate the
                // device and surface instead of retrying the present.
                if matches!(
                    hr,
                    Dxgi::DXGI_ERROR_DEVICE_REMOVED
                        | Dxgi::DXGI_ERROR_DEVICE_RESET
                        | Dxgi::DXGI_ERROR_DEVICE_HUNG
                ) {
                    crate::SurfaceError::Lost
                } else {
                    crate::SurfaceError::Other("D3D11 swap-chain present failed")
                },
            );
        }

        log::trace!(
            "presented the D3D11 swap chain (interval {})",
            swapchain.present_interval
        );
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
    maximum_frame_latency: u32,
) -> Result<SwapChain, crate::SurfaceError> {
    profiling::scope!("IDXGIFactory2::CreateSwapChainForHwnd");
    let raw =
        unsafe { factory.CreateSwapChainForHwnd(device.raw_device(), hwnd, desc, None, None) }
            .map_err(|e| {
                log::error!("failed to create the D3D11 swap chain: {e}");
                crate::SurfaceError::Other("D3D11 swap-chain creation failed")
            })?;

    // DXGI's default queue can run one frame ahead. Cinnabar requests a
    // smaller window for synchronized presentation; apply it explicitly when
    // the Windows 8+ interface is present. Failure is not fatal: the default
    // DXGI queue is still a valid fallback.
    if let Ok(raw2) = raw.cast::<Dxgi::IDXGISwapChain2>() {
        if let Err(e) = unsafe { raw2.SetMaximumFrameLatency(maximum_frame_latency) } {
            log::warn!(
                "IDXGISwapChain2::SetMaximumFrameLatency({maximum_frame_latency}) failed: {e}"
            );
        }
    } else {
        log::warn!(
            "IDXGISwapChain2 is unavailable; maximum_frame_latency={maximum_frame_latency} is not applied"
        );
    }

    // `IDXGISwapChain3::GetCurrentBackBufferIndex` is the only reliable way
    // to know which flip-model buffer is the current one. It is available
    // on Windows 10+, which all supported D3D11 targets have in practice.
    let raw3 = raw.cast::<Dxgi::IDXGISwapChain3>().ok();
    if raw3.is_none() {
        log::warn!("IDXGISwapChain3 is unavailable; falling back to back-buffer 0 for acquisition");
    }

    // D3D11 flip-model swap chains only expose buffer 0 to the application;
    // DXGI rotates the remaining buffers internally. Only flip-sequential
    // chains use GetCurrentBackBufferIndex to select among accessible buffers.
    let buffer_count = if desc.SwapEffect == Dxgi::DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL {
        desc.BufferCount
    } else {
        1
    };
    let mut views = Vec::with_capacity(buffer_count as usize);
    let mut textures = Vec::with_capacity(buffer_count as usize);
    for index in 0..buffer_count {
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
        swap_effect: desc.SwapEffect,
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
        // also covers reconfiguration and window resize. `SwapChain::drop`
        // clears the render-target views and texture identities before the raw
        // swap chain, so DXGI sees no outstanding back-buffer references when
        // the replacement chain is created below.
        let old_swapchain = self.state.lock().swapchain.take();
        if old_swapchain.is_some() {
            log::debug!("releasing the previous D3D11 swap chain before reconfiguring");
        }
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
            config.maximum_frame_latency,
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
                    config.maximum_frame_latency,
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
        let swapchain = self.state.lock().swapchain.take();
        if swapchain.is_some() {
            log::debug!("unconfigured the D3D11 surface");
        }
        // Release the chain outside the state lock; `SwapChain::drop` releases
        // the views before the raw swap chain.
        drop(swapchain);
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

        // A swap chain always has at least one buffer, but never index into it
        // blind: an inconsistent chain must be an explicit error rather than a
        // panic in the HAL.
        if swapchain.views.is_empty() || swapchain.views.len() != swapchain.textures.len() {
            log::error!(
                "the D3D11 swap chain has {} views for {} textures",
                swapchain.views.len(),
                swapchain.textures.len()
            );
            return Err(crate::SurfaceError::Other(
                "D3D11 swap chain has no usable back buffers",
            ));
        }

        let index = if swapchain.swap_effect == Dxgi::DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL {
            swapchain
                .raw3
                .as_ref()
                .map(|raw3| unsafe { raw3.GetCurrentBackBufferIndex() } as usize)
                .unwrap_or(0)
                % swapchain.views.len()
        } else {
            0
        };

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

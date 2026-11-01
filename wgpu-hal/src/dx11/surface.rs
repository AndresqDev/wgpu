//! Surface stub.
//!
//! `Instance::create_surface` currently returns an error, so this type is never
//! constructed. It exists to satisfy `Api::Surface`. The next step is to store
//! an `HWND` (or a DXGI swap-chain target) and create an `IDXGISwapChain1`
//! during `Surface::configure`.

use core::time::Duration;

use super::{Api, Fence, Resource};

/// A swap-chain-backed surface.
#[derive(Debug)]
pub struct Surface;

impl crate::Surface for Surface {
    type A = Api;

    unsafe fn configure(
        &self,
        _device: &super::Device,
        _config: &crate::SurfaceConfiguration,
    ) -> Result<(), crate::SurfaceError> {
        Err(crate::SurfaceError::Other(
            "D3D11 surface configuration is not implemented yet",
        ))
    }

    unsafe fn unconfigure(&self, _device: &super::Device) {}

    unsafe fn acquire_texture(
        &self,
        _timeout: Option<Duration>,
        _fence: &Fence,
    ) -> Result<crate::AcquiredSurfaceTexture<Api>, crate::SurfaceError> {
        Err(crate::SurfaceError::Other(
            "D3D11 surface acquisition is not implemented yet",
        ))
    }

    unsafe fn discard_texture(&self, _texture: Resource) {}
}

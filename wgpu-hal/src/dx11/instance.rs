//! DXGI instance creation and adapter enumeration.

use alloc::{string::String, vec::Vec};
use core::fmt;

use windows::Win32::Graphics::Dxgi;

use super::Adapter;

/// D3D11 instance backed by a DXGI factory.
///
/// `IDXGIFactory1` is enough for `EnumAdapters1`, which is all the skeleton
/// needs. The final backend will keep an `IDXGIFactory2` (or newer) as well for
/// swap-chain creation.
pub struct Instance {
    factory: Dxgi::IDXGIFactory1,
    flags: wgt::InstanceFlags,
}

impl fmt::Debug for Instance {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Instance")
            .field("flags", &self.flags)
            .finish_non_exhaustive()
    }
}

impl crate::Instance for Instance {
    type A = super::Api;

    unsafe fn init(desc: &crate::InstanceDescriptor<'_>) -> Result<Self, crate::InstanceError> {
        profiling::scope!("Init DX11 Backend");

        // `CreateDXGIFactory1` is available on every Windows version that has
        // D3D11. Asking for `IDXGIFactory2` as well would only add swap-chain
        // features that this skeleton does not use yet.
        let factory =
            unsafe { Dxgi::CreateDXGIFactory1::<Dxgi::IDXGIFactory1>() }.map_err(|e| {
                crate::InstanceError::with_source(String::from("failed to create IDXGIFactory1"), e)
            })?;

        Ok(Self {
            factory,
            flags: desc.flags,
        })
    }

    unsafe fn create_surface(
        &self,
        _display_handle: raw_window_handle::RawDisplayHandle,
        _window_handle: raw_window_handle::RawWindowHandle,
    ) -> Result<super::Surface, crate::InstanceError> {
        Err(crate::InstanceError::new(String::from(
            "D3D11 surface creation is not implemented yet",
        )))
    }

    unsafe fn enumerate_adapters(
        &self,
        _surface_hint: Option<&super::Surface>,
    ) -> Vec<crate::ExposedAdapter<super::Api>> {
        let mut adapters = Vec::new();

        for index in 0.. {
            profiling::scope!("IDXGIFactory1::EnumAdapters1");
            let raw = match unsafe { self.factory.EnumAdapters1(index) } {
                Ok(adapter) => adapter,
                Err(e) => {
                    // DXGI_ERROR_NOT_FOUND is the normal end-of-list result.
                    log::trace!("EnumAdapters1({index}) stopped: {e}");
                    break;
                }
            };

            if let Some(exposed) = unsafe { Adapter::expose(raw) } {
                adapters.push(exposed);
            }
        }

        adapters
    }
}

impl Instance {
    /// Returns the DXGI factory used to enumerate adapters.
    pub fn raw_factory(&self) -> &Dxgi::IDXGIFactory1 {
        &self.factory
    }
}

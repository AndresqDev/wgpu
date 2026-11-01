//! DXGI instance creation, adapter enumeration, and surface creation.

use alloc::{string::String, vec::Vec};
use core::fmt;

use raw_window_handle::{RawDisplayHandle, RawWindowHandle};
use windows::Win32::{Foundation, Graphics::Dxgi};

use super::Adapter;

/// D3D11 instance backed by a DXGI factory.
///
/// The factory is requested as `IDXGIFactory2` because windowed presentation
/// needs `CreateSwapChainForHwnd`, which does not exist on `IDXGIFactory1`.
/// `CreateDXGIFactory1::<IDXGIFactory2>()` has been supported since Windows 8,
/// which is also the minimum version that supports the flip-model swap chains
/// used by [`super::Surface`]. No `IDXGIFactory1` fallback is needed: on older
/// systems the factory creation fails with a clear `E_NOINTERFACE` error.
pub struct Instance {
    factory: Dxgi::IDXGIFactory2,
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

        let factory =
            unsafe { Dxgi::CreateDXGIFactory1::<Dxgi::IDXGIFactory2>() }.map_err(|e| {
                crate::InstanceError::new(format!("failed to create IDXGIFactory2: {e}"))
            })?;

        Ok(Self {
            factory,
            flags: desc.flags,
        })
    }

    unsafe fn create_surface(
        &self,
        _display_handle: RawDisplayHandle,
        window_handle: RawWindowHandle,
    ) -> Result<super::Surface, crate::InstanceError> {
        // DXGI only needs the window handle for `CreateSwapChainForHwnd`.
        // The display handle carries no additional information that D3D11
        // surface creation can use, so any Win32 (or empty) display handle is
        // accepted here.
        let hwnd = match window_handle {
            RawWindowHandle::Win32(handle) => {
                Foundation::HWND(handle.hwnd.get() as *mut core::ffi::c_void)
            }
            _ => {
                return Err(crate::InstanceError::new(String::from(
                    "the D3D11 backend only supports Win32 windows",
                )));
            }
        };

        Ok(super::Surface::new(self.factory.clone(), hwnd))
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
    /// Returns the DXGI factory used to enumerate adapters and create surfaces.
    pub fn raw_factory(&self) -> &Dxgi::IDXGIFactory2 {
        &self.factory
    }
}

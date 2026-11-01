//! D3D11 adapter enumeration, device creation, and capability queries.

use alloc::{string::String, vec::Vec};
use core::{fmt, mem::size_of_val};

use windows::Win32::{
    Foundation,
    Graphics::{Direct3D, Direct3D11, Dxgi},
};

use super::{conv, device::Device, queue::Queue};

/// Creates an `ID3D11Device` for the given DXGI adapter.
///
/// `D3D_DRIVER_TYPE_UNKNOWN` is required when an explicit adapter is passed.
/// The requested feature levels are the ones the backend promises to support:
/// Direct3D 11.0 and 10.1.
pub(super) unsafe fn create_device(
    adapter: &Dxgi::IDXGIAdapter1,
) -> windows::core::Result<(
    Direct3D11::ID3D11Device,
    Direct3D11::ID3D11DeviceContext,
    Direct3D::D3D_FEATURE_LEVEL,
)> {
    let adapter: &Dxgi::IDXGIAdapter = adapter;
    let feature_levels = [
        Direct3D::D3D_FEATURE_LEVEL_11_0,
        Direct3D::D3D_FEATURE_LEVEL_10_1,
    ];

    let mut device: Option<Direct3D11::ID3D11Device> = None;
    let mut context: Option<Direct3D11::ID3D11DeviceContext> = None;
    let mut feature_level = Direct3D::D3D_FEATURE_LEVEL::default();

    unsafe {
        Direct3D11::D3D11CreateDevice(
            adapter,
            Direct3D::D3D_DRIVER_TYPE_UNKNOWN,
            Foundation::HMODULE::default(),
            Direct3D11::D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            Some(&feature_levels),
            Direct3D11::D3D11_SDK_VERSION,
            Some(&mut device),
            Some(&mut feature_level),
            Some(&mut context),
        )?;
    }

    let device =
        device.ok_or_else(|| windows::core::Error::from_hresult(Foundation::E_UNEXPECTED))?;
    let context =
        context.ok_or_else(|| windows::core::Error::from_hresult(Foundation::E_UNEXPECTED))?;

    Ok((device, context, feature_level))
}

/// A D3D11 adapter.
pub struct Adapter {
    raw: Dxgi::IDXGIAdapter1,
    /// Kept alive for capability queries such as format support.
    device: Direct3D11::ID3D11Device,
    info: wgt::AdapterInfo,
    feature_level: Direct3D::D3D_FEATURE_LEVEL,
    dedicated_video_memory: u64,
    shared_system_memory: u64,
    capabilities: crate::Capabilities,
}

impl fmt::Debug for Adapter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Adapter")
            .field("name", &self.info.name)
            .field(
                "feature_level",
                &conv::feature_level_name(self.feature_level),
            )
            .field("dedicated_video_memory", &self.dedicated_video_memory)
            .finish_non_exhaustive()
    }
}

impl Adapter {
    /// Enumerates one DXGI adapter and creates a D3D11 device to validate it.
    pub(super) unsafe fn expose(
        raw: Dxgi::IDXGIAdapter1,
    ) -> Option<crate::ExposedAdapter<super::Api>> {
        let desc = unsafe { raw.GetDesc1() }.ok()?;
        let (device, _context, feature_level) = match unsafe { create_device(&raw) } {
            Ok(created) => created,
            Err(e) => {
                log::warn!(
                    "D3D11CreateDevice failed for adapter {}: {e}",
                    conv::map_adapter_name(desc.Description)
                );
                return None;
            }
        };

        let software =
            Dxgi::DXGI_ADAPTER_FLAG(desc.Flags as i32).contains(Dxgi::DXGI_ADAPTER_FLAG_SOFTWARE);

        let mut options2 = Direct3D11::D3D11_FEATURE_DATA_D3D11_OPTIONS2::default();
        let uma = unsafe {
            device.CheckFeatureSupport(
                Direct3D11::D3D11_FEATURE_D3D11_OPTIONS2,
                (&mut options2 as *mut Direct3D11::D3D11_FEATURE_DATA_D3D11_OPTIONS2)
                    .cast::<core::ffi::c_void>(),
                size_of_val(&options2) as u32,
            )
        }
        .is_ok()
            && options2.UnifiedMemoryArchitecture.as_bool();

        let device_type = if software {
            wgt::DeviceType::Cpu
        } else if uma || desc.DedicatedVideoMemory == 0 {
            wgt::DeviceType::IntegratedGpu
        } else {
            wgt::DeviceType::DiscreteGpu
        };

        let info = wgt::AdapterInfo {
            name: conv::map_adapter_name(desc.Description),
            vendor: desc.VendorId,
            device: desc.DeviceId,
            device_type,
            device_pci_bus_id: String::new(),
            driver: String::new(),
            driver_info: String::new(),
            backend: wgt::Backend::Dx11,
            subgroup_min_size: wgt::MINIMUM_SUBGROUP_MIN_SIZE,
            subgroup_max_size: wgt::MAXIMUM_SUBGROUP_MAX_SIZE,
            transient_saves_memory: Some(false),
            limit_bucket: None,
        };

        let limits = conv::limits_for_feature_level(feature_level);
        let capabilities = crate::Capabilities {
            limits,
            alignments: conv::alignments(),
            downlevel: conv::downlevel_for_feature_level(feature_level),
            cooperative_matrix_properties: Vec::new(),
        };
        let features = conv::features_for_feature_level(feature_level);

        Some(crate::ExposedAdapter {
            adapter: Self {
                raw,
                device,
                info: info.clone(),
                feature_level,
                dedicated_video_memory: desc.DedicatedVideoMemory as u64,
                shared_system_memory: desc.SharedSystemMemory as u64,
                capabilities: capabilities.clone(),
            },
            info,
            features,
            capabilities,
        })
    }

    /// Returns the raw DXGI adapter.
    pub fn raw_adapter(&self) -> &Dxgi::IDXGIAdapter1 {
        &self.raw
    }

    /// Returns the feature level reported by `D3D11CreateDevice`.
    pub fn feature_level(&self) -> Direct3D::D3D_FEATURE_LEVEL {
        self.feature_level
    }

    /// Returns the D3D feature level as a short string, e.g. `"11_0"`.
    pub fn feature_level_name(&self) -> &'static str {
        conv::feature_level_name(self.feature_level)
    }

    /// Dedicated video memory reported by DXGI, in bytes.
    pub fn dedicated_video_memory(&self) -> u64 {
        self.dedicated_video_memory
    }

    /// Shared system memory reported by DXGI, in bytes.
    pub fn shared_system_memory(&self) -> u64 {
        self.shared_system_memory
    }
}

impl crate::Adapter for Adapter {
    type A = super::Api;

    unsafe fn open(
        &self,
        _features: wgt::Features,
        _limits: &wgt::Limits,
        _memory_hints: &wgt::MemoryHints,
    ) -> Result<crate::OpenDevice<super::Api>, crate::DeviceError> {
        let (raw_device, context, feature_level) =
            unsafe { create_device(&self.raw) }.map_err(|e| {
                log::error!("D3D11CreateDevice failed while opening adapter: {e}");
                crate::DeviceError::Lost
            })?;

        let device = Device::new(
            raw_device.clone(),
            context.clone(),
            self.info.clone(),
            feature_level,
            self.capabilities.limits.clone(),
        );
        let queue = Queue::new(context, raw_device);

        Ok(crate::OpenDevice { device, queue })
    }

    unsafe fn texture_format_capabilities(
        &self,
        format: wgt::TextureFormat,
    ) -> crate::TextureFormatCapabilities {
        use crate::TextureFormatCapabilities as Tfc;

        let Some(dxgi_format) = conv::map_texture_format(format) else {
            return Tfc::empty();
        };

        let mut support = Direct3D11::D3D11_FEATURE_DATA_FORMAT_SUPPORT {
            InFormat: dxgi_format,
            OutFormatSupport: 0,
        };
        let result = unsafe {
            self.device.CheckFeatureSupport(
                Direct3D11::D3D11_FEATURE_FORMAT_SUPPORT,
                (&mut support as *mut Direct3D11::D3D11_FEATURE_DATA_FORMAT_SUPPORT)
                    .cast::<core::ffi::c_void>(),
                size_of_val(&support) as u32,
            )
        };
        if result.is_err() {
            return Tfc::empty();
        }

        let bits = support.OutFormatSupport;
        let supports = |flag: Direct3D11::D3D11_FORMAT_SUPPORT| bits & flag.0 as u32 != 0;

        let mut caps = Tfc::empty();
        if supports(Direct3D11::D3D11_FORMAT_SUPPORT_TEXTURE2D) {
            caps |= Tfc::SAMPLED | Tfc::COPY_SRC | Tfc::COPY_DST;
        }
        if supports(Direct3D11::D3D11_FORMAT_SUPPORT_SHADER_SAMPLE) {
            caps |= Tfc::SAMPLED_LINEAR;
        }
        if supports(Direct3D11::D3D11_FORMAT_SUPPORT_RENDER_TARGET) {
            // Multisampled rendering is not supported by this backend yet, so no
            // `MULTISAMPLE_*` capability is ever advertised.
            caps |= Tfc::COLOR_ATTACHMENT;
        }
        if supports(Direct3D11::D3D11_FORMAT_SUPPORT_BLENDABLE) {
            caps |= Tfc::COLOR_ATTACHMENT_BLEND;
        }
        if supports(Direct3D11::D3D11_FORMAT_SUPPORT_DEPTH_STENCIL) {
            caps |= Tfc::DEPTH_STENCIL_ATTACHMENT;
        }

        caps
    }

    unsafe fn surface_capabilities(
        &self,
        _surface: &super::Surface,
    ) -> Option<crate::SurfaceCapabilities> {
        let formats: Vec<wgt::SurfaceFormatCapabilities> = [
            wgt::TextureFormat::Bgra8UnormSrgb,
            wgt::TextureFormat::Bgra8Unorm,
        ]
        .into_iter()
        .map(|format| wgt::SurfaceFormatCapabilities {
            format,
            color_spaces: wgt::SurfaceColorSpaces::SRGB,
        })
        .collect();

        Some(crate::SurfaceCapabilities {
            formats,
            // Advertised for API completeness only: `configure` ignores the
            // requested latency, so this value is not enforced. Documented
            // limitation, not a verified capability.
            maximum_frame_latency: 1..=2,
            current_extent: None,
            usage: wgt::TextureUses::COLOR_TARGET,
            present_modes: alloc::vec![wgt::PresentMode::Fifo, wgt::PresentMode::Immediate,],
            composite_alpha_modes: alloc::vec![wgt::CompositeAlphaMode::Opaque],
        })
    }

    unsafe fn get_presentation_timestamp(&self) -> wgt::PresentationTimestamp {
        wgt::PresentationTimestamp::INVALID_TIMESTAMP
    }

    fn get_ordered_buffer_usages(&self) -> wgt::BufferUses {
        // D3D11 performs hazard tracking inside the driver; there are no
        // application-visible resource transitions.
        wgt::BufferUses::all()
    }

    fn get_ordered_texture_usages(&self) -> wgt::TextureUses {
        // See `get_ordered_buffer_usages`.
        wgt::TextureUses::all()
    }
}

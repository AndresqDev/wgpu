//! Conversions between wgpu types and D3D11/DXGI types.
//!
//! Only the small subset needed by adapter enumeration and device creation is
//! implemented here. Format mapping is intentionally conservative: returning
//! `None` means that a format is not supported by this skeleton yet.

use alloc::string::String;

use windows::Win32::Graphics::{Direct3D, Direct3D11, Dxgi};

/// Converts the fixed-size UTF-16 adapter description into a Rust `String`.
pub fn map_adapter_name(name: [u16; 128]) -> String {
    let len = name.iter().take_while(|&&c| c != 0).count();
    String::from_utf16_lossy(&name[..len])
}

/// Human-readable D3D feature level, used by logging and the raw example.
pub fn feature_level_name(level: Direct3D::D3D_FEATURE_LEVEL) -> &'static str {
    if level == Direct3D::D3D_FEATURE_LEVEL_11_1 {
        "11_1"
    } else if level == Direct3D::D3D_FEATURE_LEVEL_11_0 {
        "11_0"
    } else if level == Direct3D::D3D_FEATURE_LEVEL_10_1 {
        "10_1"
    } else if level == Direct3D::D3D_FEATURE_LEVEL_10_0 {
        "10_0"
    } else {
        "unknown"
    }
}

/// Maps a wgpu texture format to a DXGI format for the basic formats.
pub fn map_texture_format(format: wgt::TextureFormat) -> Option<Dxgi::Common::DXGI_FORMAT> {
    use wgt::TextureFormat as Tf;
    use Dxgi::Common::*;

    Some(match format {
        Tf::R8Unorm => DXGI_FORMAT_R8_UNORM,
        Tf::R8Snorm => DXGI_FORMAT_R8_SNORM,
        Tf::R8Uint => DXGI_FORMAT_R8_UINT,
        Tf::R8Sint => DXGI_FORMAT_R8_SINT,
        Tf::R16Uint => DXGI_FORMAT_R16_UINT,
        Tf::R16Sint => DXGI_FORMAT_R16_SINT,
        Tf::R16Unorm => DXGI_FORMAT_R16_UNORM,
        Tf::R16Snorm => DXGI_FORMAT_R16_SNORM,
        Tf::R16Float => DXGI_FORMAT_R16_FLOAT,
        Tf::Rg8Unorm => DXGI_FORMAT_R8G8_UNORM,
        Tf::Rg8Snorm => DXGI_FORMAT_R8G8_SNORM,
        Tf::Rg8Uint => DXGI_FORMAT_R8G8_UINT,
        Tf::Rg8Sint => DXGI_FORMAT_R8G8_SINT,
        Tf::R32Uint => DXGI_FORMAT_R32_UINT,
        Tf::R32Sint => DXGI_FORMAT_R32_SINT,
        Tf::R32Float => DXGI_FORMAT_R32_FLOAT,
        Tf::Rg16Uint => DXGI_FORMAT_R16G16_UINT,
        Tf::Rg16Sint => DXGI_FORMAT_R16G16_SINT,
        Tf::Rg16Float => DXGI_FORMAT_R16G16_FLOAT,
        Tf::Rgba8Unorm => DXGI_FORMAT_R8G8B8A8_UNORM,
        Tf::Rgba8UnormSrgb => DXGI_FORMAT_R8G8B8A8_UNORM_SRGB,
        Tf::Bgra8Unorm => DXGI_FORMAT_B8G8R8A8_UNORM,
        Tf::Bgra8UnormSrgb => DXGI_FORMAT_B8G8R8A8_UNORM_SRGB,
        Tf::Rgba8Snorm => DXGI_FORMAT_R8G8B8A8_SNORM,
        Tf::Rgba8Uint => DXGI_FORMAT_R8G8B8A8_UINT,
        Tf::Rgba8Sint => DXGI_FORMAT_R8G8B8A8_SINT,
        Tf::Rgb10a2Unorm => DXGI_FORMAT_R10G10B10A2_UNORM,
        Tf::Rgb10a2Uint => DXGI_FORMAT_R10G10B10A2_UINT,
        Tf::Rg11b10Ufloat => DXGI_FORMAT_R11G11B10_FLOAT,
        Tf::Rgba16Float => DXGI_FORMAT_R16G16B16A16_FLOAT,
        Tf::Rgba16Unorm => DXGI_FORMAT_R16G16B16A16_UNORM,
        Tf::Rgba16Snorm => DXGI_FORMAT_R16G16B16A16_SNORM,
        Tf::Rgba16Uint => DXGI_FORMAT_R16G16B16A16_UINT,
        Tf::Rgba16Sint => DXGI_FORMAT_R16G16B16A16_SINT,
        Tf::Rgba32Float => DXGI_FORMAT_R32G32B32A32_FLOAT,
        Tf::Rgba32Uint => DXGI_FORMAT_R32G32B32A32_UINT,
        Tf::Rgba32Sint => DXGI_FORMAT_R32G32B32A32_SINT,
        Tf::Depth16Unorm => DXGI_FORMAT_D16_UNORM,
        Tf::Depth24Plus | Tf::Depth24PlusStencil8 => DXGI_FORMAT_D24_UNORM_S8_UINT,
        Tf::Depth32Float => DXGI_FORMAT_D32_FLOAT,
        Tf::Depth32FloatStencil8 => DXGI_FORMAT_D32_FLOAT_S8X24_UINT,
        _ => return None,
    })
}

/// Maps the format used by a shader resource view.
///
/// D3D11 depth textures expose their depth/stencil data through the
/// corresponding typeless/color formats when sampled or read from a shader.
pub fn map_srv_format(
    format: wgt::TextureFormat,
    aspect: wgt::TextureAspect,
) -> Option<Dxgi::Common::DXGI_FORMAT> {
    use wgt::TextureAspect as Ta;
    use Dxgi::Common::*;

    let format = map_texture_format(format)?;
    Some(match (format, aspect) {
        (DXGI_FORMAT_D32_FLOAT_S8X24_UINT, Ta::StencilOnly) => DXGI_FORMAT_X32_TYPELESS_G8X24_UINT,
        (DXGI_FORMAT_D24_UNORM_S8_UINT, Ta::StencilOnly) => DXGI_FORMAT_X24_TYPELESS_G8_UINT,
        (DXGI_FORMAT_D32_FLOAT_S8X24_UINT, _) => DXGI_FORMAT_R32_FLOAT_X8X24_TYPELESS,
        (DXGI_FORMAT_D24_UNORM_S8_UINT, _) => DXGI_FORMAT_R24_UNORM_X8_TYPELESS,
        (DXGI_FORMAT_D32_FLOAT, _) => DXGI_FORMAT_R32_FLOAT,
        (DXGI_FORMAT_D16_UNORM, _) => DXGI_FORMAT_R16_UNORM,
        (format, _) => format,
    })
}

/// Maps the format used by a depth-stencil view for the selected aspect.
pub fn map_dsv_format(
    format: wgt::TextureFormat,
    aspect: wgt::TextureAspect,
) -> Option<Dxgi::Common::DXGI_FORMAT> {
    use wgt::TextureAspect as Ta;
    use Dxgi::Common::*;

    let format = map_texture_format(format)?;
    Some(match (format, aspect) {
        (DXGI_FORMAT_D32_FLOAT_S8X24_UINT, Ta::StencilOnly) => DXGI_FORMAT_X32_TYPELESS_G8X24_UINT,
        (DXGI_FORMAT_D24_UNORM_S8_UINT, Ta::StencilOnly) => DXGI_FORMAT_X24_TYPELESS_G8_UINT,
        (DXGI_FORMAT_D32_FLOAT_S8X24_UINT, Ta::DepthOnly) => DXGI_FORMAT_D32_FLOAT,
        (format, _) => format,
    })
}

/// Builds a D3D11 filter from the WebGPU filter description.
pub fn map_filter(
    mag_filter: wgt::FilterMode,
    min_filter: wgt::FilterMode,
    mipmap_filter: wgt::MipmapFilterMode,
    comparison: Option<wgt::CompareFunction>,
    anisotropy_clamp: u16,
) -> Direct3D11::D3D11_FILTER {
    use Direct3D11::*;

    if anisotropy_clamp > 1 {
        return if comparison.is_some() {
            D3D11_FILTER_COMPARISON_ANISOTROPIC
        } else {
            D3D11_FILTER_ANISOTROPIC
        };
    }

    // D3D11_FILTER packs the min/mag/mip filter types in the low six bits and
    // the comparison-reduction type in bit 7.
    let min = (min_filter == wgt::FilterMode::Linear) as i32;
    let mag = (mag_filter == wgt::FilterMode::Linear) as i32;
    let mip = (mipmap_filter == wgt::MipmapFilterMode::Linear) as i32;
    let reduction = comparison.is_some() as i32;
    D3D11_FILTER(min | (mag << 2) | (mip << 4) | (reduction << 7))
}

/// Maps a WebGPU address mode to a D3D11 texture address mode.
pub fn map_address_mode(mode: wgt::AddressMode) -> Direct3D11::D3D11_TEXTURE_ADDRESS_MODE {
    use Direct3D11::*;

    match mode {
        wgt::AddressMode::ClampToEdge => D3D11_TEXTURE_ADDRESS_CLAMP,
        wgt::AddressMode::Repeat => D3D11_TEXTURE_ADDRESS_WRAP,
        wgt::AddressMode::MirrorRepeat => D3D11_TEXTURE_ADDRESS_MIRROR,
        wgt::AddressMode::ClampToBorder => D3D11_TEXTURE_ADDRESS_BORDER,
    }
}

/// Maps a WebGPU comparison function to a D3D11 comparison function.
pub fn map_comparison(compare: wgt::CompareFunction) -> Direct3D11::D3D11_COMPARISON_FUNC {
    use Direct3D11::*;

    match compare {
        wgt::CompareFunction::Never => D3D11_COMPARISON_NEVER,
        wgt::CompareFunction::Less => D3D11_COMPARISON_LESS,
        wgt::CompareFunction::Equal => D3D11_COMPARISON_EQUAL,
        wgt::CompareFunction::LessEqual => D3D11_COMPARISON_LESS_EQUAL,
        wgt::CompareFunction::Greater => D3D11_COMPARISON_GREATER,
        wgt::CompareFunction::NotEqual => D3D11_COMPARISON_NOT_EQUAL,
        wgt::CompareFunction::GreaterEqual => D3D11_COMPARISON_GREATER_EQUAL,
        wgt::CompareFunction::Always => D3D11_COMPARISON_ALWAYS,
    }
}

/// Maps a WebGPU sampler border color to the four floats D3D11 expects.
pub fn map_border_color(color: wgt::SamplerBorderColor) -> [f32; 4] {
    match color {
        wgt::SamplerBorderColor::TransparentBlack | wgt::SamplerBorderColor::Zero => [0.0; 4],
        wgt::SamplerBorderColor::OpaqueBlack => [0.0, 0.0, 0.0, 1.0],
        wgt::SamplerBorderColor::OpaqueWhite => [1.0; 4],
    }
}

/// Feature-dependent limits.
///
/// These are deliberately conservative. The actual D3D11 limits are queried
/// from the device in the full implementation; this is enough for the raw
/// device-creation smoke test and for `wgpu-core` to enumerate the adapter.
pub fn limits_for_feature_level(level: Direct3D::D3D_FEATURE_LEVEL) -> wgt::Limits {
    let mut limits = wgt::Limits::downlevel_defaults();
    let max_texture_dimension = if level.0 >= (Direct3D::D3D_FEATURE_LEVEL_11_0).0 {
        16384
    } else {
        8192
    };

    limits.max_texture_dimension_1d = max_texture_dimension;
    limits.max_texture_dimension_2d = max_texture_dimension;
    limits.max_texture_dimension_3d = 2048;
    limits.max_texture_array_layers = 2048;
    limits
}

/// Feature-dependent downlevel capabilities.
pub fn downlevel_for_feature_level(
    level: Direct3D::D3D_FEATURE_LEVEL,
) -> wgt::DownlevelCapabilities {
    let mut flags = wgt::DownlevelFlags::compliant();

    // D3D11 does not expose arbitrary view formats, and query resolves are not
    // implemented by this skeleton.
    flags.remove(
        wgt::DownlevelFlags::VIEW_FORMATS
            | wgt::DownlevelFlags::SURFACE_VIEW_FORMATS
            | wgt::DownlevelFlags::NONBLOCKING_QUERY_RESOLVE,
    );

    if level.0 < (Direct3D::D3D_FEATURE_LEVEL_11_0).0 {
        // Feature level 10_1 has no guaranteed compute support, and therefore
        // no writable storage images or vertex/fragment storage buffers.
        flags.remove(
            wgt::DownlevelFlags::COMPUTE_SHADERS
                | wgt::DownlevelFlags::FRAGMENT_WRITABLE_STORAGE
                | wgt::DownlevelFlags::FRAGMENT_STORAGE
                | wgt::DownlevelFlags::VERTEX_STORAGE,
        );
    }

    wgt::DownlevelCapabilities {
        flags,
        limits: wgt::DownlevelLimits::default(),
        shader_model: if level.0 >= (Direct3D::D3D_FEATURE_LEVEL_11_0).0 {
            wgt::ShaderModel::Sm5
        } else {
            wgt::ShaderModel::Sm4
        },
    }
}

/// Minimal set of features that are safe to expose from the skeleton.
pub fn features_for_feature_level(level: Direct3D::D3D_FEATURE_LEVEL) -> wgt::Features {
    let mut features = wgt::Features::DEPTH_CLIP_CONTROL;
    if level.0 >= (Direct3D::D3D_FEATURE_LEVEL_11_0).0 {
        features |= wgt::Features::TEXTURE_COMPRESSION_BC;
    }
    features
}

/// Alignment requirements used by the stub device.
pub fn alignments() -> crate::Alignments {
    crate::Alignments {
        buffer_copy_offset: wgt::BufferSize::MIN,
        buffer_copy_pitch: wgt::BufferSize::MIN,
        uniform_bounds_check_alignment: wgt::BufferSize::MIN,
        raw_tlas_instance_size: 0,
        ray_tracing_scratch_buffer_alignment: 1,
        ray_tracing_pipeline_group_data_size: 1,
        ray_tracing_pipeline_group_data_alignment: 1,
        ray_tracing_pipeline_data_offset_alignment: 1,
    }
}

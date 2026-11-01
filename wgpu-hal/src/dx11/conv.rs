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

/// Maps the resource format used by a texture.
///
/// Depth-stencil resources must be created with a typeless DXGI format so
/// that D3D11 can create typed DSV/SRV views and so that staging textures can
/// be copied into them. `map_texture_format` remains the typed format used by
/// views, pipelines and adapter capability queries.
pub fn map_texture_format_resource(
    format: wgt::TextureFormat,
) -> Option<Dxgi::Common::DXGI_FORMAT> {
    use wgt::TextureFormat as Tf;
    use Dxgi::Common::*;

    Some(match format {
        Tf::Depth16Unorm => DXGI_FORMAT_R16_TYPELESS,
        Tf::Depth24Plus | Tf::Depth24PlusStencil8 => DXGI_FORMAT_R24G8_TYPELESS,
        Tf::Depth32Float => DXGI_FORMAT_R32_TYPELESS,
        Tf::Depth32FloatStencil8 => DXGI_FORMAT_R32G8X24_TYPELESS,
        _ => return map_texture_format(format),
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
        // A depth-only view of a packed depth/stencil resource still has to
        // use the packed DSV format; D32_FLOAT is not compatible with the
        // R32G8X24_TYPELESS resource that backs Depth32FloatStencil8.
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

/// Maps a WebGPU vertex format to a DXGI format usable in an input layout.
///
/// 64-bit vertex attributes are not supported by this backend yet.
pub fn map_vertex_format(format: wgt::VertexFormat) -> Option<Dxgi::Common::DXGI_FORMAT> {
    use wgt::VertexFormat as Vf;
    use Dxgi::Common::*;

    Some(match format {
        Vf::Uint8 => DXGI_FORMAT_R8_UINT,
        Vf::Uint8x2 => DXGI_FORMAT_R8G8_UINT,
        Vf::Uint8x4 => DXGI_FORMAT_R8G8B8A8_UINT,
        Vf::Sint8 => DXGI_FORMAT_R8_SINT,
        Vf::Sint8x2 => DXGI_FORMAT_R8G8_SINT,
        Vf::Sint8x4 => DXGI_FORMAT_R8G8B8A8_SINT,
        Vf::Unorm8 => DXGI_FORMAT_R8_UNORM,
        Vf::Unorm8x2 => DXGI_FORMAT_R8G8_UNORM,
        Vf::Unorm8x4 => DXGI_FORMAT_R8G8B8A8_UNORM,
        Vf::Snorm8 => DXGI_FORMAT_R8_SNORM,
        Vf::Snorm8x2 => DXGI_FORMAT_R8G8_SNORM,
        Vf::Snorm8x4 => DXGI_FORMAT_R8G8B8A8_SNORM,
        Vf::Uint16 => DXGI_FORMAT_R16_UINT,
        Vf::Uint16x2 => DXGI_FORMAT_R16G16_UINT,
        Vf::Uint16x4 => DXGI_FORMAT_R16G16B16A16_UINT,
        Vf::Sint16 => DXGI_FORMAT_R16_SINT,
        Vf::Sint16x2 => DXGI_FORMAT_R16G16_SINT,
        Vf::Sint16x4 => DXGI_FORMAT_R16G16B16A16_SINT,
        Vf::Unorm16 => DXGI_FORMAT_R16_UNORM,
        Vf::Unorm16x2 => DXGI_FORMAT_R16G16_UNORM,
        Vf::Unorm16x4 => DXGI_FORMAT_R16G16B16A16_UNORM,
        Vf::Snorm16 => DXGI_FORMAT_R16_SNORM,
        Vf::Snorm16x2 => DXGI_FORMAT_R16G16_SNORM,
        Vf::Snorm16x4 => DXGI_FORMAT_R16G16B16A16_SNORM,
        Vf::Float16 => DXGI_FORMAT_R16_FLOAT,
        Vf::Float16x2 => DXGI_FORMAT_R16G16_FLOAT,
        Vf::Float16x4 => DXGI_FORMAT_R16G16B16A16_FLOAT,
        Vf::Float32 => DXGI_FORMAT_R32_FLOAT,
        Vf::Float32x2 => DXGI_FORMAT_R32G32_FLOAT,
        Vf::Float32x3 => DXGI_FORMAT_R32G32B32_FLOAT,
        Vf::Float32x4 => DXGI_FORMAT_R32G32B32A32_FLOAT,
        Vf::Uint32 => DXGI_FORMAT_R32_UINT,
        Vf::Uint32x2 => DXGI_FORMAT_R32G32_UINT,
        Vf::Uint32x3 => DXGI_FORMAT_R32G32B32_UINT,
        Vf::Uint32x4 => DXGI_FORMAT_R32G32B32A32_UINT,
        Vf::Sint32 => DXGI_FORMAT_R32_SINT,
        Vf::Sint32x2 => DXGI_FORMAT_R32G32_SINT,
        Vf::Sint32x3 => DXGI_FORMAT_R32G32B32_SINT,
        Vf::Sint32x4 => DXGI_FORMAT_R32G32B32A32_SINT,
        Vf::Unorm10_10_10_2 => DXGI_FORMAT_R10G10B10A2_UNORM,
        Vf::Unorm8x4Bgra => DXGI_FORMAT_B8G8R8A8_UNORM,
        Vf::Float64 | Vf::Float64x2 | Vf::Float64x3 | Vf::Float64x4 => return None,
    })
}

/// Maps a WebGPU index format to a DXGI format for `IASetIndexBuffer`.
pub fn map_index_format(format: wgt::IndexFormat) -> Dxgi::Common::DXGI_FORMAT {
    match format {
        wgt::IndexFormat::Uint16 => Dxgi::Common::DXGI_FORMAT_R16_UINT,
        wgt::IndexFormat::Uint32 => Dxgi::Common::DXGI_FORMAT_R32_UINT,
    }
}

/// Maps a WebGPU primitive topology to a D3D11 primitive topology.
pub fn map_primitive_topology(
    topology: wgt::PrimitiveTopology,
) -> Direct3D::D3D_PRIMITIVE_TOPOLOGY {
    use wgt::PrimitiveTopology as Pt;

    match topology {
        Pt::PointList => Direct3D::D3D_PRIMITIVE_TOPOLOGY_POINTLIST,
        Pt::LineList => Direct3D::D3D_PRIMITIVE_TOPOLOGY_LINELIST,
        Pt::LineStrip => Direct3D::D3D_PRIMITIVE_TOPOLOGY_LINESTRIP,
        Pt::TriangleList => Direct3D::D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST,
        Pt::TriangleStrip => Direct3D::D3D_PRIMITIVE_TOPOLOGY_TRIANGLESTRIP,
    }
}

/// Maps a WebGPU blend factor, rejecting dual-source blend factors.
///
/// `is_alpha` selects the alpha equivalent for factors that use the source or
/// destination color in the color equation.
pub fn map_blend_factor(
    factor: wgt::BlendFactor,
    is_alpha: bool,
) -> Option<Direct3D11::D3D11_BLEND> {
    use wgt::BlendFactor as Bf;

    Some(match factor {
        Bf::Zero => Direct3D11::D3D11_BLEND_ZERO,
        Bf::One => Direct3D11::D3D11_BLEND_ONE,
        Bf::Src if is_alpha => Direct3D11::D3D11_BLEND_SRC_ALPHA,
        Bf::Src => Direct3D11::D3D11_BLEND_SRC_COLOR,
        Bf::OneMinusSrc if is_alpha => Direct3D11::D3D11_BLEND_INV_SRC_ALPHA,
        Bf::OneMinusSrc => Direct3D11::D3D11_BLEND_INV_SRC_COLOR,
        Bf::Dst if is_alpha => Direct3D11::D3D11_BLEND_DEST_ALPHA,
        Bf::Dst => Direct3D11::D3D11_BLEND_DEST_COLOR,
        Bf::OneMinusDst if is_alpha => Direct3D11::D3D11_BLEND_INV_DEST_ALPHA,
        Bf::OneMinusDst => Direct3D11::D3D11_BLEND_INV_DEST_COLOR,
        Bf::SrcAlpha => Direct3D11::D3D11_BLEND_SRC_ALPHA,
        Bf::OneMinusSrcAlpha => Direct3D11::D3D11_BLEND_INV_SRC_ALPHA,
        Bf::DstAlpha => Direct3D11::D3D11_BLEND_DEST_ALPHA,
        Bf::OneMinusDstAlpha => Direct3D11::D3D11_BLEND_INV_DEST_ALPHA,
        Bf::Constant => Direct3D11::D3D11_BLEND_BLEND_FACTOR,
        Bf::OneMinusConstant => Direct3D11::D3D11_BLEND_INV_BLEND_FACTOR,
        Bf::SrcAlphaSaturated => Direct3D11::D3D11_BLEND_SRC_ALPHA_SAT,
        Bf::Src1 | Bf::OneMinusSrc1 | Bf::Src1Alpha | Bf::OneMinusSrc1Alpha => return None,
    })
}

/// Maps a WebGPU blend operation to a D3D11 blend operation.
pub fn map_blend_operation(operation: wgt::BlendOperation) -> Direct3D11::D3D11_BLEND_OP {
    match operation {
        wgt::BlendOperation::Add => Direct3D11::D3D11_BLEND_OP_ADD,
        wgt::BlendOperation::Subtract => Direct3D11::D3D11_BLEND_OP_SUBTRACT,
        wgt::BlendOperation::ReverseSubtract => Direct3D11::D3D11_BLEND_OP_REV_SUBTRACT,
        wgt::BlendOperation::Min => Direct3D11::D3D11_BLEND_OP_MIN,
        wgt::BlendOperation::Max => Direct3D11::D3D11_BLEND_OP_MAX,
    }
}

/// Maps a WebGPU stencil operation to a D3D11 stencil operation.
pub fn map_stencil_operation(operation: wgt::StencilOperation) -> Direct3D11::D3D11_STENCIL_OP {
    use wgt::StencilOperation as So;

    match operation {
        So::Keep => Direct3D11::D3D11_STENCIL_OP_KEEP,
        So::Zero => Direct3D11::D3D11_STENCIL_OP_ZERO,
        So::Replace => Direct3D11::D3D11_STENCIL_OP_REPLACE,
        So::IncrementClamp => Direct3D11::D3D11_STENCIL_OP_INCR_SAT,
        So::IncrementWrap => Direct3D11::D3D11_STENCIL_OP_INCR,
        So::DecrementClamp => Direct3D11::D3D11_STENCIL_OP_DECR_SAT,
        So::DecrementWrap => Direct3D11::D3D11_STENCIL_OP_DECR,
        So::Invert => Direct3D11::D3D11_STENCIL_OP_INVERT,
    }
}

/// Maps one stencil face state to its D3D11 description.
pub fn map_stencil_face(face: &wgt::StencilFaceState) -> Direct3D11::D3D11_DEPTH_STENCILOP_DESC {
    Direct3D11::D3D11_DEPTH_STENCILOP_DESC {
        StencilFailOp: map_stencil_operation(face.fail_op),
        StencilDepthFailOp: map_stencil_operation(face.depth_fail_op),
        StencilPassOp: map_stencil_operation(face.pass_op),
        StencilFunc: map_comparison(face.compare),
    }
}

/// Feature-dependent limits.
///
/// This is the Gate 0 contract table: every field is spelled out explicitly so
/// that the adapter report matches the documented target exactly. Storage and
/// compute limits are zero because the backend does not implement them yet.
pub fn limits_for_feature_level(level: Direct3D::D3D_FEATURE_LEVEL) -> wgt::Limits {
    let max_texture_dimension_2d = if level.0 >= (Direct3D::D3D_FEATURE_LEVEL_11_0).0 {
        16384
    } else {
        8192
    };

    wgt::Limits {
        max_texture_dimension_1d: 0,
        max_texture_dimension_2d,
        max_texture_dimension_3d: 0,
        max_texture_array_layers: 2048,
        max_bind_groups: 4,
        max_bind_groups_plus_vertex_buffers: 24,
        max_bindings_per_bind_group: 64,
        max_dynamic_uniform_buffers_per_pipeline_layout: 8,
        max_dynamic_storage_buffers_per_pipeline_layout: 0,
        max_sampled_textures_per_shader_stage: 16,
        max_samplers_per_shader_stage: 8,
        max_storage_buffers_per_shader_stage: 0,
        max_storage_textures_per_shader_stage: 0,
        max_uniform_buffers_per_shader_stage: 7,
        max_binding_array_elements_per_shader_stage: 0,
        max_binding_array_acceleration_structure_elements_per_shader_stage: 0,
        max_binding_array_sampler_elements_per_shader_stage: 0,
        max_uniform_buffer_binding_size: 16384,
        max_storage_buffer_binding_size: 0,
        max_vertex_buffers: 8,
        max_buffer_size: 268435456,
        max_vertex_attributes: 16,
        max_vertex_buffer_array_stride: 2048,
        max_inter_stage_shader_variables: 15,
        min_uniform_buffer_offset_alignment: 256,
        min_storage_buffer_offset_alignment: 256,
        max_color_attachments: 4,
        max_color_attachment_bytes_per_sample: 32,
        max_compute_workgroup_storage_size: 0,
        max_compute_invocations_per_workgroup: 0,
        max_compute_workgroup_size_x: 0,
        max_compute_workgroup_size_y: 0,
        max_compute_workgroup_size_z: 0,
        max_compute_workgroups_per_dimension: 0,
        max_immediate_size: 0,
        max_non_sampler_bindings: 1_000_000,
        max_task_workgroup_total_count: 0,
        max_task_workgroups_per_dimension: 0,
        max_mesh_workgroup_total_count: 0,
        max_mesh_workgroups_per_dimension: 0,
        max_task_invocations_per_workgroup: 0,
        max_task_invocations_per_dimension: 0,
        max_mesh_invocations_per_workgroup: 0,
        max_mesh_invocations_per_dimension: 0,
        max_task_payload_size: 0,
        max_mesh_output_vertices: 0,
        max_mesh_output_primitives: 0,
        max_mesh_output_layers: 0,
        max_mesh_multiview_view_count: 0,
        max_blas_primitive_count: 0,
        max_blas_geometry_count: 0,
        max_tlas_instance_count: 0,
        max_acceleration_structures_per_shader_stage: 0,
        max_buffers_and_acceleration_structures_per_shader_stage: 12,
        max_multiview_view_count: 0,
        max_ray_dispatch_count: 0,
        max_ray_recursion_depth: 0,
    }
}

/// Feature-dependent downlevel capabilities.
///
/// Gate 0 exposes exactly the flags below and nothing else; in particular it
/// does not start from [`wgt::DownlevelFlags::compliant`], so
/// `is_webgpu_compliant()` is always false.
pub fn downlevel_for_feature_level(
    level: Direct3D::D3D_FEATURE_LEVEL,
) -> wgt::DownlevelCapabilities {
    let flags = wgt::DownlevelFlags::BASE_VERTEX
        | wgt::DownlevelFlags::NON_POWER_OF_TWO_MIPMAPPED_TEXTURES
        | wgt::DownlevelFlags::CUBE_ARRAY_TEXTURES
        | wgt::DownlevelFlags::COMPARISON_SAMPLERS
        | wgt::DownlevelFlags::INDEPENDENT_BLEND
        | wgt::DownlevelFlags::FULL_DRAW_INDEX_UINT32
        | wgt::DownlevelFlags::DEPTH_BIAS_CLAMP;

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

/// Minimal set of features that are safe to expose from the backend.
///
/// Both feature levels advertise the same set: texture compression (BC) is not
/// supported by the mapped format allowlist, so it is not exposed at all.
pub fn features_for_feature_level(_level: Direct3D::D3D_FEATURE_LEVEL) -> wgt::Features {
    wgt::Features::DEPTH_CLIP_CONTROL | wgt::Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES
}

/// Alignment requirements used by the device.
///
/// D3D11 buffer copies are expressed as subresource boxes whose coordinates
/// are byte offsets, and the backend requires them to be multiples of four
/// bytes. Textures are uploaded/read back through staging buffers with a row
/// pitch the backend repacks itself, but the public layout is still limited to
/// 4-byte offsets and pitches instead of advertising an alignment the copies
/// reject. D3D11 constant-buffer windows are 16-byte granular.
/// checked against the 16-byte D3D11 requirement in `set_bind_group`.
pub fn alignments() -> crate::Alignments {
    crate::Alignments {
        buffer_copy_offset: wgt::BufferSize::new(16).unwrap(),
        buffer_copy_pitch: wgt::BufferSize::new(16).unwrap(),
        uniform_bounds_check_alignment: wgt::BufferSize::new(16).unwrap(),
        raw_tlas_instance_size: 0,
        ray_tracing_scratch_buffer_alignment: 1,
        ray_tracing_pipeline_group_data_size: 1,
        ray_tracing_pipeline_group_data_alignment: 1,
        ray_tracing_pipeline_data_offset_alignment: 1,
    }
}

/// Pure Gate 0 contract tests: no GPU, no device, no adapter.
#[cfg(test)]
mod tests {
    use super::*;

    use windows::Win32::Graphics::Direct3D::{D3D_FEATURE_LEVEL_10_1, D3D_FEATURE_LEVEL_11_0};

    fn expected_downlevel_flags() -> wgt::DownlevelFlags {
        wgt::DownlevelFlags::BASE_VERTEX
            | wgt::DownlevelFlags::NON_POWER_OF_TWO_MIPMAPPED_TEXTURES
            | wgt::DownlevelFlags::CUBE_ARRAY_TEXTURES
            | wgt::DownlevelFlags::COMPARISON_SAMPLERS
            | wgt::DownlevelFlags::INDEPENDENT_BLEND
            | wgt::DownlevelFlags::FULL_DRAW_INDEX_UINT32
            | wgt::DownlevelFlags::DEPTH_BIAS_CLAMP
    }

    #[test]
    fn features_are_exactly_the_two_gate0_flags() {
        let expected = wgt::Features::DEPTH_CLIP_CONTROL
            | wgt::Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES;

        for level in [D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_10_1] {
            let features = features_for_feature_level(level);
            assert_eq!(features, expected, "unexpected features for {level:?}");
            assert!(
                !features.contains(wgt::Features::TEXTURE_COMPRESSION_BC),
                "TEXTURE_COMPRESSION_BC must not be advertised for {level:?}"
            );
        }
    }

    #[test]
    fn downlevel_flags_shader_model_and_compliance() {
        for (level, shader_model) in [
            (D3D_FEATURE_LEVEL_11_0, wgt::ShaderModel::Sm5),
            (D3D_FEATURE_LEVEL_10_1, wgt::ShaderModel::Sm4),
        ] {
            let downlevel = downlevel_for_feature_level(level);
            assert_eq!(
                downlevel.flags,
                expected_downlevel_flags(),
                "unexpected downlevel flags for {level:?}"
            );
            assert_eq!(downlevel.shader_model, shader_model);
            assert_eq!(downlevel.limits, wgt::DownlevelLimits::default());
            assert!(
                !downlevel.is_webgpu_compliant(),
                "DX11 must never report WebGPU compliance for {level:?}"
            );
        }
    }

    #[test]
    fn limits_match_the_gate0_table_for_11_0() {
        assert_gate0_limits(limits_for_feature_level(D3D_FEATURE_LEVEL_11_0), 16384);
    }

    #[test]
    fn limits_match_the_gate0_table_for_10_1() {
        assert_gate0_limits(limits_for_feature_level(D3D_FEATURE_LEVEL_10_1), 8192);
    }

    /// Asserts every Gate 0 limit value, plus that no other field differs from
    /// the explicit table (the table is spelled out field by field).
    fn assert_gate0_limits(limits: wgt::Limits, expected_2d: u32) {
        assert_eq!(limits.max_texture_dimension_1d, 0);
        assert_eq!(limits.max_texture_dimension_2d, expected_2d);
        assert_eq!(limits.max_texture_dimension_3d, 0);
        assert_eq!(limits.max_texture_array_layers, 2048);
        assert_eq!(limits.max_bind_groups, 4);
        assert_eq!(limits.max_bind_groups_plus_vertex_buffers, 24);
        assert_eq!(limits.max_bindings_per_bind_group, 64);
        assert_eq!(limits.max_dynamic_uniform_buffers_per_pipeline_layout, 8);
        assert_eq!(limits.max_dynamic_storage_buffers_per_pipeline_layout, 0);
        assert_eq!(limits.max_sampled_textures_per_shader_stage, 16);
        assert_eq!(limits.max_samplers_per_shader_stage, 8);
        assert_eq!(limits.max_storage_buffers_per_shader_stage, 0);
        assert_eq!(limits.max_storage_textures_per_shader_stage, 0);
        assert_eq!(limits.max_uniform_buffers_per_shader_stage, 7);
        assert_eq!(limits.max_binding_array_elements_per_shader_stage, 0);
        assert_eq!(
            limits.max_binding_array_acceleration_structure_elements_per_shader_stage,
            0
        );
        assert_eq!(
            limits.max_binding_array_sampler_elements_per_shader_stage,
            0
        );
        assert_eq!(limits.max_uniform_buffer_binding_size, 16384);
        assert_eq!(limits.max_storage_buffer_binding_size, 0);
        assert_eq!(limits.max_vertex_buffers, 8);
        assert_eq!(limits.max_buffer_size, 268435456);
        assert_eq!(limits.max_vertex_attributes, 16);
        assert_eq!(limits.max_vertex_buffer_array_stride, 2048);
        assert_eq!(limits.max_inter_stage_shader_variables, 15);
        assert_eq!(limits.min_uniform_buffer_offset_alignment, 256);
        assert_eq!(limits.min_storage_buffer_offset_alignment, 256);
        assert_eq!(limits.max_color_attachments, 4);
        assert_eq!(limits.max_color_attachment_bytes_per_sample, 32);
        assert_eq!(limits.max_compute_workgroup_storage_size, 0);
        assert_eq!(limits.max_compute_invocations_per_workgroup, 0);
        assert_eq!(limits.max_compute_workgroup_size_x, 0);
        assert_eq!(limits.max_compute_workgroup_size_y, 0);
        assert_eq!(limits.max_compute_workgroup_size_z, 0);
        assert_eq!(limits.max_compute_workgroups_per_dimension, 0);
        assert_eq!(limits.max_immediate_size, 0);
        assert_eq!(limits.max_non_sampler_bindings, 1_000_000);
        assert_eq!(limits.max_task_workgroup_total_count, 0);
        assert_eq!(limits.max_task_workgroups_per_dimension, 0);
        assert_eq!(limits.max_mesh_workgroup_total_count, 0);
        assert_eq!(limits.max_mesh_workgroups_per_dimension, 0);
        assert_eq!(limits.max_task_invocations_per_workgroup, 0);
        assert_eq!(limits.max_task_invocations_per_dimension, 0);
        assert_eq!(limits.max_mesh_invocations_per_workgroup, 0);
        assert_eq!(limits.max_mesh_invocations_per_dimension, 0);
        assert_eq!(limits.max_task_payload_size, 0);
        assert_eq!(limits.max_mesh_output_vertices, 0);
        assert_eq!(limits.max_mesh_output_primitives, 0);
        assert_eq!(limits.max_mesh_output_layers, 0);
        assert_eq!(limits.max_mesh_multiview_view_count, 0);
        assert_eq!(limits.max_blas_primitive_count, 0);
        assert_eq!(limits.max_blas_geometry_count, 0);
        assert_eq!(limits.max_tlas_instance_count, 0);
        assert_eq!(limits.max_acceleration_structures_per_shader_stage, 0);
        assert_eq!(
            limits.max_buffers_and_acceleration_structures_per_shader_stage,
            12
        );
        assert_eq!(limits.max_multiview_view_count, 0);
        assert_eq!(limits.max_ray_dispatch_count, 0);
        assert_eq!(limits.max_ray_recursion_depth, 0);
    }
}

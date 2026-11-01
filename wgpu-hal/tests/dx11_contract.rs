//! Gate 0 capability contract for the experimental Direct3D 11 backend.
//!
//! This test is the executable form of the Gate 0 snapshot in
//! `dx11-gate0-coordination.md` and sections 6-9 of the Cinnabar DX11
//! capability contract. It enumerates every DXGI adapter, snapshots the
//! advertised features, downlevel flags, shader model, limits, per-format
//! capabilities and surface capabilities, and then runs positive and negative
//! HAL smoke checks against the same adapter.
//!
//! The test is deliberately RED against the current backend pin: every
//! over-announcement and every `unimplemented!()` stub is reported as a
//! contract violation instead of being silently tolerated. Assertions must not
//! be weakened to make the run green.
//!
//! Run on Windows CI with:
//!
//! ```text
//! cargo test -p wgpu-hal --features dx11 --test dx11_contract
//! ```

#![cfg(all(target_os = "windows", feature = "dx11"))]

use std::borrow::Cow;
use std::num::NonZeroIsize;
use std::panic::{catch_unwind, AssertUnwindSafe};

use raw_window_handle::{
    RawDisplayHandle, RawWindowHandle, Win32WindowHandle, WindowsDisplayHandle,
};
use wgpu_hal::{
    Adapter as _, CommandEncoder as _, Device as _, Instance as _, Surface as _,
    TextureFormatCapabilities as Tfc,
};
use wgpu_types as wgt;
use windows::core::{w, PCWSTR};
use windows::Win32::{
    Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM},
    Graphics::Gdi::HBRUSH,
    System::LibraryLoader::GetModuleHandleW,
    UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassW, UnregisterClassW,
        CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, HCURSOR, HICON, WINDOW_EX_STYLE, WNDCLASSW,
        WS_OVERLAPPEDWINDOW,
    },
};

const WINDOW_WIDTH: u32 = 320;
const WINDOW_HEIGHT: u32 = 240;
const UNIFORM_SIZE: u64 = 32;
const SMOKE_TEXTURE_SIZE: u32 = 8;

const SHADER: &str = r#"
struct Globals {
    color: vec4<f32>,
    scale: f32,
}

@group(0) @binding(0) var<uniform> globals: Globals;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
}

@vertex
fn vs_main(@location(0) position: vec2<f32>) -> VertexOutput {
    var output: VertexOutput;
    output.position = vec4<f32>(position * globals.scale, 0.0, 1.0);
    return output;
}

@fragment
fn fs_main() -> @location(0) vec4<f32> {
    return globals.color;
}
"#;

/// Every `wgpu_types::TextureFormat` variant, used to prove that no format
/// (not only the allowlist) advertises multisample or storage capabilities.
const ALL_FORMATS: &[wgt::TextureFormat] = &[
    wgt::TextureFormat::R8Unorm,
    wgt::TextureFormat::R8Snorm,
    wgt::TextureFormat::R8Uint,
    wgt::TextureFormat::R8Sint,
    wgt::TextureFormat::R16Uint,
    wgt::TextureFormat::R16Sint,
    wgt::TextureFormat::R16Unorm,
    wgt::TextureFormat::R16Snorm,
    wgt::TextureFormat::R16Float,
    wgt::TextureFormat::Rg8Unorm,
    wgt::TextureFormat::Rg8Snorm,
    wgt::TextureFormat::Rg8Uint,
    wgt::TextureFormat::Rg8Sint,
    wgt::TextureFormat::R32Uint,
    wgt::TextureFormat::R32Sint,
    wgt::TextureFormat::R32Float,
    wgt::TextureFormat::Rg16Uint,
    wgt::TextureFormat::Rg16Sint,
    wgt::TextureFormat::Rg16Unorm,
    wgt::TextureFormat::Rg16Snorm,
    wgt::TextureFormat::Rg16Float,
    wgt::TextureFormat::Rgba8Unorm,
    wgt::TextureFormat::Rgba8UnormSrgb,
    wgt::TextureFormat::Rgba8Snorm,
    wgt::TextureFormat::Rgba8Uint,
    wgt::TextureFormat::Rgba8Sint,
    wgt::TextureFormat::Bgra8Unorm,
    wgt::TextureFormat::Bgra8UnormSrgb,
    wgt::TextureFormat::Rgb9e5Ufloat,
    wgt::TextureFormat::Rgb10a2Uint,
    wgt::TextureFormat::Rgb10a2Unorm,
    wgt::TextureFormat::Rg11b10Ufloat,
    wgt::TextureFormat::R64Uint,
    wgt::TextureFormat::Rg32Uint,
    wgt::TextureFormat::Rg32Sint,
    wgt::TextureFormat::Rg32Float,
    wgt::TextureFormat::Rgba16Uint,
    wgt::TextureFormat::Rgba16Sint,
    wgt::TextureFormat::Rgba16Unorm,
    wgt::TextureFormat::Rgba16Snorm,
    wgt::TextureFormat::Rgba16Float,
    wgt::TextureFormat::Rgba32Uint,
    wgt::TextureFormat::Rgba32Sint,
    wgt::TextureFormat::Rgba32Float,
    wgt::TextureFormat::Stencil8,
    wgt::TextureFormat::Depth16Unorm,
    wgt::TextureFormat::Depth24Plus,
    wgt::TextureFormat::Depth24PlusStencil8,
    wgt::TextureFormat::Depth32Float,
    wgt::TextureFormat::Depth32FloatStencil8,
    wgt::TextureFormat::NV12,
    wgt::TextureFormat::P010,
    wgt::TextureFormat::Bc1RgbaUnorm,
    wgt::TextureFormat::Bc1RgbaUnormSrgb,
    wgt::TextureFormat::Bc2RgbaUnorm,
    wgt::TextureFormat::Bc2RgbaUnormSrgb,
    wgt::TextureFormat::Bc3RgbaUnorm,
    wgt::TextureFormat::Bc3RgbaUnormSrgb,
    wgt::TextureFormat::Bc4RUnorm,
    wgt::TextureFormat::Bc4RSnorm,
    wgt::TextureFormat::Bc5RgUnorm,
    wgt::TextureFormat::Bc5RgSnorm,
    wgt::TextureFormat::Bc6hRgbUfloat,
    wgt::TextureFormat::Bc6hRgbFloat,
    wgt::TextureFormat::Bc7RgbaUnorm,
    wgt::TextureFormat::Bc7RgbaUnormSrgb,
    wgt::TextureFormat::Etc2Rgb8Unorm,
    wgt::TextureFormat::Etc2Rgb8UnormSrgb,
    wgt::TextureFormat::Etc2Rgb8A1Unorm,
    wgt::TextureFormat::Etc2Rgb8A1UnormSrgb,
    wgt::TextureFormat::Etc2Rgba8Unorm,
    wgt::TextureFormat::Etc2Rgba8UnormSrgb,
    wgt::TextureFormat::EacR11Unorm,
    wgt::TextureFormat::EacR11Snorm,
    wgt::TextureFormat::EacRg11Unorm,
    wgt::TextureFormat::EacRg11Snorm,
    wgt::TextureFormat::Astc {
        block: wgt::AstcBlock::B4x4,
        channel: wgt::AstcChannel::Unorm,
    },
];

/// Contract §6.1: the mapped format allowlist.
const ALLOWLIST_FORMATS: &[wgt::TextureFormat] = &[
    // 8 bit
    wgt::TextureFormat::R8Unorm,
    wgt::TextureFormat::R8Snorm,
    wgt::TextureFormat::R8Uint,
    wgt::TextureFormat::R8Sint,
    wgt::TextureFormat::Rg8Unorm,
    wgt::TextureFormat::Rg8Snorm,
    wgt::TextureFormat::Rg8Uint,
    wgt::TextureFormat::Rg8Sint,
    wgt::TextureFormat::Rgba8Unorm,
    wgt::TextureFormat::Rgba8UnormSrgb,
    wgt::TextureFormat::Rgba8Snorm,
    wgt::TextureFormat::Rgba8Uint,
    wgt::TextureFormat::Rgba8Sint,
    wgt::TextureFormat::Bgra8Unorm,
    wgt::TextureFormat::Bgra8UnormSrgb,
    // 16 bit
    wgt::TextureFormat::R16Uint,
    wgt::TextureFormat::R16Sint,
    wgt::TextureFormat::R16Unorm,
    wgt::TextureFormat::R16Snorm,
    wgt::TextureFormat::R16Float,
    wgt::TextureFormat::Rg16Uint,
    wgt::TextureFormat::Rg16Sint,
    wgt::TextureFormat::Rg16Float,
    wgt::TextureFormat::Rgba16Float,
    wgt::TextureFormat::Rgba16Unorm,
    wgt::TextureFormat::Rgba16Snorm,
    wgt::TextureFormat::Rgba16Uint,
    wgt::TextureFormat::Rgba16Sint,
    // 32 bit
    wgt::TextureFormat::R32Uint,
    wgt::TextureFormat::R32Sint,
    wgt::TextureFormat::R32Float,
    wgt::TextureFormat::Rgba32Float,
    wgt::TextureFormat::Rgba32Uint,
    wgt::TextureFormat::Rgba32Sint,
    // 10/11 bit
    wgt::TextureFormat::Rgb10a2Unorm,
    wgt::TextureFormat::Rgb10a2Uint,
    wgt::TextureFormat::Rg11b10Ufloat,
    // depth / stencil
    wgt::TextureFormat::Depth16Unorm,
    wgt::TextureFormat::Depth24Plus,
    wgt::TextureFormat::Depth24PlusStencil8,
    wgt::TextureFormat::Depth32Float,
    wgt::TextureFormat::Depth32FloatStencil8,
];

const BC_FORMATS: [wgt::TextureFormat; 14] = [
    wgt::TextureFormat::Bc1RgbaUnorm,
    wgt::TextureFormat::Bc1RgbaUnormSrgb,
    wgt::TextureFormat::Bc2RgbaUnorm,
    wgt::TextureFormat::Bc2RgbaUnormSrgb,
    wgt::TextureFormat::Bc3RgbaUnorm,
    wgt::TextureFormat::Bc3RgbaUnormSrgb,
    wgt::TextureFormat::Bc4RUnorm,
    wgt::TextureFormat::Bc4RSnorm,
    wgt::TextureFormat::Bc5RgUnorm,
    wgt::TextureFormat::Bc5RgSnorm,
    wgt::TextureFormat::Bc6hRgbUfloat,
    wgt::TextureFormat::Bc6hRgbFloat,
    wgt::TextureFormat::Bc7RgbaUnorm,
    wgt::TextureFormat::Bc7RgbaUnormSrgb,
];

const ETC2_FORMATS: [wgt::TextureFormat; 10] = [
    wgt::TextureFormat::Etc2Rgb8Unorm,
    wgt::TextureFormat::Etc2Rgb8UnormSrgb,
    wgt::TextureFormat::Etc2Rgb8A1Unorm,
    wgt::TextureFormat::Etc2Rgb8A1UnormSrgb,
    wgt::TextureFormat::Etc2Rgba8Unorm,
    wgt::TextureFormat::Etc2Rgba8UnormSrgb,
    wgt::TextureFormat::EacR11Unorm,
    wgt::TextureFormat::EacR11Snorm,
    wgt::TextureFormat::EacRg11Unorm,
    wgt::TextureFormat::EacRg11Snorm,
];

const ASTC_BLOCKS: [wgt::AstcBlock; 14] = [
    wgt::AstcBlock::B4x4,
    wgt::AstcBlock::B5x4,
    wgt::AstcBlock::B5x5,
    wgt::AstcBlock::B6x5,
    wgt::AstcBlock::B6x6,
    wgt::AstcBlock::B8x5,
    wgt::AstcBlock::B8x6,
    wgt::AstcBlock::B8x8,
    wgt::AstcBlock::B10x5,
    wgt::AstcBlock::B10x6,
    wgt::AstcBlock::B10x8,
    wgt::AstcBlock::B10x10,
    wgt::AstcBlock::B12x10,
    wgt::AstcBlock::B12x12,
];

const ASTC_CHANNELS: [wgt::AstcChannel; 3] = [
    wgt::AstcChannel::Unorm,
    wgt::AstcChannel::UnormSrgb,
    wgt::AstcChannel::Hdr,
];

const RGBA8_COLOR_FORMATS: [wgt::TextureFormat; 4] = [
    wgt::TextureFormat::Rgba8Unorm,
    wgt::TextureFormat::Rgba8UnormSrgb,
    wgt::TextureFormat::Bgra8Unorm,
    wgt::TextureFormat::Bgra8UnormSrgb,
];

const DEPTH_FORMATS: [wgt::TextureFormat; 5] = [
    wgt::TextureFormat::Depth16Unorm,
    wgt::TextureFormat::Depth24Plus,
    wgt::TextureFormat::Depth24PlusStencil8,
    wgt::TextureFormat::Depth32Float,
    wgt::TextureFormat::Depth32FloatStencil8,
];

#[test]
fn dx11_gate0_contract() {
    let mut failures: Vec<String> = Vec::new();
    if let Err(error) = run_contract(&mut failures) {
        failures.push(format!("setup/enumeration failed: {error}"));
    }

    if !failures.is_empty() {
        panic!(
            "DX11 Gate 0 contract: {} violation(s):\n{}",
            failures.len(),
            failures.join("\n")
        );
    }
}

fn run_contract(failures: &mut Vec<String>) -> Result<(), String> {
    let window = HiddenWindow::create()?;

    let instance_desc = wgpu_hal::InstanceDescriptor {
        name: "wgpu-hal-dx11-contract",
        flags: wgt::InstanceFlags::default(),
        memory_budget_thresholds: wgt::MemoryBudgetThresholds::default(),
        backend_options: wgt::BackendOptions::default(),
        telemetry: None,
        display: None,
    };

    let instance =
        unsafe { <wgpu_hal::dx11::Instance as wgpu_hal::Instance>::init(&instance_desc) }
            .map_err(|e| format!("D3D11 instance initialization failed: {e:?}"))?;

    let surface = unsafe {
        instance.create_surface(
            RawDisplayHandle::Windows(WindowsDisplayHandle::new()),
            RawWindowHandle::Win32(window.raw_window_handle()),
        )
    }
    .map_err(|e| format!("D3D11 surface creation failed: {e:?}"))?;

    let adapters = unsafe { instance.enumerate_adapters(Some(&surface)) };
    println!(
        "DX11 Gate 0 contract: enumerated {} DXGI adapter(s)",
        adapters.len()
    );
    if adapters.is_empty() {
        return Err("no DXGI/D3D11 adapters were found; the contract cannot be checked".to_owned());
    }

    for (index, exposed) in adapters.iter().enumerate() {
        let label = format!(
            "adapter[{index}] '{}' vendor=0x{:04x} device=0x{:04x}",
            exposed.info.name, exposed.info.vendor, exposed.info.device
        );
        println!("=== {label} ===");
        println!(
            "  feature level: {} ({:?})",
            exposed.adapter.feature_level_name(),
            exposed.adapter.feature_level()
        );
        println!("  device type: {:?}", exposed.info.device_type);
        println!(
            "  software/WARP: {}",
            matches!(exposed.info.device_type, wgt::DeviceType::Cpu)
        );

        check_features_and_downlevel(&label, exposed, failures);
        check_limits(&label, exposed, failures);
        check_format_capabilities(&label, &exposed.adapter, failures);
        check_surface_capabilities(&label, &exposed.adapter, &surface, failures);

        let opened = unsafe {
            exposed.adapter.open(
                exposed.features,
                &exposed.capabilities.limits,
                &wgt::MemoryHints::default(),
            )
        };
        let wgpu_hal::OpenDevice { device, queue } = match opened {
            Ok(opened) => opened,
            Err(e) => {
                failures.push(format!("{label}: Adapter::open failed with {e:?}"));
                continue;
            }
        };

        if let Err(error) = positive_smoke(&device) {
            failures.push(format!("{label}: positive HAL smoke failed: {error}"));
        }
        if exposed.adapter.feature_level_name() == "11_0" {
            if let Err(error) = positive_storage_smoke(&device, &queue) {
                failures.push(format!("{label}: positive storage smoke failed: {error}"));
            }
        } else {
            println!(
                "  note: feature level {} does not advertise read-only storage; \
                 positive storage smoke skipped",
                exposed.adapter.feature_level_name()
            );
        }

        negative_tests(&label, &device, &queue, failures);
        surface_negative_tests(&label, &surface, &device, failures);

        drop(queue);
        drop(device);
    }

    Ok(())
}

fn check_features_and_downlevel(
    label: &str,
    exposed: &wgpu_hal::ExposedAdapter<wgpu_hal::dx11::Api>,
    failures: &mut Vec<String>,
) {
    let mut expected_features = wgt::Features::DEPTH_CLIP_CONTROL;
    expected_features |= wgt::Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES;
    let announced: Vec<&str> = exposed
        .features
        .iter_names()
        .map(|(name, _)| name)
        .collect();
    println!("  features: {announced:?}");
    if exposed.features != expected_features {
        for (name, bit) in exposed.features.iter_names() {
            if !expected_features.contains(bit) {
                failures.push(format!("{label}: extra feature announced: {name}"));
            }
        }
        for (name, bit) in expected_features.iter_names() {
            if !exposed.features.contains(bit) {
                failures.push(format!("{label}: required feature not announced: {name}"));
            }
        }
    }

    let downlevel = &exposed.capabilities.downlevel;
    let flags = downlevel.flags;
    let feature_level = exposed.adapter.feature_level_name();
    let mut expected_flags = wgt::DownlevelFlags::BASE_VERTEX
        | wgt::DownlevelFlags::NON_POWER_OF_TWO_MIPMAPPED_TEXTURES
        | wgt::DownlevelFlags::CUBE_ARRAY_TEXTURES
        | wgt::DownlevelFlags::COMPARISON_SAMPLERS
        | wgt::DownlevelFlags::INDEPENDENT_BLEND
        | wgt::DownlevelFlags::FULL_DRAW_INDEX_UINT32
        | wgt::DownlevelFlags::DEPTH_BIAS_CLAMP;
    // Read-only storage buffers are exposed through raw SRVs at feature level
    // 11_0. Feature level 10_1 does not advertise them: D3D11 only guarantees
    // optional `ByteAddressBuffer` support there.
    if feature_level == "11_0" {
        expected_flags |=
            wgt::DownlevelFlags::VERTEX_STORAGE | wgt::DownlevelFlags::FRAGMENT_STORAGE;
    }
    let announced_flags: Vec<&str> = flags.iter_names().map(|(name, _)| name).collect();
    println!("  downlevel flags: {announced_flags:?}");
    for (name, bit) in flags.iter_names() {
        if !expected_flags.contains(bit) {
            failures.push(format!("{label}: extra downlevel flag: {name}"));
        }
    }
    for (name, bit) in expected_flags.iter_names() {
        if !flags.contains(bit) {
            failures.push(format!("{label}: required downlevel flag missing: {name}"));
        }
    }
    let unnamed_extra = flags.bits() & !expected_flags.bits();
    if unnamed_extra != 0 {
        failures.push(format!(
            "{label}: extra unnamed downlevel flag bits 0x{unnamed_extra:x}"
        ));
    }

    // Writable storage, compute and non-16-byte-aligned buffer bindings are
    // still not implemented. Advertising any of them would let `wgpu-core`
    // build pipelines the backend cannot bind.
    for (name, flag) in [
        (
            "FRAGMENT_WRITABLE_STORAGE",
            wgt::DownlevelFlags::FRAGMENT_WRITABLE_STORAGE,
        ),
        ("COMPUTE_SHADERS", wgt::DownlevelFlags::COMPUTE_SHADERS),
        (
            "BUFFER_BINDINGS_NOT_16_BYTE_ALIGNED",
            wgt::DownlevelFlags::BUFFER_BINDINGS_NOT_16_BYTE_ALIGNED,
        ),
    ] {
        if flags.contains(flag) {
            failures.push(format!(
                "{label}: {name} must not be advertised by the DX11 backend"
            ));
        }
    }

    if downlevel.is_webgpu_compliant() {
        failures.push(format!("{label}: is_webgpu_compliant() must be false"));
    }

    let expected_shader_model = match feature_level {
        "11_0" => Some(wgt::ShaderModel::Sm5),
        "10_1" => Some(wgt::ShaderModel::Sm4),
        _ => None,
    };
    match expected_shader_model {
        Some(expected) if downlevel.shader_model == expected => {}
        Some(expected) => failures.push(format!(
            "{label}: shader_model is {:?} for feature level {} but Gate 0 requires {:?}",
            downlevel.shader_model, feature_level, expected
        )),
        None => failures.push(format!(
            "{label}: unsupported feature level {} (Gate 0 covers 11_0 and 10_1 only)",
            feature_level
        )),
    }
}

fn check_limits(
    label: &str,
    exposed: &wgpu_hal::ExposedAdapter<wgpu_hal::dx11::Api>,
    failures: &mut Vec<String>,
) {
    let limits = &exposed.capabilities.limits;
    let feature_level = exposed.adapter.feature_level_name();
    let max_texture_dimension_2d = if feature_level == "10_1" { 8192 } else { 16384 };
    // Read-only storage buffers are advertised only at feature level 11_0.
    let supports_storage = feature_level == "11_0";

    // Defined inside the function so the local `limits`/`label`/`failures`
    // bindings are visible at the macro definition site (macro_rules is
    // hygienic for local variables).
    macro_rules! check_limit {
        ($field:ident, $expected:expr) => {{
            let actual = limits.$field;
            let expected = $expected;
            if actual != expected {
                failures.push(format!(
                    "{}: limit `{}` is {} but Gate 0 requires {}",
                    label,
                    stringify!($field),
                    actual,
                    expected
                ));
            }
        }};
    }

    println!(
        "  limits: max_texture_dimension_2d = {} (feature level {}); \
         alignments are part of the stability contract",
        limits.max_texture_dimension_2d, feature_level
    );
    let alignments = &exposed.capabilities.alignments;
    println!(
        "  alignments: buffer_copy_offset={:?} buffer_copy_pitch={:?} \
         uniform_bounds_check_alignment={:?}",
        alignments.buffer_copy_offset,
        alignments.buffer_copy_pitch,
        alignments.uniform_bounds_check_alignment
    );
    for (name, value) in [
        ("buffer_copy_offset", alignments.buffer_copy_offset),
        ("buffer_copy_pitch", alignments.buffer_copy_pitch),
        (
            "uniform_bounds_check_alignment",
            alignments.uniform_bounds_check_alignment,
        ),
    ] {
        if value.get() != 16 {
            failures.push(format!(
                "{label}: alignment `{name}` is {} but Gate 0 requires 16",
                value.get()
            ));
        }
    }

    check_limit!(max_texture_dimension_1d, 0);
    check_limit!(max_texture_dimension_2d, max_texture_dimension_2d);
    check_limit!(max_texture_dimension_3d, 1);
    check_limit!(max_texture_array_layers, 2048);
    check_limit!(max_bind_groups, 4);
    check_limit!(max_bind_groups_plus_vertex_buffers, 24);
    check_limit!(max_bindings_per_bind_group, 64);
    check_limit!(max_dynamic_uniform_buffers_per_pipeline_layout, 8);
    check_limit!(max_dynamic_storage_buffers_per_pipeline_layout, 0);
    check_limit!(max_sampled_textures_per_shader_stage, 16);
    check_limit!(max_samplers_per_shader_stage, 8);
    check_limit!(
        max_storage_buffers_per_shader_stage,
        if supports_storage { 4 } else { 0 }
    );
    check_limit!(max_storage_textures_per_shader_stage, 0);
    check_limit!(max_uniform_buffers_per_shader_stage, 7);
    check_limit!(max_binding_array_elements_per_shader_stage, 0);
    check_limit!(
        max_binding_array_acceleration_structure_elements_per_shader_stage,
        0
    );
    check_limit!(max_binding_array_sampler_elements_per_shader_stage, 0);
    check_limit!(max_uniform_buffer_binding_size, 16384);
    check_limit!(
        max_storage_buffer_binding_size,
        if supports_storage { 128 << 20 } else { 0 }
    );
    check_limit!(max_vertex_buffers, 8);
    check_limit!(max_buffer_size, 268435456);
    check_limit!(max_vertex_attributes, 16);
    check_limit!(max_vertex_buffer_array_stride, 2048);
    check_limit!(max_inter_stage_shader_variables, 15);
    check_limit!(min_uniform_buffer_offset_alignment, 256);
    check_limit!(min_storage_buffer_offset_alignment, 256);
    check_limit!(max_color_attachments, 4);
    check_limit!(max_color_attachment_bytes_per_sample, 32);
    check_limit!(max_compute_workgroup_storage_size, 0);
    check_limit!(max_compute_invocations_per_workgroup, 0);
    check_limit!(max_compute_workgroup_size_x, 0);
    check_limit!(max_compute_workgroup_size_y, 0);
    check_limit!(max_compute_workgroup_size_z, 0);
    check_limit!(max_compute_workgroups_per_dimension, 0);
    check_limit!(max_immediate_size, 0);
    check_limit!(max_non_sampler_bindings, 1000000);
    check_limit!(max_task_workgroup_total_count, 0);
    check_limit!(max_task_workgroups_per_dimension, 0);
    check_limit!(max_mesh_workgroup_total_count, 0);
    check_limit!(max_mesh_workgroups_per_dimension, 0);
    check_limit!(max_task_invocations_per_workgroup, 0);
    check_limit!(max_task_invocations_per_dimension, 0);
    check_limit!(max_mesh_invocations_per_workgroup, 0);
    check_limit!(max_mesh_invocations_per_dimension, 0);
    check_limit!(max_task_payload_size, 0);
    check_limit!(max_mesh_output_vertices, 0);
    check_limit!(max_mesh_output_primitives, 0);
    check_limit!(max_mesh_output_layers, 0);
    check_limit!(max_mesh_multiview_view_count, 0);
    check_limit!(max_blas_primitive_count, 0);
    check_limit!(max_blas_geometry_count, 0);
    check_limit!(max_tlas_instance_count, 0);
    check_limit!(max_acceleration_structures_per_shader_stage, 0);
    check_limit!(max_buffers_and_acceleration_structures_per_shader_stage, 12);
    check_limit!(max_multiview_view_count, 0);
    check_limit!(max_ray_dispatch_count, 0);
    check_limit!(max_ray_recursion_depth, 0);
}

fn check_format_capabilities(
    label: &str,
    adapter: &wgpu_hal::dx11::Adapter,
    failures: &mut Vec<String>,
) {
    let allowed = Tfc::SAMPLED
        | Tfc::SAMPLED_LINEAR
        | Tfc::COLOR_ATTACHMENT
        | Tfc::COLOR_ATTACHMENT_BLEND
        | Tfc::DEPTH_STENCIL_ATTACHMENT
        | Tfc::COPY_SRC
        | Tfc::COPY_DST;
    let multisample = Tfc::MULTISAMPLE_X2
        | Tfc::MULTISAMPLE_X4
        | Tfc::MULTISAMPLE_X8
        | Tfc::MULTISAMPLE_X16
        | Tfc::MULTISAMPLE_RESOLVE;
    let storage = Tfc::STORAGE_READ_ONLY
        | Tfc::STORAGE_WRITE_ONLY
        | Tfc::STORAGE_READ_WRITE
        | Tfc::STORAGE_ATOMIC;

    let format_caps =
        |format: wgt::TextureFormat| unsafe { adapter.texture_format_capabilities(format) };

    println!("  format capability matrix (contract §6.1 allowlist):");
    for &format in ALLOWLIST_FORMATS {
        let caps = format_caps(format);
        println!("    {format:?} => {caps:?}");
        let unexpected = caps & !allowed & !multisample & !storage;
        if !unexpected.is_empty() {
            failures.push(format!(
                "{label}: {format:?} advertises {unexpected:?}, outside the Gate 0 allowlist \
                 (multisample and storage bits are reported separately)"
            ));
        }
    }

    // Every format, including the ones outside the allowlist, must be free of
    // multisample and storage capabilities.
    for &format in ALL_FORMATS {
        let caps = format_caps(format);
        let advertised_multisample = caps & multisample;
        if !advertised_multisample.is_empty() {
            failures.push(format!(
                "{label}: {format:?} advertises {advertised_multisample:?}"
            ));
        }
        let advertised_storage = caps & storage;
        if !advertised_storage.is_empty() {
            failures.push(format!(
                "{label}: {format:?} advertises storage capability {advertised_storage:?}"
            ));
        }
    }

    // BC, ETC2/EAC, ASTC, Stencil8 and Rgb9e5Ufloat are not mapped and must
    // report empty capabilities.
    for format in BC_FORMATS {
        let caps = format_caps(format);
        if !caps.is_empty() {
            failures.push(format!(
                "{label}: {format:?} must have empty capabilities but advertises {caps:?}"
            ));
        }
    }
    for format in ETC2_FORMATS {
        let caps = format_caps(format);
        if !caps.is_empty() {
            failures.push(format!(
                "{label}: {format:?} must have empty capabilities but advertises {caps:?}"
            ));
        }
    }
    for block in ASTC_BLOCKS {
        for channel in ASTC_CHANNELS {
            let format = wgt::TextureFormat::Astc { block, channel };
            let caps = format_caps(format);
            if !caps.is_empty() {
                failures.push(format!(
                    "{label}: {format:?} must have empty capabilities but advertises {caps:?}"
                ));
            }
        }
    }
    for format in [
        wgt::TextureFormat::Stencil8,
        wgt::TextureFormat::Rgb9e5Ufloat,
    ] {
        let caps = format_caps(format);
        if !caps.is_empty() {
            failures.push(format!(
                "{label}: {format:?} must have empty capabilities but advertises {caps:?}"
            ));
        }
    }

    // Renderable 8-bit color formats must be sampled and copyable.
    let required_rgba8 = Tfc::SAMPLED | Tfc::COPY_SRC | Tfc::COPY_DST | Tfc::COLOR_ATTACHMENT;
    for format in RGBA8_COLOR_FORMATS {
        let caps = format_caps(format);
        if !caps.contains(required_rgba8) {
            failures.push(format!(
                "{label}: {format:?} capabilities {caps:?} do not contain {required_rgba8:?}"
            ));
        }
    }

    // Depth formats must be attachable as depth/stencil and must not be color
    // attachments.
    for format in DEPTH_FORMATS {
        let caps = format_caps(format);
        if !caps.contains(Tfc::DEPTH_STENCIL_ATTACHMENT) {
            failures.push(format!(
                "{label}: {format:?} capabilities {caps:?} lack DEPTH_STENCIL_ATTACHMENT"
            ));
        }
        if caps.contains(Tfc::COLOR_ATTACHMENT) {
            failures.push(format!(
                "{label}: {format:?} capabilities {caps:?} must not contain COLOR_ATTACHMENT"
            ));
        }
    }
}

fn check_surface_capabilities(
    label: &str,
    adapter: &wgpu_hal::dx11::Adapter,
    surface: &wgpu_hal::dx11::Surface,
    failures: &mut Vec<String>,
) {
    let caps = match unsafe { adapter.surface_capabilities(surface) } {
        Some(caps) => caps,
        None => {
            failures.push(format!(
                "{label}: surface_capabilities returned None for the hidden Win32 surface"
            ));
            return;
        }
    };

    let formats: Vec<wgt::TextureFormat> = caps.formats.iter().map(|entry| entry.format).collect();
    println!("  surface formats: {formats:?}");
    println!("  surface present modes: {:?}", caps.present_modes);
    println!(
        "  surface composite alpha modes: {:?}",
        caps.composite_alpha_modes
    );
    println!(
        "  surface maximum_frame_latency: {:?} (LIMITATION: Surface::configure does not \
         enforce this range; it is recorded, not asserted)",
        caps.maximum_frame_latency
    );

    let expected_formats = [
        wgt::TextureFormat::Bgra8Unorm,
        wgt::TextureFormat::Bgra8UnormSrgb,
    ];
    if !is_same_set(&formats, &expected_formats) {
        failures.push(format!(
            "{label}: surface formats {formats:?} are not exactly {expected_formats:?}"
        ));
    }

    for entry in &caps.formats {
        if entry.color_spaces != wgt::SurfaceColorSpaces::SRGB {
            failures.push(format!(
                "{label}: surface format {:?} advertises color spaces {:?}, expected SRGB only",
                entry.format, entry.color_spaces
            ));
        }
    }

    if caps.usage != wgt::TextureUses::COLOR_TARGET {
        failures.push(format!(
            "{label}: surface usage is {:?}, expected COLOR_TARGET only",
            caps.usage
        ));
    }

    let expected_present = [wgt::PresentMode::Fifo, wgt::PresentMode::Immediate];
    if !is_same_set(&caps.present_modes, &expected_present) {
        failures.push(format!(
            "{label}: surface present modes {:?} are not exactly {:?}",
            caps.present_modes, expected_present
        ));
    }

    if caps.composite_alpha_modes != vec![wgt::CompositeAlphaMode::Opaque] {
        failures.push(format!(
            "{label}: surface composite alpha modes {:?} are not exactly [Opaque]",
            caps.composite_alpha_modes
        ));
    }
}

fn positive_smoke(device: &wgpu_hal::dx11::Device) -> Result<(), String> {
    let vertex_buffer = unsafe {
        device.create_buffer(&wgpu_hal::BufferDescriptor {
            label: Some("contract vertex buffer"),
            size: 32,
            usage: wgt::BufferUses::VERTEX | wgt::BufferUses::COPY_DST,
            memory_flags: wgpu_hal::MemoryFlags::empty(),
        })
    }
    .map_err(|e| format!("create vertex buffer: {e:?}"))?;

    let index_buffer = unsafe {
        device.create_buffer(&wgpu_hal::BufferDescriptor {
            label: Some("contract index buffer"),
            size: 16,
            usage: wgt::BufferUses::INDEX | wgt::BufferUses::COPY_DST,
            memory_flags: wgpu_hal::MemoryFlags::empty(),
        })
    }
    .map_err(|e| format!("create index buffer: {e:?}"))?;

    let uniform_buffer = unsafe {
        device.create_buffer(&wgpu_hal::BufferDescriptor {
            label: Some("contract uniform buffer"),
            size: UNIFORM_SIZE,
            usage: wgt::BufferUses::UNIFORM | wgt::BufferUses::COPY_DST,
            memory_flags: wgpu_hal::MemoryFlags::empty(),
        })
    }
    .map_err(|e| format!("create uniform buffer: {e:?}"))?;

    let copy_buffer = unsafe {
        device.create_buffer(&wgpu_hal::BufferDescriptor {
            label: Some("contract copy buffer"),
            size: 16,
            usage: wgt::BufferUses::COPY_SRC | wgt::BufferUses::COPY_DST,
            memory_flags: wgpu_hal::MemoryFlags::empty(),
        })
    }
    .map_err(|e| format!("create copy buffer: {e:?}"))?;

    let texture = unsafe {
        device.create_texture(&texture_desc(
            "contract 2D texture",
            wgt::TextureFormat::Rgba8Unorm,
            wgt::TextureDimension::D2,
            extent(SMOKE_TEXTURE_SIZE, SMOKE_TEXTURE_SIZE, 1),
            1,
            wgt::TextureUses::RESOURCE
                | wgt::TextureUses::COLOR_TARGET
                | wgt::TextureUses::COPY_SRC
                | wgt::TextureUses::COPY_DST,
        ))
    }
    .map_err(|e| format!("create 2D texture: {e:?}"))?;

    let texture_view = unsafe {
        device.create_texture_view(
            &texture,
            &wgpu_hal::TextureViewDescriptor {
                label: Some("contract 2D texture view"),
                format: wgt::TextureFormat::Rgba8Unorm,
                dimension: wgt::TextureViewDimension::D2,
                usage: wgt::TextureUses::RESOURCE | wgt::TextureUses::COLOR_TARGET,
                range: full_view_range(),
            },
        )
    }
    .map_err(|e| format!("create 2D texture view: {e:?}"))?;

    let sampler = unsafe {
        device.create_sampler(&wgpu_hal::SamplerDescriptor {
            label: Some("contract sampler"),
            address_modes: [
                wgt::AddressMode::ClampToEdge,
                wgt::AddressMode::ClampToEdge,
                wgt::AddressMode::ClampToEdge,
            ],
            mag_filter: wgt::FilterMode::Linear,
            min_filter: wgt::FilterMode::Linear,
            mipmap_filter: wgt::MipmapFilterMode::Nearest,
            lod_clamp: 0.0..32.0,
            compare: None,
            anisotropy_clamp: 1,
            border_color: None,
        })
    }
    .map_err(|e| format!("create sampler: {e:?}"))?;

    let layout_entries = [wgt::BindGroupLayoutEntry {
        binding: 0,
        visibility: wgt::ShaderStages::VERTEX_FRAGMENT,
        ty: wgt::BindingType::Buffer {
            ty: wgt::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: wgt::BufferSize::new(UNIFORM_SIZE),
        },
        count: None,
    }];
    let bind_group_layout = unsafe {
        device.create_bind_group_layout(&wgpu_hal::BindGroupLayoutDescriptor {
            label: Some("contract bind group layout"),
            flags: wgpu_hal::BindGroupLayoutFlags::empty(),
            entries: &layout_entries,
        })
    }
    .map_err(|e| format!("create bind group layout: {e:?}"))?;

    let pipeline_layout = unsafe {
        device.create_pipeline_layout(&wgpu_hal::PipelineLayoutDescriptor {
            label: Some("contract pipeline layout"),
            flags: wgpu_hal::PipelineLayoutFlags::empty(),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        })
    }
    .map_err(|e| format!("create pipeline layout: {e:?}"))?;

    let buffer_binding = wgpu_hal::BufferBinding::new_unchecked(
        &uniform_buffer,
        0,
        wgt::BufferSize::new(UNIFORM_SIZE),
    );
    let bind_entries = [wgpu_hal::BindGroupEntry {
        binding: 0,
        resource_index: 0,
        count: 1,
    }];
    let bind_group = unsafe {
        device.create_bind_group(&wgpu_hal::BindGroupDescriptor {
            label: Some("contract bind group"),
            layout: &bind_group_layout,
            buffers: &[buffer_binding],
            samplers: &[],
            textures: &[],
            entries: &bind_entries,
            acceleration_structures: &[],
            external_textures: &[],
        })
    }
    .map_err(|e| format!("create bind group: {e:?}"))?;

    let constants = naga::back::PipelineConstants::default();
    let shader_module = create_test_shader_module(device)?;

    let pipeline = unsafe {
        device.create_render_pipeline(&wgpu_hal::RenderPipelineDescriptor {
            label: Some("contract render pipeline"),
            layout: &pipeline_layout,
            vertex_processor: wgpu_hal::VertexProcessor::Standard {
                vertex_buffers: &[Some(wgpu_hal::VertexBufferLayout {
                    array_stride: 8,
                    step_mode: wgt::VertexStepMode::Vertex,
                    attributes: &[wgt::VertexAttribute {
                        format: wgt::VertexFormat::Float32x2,
                        offset: 0,
                        shader_location: 0,
                    }],
                })],
                vertex_stage: wgpu_hal::ProgrammableStage {
                    module: &shader_module,
                    entry_point: "vs_main",
                    constants: &constants,
                    zero_initialize_workgroup_memory: false,
                },
            },
            primitive: wgt::PrimitiveState {
                topology: wgt::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgt::MultisampleState::default(),
            fragment_stage: Some(wgpu_hal::ProgrammableStage {
                module: &shader_module,
                entry_point: "fs_main",
                constants: &constants,
                zero_initialize_workgroup_memory: false,
            }),
            color_targets: &[Some(wgt::ColorTargetState {
                format: wgt::TextureFormat::Rgba8Unorm,
                blend: None,
                write_mask: wgt::ColorWrites::ALL,
            })],
            multiview_mask: None,
            cache: None,
        })
    }
    .map_err(|e| format!("create render pipeline: {e:?}"))?;
    println!(
        "  positive smoke: render pipeline topology {:?}",
        pipeline.topology()
    );

    unsafe {
        device.destroy_render_pipeline(pipeline);
        device.destroy_shader_module(shader_module);
        device.destroy_bind_group(bind_group);
        device.destroy_pipeline_layout(pipeline_layout);
        device.destroy_bind_group_layout(bind_group_layout);
        device.destroy_sampler(sampler);
        device.destroy_texture_view(texture_view);
        device.destroy_texture(texture);
        device.destroy_buffer(copy_buffer);
        device.destroy_buffer(uniform_buffer);
        device.destroy_buffer(index_buffer);
        device.destroy_buffer(vertex_buffer);
    }

    println!("  positive smoke: buffers, texture, view, sampler, bind group, pipeline OK");
    Ok(())
}

/// Positive smoke for the read-only storage path: bind group layout, pipeline
/// layout, a storage buffer, a raw-SRV bind group and `set_bind_group`.
///
/// Only called for feature level 11_0, which is where the backend advertises
/// storage buffers. It does not draw; the end-to-end draw/read-back coverage
/// lives in `dx11_storage.rs`.
fn positive_storage_smoke(
    device: &wgpu_hal::dx11::Device,
    queue: &wgpu_hal::dx11::Queue,
) -> Result<(), String> {
    const STORAGE_SIZE: u64 = 512;
    const STORAGE_OFFSET: u64 = 256;
    const STORAGE_BINDING_SIZE: u64 = 16;

    let storage_buffer = unsafe {
        device.create_buffer(&wgpu_hal::BufferDescriptor {
            label: Some("contract storage buffer"),
            size: STORAGE_SIZE,
            usage: wgt::BufferUses::STORAGE_READ_ONLY
                | wgt::BufferUses::STORAGE_READ_WRITE
                | wgt::BufferUses::COPY_DST,
            memory_flags: wgpu_hal::MemoryFlags::empty(),
        })
    }
    .map_err(|e| format!("create storage buffer: {e:?}"))?;

    let layout_entries = [wgt::BindGroupLayoutEntry {
        binding: 0,
        visibility: wgt::ShaderStages::VERTEX_FRAGMENT,
        ty: wgt::BindingType::Buffer {
            ty: wgt::BufferBindingType::Storage { read_only: true },
            has_dynamic_offset: false,
            min_binding_size: wgt::BufferSize::new(STORAGE_BINDING_SIZE),
        },
        count: None,
    }];
    let bind_group_layout = unsafe {
        device.create_bind_group_layout(&wgpu_hal::BindGroupLayoutDescriptor {
            label: Some("contract storage bind group layout"),
            flags: wgpu_hal::BindGroupLayoutFlags::empty(),
            entries: &layout_entries,
        })
    }
    .map_err(|e| format!("create storage bind group layout: {e:?}"))?;

    let pipeline_layout = unsafe {
        device.create_pipeline_layout(&wgpu_hal::PipelineLayoutDescriptor {
            label: Some("contract storage pipeline layout"),
            flags: wgpu_hal::PipelineLayoutFlags::empty(),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        })
    }
    .map_err(|e| format!("create storage pipeline layout: {e:?}"))?;

    let buffer_binding = wgpu_hal::BufferBinding::new_unchecked(
        &storage_buffer,
        STORAGE_OFFSET,
        wgt::BufferSize::new(STORAGE_BINDING_SIZE),
    );
    let bind_entries = [wgpu_hal::BindGroupEntry {
        binding: 0,
        resource_index: 0,
        count: 1,
    }];
    let bind_group = unsafe {
        device.create_bind_group(&wgpu_hal::BindGroupDescriptor {
            label: Some("contract storage bind group"),
            layout: &bind_group_layout,
            buffers: &[buffer_binding],
            samplers: &[],
            textures: &[],
            entries: &bind_entries,
            acceleration_structures: &[],
            external_textures: &[],
        })
    }
    .map_err(|e| format!("create storage bind group: {e:?}"))?;

    pipeline_layout
        .validate_bind_group(0, &bind_group, &[], false)
        .map_err(|e| format!("validate_bind_group(storage): {e}"))?;

    let mut encoder = unsafe {
        device.create_command_encoder(&wgpu_hal::CommandEncoderDescriptor {
            label: Some("contract storage encoder"),
            queue,
        })
    }
    .map_err(|e| format!("create storage command encoder: {e:?}"))?;
    unsafe { encoder.begin_encoding(Some("contract storage")) }
        .map_err(|e| format!("begin_encoding(storage): {e:?}"))?;
    unsafe { encoder.set_bind_group(&pipeline_layout, 0, &bind_group, &[]) };
    unsafe { encoder.end_encoding() }.map_err(|e| format!("end_encoding(storage): {e:?}"))?;

    unsafe {
        device.destroy_bind_group(bind_group);
        device.destroy_pipeline_layout(pipeline_layout);
        device.destroy_bind_group_layout(bind_group_layout);
        device.destroy_buffer(storage_buffer);
    }

    println!(
        "  positive storage smoke: read-only layout, t0 binding, offset {STORAGE_OFFSET}, \
         size {STORAGE_BINDING_SIZE} OK"
    );
    Ok(())
}

fn negative_tests(
    label: &str,
    device: &wgpu_hal::dx11::Device,
    queue: &wgpu_hal::dx11::Queue,
    failures: &mut Vec<String>,
) {
    let shader_module = match create_test_shader_module(device) {
        Ok(module) => module,
        Err(error) => {
            failures.push(format!(
                "{label}: negative-test shader setup failed: {error}"
            ));
            return;
        }
    };
    let constants = naga::back::PipelineConstants::default();
    let zero_pipeline_layout = unsafe {
        device.create_pipeline_layout(&wgpu_hal::PipelineLayoutDescriptor {
            label: Some("contract zero-immediate layout"),
            flags: wgpu_hal::PipelineLayoutFlags::empty(),
            bind_group_layouts: &[],
            immediate_size: 0,
        })
    };
    let zero_pipeline_layout = match zero_pipeline_layout {
        Ok(layout) => layout,
        Err(e) => {
            failures.push(format!(
                "{label}: zero-immediate pipeline layout setup failed: {e:?}"
            ));
            return;
        }
    };

    // 1. Compressed BC textures are not mapped and must be rejected.
    for format in [
        wgt::TextureFormat::Bc1RgbaUnorm,
        wgt::TextureFormat::Bc7RgbaUnormSrgb,
    ] {
        let description = format!("{label}: create_texture({format:?})");
        let call = || unsafe {
            let result = device.create_texture(&texture_desc(
                "contract BC texture",
                format,
                wgt::TextureDimension::D2,
                extent(4, 4, 1),
                1,
                wgt::TextureUses::RESOURCE,
            ));
            result.map(|_| ())
        };
        expect_err(failures, &description, call);
    }

    // 2. Multisampled textures are not implemented.
    let description = format!("{label}: create_texture(sample_count = 4)");
    let call = || unsafe {
        let result = device.create_texture(&texture_desc(
            "contract MSAA texture",
            wgt::TextureFormat::Rgba8Unorm,
            wgt::TextureDimension::D2,
            extent(4, 4, 1),
            4,
            wgt::TextureUses::RESOURCE | wgt::TextureUses::COLOR_TARGET,
        ));
        result.map(|_| ())
    };
    expect_err(failures, &description, call);

    // 3. Multisampled render passes are not implemented.
    msaa_render_pass_must_fail(label, device, queue, failures);

    // 4. Multisampled render pipelines are not implemented.
    let description = format!("{label}: create_render_pipeline(multisample.count = 4)");
    let call = || unsafe {
        let result = device.create_render_pipeline(&wgpu_hal::RenderPipelineDescriptor {
            label: Some("contract MSAA render pipeline"),
            layout: &zero_pipeline_layout,
            vertex_processor: wgpu_hal::VertexProcessor::Standard {
                vertex_buffers: &[Some(wgpu_hal::VertexBufferLayout {
                    array_stride: 8,
                    step_mode: wgt::VertexStepMode::Vertex,
                    attributes: &[wgt::VertexAttribute {
                        format: wgt::VertexFormat::Float32x2,
                        offset: 0,
                        shader_location: 0,
                    }],
                })],
                vertex_stage: wgpu_hal::ProgrammableStage {
                    module: &shader_module,
                    entry_point: "vs_main",
                    constants: &constants,
                    zero_initialize_workgroup_memory: false,
                },
            },
            primitive: wgt::PrimitiveState {
                topology: wgt::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgt::MultisampleState {
                count: 4,
                ..Default::default()
            },
            fragment_stage: Some(wgpu_hal::ProgrammableStage {
                module: &shader_module,
                entry_point: "fs_main",
                constants: &constants,
                zero_initialize_workgroup_memory: false,
            }),
            color_targets: &[Some(wgt::ColorTargetState {
                format: wgt::TextureFormat::Rgba8Unorm,
                blend: None,
                write_mask: wgt::ColorWrites::ALL,
            })],
            multiview_mask: None,
            cache: None,
        });
        result.map(|_| ())
    };
    expect_err(failures, &description, call);

    // 5. 1D textures are not implemented; only the 1x1x1 sampled volume
    // placeholder used by Bevy's tonemapping pipeline is supported.
    let description = format!("{label}: create_texture(TextureDimension::D1)");
    let call = || unsafe {
        let result = device.create_texture(&texture_desc(
            "contract 1D texture",
            wgt::TextureFormat::Rgba8Unorm,
            wgt::TextureDimension::D1,
            extent(4, 1, 1),
            1,
            wgt::TextureUses::RESOURCE,
        ));
        result.map(|_| ())
    };
    expect_err(failures, &description, call);

    let maybe_volume = unsafe {
        device.create_texture(&texture_desc(
            "contract 3D LUT placeholder",
            wgt::TextureFormat::Rgba8Unorm,
            wgt::TextureDimension::D3,
            extent(1, 1, 1),
            1,
            wgt::TextureUses::RESOURCE | wgt::TextureUses::COPY_DST,
        ))
    };
    match maybe_volume {
        Ok(volume) => {
            let maybe_view = unsafe {
                device.create_texture_view(
                    &volume,
                    &wgpu_hal::TextureViewDescriptor {
                        label: Some("contract 3D LUT view"),
                        format: wgt::TextureFormat::Rgba8Unorm,
                        dimension: wgt::TextureViewDimension::D3,
                        usage: wgt::TextureUses::RESOURCE,
                        range: full_view_range(),
                    },
                )
            };
            match maybe_view {
                Ok(view) => unsafe { device.destroy_texture_view(view) },
                Err(error) => failures.push(format!(
                    "{label}: create_texture_view(TextureDimension::D3) failed: {error:?}"
                )),
            }
            unsafe { device.destroy_texture(volume) };
        }
        Err(error) => failures.push(format!(
            "{label}: create_texture(TextureDimension::D3 1x1x1) failed: {error:?}"
        )),
    }

    // 6. Arbitrary view formats are rejected.
    view_format_must_fail(label, device, failures);

    // 7. Read-only storage buffers are implemented through raw SRVs; writable
    // storage and dynamic storage offsets are still rejected by the layout.
    // The positive read-only path is covered by `positive_storage_smoke`.
    let read_write_storage_entry = [wgt::BindGroupLayoutEntry {
        binding: 0,
        visibility: wgt::ShaderStages::VERTEX_FRAGMENT,
        ty: wgt::BindingType::Buffer {
            ty: wgt::BufferBindingType::Storage { read_only: false },
            has_dynamic_offset: false,
            min_binding_size: wgt::BufferSize::new(16),
        },
        count: None,
    }];
    let description = format!("{label}: create_bind_group_layout(storage read-write)");
    let call = || unsafe {
        let result = device.create_bind_group_layout(&wgpu_hal::BindGroupLayoutDescriptor {
            label: Some("contract read-write storage buffer layout"),
            flags: wgpu_hal::BindGroupLayoutFlags::empty(),
            entries: &read_write_storage_entry,
        });
        result.map(|_| ())
    };
    expect_err(failures, &description, call);

    let dynamic_storage_entry = [wgt::BindGroupLayoutEntry {
        binding: 0,
        visibility: wgt::ShaderStages::VERTEX_FRAGMENT,
        ty: wgt::BindingType::Buffer {
            ty: wgt::BufferBindingType::Storage { read_only: true },
            has_dynamic_offset: true,
            min_binding_size: wgt::BufferSize::new(16),
        },
        count: None,
    }];
    let description = format!("{label}: create_bind_group_layout(storage dynamic offset)");
    let call = || unsafe {
        let result = device.create_bind_group_layout(&wgpu_hal::BindGroupLayoutDescriptor {
            label: Some("contract dynamic storage buffer layout"),
            flags: wgpu_hal::BindGroupLayoutFlags::empty(),
            entries: &dynamic_storage_entry,
        });
        result.map(|_| ())
    };
    expect_err(failures, &description, call);

    // 8. Storage texture bindings must be rejected by the layout.
    let storage_texture_entry = [wgt::BindGroupLayoutEntry {
        binding: 0,
        visibility: wgt::ShaderStages::FRAGMENT,
        ty: wgt::BindingType::StorageTexture {
            access: wgt::StorageTextureAccess::WriteOnly,
            format: wgt::TextureFormat::Rgba8Unorm,
            view_dimension: wgt::TextureViewDimension::D2,
        },
        count: None,
    }];
    let description = format!("{label}: create_bind_group_layout(storage texture)");
    let call = || unsafe {
        let result = device.create_bind_group_layout(&wgpu_hal::BindGroupLayoutDescriptor {
            label: Some("contract storage texture layout"),
            flags: wgpu_hal::BindGroupLayoutFlags::empty(),
            entries: &storage_texture_entry,
        });
        result.map(|_| ())
    };
    expect_err(failures, &description, call);

    // 9. A non-zero immediate size must be rejected while max_immediate_size
    // is zero. `set_immediates` is never called: the layout must be
    // unreachable, so the no-op path stays unreachable by construction.
    let description = format!("{label}: create_pipeline_layout(immediate_size = 4)");
    let call = || unsafe {
        let result = device.create_pipeline_layout(&wgpu_hal::PipelineLayoutDescriptor {
            label: Some("contract immediate layout"),
            flags: wgpu_hal::PipelineLayoutFlags::empty(),
            bind_group_layouts: &[],
            immediate_size: 4,
        });
        result.map(|_| ())
    };
    expect_err(failures, &description, call);

    // 10. Compute pipelines must return an explicit error, not panic.
    let description = format!("{label}: create_compute_pipeline");
    let call = || unsafe {
        let result = device.create_compute_pipeline(&wgpu_hal::ComputePipelineDescriptor {
            label: Some("contract compute pipeline"),
            layout: &zero_pipeline_layout,
            stage: wgpu_hal::ProgrammableStage {
                module: &shader_module,
                entry_point: "cs_main",
                constants: &constants,
                zero_initialize_workgroup_memory: false,
            },
            cache: None,
        });
        result.map(|_| ())
    };
    expect_err(failures, &description, call);

    // 11. Query sets must return an explicit error, not panic.
    let description = format!("{label}: create_query_set");
    let call = || unsafe {
        let result = device.create_query_set(&wgt::QuerySetDescriptor {
            label: Some("contract timestamp query set"),
            ty: wgt::QueryType::Timestamp,
            count: 1,
        });
        result.map(|_| ())
    };
    expect_err(failures, &description, call);

    // 12. Ray-tracing entry points must return an explicit error, not panic.
    let description = format!("{label}: create_ray_tracing_pipeline");
    let call = || unsafe {
        let result = device.create_ray_tracing_pipeline(&wgpu_hal::RayTracingPipelineDescriptor {
            label: Some("contract ray tracing pipeline"),
            layout: &zero_pipeline_layout,
            ray_generation: wgpu_hal::ProgrammableStage {
                module: &shader_module,
                entry_point: "rgen",
                constants: &constants,
                zero_initialize_workgroup_memory: false,
            },
            miss: wgpu_hal::ProgrammableStage {
                module: &shader_module,
                entry_point: "rmiss",
                constants: &constants,
                zero_initialize_workgroup_memory: false,
            },
            intersection: &[],
            max_recursion_depth: 1,
            cache: None,
        });
        result.map(|_| ())
    };
    expect_err(failures, &description, call);

    let description = format!("{label}: create_acceleration_structure");
    let call = || unsafe {
        let result =
            device.create_acceleration_structure(&wgpu_hal::AccelerationStructureDescriptor {
                label: Some("contract acceleration structure"),
                size: 0,
                format: wgpu_hal::AccelerationStructureFormat::BottomLevel,
                allow_compaction: false,
            });
        result.map(|_| ())
    };
    expect_err(failures, &description, call);

    println!(
        "  note: command-encoder ray-tracing methods are silent no-ops on this pin; \
         reachability is not asserted because the descriptor entry points must fail first"
    );

    unsafe {
        device.destroy_pipeline_layout(zero_pipeline_layout);
        device.destroy_shader_module(shader_module);
    }
}

fn msaa_render_pass_must_fail(
    label: &str,
    device: &wgpu_hal::dx11::Device,
    queue: &wgpu_hal::dx11::Queue,
    failures: &mut Vec<String>,
) {
    let target = unsafe {
        device.create_texture(&texture_desc(
            "contract render pass target",
            wgt::TextureFormat::Rgba8Unorm,
            wgt::TextureDimension::D2,
            extent(4, 4, 1),
            1,
            wgt::TextureUses::RESOURCE | wgt::TextureUses::COLOR_TARGET,
        ))
    };
    let target = match target {
        Ok(target) => target,
        Err(e) => {
            failures.push(format!(
                "{label}: MSAA render pass target setup failed: {e:?}"
            ));
            return;
        }
    };

    let target_view = unsafe {
        device.create_texture_view(
            &target,
            &wgpu_hal::TextureViewDescriptor {
                label: Some("contract render pass RTV"),
                format: wgt::TextureFormat::Rgba8Unorm,
                dimension: wgt::TextureViewDimension::D2,
                usage: wgt::TextureUses::COLOR_TARGET,
                range: full_view_range(),
            },
        )
    };
    let target_view = match target_view {
        Ok(target_view) => target_view,
        Err(e) => {
            failures.push(format!("{label}: MSAA render pass RTV setup failed: {e:?}"));
            unsafe { device.destroy_texture(target) };
            return;
        }
    };

    let encoder = unsafe {
        device.create_command_encoder(&wgpu_hal::CommandEncoderDescriptor {
            label: Some("contract MSAA render pass encoder"),
            queue,
        })
    };
    let mut encoder = match encoder {
        Ok(encoder) => encoder,
        Err(e) => {
            failures.push(format!(
                "{label}: MSAA render pass encoder setup failed: {e:?}"
            ));
            unsafe {
                device.destroy_texture_view(target_view);
                device.destroy_texture(target);
            }
            return;
        }
    };

    let description = format!("{label}: begin_render_pass(sample_count = 4)");
    let call = || unsafe {
        encoder.begin_encoding(Some("contract MSAA render pass"))?;
        let color_attachment = wgpu_hal::ColorAttachment {
            target: wgpu_hal::Attachment {
                view: &target_view,
                usage: wgt::TextureUses::COLOR_TARGET,
            },
            depth_slice: None,
            resolve_target: None,
            ops: wgpu_hal::AttachmentOps::LOAD_CLEAR | wgpu_hal::AttachmentOps::STORE,
            clear_value: wgt::Color {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 1.0,
            },
        };
        let descriptor = wgpu_hal::RenderPassDescriptor {
            label: Some("contract MSAA render pass"),
            extent: extent(4, 4, 1),
            sample_count: 4,
            color_attachments: &[Some(color_attachment)],
            depth_stencil_attachment: None,
            multiview_mask: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        };
        let result = encoder.begin_render_pass(&descriptor);
        if result.is_ok() {
            encoder.end_render_pass();
        }
        let _ = encoder.end_encoding();
        result
    };
    expect_err(failures, &description, call);

    unsafe {
        device.destroy_texture_view(target_view);
        device.destroy_texture(target);
    }
}

fn view_format_must_fail(label: &str, device: &wgpu_hal::dx11::Device, failures: &mut Vec<String>) {
    let texture = unsafe {
        device.create_texture(&texture_desc(
            "contract view format texture",
            wgt::TextureFormat::Rgba8Unorm,
            wgt::TextureDimension::D2,
            extent(4, 4, 1),
            1,
            wgt::TextureUses::RESOURCE | wgt::TextureUses::COLOR_TARGET,
        ))
    };
    let texture = match texture {
        Ok(texture) => texture,
        Err(e) => {
            failures.push(format!(
                "{label}: view-format test texture setup failed: {e:?}"
            ));
            return;
        }
    };

    let description = format!("{label}: create_texture_view(format = Rgba8Uint)");
    let call = || unsafe {
        let result = device.create_texture_view(
            &texture,
            &wgpu_hal::TextureViewDescriptor {
                label: Some("contract Rgba8Uint view"),
                format: wgt::TextureFormat::Rgba8Uint,
                dimension: wgt::TextureViewDimension::D2,
                usage: wgt::TextureUses::RESOURCE,
                range: full_view_range(),
            },
        );
        result.map(|_| ())
    };
    expect_err(failures, &description, call);

    unsafe { device.destroy_texture(texture) };
}

fn surface_negative_tests(
    label: &str,
    surface: &wgpu_hal::dx11::Surface,
    device: &wgpu_hal::dx11::Device,
    failures: &mut Vec<String>,
) {
    let base = wgpu_hal::SurfaceConfiguration {
        maximum_frame_latency: 1,
        present_mode: wgt::PresentMode::Fifo,
        composite_alpha_mode: wgt::CompositeAlphaMode::Opaque,
        format: wgt::TextureFormat::Bgra8UnormSrgb,
        color_space: wgt::SurfaceColorSpace::Srgb,
        extent: extent(WINDOW_WIDTH, WINDOW_HEIGHT, 1),
        usage: wgt::TextureUses::COLOR_TARGET,
        view_formats: Vec::new(),
    };

    let invalid_format = wgpu_hal::SurfaceConfiguration {
        format: wgt::TextureFormat::Rgba8Unorm,
        ..base.clone()
    };
    let description = format!("{label}: surface.configure(format = Rgba8Unorm)");
    let call = || unsafe { surface.configure(device, &invalid_format) };
    expect_err(failures, &description, call);

    let invalid_present_mode = wgpu_hal::SurfaceConfiguration {
        present_mode: wgt::PresentMode::Mailbox,
        ..base
    };
    let description = format!("{label}: surface.configure(present_mode = Mailbox)");
    let call = || unsafe { surface.configure(device, &invalid_present_mode) };
    expect_err(failures, &description, call);

    println!(
        "  note: both configure calls failed before touching the swap chain, so the surface \
         is left unconfigured and no invalid configuration is retained"
    );
}

fn expect_err<T, E>(
    failures: &mut Vec<String>,
    description: &str,
    call: impl FnOnce() -> Result<T, E>,
) {
    match catch_unwind(AssertUnwindSafe(call)) {
        Ok(Ok(_)) => failures.push(format!(
            "{description}: accepted, but Gate 0 requires an explicit error"
        )),
        Ok(Err(_)) => println!("  ok: {description} returned an explicit Err"),
        Err(payload) => failures.push(format!(
            "{description}: panicked ({}) instead of returning an explicit error",
            panic_message(payload.as_ref())
        )),
    }
}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_owned()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "<non-string panic payload>".to_owned()
    }
}

fn create_test_shader_module(
    device: &wgpu_hal::dx11::Device,
) -> Result<wgpu_hal::dx11::ShaderModule, String> {
    let module =
        naga::front::wgsl::parse_str(SHADER).map_err(|e| format!("WGSL parse failed: {e:?}"))?;
    let info = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .map_err(|e| format!("Naga validation failed: {e:?}"))?;

    unsafe {
        device.create_shader_module(
            &wgpu_hal::ShaderModuleDescriptor {
                label: Some("contract shader"),
                runtime_checks: wgt::ShaderRuntimeChecks::checked(),
            },
            wgpu_hal::ShaderInput::Naga(wgpu_hal::NagaShader {
                module: Cow::Owned(module),
                info,
                debug_source: None,
            }),
        )
    }
    .map_err(|e| format!("create shader module: {e:?}"))
}

fn is_same_set<T: PartialEq>(actual: &[T], expected: &[T]) -> bool {
    actual.len() == expected.len() && expected.iter().all(|item| actual.contains(item))
}

fn texture_desc(
    label: &'static str,
    format: wgt::TextureFormat,
    dimension: wgt::TextureDimension,
    size: wgt::Extent3d,
    sample_count: u32,
    usage: wgt::TextureUses,
) -> wgpu_hal::TextureDescriptor<'static> {
    wgpu_hal::TextureDescriptor {
        label: Some(label),
        size,
        mip_level_count: 1,
        sample_count,
        dimension,
        format,
        usage,
        memory_flags: wgpu_hal::MemoryFlags::empty(),
        view_formats: Vec::new(),
    }
}

fn extent(width: u32, height: u32, depth_or_array_layers: u32) -> wgt::Extent3d {
    wgt::Extent3d {
        width,
        height,
        depth_or_array_layers,
    }
}

fn full_view_range() -> wgt::ImageSubresourceRange {
    wgt::ImageSubresourceRange {
        aspect: wgt::TextureAspect::All,
        base_mip_level: 0,
        mip_level_count: Some(1),
        base_array_layer: 0,
        array_layer_count: Some(1),
    }
}

struct HiddenWindow {
    hwnd: HWND,
    hinstance: HINSTANCE,
    class_name: PCWSTR,
}

impl HiddenWindow {
    fn create() -> Result<Self, String> {
        let hmodule = unsafe { GetModuleHandleW(PCWSTR::null()) }
            .map_err(|e| format!("GetModuleHandleW failed: {e:?}"))?;
        let hinstance = HINSTANCE(hmodule.0);
        let class_name = w!("wgpu_hal_dx11_contract_window");
        let window_class = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(window_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: hinstance,
            hIcon: HICON(core::ptr::null_mut()),
            hCursor: HCURSOR(core::ptr::null_mut()),
            hbrBackground: HBRUSH(core::ptr::null_mut()),
            lpszMenuName: PCWSTR::null(),
            lpszClassName: class_name,
        };
        if unsafe { RegisterClassW(&window_class) } == 0 {
            return Err("RegisterClassW failed".to_owned());
        }

        let hwnd = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                class_name,
                w!("wgpu-hal D3D11 contract surface"),
                WS_OVERLAPPEDWINDOW,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                WINDOW_WIDTH as i32,
                WINDOW_HEIGHT as i32,
                None,
                None,
                Some(hinstance),
                None,
            )
        }
        .map_err(|e| format!("CreateWindowExW failed: {e:?}"))?;

        Ok(Self {
            hwnd,
            hinstance,
            class_name,
        })
    }

    fn raw_window_handle(&self) -> Win32WindowHandle {
        let hwnd = NonZeroIsize::new(self.hwnd.0 as isize)
            .expect("CreateWindowExW must not return a null HWND");
        let mut handle = Win32WindowHandle::new(hwnd);
        handle.hinstance = NonZeroIsize::new(self.hinstance.0 as isize);
        handle
    }
}

impl Drop for HiddenWindow {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.hwnd);
            let _ = UnregisterClassW(self.class_name, Some(self.hinstance));
        }
    }
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
}

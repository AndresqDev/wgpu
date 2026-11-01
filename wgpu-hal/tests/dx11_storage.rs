//! End-to-end coverage for read-only storage buffers in the experimental
//! Direct3D 11 backend.
//!
//! The backend exposes `var<storage, read>` through a raw buffer SRV
//! (`ByteAddressBuffer` in Naga's HLSL) bound to the shared `t` register
//! space. This test opens the HAL without a window, checks the advertised
//! storage capabilities, creates a storage buffer with a non-zero static
//! offset, binds it with `set_bind_group`, draws a fullscreen triangle and
//! reads back the rendered texel. A wrong offset or a missing raw SRV shows up
//! as the wrong pixel, not as a silently dropped binding.
//!
//! Only read-only storage buffers are covered. Compute, UAV/read-write
//! storage, storage textures and dynamic storage offsets are not implemented
//! and are checked as explicit rejections.
//!
//! The test uses `Features::empty()` and the adapter's own limits, so it runs
//! unchanged on WARP and on hardware. The runner is WARP at feature level
//! 11_0; on a 10_1 adapter the GPU sections are skipped after asserting that
//! storage is not advertised there.
//!
//! Run on Windows CI with:
//!
//! ```text
//! cargo test -p wgpu-hal --features dx11 --test dx11_storage -- --nocapture
//! ```

#![cfg(all(target_os = "windows", feature = "dx11"))]

use std::borrow::Cow;
use std::num::NonZeroU32;

use wgpu_hal::{Adapter as _, CommandEncoder as _, Device as _, Instance as _, Queue as _};
use wgpu_types as wgt;

/// Read-back row pitch. It is a multiple of 256, as D3D11 texture copies
/// require, and it is larger than the visible 4x4 row so a backend that
/// ignores `bytes_per_row` reads the wrong texel.
const PITCH: u32 = 256;

const STORAGE_BUFFER_SIZE: u64 = 512;
const STORAGE_BINDING_OFFSET: u64 = 256;
const STORAGE_BINDING_SIZE: u64 = 16;
const TARGET_WIDTH: u32 = 4;
const TARGET_HEIGHT: u32 = 4;

/// `255` in the bound subrange produces red after the fragment divides by
/// 255.0; the rest of the buffer is zero, so ignoring the static offset would
/// render black.
const RED: [u8; 4] = [255, 0, 0, 255];

const SHADER: &str = r#"
struct Data {
    value: vec4<u32>,
}

@group(0) @binding(0) var<storage, read> data: Data;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> VertexOutput {
    // Fullscreen triangle: (-1, -1), (3, -1), (-1, 3).
    let x = f32((vertex_index << 1u) & 2u) * 2.0 - 1.0;
    let y = f32(vertex_index & 2u) * 2.0 - 1.0;
    var output: VertexOutput;
    output.position = vec4<f32>(x, y, 0.0, 1.0);
    return output;
}

@fragment
fn fs_main() -> @location(0) vec4<f32> {
    return vec4<f32>(f32(data.value.x) / 255.0, 0.0, 0.0, 1.0);
}
"#;

#[test]
fn dx11_storage() {
    if let Err(error) = run() {
        panic!("DX11 storage test failed: {error}");
    }
    println!("dx11_storage: all storage sections passed");
}

fn run() -> Result<(), String> {
    let instance_desc = wgpu_hal::InstanceDescriptor {
        name: "wgpu-hal-dx11-storage",
        flags: wgt::InstanceFlags::default(),
        memory_budget_thresholds: wgt::MemoryBudgetThresholds::default(),
        backend_options: wgt::BackendOptions::default(),
        telemetry: None,
        display: None,
    };

    let instance =
        unsafe { <wgpu_hal::dx11::Instance as wgpu_hal::Instance>::init(&instance_desc) }
            .map_err(|e| format!("D3D11 instance initialization failed: {e:?}"))?;
    let adapters = unsafe { instance.enumerate_adapters(None) };
    println!(
        "dx11_storage: enumerated {} DXGI adapter(s)",
        adapters.len()
    );
    if adapters.is_empty() {
        return Err("no DXGI/D3D11 adapters were found".to_owned());
    }

    // WARP is the only adapter in CI. Try every enumerated adapter and use the
    // first one that can open a device.
    let mut opened = None;
    for (index, exposed) in adapters.iter().enumerate() {
        println!(
            "adapter[{index}] '{}' ({:?})",
            exposed.info.name, exposed.info.device_type
        );
        match unsafe {
            exposed.adapter.open(
                wgt::Features::empty(),
                &exposed.capabilities.limits,
                &wgt::MemoryHints::default(),
            )
        } {
            Ok(open) => {
                opened = Some((index, open));
                break;
            }
            Err(error) => println!("  adapter[{index}] could not be opened: {error}"),
        }
    }
    let (adapter_index, wgpu_hal::OpenDevice { device, queue }) =
        opened.ok_or_else(|| "no enumerated D3D11 adapter could open a device".to_owned())?;
    let exposed = &adapters[adapter_index];
    let feature_level = exposed.adapter.feature_level_name();
    println!(
        "opened D3D11 device on '{}' with Features::empty() (feature level {feature_level})",
        exposed.info.name
    );

    let storage_supported = check_capabilities(feature_level, &exposed.capabilities)?;
    if !storage_supported {
        println!(
            "dx11_storage: feature level {feature_level} does not advertise read-only storage; \
             GPU storage coverage skipped"
        );
        drop(queue);
        drop(device);
        return Ok(());
    }

    section_buffer_creation(&device)?;
    let (bind_group_layout, pipeline_layout) = section_layouts(&device)?;
    let shader = create_shader_module(&device)?;

    if let Err(error) = section_render_and_readback(
        &device,
        &queue,
        &shader,
        &bind_group_layout,
        &pipeline_layout,
    ) {
        unsafe {
            device.destroy_pipeline_layout(pipeline_layout);
            device.destroy_bind_group_layout(bind_group_layout);
            device.destroy_shader_module(shader);
        }
        return Err(error);
    }

    let negatives = section_bind_group_negatives(&device, &bind_group_layout);

    unsafe {
        device.destroy_pipeline_layout(pipeline_layout);
        device.destroy_bind_group_layout(bind_group_layout);
        device.destroy_shader_module(shader);
    }
    negatives?;

    println!("dx11_storage: destroying the queue and device");
    drop(queue);
    drop(device);
    Ok(())
}

// ---------------------------------------------------------------------------
// 1. Advertised capabilities
// ---------------------------------------------------------------------------

fn check_capabilities(
    feature_level: &str,
    capabilities: &wgpu_hal::Capabilities,
) -> Result<bool, String> {
    println!("\n=== 1. storage capabilities ===");
    let flags = capabilities.downlevel.flags;
    let limits = &capabilities.limits;
    let announced: Vec<&str> = flags.iter_names().map(|(name, _)| name).collect();
    println!("  feature level: {feature_level}");
    println!("  downlevel flags: {announced:?}");
    println!(
        "  max_storage_buffers_per_shader_stage = {}",
        limits.max_storage_buffers_per_shader_stage
    );
    println!(
        "  max_storage_buffer_binding_size = {}",
        limits.max_storage_buffer_binding_size
    );
    println!(
        "  max_dynamic_storage_buffers_per_pipeline_layout = {}",
        limits.max_dynamic_storage_buffers_per_pipeline_layout
    );
    println!(
        "  max_storage_textures_per_shader_stage = {}",
        limits.max_storage_textures_per_shader_stage
    );
    println!(
        "  min_storage_buffer_offset_alignment = {}",
        limits.min_storage_buffer_offset_alignment
    );

    // These three flags describe writable storage, compute and unaligned
    // buffer bindings. None of them is implemented, so none may be advertised
    // at any feature level.
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
            return Err(format!("{name} must not be advertised"));
        }
    }

    if feature_level == "11_0" {
        for (name, flag) in [
            ("VERTEX_STORAGE", wgt::DownlevelFlags::VERTEX_STORAGE),
            ("FRAGMENT_STORAGE", wgt::DownlevelFlags::FRAGMENT_STORAGE),
        ] {
            if !flags.contains(flag) {
                return Err(format!("{name} is missing at feature level 11_0"));
            }
        }
        for (name, actual, expected) in [
            (
                "max_storage_buffers_per_shader_stage",
                u64::from(limits.max_storage_buffers_per_shader_stage),
                4,
            ),
            (
                "max_storage_buffer_binding_size",
                limits.max_storage_buffer_binding_size,
                128 << 20,
            ),
            (
                "max_dynamic_storage_buffers_per_pipeline_layout",
                u64::from(limits.max_dynamic_storage_buffers_per_pipeline_layout),
                0,
            ),
            (
                "max_storage_textures_per_shader_stage",
                u64::from(limits.max_storage_textures_per_shader_stage),
                0,
            ),
            (
                "min_storage_buffer_offset_alignment",
                u64::from(limits.min_storage_buffer_offset_alignment),
                256,
            ),
        ] {
            if actual != expected {
                return Err(format!(
                    "feature level 11_0: {name} is {actual}, expected {expected}"
                ));
            }
        }
        println!("  feature level 11_0 advertises read-only storage buffers only");
        Ok(true)
    } else if feature_level == "10_1" {
        if flags
            .intersects(wgt::DownlevelFlags::VERTEX_STORAGE | wgt::DownlevelFlags::FRAGMENT_STORAGE)
        {
            return Err(format!(
                "feature level 10_1 must not advertise storage flags: {flags:?}"
            ));
        }
        if limits.max_storage_buffers_per_shader_stage != 0
            || limits.max_storage_buffer_binding_size != 0
        {
            return Err(format!(
                "feature level 10_1 must not advertise storage limits: \
                 buffers={}, binding size={}",
                limits.max_storage_buffers_per_shader_stage, limits.max_storage_buffer_binding_size
            ));
        }
        println!("  feature level 10_1 does not advertise storage");
        Ok(false)
    } else {
        println!("  feature level {feature_level} is outside the 11_0/10_1 contract; skipping");
        Ok(false)
    }
}

// ---------------------------------------------------------------------------
// 2. Buffer creation
// ---------------------------------------------------------------------------

fn section_buffer_creation(device: &wgpu_hal::dx11::Device) -> Result<(), String> {
    println!("\n=== 2. storage buffer creation ===");

    let accepted = create_buffer(
        device,
        "storage read-only + read-write + COPY_DST",
        STORAGE_BUFFER_SIZE,
        wgt::BufferUses::STORAGE_READ_ONLY
            | wgt::BufferUses::STORAGE_READ_WRITE
            | wgt::BufferUses::COPY_DST,
    )?;
    println!(
        "  accepted storage read-only + storage read-write + COPY_DST \
         (the usage shape wgpu-core produces)"
    );
    unsafe { device.destroy_buffer(accepted) };

    let rejected = [
        (
            "storage read-write without storage read-only",
            wgt::BufferUses::STORAGE_READ_WRITE,
        ),
        (
            "storage read-only + VERTEX",
            wgt::BufferUses::STORAGE_READ_ONLY | wgt::BufferUses::VERTEX,
        ),
    ];
    for (label, usage) in rejected {
        expect_err(
            &format!("create_buffer({label})"),
            create_buffer(device, label, STORAGE_BUFFER_SIZE, usage),
        )?;
    }

    println!("  storage buffer positives and negatives OK");
    Ok(())
}

// ---------------------------------------------------------------------------
// 3. Bind group and pipeline layouts
// ---------------------------------------------------------------------------

fn section_layouts(
    device: &wgpu_hal::dx11::Device,
) -> Result<
    (
        wgpu_hal::dx11::BindGroupLayout,
        wgpu_hal::dx11::PipelineLayout,
    ),
    String,
> {
    println!("\n=== 3. storage layouts ===");

    let entries = [storage_entry(0, true, false)];
    let bind_group_layout = create_layout(device, "storage read-only", &entries)?;
    let pipeline_layout =
        match create_pipeline_layout(device, "storage read-only", &[&bind_group_layout]) {
            Ok(layout) => layout,
            Err(error) => {
                unsafe { device.destroy_bind_group_layout(bind_group_layout) };
                return Err(error);
            }
        };

    let target = pipeline_layout
        .bind_target(0, 0)
        .ok_or_else(|| "storage binding has no flattened register".to_owned())?;
    if target.space != 0 || target.register != 0 {
        return Err(format!(
            "storage binding mapped to space {} register {}, expected t0",
            target.space, target.register
        ));
    }
    println!("  read-only storage layout accepted; binding maps to t0");

    let count_two = NonZeroU32::new(2).unwrap();
    let negative_entries: [(&str, wgt::BindGroupLayoutEntry); 5] = [
        ("storage read-write", storage_entry(0, false, false)),
        ("storage dynamic offset", storage_entry(0, true, true)),
        (
            "storage binding array",
            wgt::BindGroupLayoutEntry {
                count: Some(count_two),
                ..storage_entry(0, true, false)
            },
        ),
        (
            "storage compute visibility",
            wgt::BindGroupLayoutEntry {
                visibility: wgt::ShaderStages::COMPUTE,
                ..storage_entry(0, true, false)
            },
        ),
        (
            "storage texture",
            wgt::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgt::ShaderStages::FRAGMENT,
                ty: wgt::BindingType::StorageTexture {
                    access: wgt::StorageTextureAccess::WriteOnly,
                    format: wgt::TextureFormat::Rgba8Unorm,
                    view_dimension: wgt::TextureViewDimension::D2,
                },
                count: None,
            },
        ),
    ];
    for (label, entry) in negative_entries {
        expect_layout_error(device, label, &[entry])?;
    }

    println!("  read-write/UAV, dynamic offsets, arrays, compute and storage textures rejected");
    Ok((bind_group_layout, pipeline_layout))
}

// ---------------------------------------------------------------------------
// 4. Shader, upload, draw and read-back
// ---------------------------------------------------------------------------

fn section_render_and_readback(
    device: &wgpu_hal::dx11::Device,
    queue: &wgpu_hal::dx11::Queue,
    shader: &wgpu_hal::dx11::ShaderModule,
    bind_group_layout: &wgpu_hal::dx11::BindGroupLayout,
    pipeline_layout: &wgpu_hal::dx11::PipelineLayout,
) -> Result<(), String> {
    println!("\n=== 4. storage shader and draw ===");

    check_generated_hlsl(shader, pipeline_layout)?;
    let pipeline = create_render_pipeline(device, shader, pipeline_layout)?;

    let storage_buffer = create_buffer(
        device,
        "storage buffer",
        STORAGE_BUFFER_SIZE,
        wgt::BufferUses::STORAGE_READ_ONLY
            | wgt::BufferUses::STORAGE_READ_WRITE
            | wgt::BufferUses::COPY_DST,
    )?;

    // Upload 16 bytes into 256..272. The bound vec4 is (255, 0, 0, 0); every
    // other byte of the buffer stays zero.
    let mut upload = [0u8; STORAGE_BINDING_SIZE as usize];
    upload[0..4].copy_from_slice(&255u32.to_le_bytes());
    let staging = create_buffer(
        device,
        "storage staging",
        STORAGE_BINDING_SIZE,
        wgt::BufferUses::MAP_WRITE | wgt::BufferUses::COPY_SRC,
    )?;
    write_buffer(device, "storage staging", &staging, &upload)?;
    copy_buffer_region(
        device,
        queue,
        "upload storage subrange",
        &staging,
        &storage_buffer,
        0,
        STORAGE_BINDING_OFFSET,
        STORAGE_BINDING_SIZE,
    )?;
    println!("  uploaded 16 bytes at buffer offset {STORAGE_BINDING_OFFSET}");

    let bind_group = create_storage_group(
        device,
        "storage group",
        bind_group_layout,
        &storage_buffer,
        STORAGE_BINDING_OFFSET,
        STORAGE_BINDING_SIZE,
    )?;
    pipeline_layout
        .validate_bind_group(0, &bind_group, &[], false)
        .map_err(|e| format!("validate_bind_group(storage): {e}"))?;
    println!("  storage bind group validated with offset 256 and size 16");

    let target = create_texture(device, "storage target")?;
    let target_view = create_view(device, &target, "storage RTV")?;

    submit_encoder(device, queue, "storage draw", |encoder| {
        let color_attachments = [Some(make_color_attachment(&target_view, wgt::Color::BLACK))];
        let pass = render_pass_descriptor(
            "storage draw",
            wgt::Extent3d {
                width: TARGET_WIDTH,
                height: TARGET_HEIGHT,
                depth_or_array_layers: 1,
            },
            1,
            &color_attachments,
        );
        unsafe {
            encoder
                .begin_render_pass(&pass)
                .map_err(|e| format!("begin_render_pass(storage draw): {e:?}"))?;
            encoder.set_render_pipeline(&pipeline);
            encoder.set_bind_group(pipeline_layout, 0, &bind_group, &[]);
            encoder.draw(0, 3, 0, 1);
            encoder.end_render_pass();
        }
        Ok(())
    })?;

    let data = read_texture(
        device,
        queue,
        "storage readback",
        &target,
        TARGET_WIDTH,
        TARGET_HEIGHT,
        PITCH,
    )?;
    expect_pixel("storage draw", &data, 0, 0, RED)?;
    expect_pixel(
        "storage draw",
        &data,
        TARGET_WIDTH - 1,
        TARGET_HEIGHT - 1,
        RED,
    )?;

    // This mirrors `create_buffer_with_data`: wgpu-core marks the buffer with
    // MAP_WRITE while it stages the initial contents. D3D11 cannot bind a
    // staging resource as an SRV, so the backend keeps a CPU staging copy and
    // publishes it on unmap.
    let mapped_storage = create_buffer(
        device,
        "mapped storage",
        STORAGE_BINDING_SIZE,
        wgt::BufferUses::STORAGE_READ_ONLY | wgt::BufferUses::MAP_WRITE,
    )?;
    let mapping = unsafe { device.map_buffer(&mapped_storage, 0..STORAGE_BINDING_SIZE) }
        .map_err(|e| format!("map_buffer(mapped storage): {e:?}"))?;
    let mut initial = [0u8; STORAGE_BINDING_SIZE as usize];
    initial[0..4].copy_from_slice(&255u32.to_le_bytes());
    unsafe {
        core::ptr::copy_nonoverlapping(initial.as_ptr(), mapping.ptr.as_ptr(), initial.len());
        device.unmap_buffer(&mapped_storage);
    }
    println!("  mapped storage buffer initialized through its staging copy");

    let mapped_group = create_storage_group(
        device,
        "mapped storage group",
        bind_group_layout,
        &mapped_storage,
        0,
        STORAGE_BINDING_SIZE,
    )?;
    let mapped_target = create_texture(device, "mapped storage target")?;
    let mapped_view = create_view(device, &mapped_target, "mapped storage RTV")?;
    submit_encoder(device, queue, "mapped storage draw", |encoder| {
        let color_attachments = [Some(make_color_attachment(&mapped_view, wgt::Color::BLACK))];
        let pass = render_pass_descriptor(
            "mapped storage draw",
            wgt::Extent3d {
                width: TARGET_WIDTH,
                height: TARGET_HEIGHT,
                depth_or_array_layers: 1,
            },
            1,
            &color_attachments,
        );
        unsafe {
            encoder
                .begin_render_pass(&pass)
                .map_err(|e| format!("begin_render_pass(mapped storage draw): {e:?}"))?;
            encoder.set_render_pipeline(&pipeline);
            encoder.set_bind_group(pipeline_layout, 0, &mapped_group, &[]);
            encoder.draw(0, 3, 0, 1);
            encoder.end_render_pass();
        }
        Ok(())
    })?;
    let mapped_data = read_texture(
        device,
        queue,
        "mapped storage readback",
        &mapped_target,
        TARGET_WIDTH,
        TARGET_HEIGHT,
        PITCH,
    )?;
    expect_pixel("mapped storage draw", &mapped_data, 0, 0, RED)?;

    unsafe {
        device.destroy_render_pipeline(pipeline);
        device.destroy_texture_view(mapped_view);
        device.destroy_texture(mapped_target);
        device.destroy_bind_group(mapped_group);
        device.destroy_buffer(mapped_storage);
        device.destroy_texture_view(target_view);
        device.destroy_texture(target);
        device.destroy_bind_group(bind_group);
        device.destroy_buffer(staging);
        device.destroy_buffer(storage_buffer);
    }
    println!("  fullscreen draw read the raw SRV at offset 256 and produced red");
    Ok(())
}

fn check_generated_hlsl(
    shader: &wgpu_hal::dx11::ShaderModule,
    pipeline_layout: &wgpu_hal::dx11::PipelineLayout,
) -> Result<(), String> {
    let constants = naga::back::PipelineConstants::default();
    let vertex = shader
        .compile(
            "vs_main",
            naga::ShaderStage::Vertex,
            pipeline_layout,
            &constants,
        )
        .map_err(|e| format!("compile(vs_main): {e}"))?;
    let fragment = shader
        .compile(
            "fs_main",
            naga::ShaderStage::Fragment,
            pipeline_layout,
            &constants,
        )
        .map_err(|e| format!("compile(fs_main): {e}"))?;

    println!("  vertex HLSL:\n{}", vertex.hlsl);
    println!("  fragment HLSL:\n{}", fragment.hlsl);

    if !fragment.hlsl.contains("ByteAddressBuffer") {
        return Err(format!(
            "fragment HLSL does not declare a ByteAddressBuffer:\n{}",
            fragment.hlsl
        ));
    }
    if fragment.hlsl.contains("RWByteAddressBuffer") {
        return Err(format!(
            "fragment HLSL declares a writable RWByteAddressBuffer, but the storage binding is read-only:\n{}",
            fragment.hlsl
        ));
    }
    let has_expected_register =
        fragment.hlsl.contains("register(t0)") || fragment.hlsl.contains("register(t0, space0)");
    if !has_expected_register {
        return Err(format!(
            "fragment HLSL does not bind the storage buffer to t0:\n{}",
            fragment.hlsl
        ));
    }
    if vertex.bytecode.is_empty() || fragment.bytecode.is_empty() {
        return Err("FXC returned empty bytecode".to_owned());
    }
    println!("  HLSL uses a read-only ByteAddressBuffer at t0");
    Ok(())
}

// ---------------------------------------------------------------------------
// 5. Bind group negatives
// ---------------------------------------------------------------------------

fn section_bind_group_negatives(
    device: &wgpu_hal::dx11::Device,
    bind_group_layout: &wgpu_hal::dx11::BindGroupLayout,
) -> Result<(), String> {
    println!("\n=== 5. storage bind group negatives ===");

    let non_storage_buffer =
        create_buffer(device, "non-storage buffer", 64, wgt::BufferUses::COPY_DST)?;
    let result = create_storage_group(
        device,
        "storage group with a non-storage buffer",
        bind_group_layout,
        &non_storage_buffer,
        0,
        64,
    );
    unsafe { device.destroy_buffer(non_storage_buffer) };

    match result {
        Ok(_) => Err(
            "create_bind_group accepted a buffer without STORAGE usage for a storage binding"
                .to_owned(),
        ),
        Err(error) => {
            println!("  rejected as expected: storage bind group without STORAGE usage ({error})");
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// Shader and resource helpers
// ---------------------------------------------------------------------------

fn storage_entry(
    binding: u32,
    read_only: bool,
    has_dynamic_offset: bool,
) -> wgt::BindGroupLayoutEntry {
    wgt::BindGroupLayoutEntry {
        binding,
        visibility: wgt::ShaderStages::VERTEX_FRAGMENT,
        ty: wgt::BindingType::Buffer {
            ty: wgt::BufferBindingType::Storage { read_only },
            has_dynamic_offset,
            min_binding_size: wgt::BufferSize::new(STORAGE_BINDING_SIZE),
        },
        count: None,
    }
}

fn expect_layout_error(
    device: &wgpu_hal::dx11::Device,
    label: &str,
    entries: &[wgt::BindGroupLayoutEntry],
) -> Result<(), String> {
    match create_layout(device, label, entries) {
        Ok(_) => Err(format!(
            "{label}: create_bind_group_layout unexpectedly succeeded"
        )),
        Err(error) => {
            println!("  rejected as expected: {label} ({error})");
            Ok(())
        }
    }
}

fn create_layout(
    device: &wgpu_hal::dx11::Device,
    label: &str,
    entries: &[wgt::BindGroupLayoutEntry],
) -> Result<wgpu_hal::dx11::BindGroupLayout, String> {
    unsafe {
        device.create_bind_group_layout(&wgpu_hal::BindGroupLayoutDescriptor {
            label: Some(label),
            flags: wgpu_hal::BindGroupLayoutFlags::empty(),
            entries,
        })
    }
    .map_err(|e| format!("create_bind_group_layout({label}): {e:?}"))
}

fn create_pipeline_layout(
    device: &wgpu_hal::dx11::Device,
    label: &str,
    layouts: &[&wgpu_hal::dx11::BindGroupLayout],
) -> Result<wgpu_hal::dx11::PipelineLayout, String> {
    let layout_options: Vec<_> = layouts.iter().map(|layout| Some(*layout)).collect();
    unsafe {
        device.create_pipeline_layout(&wgpu_hal::PipelineLayoutDescriptor {
            label: Some(label),
            flags: wgpu_hal::PipelineLayoutFlags::empty(),
            bind_group_layouts: &layout_options,
            immediate_size: 0,
        })
    }
    .map_err(|e| format!("create_pipeline_layout({label}): {e:?}"))
}

fn create_shader_module(
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
                label: Some("dx11 storage shader"),
                runtime_checks: wgt::ShaderRuntimeChecks::checked(),
            },
            wgpu_hal::ShaderInput::Naga(wgpu_hal::NagaShader {
                module: Cow::Owned(module),
                info,
                debug_source: None,
            }),
        )
    }
    .map_err(|e| format!("create_shader_module: {e:?}"))
}

fn create_render_pipeline(
    device: &wgpu_hal::dx11::Device,
    shader: &wgpu_hal::dx11::ShaderModule,
    pipeline_layout: &wgpu_hal::dx11::PipelineLayout,
) -> Result<wgpu_hal::dx11::RenderPipeline, String> {
    let constants = naga::back::PipelineConstants::default();
    let color_targets = [Some(wgt::ColorTargetState {
        format: wgt::TextureFormat::Rgba8Unorm,
        blend: None,
        write_mask: wgt::ColorWrites::ALL,
    })];
    unsafe {
        device.create_render_pipeline(&wgpu_hal::RenderPipelineDescriptor {
            label: Some("storage render pipeline"),
            layout: pipeline_layout,
            vertex_processor: wgpu_hal::VertexProcessor::Standard {
                vertex_buffers: &[],
                vertex_stage: wgpu_hal::ProgrammableStage {
                    module: shader,
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
                module: shader,
                entry_point: "fs_main",
                constants: &constants,
                zero_initialize_workgroup_memory: false,
            }),
            color_targets: &color_targets,
            multiview_mask: None,
            cache: None,
        })
    }
    .map_err(|e| format!("create_render_pipeline(storage): {e:?}"))
}

fn create_storage_group(
    device: &wgpu_hal::dx11::Device,
    label: &str,
    layout: &wgpu_hal::dx11::BindGroupLayout,
    buffer: &wgpu_hal::dx11::Buffer,
    offset: u64,
    size: u64,
) -> Result<wgpu_hal::dx11::BindGroup, String> {
    let size = wgt::BufferSize::new(size)
        .ok_or_else(|| format!("create_bind_group({label}): size must be non-zero"))?;
    let buffer_binding = wgpu_hal::BufferBinding::new_unchecked(buffer, offset, Some(size));
    let entries = [wgpu_hal::BindGroupEntry {
        binding: 0,
        resource_index: 0,
        count: 1,
    }];
    unsafe {
        device.create_bind_group(&wgpu_hal::BindGroupDescriptor {
            label: Some(label),
            layout,
            buffers: &[buffer_binding],
            samplers: &[],
            textures: &[],
            entries: &entries,
            acceleration_structures: &[],
            external_textures: &[],
        })
    }
    .map_err(|e| format!("create_bind_group({label}): {e:?}"))
}

fn create_buffer(
    device: &wgpu_hal::dx11::Device,
    label: &str,
    size: u64,
    usage: wgt::BufferUses,
) -> Result<wgpu_hal::dx11::Buffer, String> {
    unsafe {
        device.create_buffer(&wgpu_hal::BufferDescriptor {
            label: Some(label),
            size,
            usage,
            memory_flags: wgpu_hal::MemoryFlags::empty(),
        })
    }
    .map_err(|e| format!("create_buffer({label}): {e:?}"))
}

fn create_texture(
    device: &wgpu_hal::dx11::Device,
    label: &str,
) -> Result<wgpu_hal::dx11::Texture, String> {
    unsafe {
        device.create_texture(&wgpu_hal::TextureDescriptor {
            label: Some(label),
            size: wgt::Extent3d {
                width: TARGET_WIDTH,
                height: TARGET_HEIGHT,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgt::TextureDimension::D2,
            format: wgt::TextureFormat::Rgba8Unorm,
            usage: wgt::TextureUses::COLOR_TARGET | wgt::TextureUses::COPY_SRC,
            memory_flags: wgpu_hal::MemoryFlags::empty(),
            view_formats: Vec::new(),
        })
    }
    .map_err(|e| format!("create_texture({label}): {e:?}"))
}

fn create_view(
    device: &wgpu_hal::dx11::Device,
    texture: &wgpu_hal::dx11::Texture,
    label: &str,
) -> Result<wgpu_hal::dx11::TextureView, String> {
    unsafe {
        device.create_texture_view(
            texture,
            &wgpu_hal::TextureViewDescriptor {
                label: Some(label),
                format: wgt::TextureFormat::Rgba8Unorm,
                dimension: wgt::TextureViewDimension::D2,
                usage: wgt::TextureUses::COLOR_TARGET,
                range: wgt::ImageSubresourceRange {
                    aspect: wgt::TextureAspect::All,
                    base_mip_level: 0,
                    mip_level_count: Some(1),
                    base_array_layer: 0,
                    array_layer_count: Some(1),
                },
            },
        )
    }
    .map_err(|e| format!("create_texture_view({label}): {e:?}"))
}

fn render_pass_descriptor<'a>(
    label: &'a str,
    extent: wgt::Extent3d,
    sample_count: u32,
    color_attachments: &'a [Option<wgpu_hal::ColorAttachment<'a, wgpu_hal::dx11::TextureView>>],
) -> wgpu_hal::RenderPassDescriptor<'a, wgpu_hal::dx11::Resource, wgpu_hal::dx11::TextureView> {
    wgpu_hal::RenderPassDescriptor {
        label: Some(label),
        extent,
        sample_count,
        color_attachments,
        depth_stencil_attachment: None,
        multiview_mask: None,
        timestamp_writes: None,
        occlusion_query_set: None,
    }
}

fn make_color_attachment<'a>(
    view: &'a wgpu_hal::dx11::TextureView,
    clear_value: wgt::Color,
) -> wgpu_hal::ColorAttachment<'a, wgpu_hal::dx11::TextureView> {
    wgpu_hal::ColorAttachment {
        target: wgpu_hal::Attachment {
            view,
            usage: wgt::TextureUses::COLOR_TARGET,
        },
        depth_slice: None,
        resolve_target: None,
        ops: wgpu_hal::AttachmentOps::LOAD_CLEAR | wgpu_hal::AttachmentOps::STORE,
        clear_value,
    }
}

fn write_buffer(
    device: &wgpu_hal::dx11::Device,
    label: &str,
    buffer: &wgpu_hal::dx11::Buffer,
    data: &[u8],
) -> Result<(), String> {
    let size = data.len() as u64;
    let mapping = unsafe { device.map_buffer(buffer, 0..size) }
        .map_err(|e| format!("map_buffer({label}, write): {e:?}"))?;
    unsafe {
        core::ptr::copy_nonoverlapping(data.as_ptr(), mapping.ptr.as_ptr(), data.len());
        device.unmap_buffer(buffer);
    }
    Ok(())
}

fn copy_buffer_region(
    device: &wgpu_hal::dx11::Device,
    queue: &wgpu_hal::dx11::Queue,
    label: &str,
    src: &wgpu_hal::dx11::Buffer,
    dst: &wgpu_hal::dx11::Buffer,
    src_offset: u64,
    dst_offset: u64,
    size: u64,
) -> Result<(), String> {
    let size = wgt::BufferSize::new(size)
        .ok_or_else(|| format!("copy_buffer_region({label}): size must be non-zero"))?;
    submit_encoder(device, queue, label, |encoder| {
        unsafe {
            encoder.copy_buffer_to_buffer(
                src,
                dst,
                [wgpu_hal::BufferCopy {
                    src_offset,
                    dst_offset,
                    size,
                }]
                .into_iter(),
            );
        }
        Ok(())
    })
}

fn submit_encoder<F>(
    device: &wgpu_hal::dx11::Device,
    queue: &wgpu_hal::dx11::Queue,
    label: &str,
    encode: F,
) -> Result<(), String>
where
    F: FnOnce(&mut wgpu_hal::dx11::CommandBuffer) -> Result<(), String>,
{
    let mut encoder = unsafe {
        device.create_command_encoder(&wgpu_hal::CommandEncoderDescriptor {
            label: Some(label),
            queue,
        })
    }
    .map_err(|e| format!("create_command_encoder({label}): {e:?}"))?;
    unsafe { encoder.begin_encoding(Some(label)) }
        .map_err(|e| format!("begin_encoding({label}): {e:?}"))?;

    if let Err(error) = encode(&mut encoder) {
        unsafe { encoder.discard_encoding() };
        return Err(error);
    }

    let command_buffer =
        unsafe { encoder.end_encoding() }.map_err(|e| format!("end_encoding({label}): {e:?}"))?;

    let fence =
        unsafe { device.create_fence() }.map_err(|e| format!("create_fence({label}): {e:?}"))?;
    if let Err(error) = unsafe { queue.submit(&[&command_buffer], &[], (&fence, 1)) } {
        unsafe { device.destroy_fence(fence) };
        return Err(format!("queue.submit({label}): {error:?}"));
    }
    let reached = unsafe { device.wait(&fence, 1, None) };
    unsafe { device.destroy_fence(fence) };
    if !reached.map_err(|e| format!("device.wait({label}): {e:?}"))? {
        return Err(format!(
            "D3D11 fence did not reach the submitted value for '{label}'"
        ));
    }
    Ok(())
}

fn read_texture(
    device: &wgpu_hal::dx11::Device,
    queue: &wgpu_hal::dx11::Queue,
    label: &str,
    texture: &wgpu_hal::dx11::Texture,
    width: u32,
    height: u32,
    bytes_per_row: u32,
) -> Result<Vec<u8>, String> {
    let size = u64::from(bytes_per_row) * u64::from(height);
    let readback = create_buffer(
        device,
        label,
        size,
        wgt::BufferUses::MAP_READ | wgt::BufferUses::COPY_DST,
    )?;
    submit_encoder(device, queue, label, |encoder| {
        unsafe {
            encoder.copy_texture_to_buffer(
                texture,
                wgt::TextureUses::COPY_SRC,
                &readback,
                [wgpu_hal::BufferTextureCopy {
                    buffer_layout: wgt::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(bytes_per_row),
                        rows_per_image: Some(height),
                    },
                    texture_base: wgpu_hal::TextureCopyBase {
                        mip_level: 0,
                        array_layer: 0,
                        origin: wgt::Origin3d::ZERO,
                        aspect: wgpu_hal::FormatAspects::COLOR,
                    },
                    size: wgpu_hal::CopyExtent {
                        width,
                        height,
                        depth: 1,
                    },
                }]
                .into_iter(),
            );
        }
        Ok(())
    })?;
    let data = map_read(device, &readback, size)?;
    unsafe { device.destroy_buffer(readback) };
    Ok(data)
}

fn map_read(
    device: &wgpu_hal::dx11::Device,
    buffer: &wgpu_hal::dx11::Buffer,
    size: u64,
) -> Result<Vec<u8>, String> {
    let mapping = unsafe { device.map_buffer(buffer, 0..size) }
        .map_err(|e| format!("map_buffer(read {size} bytes): {e:?}"))?;
    let data = unsafe { core::slice::from_raw_parts(mapping.ptr.as_ptr(), size as usize) };
    let data = data.to_vec();
    unsafe { device.unmap_buffer(buffer) };
    Ok(data)
}

fn pixel(data: &[u8], x: u32, y: u32) -> Result<[u8; 4], String> {
    let offset = (y * PITCH + x * 4) as usize;
    let bytes = data
        .get(offset..offset + 4)
        .ok_or_else(|| format!("read-back buffer is too short for pixel ({x}, {y})"))?;
    Ok([bytes[0], bytes[1], bytes[2], bytes[3]])
}

fn expect_pixel(
    context: &str,
    data: &[u8],
    x: u32,
    y: u32,
    expected: [u8; 4],
) -> Result<(), String> {
    let actual = pixel(data, x, y)?;
    if actual != expected {
        return Err(format!(
            "{context}: pixel ({x}, {y}) is {actual:?}, expected {expected:?}"
        ));
    }
    println!("  {context}: pixel ({x}, {y}) = {actual:?}");
    Ok(())
}

fn expect_err<T, E: std::fmt::Display>(
    description: &str,
    result: Result<T, E>,
) -> Result<(), String> {
    match result {
        Ok(_) => Err(format!(
            "{description}: expected an explicit error, but the call succeeded"
        )),
        Err(error) => {
            println!("  rejected as expected: {description} ({error})");
            Ok(())
        }
    }
}

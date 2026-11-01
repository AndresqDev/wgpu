//! Descriptor-layout coverage for the experimental Direct3D 11 backend.
//!
//! This test is the executable form of the descriptor part of the P3 packet in
//! `dx11-task-packets.md` and sections 4, 5, 7 and 8 of the Cinnabar DX11
//! capability contract. It opens the HAL without a window, builds bind group
//! and pipeline layouts for the supported uniform/texture/sampler subset,
//! checks the flattened `b`/`t`/`s` registers, and verifies that descriptor
//! features the backend does not implement fail with an explicit error instead
//! of being accepted and silently dropped.
//!
//! The test uses `Features::empty()` and the adapter's own limits, so it runs
//! unchanged on WARP and on hardware.
//!
//! Run on Windows CI with:
//!
//! ```text
//! cargo test -p wgpu-hal --features dx11 --test dx11_descriptors -- --nocapture
//! ```

#![cfg(all(target_os = "windows", feature = "dx11"))]

use std::num::NonZeroU32;

use wgpu_hal::{Adapter as _, CommandEncoder as _, Device as _, Instance as _};
use wgpu_types as wgt;

#[test]
fn dx11_descriptors() {
    if let Err(error) = run() {
        panic!("DX11 descriptors test failed: {error}");
    }
    println!("dx11_descriptors: all descriptor sections passed");
}

fn run() -> Result<(), String> {
    let instance_desc = wgpu_hal::InstanceDescriptor {
        name: "wgpu-hal-dx11-descriptors",
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
        "dx11_descriptors: enumerated {} DXGI adapter(s)",
        adapters.len()
    );
    if adapters.is_empty() {
        return Err("no DXGI/D3D11 adapters were found".to_owned());
    }

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
    println!(
        "opened D3D11 device on '{}' with Features::empty()",
        adapters[adapter_index].info.name
    );
    let limits = adapters[adapter_index].capabilities.limits.clone();

    section_positive_static(&device, &queue, &limits)?;
    section_positive_dynamic(&device, &queue, &limits)?;
    section_negative_layouts(&device, &limits)?;
    section_negative_limits(&device, &limits)?;
    section_invalid_dynamic_offsets(&device, &queue, &limits)?;
    section_immediates(&device, &queue, &limits)?;

    println!("dx11_descriptors: destroying the queue and device");
    drop(queue);
    drop(device);
    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn uniform_entry(
    binding: u32,
    visibility: wgt::ShaderStages,
    has_dynamic_offset: bool,
) -> wgt::BindGroupLayoutEntry {
    wgt::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgt::BindingType::Buffer {
            ty: wgt::BufferBindingType::Uniform,
            has_dynamic_offset,
            min_binding_size: None,
        },
        count: None,
    }
}

fn storage_buffer_entry(binding: u32, read_only: bool) -> wgt::BindGroupLayoutEntry {
    wgt::BindGroupLayoutEntry {
        binding,
        visibility: wgt::ShaderStages::FRAGMENT,
        ty: wgt::BindingType::Buffer {
            ty: wgt::BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn texture_entry(binding: u32, visibility: wgt::ShaderStages) -> wgt::BindGroupLayoutEntry {
    wgt::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgt::BindingType::Texture {
            sample_type: wgt::TextureSampleType::Float { filterable: true },
            view_dimension: wgt::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

fn sampler_entry(binding: u32, visibility: wgt::ShaderStages) -> wgt::BindGroupLayoutEntry {
    wgt::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgt::BindingType::Sampler(wgt::SamplerBindingType::Filtering),
        count: None,
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
    .map_err(|e| format!("create_bind_group_layout({label}): {e}"))
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
        Err(_) => Ok(()),
    }
}

fn create_pipeline(
    device: &wgpu_hal::dx11::Device,
    label: &str,
    layouts: &[&wgpu_hal::dx11::BindGroupLayout],
    immediate_size: u32,
) -> Result<wgpu_hal::dx11::PipelineLayout, String> {
    let layout_options: Vec<_> = layouts.iter().map(|layout| Some(*layout)).collect();
    unsafe {
        device.create_pipeline_layout(&wgpu_hal::PipelineLayoutDescriptor {
            label: Some(label),
            flags: wgpu_hal::PipelineLayoutFlags::empty(),
            bind_group_layouts: &layout_options,
            immediate_size,
        })
    }
    .map_err(|e| format!("create_pipeline_layout({label}): {e}"))
}

fn expect_pipeline_error(
    device: &wgpu_hal::dx11::Device,
    label: &str,
    layouts: &[&wgpu_hal::dx11::BindGroupLayout],
    immediate_size: u32,
) -> Result<(), String> {
    match create_pipeline(device, label, layouts, immediate_size) {
        Ok(_) => Err(format!(
            "{label}: create_pipeline_layout unexpectedly succeeded"
        )),
        Err(_) => Ok(()),
    }
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
    .map_err(|e| format!("create_buffer({label}): {e}"))
}

fn create_texture(
    device: &wgpu_hal::dx11::Device,
    label: &str,
) -> Result<wgpu_hal::dx11::Texture, String> {
    unsafe {
        device.create_texture(&wgpu_hal::TextureDescriptor {
            label: Some(label),
            size: wgt::Extent3d {
                width: 4,
                height: 4,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgt::TextureDimension::D2,
            format: wgt::TextureFormat::Rgba8Unorm,
            usage: wgt::TextureUses::RESOURCE,
            memory_flags: wgpu_hal::MemoryFlags::empty(),
            view_formats: Vec::new(),
        })
    }
    .map_err(|e| format!("create_texture({label}): {e}"))
}

fn create_srv(
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
                usage: wgt::TextureUses::RESOURCE,
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
    .map_err(|e| format!("create_texture_view({label}): {e}"))
}

fn create_sampler(
    device: &wgpu_hal::dx11::Device,
    label: &str,
) -> Result<wgpu_hal::dx11::Sampler, String> {
    unsafe {
        device.create_sampler(&wgpu_hal::SamplerDescriptor {
            label: Some(label),
            address_modes: [wgt::AddressMode::ClampToEdge; 3],
            mag_filter: wgt::FilterMode::Linear,
            min_filter: wgt::FilterMode::Linear,
            mipmap_filter: wgt::MipmapFilterMode::Nearest,
            lod_clamp: 0.0..1.0,
            compare: None,
            anisotropy_clamp: 1,
            border_color: None,
        })
    }
    .map_err(|e| format!("create_sampler({label}): {e}"))
}

fn create_three_binding_group(
    device: &wgpu_hal::dx11::Device,
    label: &str,
    layout: &wgpu_hal::dx11::BindGroupLayout,
    buffer: &wgpu_hal::dx11::Buffer,
    buffer_size: u64,
    view: &wgpu_hal::dx11::TextureView,
    sampler: &wgpu_hal::dx11::Sampler,
) -> Result<wgpu_hal::dx11::BindGroup, String> {
    let buffer_binding =
        wgpu_hal::BufferBinding::new_unchecked(buffer, 0, wgt::BufferSize::new(buffer_size));
    let texture_binding = wgpu_hal::TextureBinding {
        view,
        usage: wgt::TextureUses::RESOURCE,
    };
    let samplers = [sampler];
    let entries = [
        wgpu_hal::BindGroupEntry {
            binding: 0,
            resource_index: 0,
            count: 1,
        },
        wgpu_hal::BindGroupEntry {
            binding: 1,
            resource_index: 0,
            count: 1,
        },
        wgpu_hal::BindGroupEntry {
            binding: 2,
            resource_index: 0,
            count: 1,
        },
    ];
    unsafe {
        device.create_bind_group(&wgpu_hal::BindGroupDescriptor {
            label: Some(label),
            layout,
            buffers: &[buffer_binding],
            samplers: &samplers,
            textures: &[texture_binding],
            entries: &entries,
            acceleration_structures: &[],
            external_textures: &[],
        })
    }
    .map_err(|e| format!("create_bind_group({label}): {e}"))
}

fn create_uniform_group(
    device: &wgpu_hal::dx11::Device,
    label: &str,
    layout: &wgpu_hal::dx11::BindGroupLayout,
    buffer: &wgpu_hal::dx11::Buffer,
    offset: u64,
    size: u64,
) -> Result<wgpu_hal::dx11::BindGroup, String> {
    let buffer_binding =
        wgpu_hal::BufferBinding::new_unchecked(buffer, offset, wgt::BufferSize::new(size));
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
    .map_err(|e| format!("create_bind_group({label}): {e}"))
}

fn create_encoder(
    device: &wgpu_hal::dx11::Device,
    queue: &wgpu_hal::dx11::Queue,
    label: &str,
) -> Result<wgpu_hal::dx11::CommandBuffer, String> {
    let mut encoder = unsafe {
        device.create_command_encoder(&wgpu_hal::CommandEncoderDescriptor {
            label: Some(label),
            queue,
        })
    }
    .map_err(|e| format!("create_command_encoder({label}): {e}"))?;
    unsafe { encoder.begin_encoding(Some(label)) }
        .map_err(|e| format!("begin_encoding({label}): {e}"))?;
    Ok(encoder)
}

fn expect_encoder_error<F>(
    device: &wgpu_hal::dx11::Device,
    queue: &wgpu_hal::dx11::Queue,
    label: &str,
    encode: F,
) -> Result<(), String>
where
    F: FnOnce(&mut wgpu_hal::dx11::CommandBuffer),
{
    let mut encoder = create_encoder(device, queue, label)?;
    encode(&mut encoder);
    match unsafe { encoder.end_encoding() } {
        Ok(_) => Err(format!(
            "{label}: end_encoding unexpectedly succeeded after an invalid descriptor command"
        )),
        Err(_) => Ok(()),
    }
}

// ---------------------------------------------------------------------------
// Positive paths
// ---------------------------------------------------------------------------

fn section_positive_static(
    device: &wgpu_hal::dx11::Device,
    queue: &wgpu_hal::dx11::Queue,
    _limits: &wgt::Limits,
) -> Result<(), String> {
    println!("\n=== 1. static UBO + texture + sampler ===");

    let entries = [
        uniform_entry(0, wgt::ShaderStages::VERTEX_FRAGMENT, false),
        texture_entry(1, wgt::ShaderStages::FRAGMENT),
        sampler_entry(2, wgt::ShaderStages::FRAGMENT),
    ];
    let bind_group_layout = create_layout(device, "static descriptors", &entries)?;
    let pipeline_layout = create_pipeline(
        device,
        "static descriptors",
        &[&bind_group_layout, &bind_group_layout],
        0,
    )?;

    for group in 0..2u32 {
        let target = pipeline_layout
            .bind_target(group, 0)
            .ok_or_else(|| format!("group {group} binding 0 has no register"))?;
        if target.space != 0 || target.register != group {
            return Err(format!(
                "group {group} binding 0 mapped to space {} register {}, expected b{group}",
                target.space, target.register
            ));
        }
        if target.binding_array_size.is_some() || target.restrict_indexing {
            return Err(format!(
                "group {group} binding 0 is marked as a binding array"
            ));
        }

        let target = pipeline_layout
            .bind_target(group, 1)
            .ok_or_else(|| format!("group {group} binding 1 has no register"))?;
        if target.space != 0 || target.register != group {
            return Err(format!(
                "group {group} binding 1 mapped to space {} register {}, expected t{group}",
                target.space, target.register
            ));
        }
        if target.binding_array_size.is_some() || target.restrict_indexing {
            return Err(format!(
                "group {group} binding 1 is marked as a binding array"
            ));
        }

        let target = pipeline_layout
            .bind_target(group, 2)
            .ok_or_else(|| format!("group {group} binding 2 has no register"))?;
        if target.space != 0 || target.register != group {
            return Err(format!(
                "group {group} binding 2 mapped to space {} register {}, expected s{group}",
                target.space, target.register
            ));
        }
        if target.binding_array_size.is_some() || target.restrict_indexing {
            return Err(format!(
                "group {group} binding 2 is marked as a binding array"
            ));
        }
    }

    if pipeline_layout.naga_options().sampler_binding_mode
        != naga::back::hlsl::SamplerBindingMode::Direct
    {
        return Err("pipeline layout did not select direct sampler bindings".to_owned());
    }
    if pipeline_layout.naga_options().fake_missing_bindings {
        return Err("pipeline layout enabled fake missing bindings".to_owned());
    }
    if pipeline_layout.naga_options().immediates_target.is_some() {
        return Err("pipeline layout unexpectedly reserved an immediates target".to_owned());
    }
    if pipeline_layout.immediate_size() != 0 {
        return Err("pipeline layout unexpectedly has immediates".to_owned());
    }

    let buffer0 = create_buffer(
        device,
        "static UBO 0",
        256,
        wgt::BufferUses::UNIFORM | wgt::BufferUses::COPY_DST,
    )?;
    let texture0 = create_texture(device, "static texture 0")?;
    let view0 = create_srv(device, &texture0, "static SRV 0")?;
    let sampler0 = create_sampler(device, "static sampler 0")?;
    let group0 = create_three_binding_group(
        device,
        "static group 0",
        &bind_group_layout,
        &buffer0,
        256,
        &view0,
        &sampler0,
    )?;

    let buffer1 = create_buffer(
        device,
        "static UBO 1",
        256,
        wgt::BufferUses::UNIFORM | wgt::BufferUses::COPY_DST,
    )?;
    let texture1 = create_texture(device, "static texture 1")?;
    let view1 = create_srv(device, &texture1, "static SRV 1")?;
    let sampler1 = create_sampler(device, "static sampler 1")?;
    let group1 = create_three_binding_group(
        device,
        "static group 1",
        &bind_group_layout,
        &buffer1,
        256,
        &view1,
        &sampler1,
    )?;

    pipeline_layout
        .validate_bind_group(0, &group0, &[], true)
        .map_err(|e| format!("validate_bind_group(static group 0): {e}"))?;
    pipeline_layout
        .validate_bind_group(1, &group1, &[], true)
        .map_err(|e| format!("validate_bind_group(static group 1): {e}"))?;

    let mut encoder = create_encoder(device, queue, "static descriptors")?;
    unsafe {
        encoder.set_bind_group(&pipeline_layout, 0, &group0, &[]);
        encoder.set_bind_group(&pipeline_layout, 1, &group1, &[]);
    }
    unsafe { encoder.end_encoding() }
        .map_err(|e| format!("end_encoding(static descriptors): {e}"))?;
    println!("static descriptors: bound b0/t0/s0 and b1/t1/s1");
    Ok(())
}

fn section_positive_dynamic(
    device: &wgpu_hal::dx11::Device,
    queue: &wgpu_hal::dx11::Queue,
    _limits: &wgt::Limits,
) -> Result<(), String> {
    println!("\n=== 2. dynamic uniform buffer ===");

    let entries = [uniform_entry(0, wgt::ShaderStages::VERTEX_FRAGMENT, true)];
    let bind_group_layout = create_layout(device, "dynamic UBO", &entries)?;
    let pipeline_layout = create_pipeline(device, "dynamic UBO", &[&bind_group_layout], 0)?;

    let buffer = create_buffer(
        device,
        "dynamic UBO",
        512,
        wgt::BufferUses::UNIFORM | wgt::BufferUses::COPY_DST,
    )?;
    let group = create_uniform_group(device, "dynamic group", &bind_group_layout, &buffer, 0, 256)?;

    pipeline_layout
        .validate_bind_group(0, &group, &[0], true)
        .map_err(|e| format!("validate_bind_group(dynamic offset 0): {e}"))?;
    pipeline_layout
        .validate_bind_group(0, &group, &[256], true)
        .map_err(|e| format!("validate_bind_group(dynamic offset 256): {e}"))?;

    let mut encoder = create_encoder(device, queue, "dynamic UBO")?;
    unsafe {
        encoder.set_bind_group(&pipeline_layout, 0, &group, &[0]);
        encoder.set_bind_group(&pipeline_layout, 0, &group, &[256]);
    }
    unsafe { encoder.end_encoding() }.map_err(|e| format!("end_encoding(dynamic UBO): {e}"))?;
    println!("dynamic UBO: offsets 0 and 256 accepted");
    Ok(())
}

// ---------------------------------------------------------------------------
// Negative layout features
// ---------------------------------------------------------------------------

fn section_negative_layouts(
    device: &wgpu_hal::dx11::Device,
    _limits: &wgt::Limits,
) -> Result<(), String> {
    println!("\n=== 3. unsupported binding types and dimensions ===");

    expect_layout_error(
        device,
        "storage buffer read-only",
        &[storage_buffer_entry(0, true)],
    )?;
    expect_layout_error(
        device,
        "storage buffer read-write",
        &[storage_buffer_entry(0, false)],
    )?;

    expect_layout_error(
        device,
        "storage texture",
        &[wgt::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgt::ShaderStages::FRAGMENT,
            ty: wgt::BindingType::StorageTexture {
                access: wgt::StorageTextureAccess::WriteOnly,
                format: wgt::TextureFormat::Rgba8Unorm,
                view_dimension: wgt::TextureViewDimension::D2,
            },
            count: None,
        }],
    )?;

    expect_layout_error(
        device,
        "acceleration structure",
        &[wgt::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgt::ShaderStages::FRAGMENT,
            ty: wgt::BindingType::AccelerationStructure {
                vertex_return: false,
            },
            count: None,
        }],
    )?;

    expect_layout_error(
        device,
        "external texture",
        &[wgt::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgt::ShaderStages::FRAGMENT,
            ty: wgt::BindingType::ExternalTexture,
            count: None,
        }],
    )?;

    expect_layout_error(
        device,
        "multisampled texture",
        &[wgt::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgt::ShaderStages::FRAGMENT,
            ty: wgt::BindingType::Texture {
                sample_type: wgt::TextureSampleType::Float { filterable: true },
                view_dimension: wgt::TextureViewDimension::D2,
                multisampled: true,
            },
            count: None,
        }],
    )?;

    for dimension in [wgt::TextureViewDimension::D1, wgt::TextureViewDimension::D3] {
        expect_layout_error(
            device,
            "D1/D3 texture view",
            &[wgt::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgt::ShaderStages::FRAGMENT,
                ty: wgt::BindingType::Texture {
                    sample_type: wgt::TextureSampleType::Float { filterable: true },
                    view_dimension: dimension,
                    multisampled: false,
                },
                count: None,
            }],
        )?;
    }

    let count_two = NonZeroU32::new(2).unwrap();
    expect_layout_error(
        device,
        "uniform buffer binding array",
        &[wgt::BindGroupLayoutEntry {
            count: Some(count_two),
            ..uniform_entry(0, wgt::ShaderStages::VERTEX, false)
        }],
    )?;
    expect_layout_error(
        device,
        "texture binding array",
        &[wgt::BindGroupLayoutEntry {
            count: Some(count_two),
            ..texture_entry(0, wgt::ShaderStages::FRAGMENT)
        }],
    )?;
    expect_layout_error(
        device,
        "sampler binding array",
        &[wgt::BindGroupLayoutEntry {
            count: Some(count_two),
            ..sampler_entry(0, wgt::ShaderStages::FRAGMENT)
        }],
    )?;

    println!("unsupported binding types, dimensions and arrays rejected");
    Ok(())
}

fn section_negative_limits(
    device: &wgpu_hal::dx11::Device,
    limits: &wgt::Limits,
) -> Result<(), String> {
    println!("\n=== 4. layout limits ===");

    let empty_layout = create_layout(device, "empty", &[])?;

    expect_pipeline_error(
        device,
        "five bind groups",
        &[
            &empty_layout,
            &empty_layout,
            &empty_layout,
            &empty_layout,
            &empty_layout,
        ],
        0,
    )?;

    expect_pipeline_error(device, "push constants", &[&empty_layout], 4)?;

    let thirteen_uniforms: Vec<_> = (0..13)
        .map(|binding| uniform_entry(binding, wgt::ShaderStages::VERTEX, false))
        .collect();
    expect_layout_error(device, "13 uniform buffers", &thirteen_uniforms)?;

    let seventeen_textures: Vec<_> = (0..17)
        .map(|binding| texture_entry(binding, wgt::ShaderStages::FRAGMENT))
        .collect();
    expect_layout_error(device, "17 sampled textures", &seventeen_textures)?;

    let seventeen_samplers: Vec<_> = (0..17)
        .map(|binding| sampler_entry(binding, wgt::ShaderStages::FRAGMENT))
        .collect();
    expect_layout_error(device, "17 samplers", &seventeen_samplers)?;

    // The contract advertises eight dynamic uniform buffers per pipeline
    // layout; nine must be rejected.
    if limits.max_dynamic_uniform_buffers_per_pipeline_layout >= 9 {
        return Err(format!(
            "adapter advertises {} dynamic uniform buffers, expected the contract value 8",
            limits.max_dynamic_uniform_buffers_per_pipeline_layout
        ));
    }
    let dynamic_entries: Vec<_> = (0..9)
        .map(|binding| uniform_entry(binding, wgt::ShaderStages::VERTEX, true))
        .collect();
    let dynamic_layout = create_layout(device, "9 dynamic uniforms", &dynamic_entries)?;
    expect_pipeline_error(device, "9 dynamic uniforms", &[&dynamic_layout], 0)?;

    println!("bind group count, immediates and binding limits rejected");
    Ok(())
}

// ---------------------------------------------------------------------------
// Dynamic offset and immediates failures
// ---------------------------------------------------------------------------

fn dynamic_fixture(
    device: &wgpu_hal::dx11::Device,
    label: &str,
) -> Result<
    (
        wgpu_hal::dx11::PipelineLayout,
        wgpu_hal::dx11::BindGroup,
        wgpu_hal::dx11::Buffer,
    ),
    String,
> {
    let entries = [uniform_entry(0, wgt::ShaderStages::VERTEX_FRAGMENT, true)];
    let bind_group_layout = create_layout(device, label, &entries)?;
    let pipeline_layout = create_pipeline(device, label, &[&bind_group_layout], 0)?;
    let buffer = create_buffer(
        device,
        label,
        512,
        wgt::BufferUses::UNIFORM | wgt::BufferUses::COPY_DST,
    )?;
    let group = create_uniform_group(device, label, &bind_group_layout, &buffer, 0, 256)?;
    Ok((pipeline_layout, group, buffer))
}

fn section_invalid_dynamic_offsets(
    device: &wgpu_hal::dx11::Device,
    queue: &wgpu_hal::dx11::Queue,
    _limits: &wgt::Limits,
) -> Result<(), String> {
    println!("\n=== 5. invalid dynamic offsets ===");

    let (pipeline_layout, group, _buffer) = dynamic_fixture(device, "invalid dynamic offsets")?;

    for (label, offsets) in [
        ("dynamic offset 128", [128u32]),
        ("dynamic offset outside the buffer", [512u32]),
    ] {
        match pipeline_layout.validate_bind_group(0, &group, &offsets, true) {
            Ok(()) => {
                return Err(format!(
                    "{label}: validate_bind_group unexpectedly succeeded"
                ));
            }
            Err(error) => println!("  {label}: {error}"),
        }

        expect_encoder_error(device, queue, label, |encoder| unsafe {
            encoder.set_bind_group(&pipeline_layout, 0, &group, &offsets);
        })?;
    }

    println!("invalid dynamic offsets recorded an encoder error");
    Ok(())
}

fn section_immediates(
    device: &wgpu_hal::dx11::Device,
    queue: &wgpu_hal::dx11::Queue,
    _limits: &wgt::Limits,
) -> Result<(), String> {
    println!("\n=== 6. push constants ===");

    let empty_layout = create_layout(device, "immediates", &[])?;
    let pipeline_layout = create_pipeline(device, "immediates", &[&empty_layout], 0)?;

    expect_encoder_error(device, queue, "set_immediates", |encoder| unsafe {
        encoder.set_immediates(&pipeline_layout, 0, &[0]);
    })?;

    println!("set_immediates recorded an encoder error");
    Ok(())
}

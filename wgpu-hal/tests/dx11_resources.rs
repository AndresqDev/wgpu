//! Resource coverage for the experimental Direct3D 11 backend.
//!
//! This test is the executable form of the resource part of the P2 packet in
//! `dx11-task-packets.md` and of sections 6-8 of the Cinnabar DX11 capability
//! contract. It creates real D3D11 buffers, textures, views, samplers and
//! fences through the `wgpu-hal` API, exercises buffer/texture copies with
//! non-trivial offsets and row pitches, verifies exact read-back, and checks
//! that unsupported resource requests fail with an explicit error instead of
//! being silently accepted.
//!
//! The test uses `Features::empty()` and the adapter's own limits, so it runs
//! unchanged on WARP and on hardware. It needs a Windows session because the
//! D3D11 backend is Windows-only; it does not create a window.
//!
//! Run on Windows CI with:
//!
//! ```text
//! cargo test -p wgpu-hal --features dx11 --test dx11_resources -- --nocapture
//! ```

#![cfg(all(target_os = "windows", feature = "dx11"))]

use wgpu_hal::{
    Adapter as _, CommandEncoder as _, Device as _, Instance as _, Queue as _,
    TextureFormatCapabilities as Tfc,
};
use wgpu_types as wgt;

/// Row pitch used by every texture upload/read-back in this test. It is larger
/// than the visible row bytes so a backend that ignores `bytes_per_row` reads
/// or writes the wrong texels.
const PITCH: u32 = 256;

/// Contract §6.1: the mapped texture format allowlist.
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

#[test]
fn dx11_resources() {
    if let Err(error) = run() {
        panic!("DX11 resources test failed: {error}");
    }
    println!("dx11_resources: all resource sections passed");
}

fn run() -> Result<(), String> {
    let instance_desc = wgpu_hal::InstanceDescriptor {
        name: "wgpu-hal-dx11-resources",
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
        "dx11_resources: enumerated {} DXGI adapter(s)",
        adapters.len()
    );
    if adapters.is_empty() {
        return Err("no DXGI/D3D11 adapters were found".to_owned());
    }

    // WARP is the only adapter in CI. Try every enumerated adapter and use the
    // first one that can open a device, so a broken software adapter does not
    // mask a usable one.
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
    let adapter = &adapters[adapter_index].adapter;
    let downlevel_flags = adapters[adapter_index].capabilities.downlevel.flags;
    println!(
        "opened D3D11 device on '{}' with Features::empty()",
        adapters[adapter_index].info.name
    );

    section_buffers(&device)?;
    section_mapping(&device)?;
    section_buffer_copies(&device, &queue)?;
    section_texture_2d(&device, &queue)?;
    section_texture_arrays(&device, &queue)?;
    section_cubemaps(&device, &queue)?;
    section_format_matrix(adapter, &device)?;
    section_view_negatives(&device)?;
    section_samplers(&device, downlevel_flags)?;

    println!("dx11_resources: destroying the queue and device");
    drop(queue);
    drop(device);
    Ok(())
}

// ---------------------------------------------------------------------------
// 1. Buffers
// ---------------------------------------------------------------------------

fn section_buffers(device: &wgpu_hal::dx11::Device) -> Result<(), String> {
    println!("\n=== 1. buffer creation ===");
    const SIZE: u64 = 64;

    let cases: [(&str, wgt::BufferUses); 10] = [
        (
            "vertex + COPY_DST",
            wgt::BufferUses::VERTEX | wgt::BufferUses::COPY_DST,
        ),
        (
            "index + COPY_DST",
            wgt::BufferUses::INDEX | wgt::BufferUses::COPY_DST,
        ),
        (
            "uniform + COPY_DST",
            wgt::BufferUses::UNIFORM | wgt::BufferUses::COPY_DST,
        ),
        (
            "COPY_SRC + COPY_DST",
            wgt::BufferUses::COPY_SRC | wgt::BufferUses::COPY_DST,
        ),
        (
            "indirect + COPY_DST",
            wgt::BufferUses::INDIRECT | wgt::BufferUses::COPY_DST,
        ),
        (
            "MAP_READ + COPY_DST",
            wgt::BufferUses::MAP_READ | wgt::BufferUses::COPY_DST,
        ),
        (
            "MAP_WRITE + COPY_SRC",
            wgt::BufferUses::MAP_WRITE | wgt::BufferUses::COPY_SRC,
        ),
        (
            "dynamic MAP_WRITE + VERTEX",
            wgt::BufferUses::MAP_WRITE | wgt::BufferUses::VERTEX,
        ),
        (
            "storage read-only + COPY_DST",
            wgt::BufferUses::STORAGE_READ_ONLY | wgt::BufferUses::COPY_DST,
        ),
        (
            "storage read-only + storage read-write + COPY_DST",
            wgt::BufferUses::STORAGE_READ_ONLY
                | wgt::BufferUses::STORAGE_READ_WRITE
                | wgt::BufferUses::COPY_DST,
        ),
    ];

    let mut buffers = Vec::with_capacity(cases.len());
    for (label, usage) in cases {
        let buffer = create_buffer(device, label, SIZE, usage)?;
        if buffer.size() != SIZE {
            return Err(format!(
                "buffer '{label}' reports size {} instead of {SIZE}",
                buffer.size()
            ));
        }
        if buffer.usage() != usage {
            return Err(format!(
                "buffer '{label}' reports usage {:?} instead of {usage:?}",
                buffer.usage()
            ));
        }
        println!("  created {label} buffer ({SIZE} bytes)");
        buffers.push(buffer);
    }

    let negatives: [(&str, u64, wgt::BufferUses); 10] = [
        (
            "storage read-write",
            SIZE,
            wgt::BufferUses::STORAGE_READ_WRITE,
        ),
        (
            "storage read-only + MAP_WRITE",
            SIZE,
            wgt::BufferUses::STORAGE_READ_ONLY | wgt::BufferUses::MAP_WRITE,
        ),
        (
            "storage read-only + VERTEX",
            SIZE,
            wgt::BufferUses::STORAGE_READ_ONLY | wgt::BufferUses::VERTEX,
        ),
        (
            "storage read-only + UNIFORM",
            SIZE,
            wgt::BufferUses::STORAGE_READ_ONLY | wgt::BufferUses::UNIFORM,
        ),
        (
            "storage read-only + INDIRECT",
            SIZE,
            wgt::BufferUses::STORAGE_READ_ONLY | wgt::BufferUses::INDIRECT,
        ),
        (
            "MAP_READ + VERTEX",
            SIZE,
            wgt::BufferUses::MAP_READ | wgt::BufferUses::VERTEX,
        ),
        (
            "UNIFORM + VERTEX",
            SIZE,
            wgt::BufferUses::UNIFORM | wgt::BufferUses::VERTEX,
        ),
        (
            "INDIRECT + MAP_WRITE",
            SIZE,
            wgt::BufferUses::INDIRECT | wgt::BufferUses::MAP_WRITE,
        ),
        (
            "size above u32::MAX",
            u64::from(u32::MAX) + 1,
            wgt::BufferUses::COPY_SRC | wgt::BufferUses::COPY_DST,
        ),
        // A constant buffer is capped by D3D11 at 64 KiB.
        (
            "uniform above 65536 bytes",
            65_536 + 16,
            wgt::BufferUses::UNIFORM | wgt::BufferUses::COPY_DST,
        ),
    ];

    for (label, size, usage) in negatives {
        expect_err(
            &format!("create_buffer({label}, {size} bytes)"),
            create_buffer(device, label, size, usage),
        )?;
    }

    for buffer in buffers {
        unsafe { device.destroy_buffer(buffer) };
    }
    println!("  buffers: positives and negatives OK");
    Ok(())
}

// ---------------------------------------------------------------------------
// 2. Mapping
// ---------------------------------------------------------------------------

fn section_mapping(device: &wgpu_hal::dx11::Device) -> Result<(), String> {
    println!("\n=== 2. buffer mapping ===");
    const SIZE: u64 = 64;
    let pattern = pattern_bytes(SIZE as usize);

    let staging = create_buffer(
        device,
        "mapping staging",
        SIZE,
        wgt::BufferUses::MAP_READ
            | wgt::BufferUses::MAP_WRITE
            | wgt::BufferUses::COPY_SRC
            | wgt::BufferUses::COPY_DST,
    )?;

    {
        let mapping = unsafe { device.map_buffer(&staging, 0..SIZE) }
            .map_err(|e| format!("map_buffer(staging, write): {e}"))?;
        expect_err("map_buffer(staging, while already mapped)", unsafe {
            device.map_buffer(&staging, 0..SIZE)
        })?;
        unsafe {
            core::ptr::copy_nonoverlapping(pattern.as_ptr(), mapping.ptr.as_ptr(), pattern.len());
            device.unmap_buffer(&staging);
        }
    }
    println!("  staging write through map_buffer succeeded");

    let read_back = map_read(device, &staging, SIZE)?;
    if read_back != pattern {
        return Err(format!(
            "staging map write/read mismatch: got {read_back:?}, expected {pattern:?}"
        ));
    }
    println!("  staging read-back through map_buffer matched the written bytes");

    expect_err("map_buffer(size..size + 1)", unsafe {
        device.map_buffer(&staging, SIZE..SIZE + 1)
    })?;
    expect_err("map_buffer(0..size + 1)", unsafe {
        device.map_buffer(&staging, 0..SIZE + 1)
    })?;
    expect_err("map_buffer(8..8)", unsafe {
        device.map_buffer(&staging, 8..8)
    })?;

    let gpu_only = create_buffer(
        device,
        "mapping GPU-only",
        SIZE,
        wgt::BufferUses::COPY_SRC | wgt::BufferUses::COPY_DST,
    )?;
    expect_err("map_buffer(GPU-only buffer)", unsafe {
        device.map_buffer(&gpu_only, 0..SIZE)
    })?;

    let dynamic = create_buffer(
        device,
        "mapping dynamic vertex",
        SIZE,
        wgt::BufferUses::MAP_WRITE | wgt::BufferUses::VERTEX,
    )?;
    {
        let mapping = unsafe { device.map_buffer(&dynamic, 0..SIZE) }
            .map_err(|e| format!("map_buffer(dynamic): {e}"))?;
        expect_err("map_buffer(dynamic while already mapped)", unsafe {
            device.map_buffer(&dynamic, 0..SIZE)
        })?;
        unsafe {
            core::ptr::copy_nonoverlapping(pattern.as_ptr(), mapping.ptr.as_ptr(), pattern.len());
            device.unmap_buffer(&dynamic);
        }
    }
    // A second mapping after `unmap_buffer` must work again.
    {
        let mapping = unsafe { device.map_buffer(&dynamic, 0..SIZE) }
            .map_err(|e| format!("map_buffer(dynamic, after unmap): {e}"))?;
        unsafe {
            core::ptr::copy_nonoverlapping(pattern.as_ptr(), mapping.ptr.as_ptr(), pattern.len());
            device.unmap_buffer(&dynamic);
        }
    }
    println!("  dynamic buffer remap after unmap succeeded");

    unsafe {
        device.destroy_buffer(dynamic);
        device.destroy_buffer(gpu_only);
        device.destroy_buffer(staging);
    }
    println!("  mapping: positive and negative cases OK");
    Ok(())
}

// ---------------------------------------------------------------------------
// 3. Buffer-to-buffer copies
// ---------------------------------------------------------------------------

fn section_buffer_copies(
    device: &wgpu_hal::dx11::Device,
    queue: &wgpu_hal::dx11::Queue,
) -> Result<(), String> {
    println!("\n=== 3. buffer-to-buffer copies ===");
    const SIZE: u64 = 64;
    let pattern = pattern_bytes(SIZE as usize);

    let upload = create_buffer(
        device,
        "copy upload",
        SIZE,
        wgt::BufferUses::MAP_WRITE | wgt::BufferUses::COPY_SRC,
    )?;
    write_buffer(device, &upload, &pattern)?;
    let middle = create_buffer(
        device,
        "copy middle",
        SIZE,
        wgt::BufferUses::COPY_SRC | wgt::BufferUses::COPY_DST,
    )?;
    let readback = create_buffer(
        device,
        "copy readback",
        SIZE,
        wgt::BufferUses::MAP_READ | wgt::BufferUses::COPY_DST,
    )?;

    copy_buffer_region(
        device,
        queue,
        "buffer copy with offsets",
        &upload,
        &middle,
        8,
        16,
        32,
    )?;
    // Same-size copies of whole buffers take the `CopyResource` path.
    copy_buffer_region(
        device,
        queue,
        "whole-buffer read-back",
        &middle,
        &readback,
        0,
        0,
        SIZE,
    )?;

    let data = map_read(device, &readback, SIZE)?;
    for index in 0..32 {
        let actual = data[16 + index];
        let expected = pattern[8 + index];
        if actual != expected {
            return Err(format!(
                "offset buffer copy mismatch at destination byte {}: got 0x{actual:02x}, \
                 expected 0x{expected:02x}",
                16 + index
            ));
        }
    }
    println!("  copy src_offset=8 -> dst_offset=16, size=32 verified");

    let full_pattern = pattern_bytes(SIZE as usize)
        .into_iter()
        .map(|byte| byte ^ 0xa5)
        .collect::<Vec<_>>();
    let full_upload = create_buffer(
        device,
        "full copy upload",
        SIZE,
        wgt::BufferUses::MAP_WRITE | wgt::BufferUses::COPY_SRC,
    )?;
    write_buffer(device, &full_upload, &full_pattern)?;
    let full_middle = create_buffer(
        device,
        "full copy middle",
        SIZE,
        wgt::BufferUses::COPY_SRC | wgt::BufferUses::COPY_DST,
    )?;
    let full_readback = create_buffer(
        device,
        "full copy readback",
        SIZE,
        wgt::BufferUses::MAP_READ | wgt::BufferUses::COPY_DST,
    )?;
    copy_buffer_region(
        device,
        queue,
        "whole-buffer upload",
        &full_upload,
        &full_middle,
        0,
        0,
        SIZE,
    )?;
    copy_buffer_region(
        device,
        queue,
        "whole-buffer read-back",
        &full_middle,
        &full_readback,
        0,
        0,
        SIZE,
    )?;
    let full_data = map_read(device, &full_readback, SIZE)?;
    if full_data != full_pattern {
        return Err(format!(
            "whole-buffer copy mismatch: got {full_data:?}, expected {full_pattern:?}"
        ));
    }
    println!("  whole-buffer CopyResource path verified");

    unsafe {
        device.destroy_buffer(full_readback);
        device.destroy_buffer(full_middle);
        device.destroy_buffer(full_upload);
        device.destroy_buffer(readback);
        device.destroy_buffer(middle);
        device.destroy_buffer(upload);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 4. 2D textures
// ---------------------------------------------------------------------------

fn section_texture_2d(
    device: &wgpu_hal::dx11::Device,
    queue: &wgpu_hal::dx11::Queue,
) -> Result<(), String> {
    println!("\n=== 4. 2D textures ===");

    let texture = create_texture(
        device,
        "2D color",
        wgt::TextureFormat::Rgba8Unorm,
        8,
        8,
        1,
        1,
        wgt::TextureUses::RESOURCE
            | wgt::TextureUses::COLOR_TARGET
            | wgt::TextureUses::COPY_SRC
            | wgt::TextureUses::COPY_DST,
    )?;
    let srv = create_view(
        device,
        &texture,
        "2D color SRV",
        wgt::TextureFormat::Rgba8Unorm,
        wgt::TextureViewDimension::D2,
        wgt::TextureUses::RESOURCE,
        full_range(),
    )?;
    let rtv = create_view(
        device,
        &texture,
        "2D color RTV",
        wgt::TextureFormat::Rgba8Unorm,
        wgt::TextureViewDimension::D2,
        wgt::TextureUses::COLOR_TARGET,
        full_range(),
    )?;
    if srv.raw_srv().is_none() {
        return Err("2D color RESOURCE view has no SRV".to_owned());
    }
    if rtv.raw_rtv().is_none() {
        return Err("2D color COLOR_TARGET view has no RTV".to_owned());
    }
    println!("  created 2D texture with SRV and RTV");

    // Upload 4x3 at (1, 2) from buffer offset 64 with a 256-byte row pitch.
    let upload_offset = 64usize;
    let mut upload_data = vec![0u8; upload_offset + (PITCH * 3) as usize];
    let expected = fill_rgba(&mut upload_data, upload_offset, PITCH, 4, 3, |x, y| {
        rgba_pattern(x + 1, y + 2)
    });
    let upload = create_buffer(
        device,
        "2D color upload",
        upload_data.len() as u64,
        wgt::BufferUses::MAP_WRITE | wgt::BufferUses::COPY_SRC,
    )?;
    write_buffer(device, &upload, &upload_data)?;

    upload_to_texture(
        device,
        queue,
        "2D color upload",
        &upload,
        &texture,
        upload_offset as u64,
        PITCH,
        3,
        0,
        0,
        wgt::Origin3d { x: 1, y: 2, z: 0 },
        4,
        3,
        wgpu_hal::FormatAspects::COLOR,
    )?;

    let readback_offset = 32usize;
    let readback_size = (readback_offset + (PITCH * 3) as usize) as u64;
    let readback = create_buffer(
        device,
        "2D color readback",
        readback_size,
        wgt::BufferUses::MAP_READ | wgt::BufferUses::COPY_DST,
    )?;
    download_from_texture(
        device,
        queue,
        "2D color read-back",
        &texture,
        &readback,
        readback_offset as u64,
        PITCH,
        3,
        0,
        0,
        wgt::Origin3d { x: 1, y: 2, z: 0 },
        4,
        3,
        wgpu_hal::FormatAspects::COLOR,
    )?;
    let data = map_read(device, &readback, readback_size)?;
    compare_region(
        "2D color upload/read-back",
        &data,
        readback_offset,
        PITCH as usize,
        &expected,
        16,
        3,
    )?;
    println!("  direct upload at (1,2), offset 64, pitch 256 verified");

    // Texture -> texture with non-trivial source and destination origins.
    let copy_source = create_texture(
        device,
        "texture copy source",
        wgt::TextureFormat::Rgba8Unorm,
        8,
        8,
        1,
        1,
        wgt::TextureUses::COPY_SRC | wgt::TextureUses::COPY_DST,
    )?;
    let copy_destination = create_texture(
        device,
        "texture copy destination",
        wgt::TextureFormat::Rgba8Unorm,
        8,
        8,
        1,
        1,
        wgt::TextureUses::COPY_SRC | wgt::TextureUses::COPY_DST,
    )?;
    let mut full_upload_data = vec![0u8; (PITCH * 8) as usize];
    let _ = fill_rgba(&mut full_upload_data, 0, PITCH, 8, 8, rgba_pattern);
    let full_upload = create_buffer(
        device,
        "texture copy upload",
        full_upload_data.len() as u64,
        wgt::BufferUses::MAP_WRITE | wgt::BufferUses::COPY_SRC,
    )?;
    write_buffer(device, &full_upload, &full_upload_data)?;
    upload_to_texture(
        device,
        queue,
        "texture copy upload",
        &full_upload,
        &copy_source,
        0,
        PITCH,
        8,
        0,
        0,
        wgt::Origin3d::ZERO,
        8,
        8,
        wgpu_hal::FormatAspects::COLOR,
    )?;
    copy_texture_region(
        device,
        queue,
        "texture-to-texture copy",
        &copy_source,
        0,
        0,
        wgt::Origin3d { x: 2, y: 1, z: 0 },
        &copy_destination,
        0,
        0,
        wgt::Origin3d { x: 1, y: 0, z: 0 },
        3,
        2,
        wgpu_hal::FormatAspects::COLOR,
    )?;
    let copy_readback_size = (PITCH * 2) as u64;
    let copy_readback = create_buffer(
        device,
        "texture copy read-back",
        copy_readback_size,
        wgt::BufferUses::MAP_READ | wgt::BufferUses::COPY_DST,
    )?;
    download_from_texture(
        device,
        queue,
        "texture copy read-back",
        &copy_destination,
        &copy_readback,
        0,
        PITCH,
        2,
        0,
        0,
        wgt::Origin3d { x: 1, y: 0, z: 0 },
        3,
        2,
        wgpu_hal::FormatAspects::COLOR,
    )?;
    let mut expected_copy = Vec::with_capacity(3 * 4 * 2);
    for y in 0..2 {
        for x in 0..3 {
            expected_copy.extend_from_slice(&rgba_pattern(2 + x, 1 + y));
        }
    }
    let data = map_read(device, &copy_readback, copy_readback_size)?;
    compare_region(
        "texture-to-texture copy",
        &data,
        0,
        PITCH as usize,
        &expected_copy,
        12,
        2,
    )?;
    println!("  texture-to-texture copy (2,1) -> (1,0), size 3x2 verified");

    // Depth32Float texture with a read-only DSV, upload and exact read-back.
    let depth_texture = create_texture(
        device,
        "depth 2D",
        wgt::TextureFormat::Depth32Float,
        4,
        4,
        1,
        1,
        wgt::TextureUses::COPY_SRC
            | wgt::TextureUses::COPY_DST
            | wgt::TextureUses::DEPTH_STENCIL_READ,
    )?;
    let dsv = create_view(
        device,
        &depth_texture,
        "depth DSV",
        wgt::TextureFormat::Depth32Float,
        wgt::TextureViewDimension::D2,
        wgt::TextureUses::DEPTH_STENCIL_READ,
        subresources(wgt::TextureAspect::DepthOnly, 0, Some(1), 0, Some(1)),
    )?;
    if dsv.raw_dsv().is_none() {
        return Err("depth view has no DSV".to_owned());
    }
    let depth_bytes = (PITCH * 4) as usize;
    let mut depth_upload_data = vec![0u8; depth_bytes];
    let expected_depth = fill_depth(&mut depth_upload_data, 0, PITCH, 4, 4);
    let depth_upload = create_buffer(
        device,
        "depth upload",
        depth_bytes as u64,
        wgt::BufferUses::MAP_WRITE | wgt::BufferUses::COPY_SRC,
    )?;
    write_buffer(device, &depth_upload, &depth_upload_data)?;
    upload_to_texture(
        device,
        queue,
        "depth upload",
        &depth_upload,
        &depth_texture,
        0,
        PITCH,
        4,
        0,
        0,
        wgt::Origin3d::ZERO,
        4,
        4,
        wgpu_hal::FormatAspects::DEPTH,
    )?;
    let depth_readback = create_buffer(
        device,
        "depth read-back",
        depth_bytes as u64,
        wgt::BufferUses::MAP_READ | wgt::BufferUses::COPY_DST,
    )?;
    download_from_texture(
        device,
        queue,
        "depth read-back",
        &depth_texture,
        &depth_readback,
        0,
        PITCH,
        4,
        0,
        0,
        wgt::Origin3d::ZERO,
        4,
        4,
        wgpu_hal::FormatAspects::DEPTH,
    )?;
    let data = map_read(device, &depth_readback, depth_bytes as u64)?;
    compare_region(
        "depth upload/read-back",
        &data,
        0,
        PITCH as usize,
        &expected_depth,
        16,
        4,
    )?;
    println!("  Depth32Float 4x4 upload/read-back verified");

    unsafe {
        device.destroy_buffer(depth_readback);
        device.destroy_buffer(depth_upload);
        device.destroy_texture_view(dsv);
        device.destroy_texture(depth_texture);
        device.destroy_buffer(copy_readback);
        device.destroy_buffer(full_upload);
        device.destroy_texture(copy_destination);
        device.destroy_texture(copy_source);
        device.destroy_buffer(readback);
        device.destroy_buffer(upload);
        device.destroy_texture_view(rtv);
        device.destroy_texture_view(srv);
        device.destroy_texture(texture);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 5. Array textures and their views
// ---------------------------------------------------------------------------

fn section_texture_arrays(
    device: &wgpu_hal::dx11::Device,
    queue: &wgpu_hal::dx11::Queue,
) -> Result<(), String> {
    println!("\n=== 5. 2D array textures ===");

    let texture = create_texture(
        device,
        "array texture",
        wgt::TextureFormat::Rgba8Unorm,
        16,
        8,
        4,
        3,
        wgt::TextureUses::RESOURCE | wgt::TextureUses::COPY_SRC | wgt::TextureUses::COPY_DST,
    )?;

    let array_srv = create_view(
        device,
        &texture,
        "array D2Array SRV",
        wgt::TextureFormat::Rgba8Unorm,
        wgt::TextureViewDimension::D2Array,
        wgt::TextureUses::RESOURCE,
        subresources(wgt::TextureAspect::All, 1, Some(2), 2, Some(2)),
    )?;
    if array_srv.raw_srv().is_none() {
        return Err("D2Array view has no SRV".to_owned());
    }
    let single_layer_srv = create_view(
        device,
        &texture,
        "array single-layer SRV",
        wgt::TextureFormat::Rgba8Unorm,
        wgt::TextureViewDimension::D2,
        wgt::TextureUses::RESOURCE,
        subresources(wgt::TextureAspect::All, 2, Some(1), 3, Some(1)),
    )?;
    if single_layer_srv.raw_srv().is_none() {
        return Err("single-layer D2 view has no SRV".to_owned());
    }
    println!("  D2Array SRV (mip 1, 2 mips, layer 2, 2 layers) and D2 SRV created");

    expect_err(
        "D2Array view with layers 3..5 of 4",
        create_view(
            device,
            &texture,
            "array out-of-range layers",
            wgt::TextureFormat::Rgba8Unorm,
            wgt::TextureViewDimension::D2Array,
            wgt::TextureUses::RESOURCE,
            subresources(wgt::TextureAspect::All, 0, Some(1), 3, Some(2)),
        ),
    )?;
    expect_err(
        "D2 view with array_layer_count = None over a 4-layer texture",
        create_view(
            device,
            &texture,
            "array D2 implicit layers",
            wgt::TextureFormat::Rgba8Unorm,
            wgt::TextureViewDimension::D2,
            wgt::TextureUses::RESOURCE,
            subresources(wgt::TextureAspect::All, 0, Some(1), 0, None),
        ),
    )?;
    expect_err(
        "D2Array view with mips 2..4 of 3",
        create_view(
            device,
            &texture,
            "array out-of-range mips",
            wgt::TextureFormat::Rgba8Unorm,
            wgt::TextureViewDimension::D2Array,
            wgt::TextureUses::RESOURCE,
            subresources(wgt::TextureAspect::All, 2, Some(2), 0, Some(1)),
        ),
    )?;

    // Mip 2 is 4x2; upload and read back layer 3 of that mip.
    let upload_data_len = (PITCH * 2) as usize;
    let mut upload_data = vec![0u8; upload_data_len];
    let expected = fill_rgba(&mut upload_data, 0, PITCH, 4, 2, |x, y| {
        rgba_pattern(x + 3, y + 4)
    });
    let upload = create_buffer(
        device,
        "array upload",
        upload_data_len as u64,
        wgt::BufferUses::MAP_WRITE | wgt::BufferUses::COPY_SRC,
    )?;
    write_buffer(device, &upload, &upload_data)?;
    upload_to_texture(
        device,
        queue,
        "array mip 2 layer 3 upload",
        &upload,
        &texture,
        0,
        PITCH,
        2,
        2,
        3,
        wgt::Origin3d::ZERO,
        4,
        2,
        wgpu_hal::FormatAspects::COLOR,
    )?;

    let readback = create_buffer(
        device,
        "array read-back",
        upload_data_len as u64,
        wgt::BufferUses::MAP_READ | wgt::BufferUses::COPY_DST,
    )?;
    download_from_texture(
        device,
        queue,
        "array mip 2 layer 3 read-back",
        &texture,
        &readback,
        0,
        PITCH,
        2,
        2,
        3,
        wgt::Origin3d::ZERO,
        4,
        2,
        wgpu_hal::FormatAspects::COLOR,
    )?;
    let data = map_read(device, &readback, upload_data_len as u64)?;
    compare_region(
        "array mip 2 layer 3",
        &data,
        0,
        PITCH as usize,
        &expected,
        16,
        2,
    )?;
    println!("  mip 2 / layer 3 (4x2) upload/read-back verified");

    unsafe {
        device.destroy_buffer(readback);
        device.destroy_buffer(upload);
        device.destroy_texture_view(single_layer_srv);
        device.destroy_texture_view(array_srv);
        device.destroy_texture(texture);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 6. Cubemaps
// ---------------------------------------------------------------------------

fn section_cubemaps(
    device: &wgpu_hal::dx11::Device,
    queue: &wgpu_hal::dx11::Queue,
) -> Result<(), String> {
    println!("\n=== 6. cubemaps ===");

    let cube = create_texture(
        device,
        "cube texture",
        wgt::TextureFormat::Rgba8Unorm,
        8,
        8,
        6,
        3,
        wgt::TextureUses::RESOURCE | wgt::TextureUses::COPY_SRC | wgt::TextureUses::COPY_DST,
    )?;
    let cube_srv = create_view(
        device,
        &cube,
        "cube SRV",
        wgt::TextureFormat::Rgba8Unorm,
        wgt::TextureViewDimension::Cube,
        wgt::TextureUses::RESOURCE,
        subresources(wgt::TextureAspect::All, 0, Some(3), 0, Some(6)),
    )?;
    if cube_srv.raw_srv().is_none() {
        return Err("cube SRV view has no SRV".to_owned());
    }
    let cube_array_srv = create_view(
        device,
        &cube,
        "cube array SRV",
        wgt::TextureFormat::Rgba8Unorm,
        wgt::TextureViewDimension::CubeArray,
        wgt::TextureUses::RESOURCE,
        subresources(wgt::TextureAspect::All, 0, Some(2), 0, Some(6)),
    )?;
    if cube_array_srv.raw_srv().is_none() {
        return Err("cube-array SRV view has no SRV".to_owned());
    }
    println!("  cube SRV (6 layers) and cube-array SRV (1 cube) created");

    // Upload and read back layer 5 of mip 2 (2x2). This exercises the
    // `TEXTURECUBE` misc flag and the D3D11 subresource index (2 + 5 * 3).
    let upload_data_len = (PITCH * 2) as usize;
    let mut upload_data = vec![0u8; upload_data_len];
    let expected = fill_rgba(&mut upload_data, 0, PITCH, 2, 2, |x, y| {
        rgba_pattern(x + 9, y + 1)
    });
    let upload = create_buffer(
        device,
        "cube upload",
        upload_data_len as u64,
        wgt::BufferUses::MAP_WRITE | wgt::BufferUses::COPY_SRC,
    )?;
    write_buffer(device, &upload, &upload_data)?;
    upload_to_texture(
        device,
        queue,
        "cube mip 2 layer 5 upload",
        &upload,
        &cube,
        0,
        PITCH,
        2,
        2,
        5,
        wgt::Origin3d::ZERO,
        2,
        2,
        wgpu_hal::FormatAspects::COLOR,
    )?;
    let readback = create_buffer(
        device,
        "cube read-back",
        upload_data_len as u64,
        wgt::BufferUses::MAP_READ | wgt::BufferUses::COPY_DST,
    )?;
    download_from_texture(
        device,
        queue,
        "cube mip 2 layer 5 read-back",
        &cube,
        &readback,
        0,
        PITCH,
        2,
        2,
        5,
        wgt::Origin3d::ZERO,
        2,
        2,
        wgpu_hal::FormatAspects::COLOR,
    )?;
    let data = map_read(device, &readback, upload_data_len as u64)?;
    compare_region(
        "cube mip 2 layer 5",
        &data,
        0,
        PITCH as usize,
        &expected,
        8,
        2,
    )?;
    println!("  cube mip 2 / layer 5 (2x2) upload/read-back verified");

    // Twelve layers are two cubes: a `Cube` view must still use exactly six
    // layers, while a `CubeArray` view starting at layer 6 is valid.
    let wide_cube = create_texture(
        device,
        "wide cube texture",
        wgt::TextureFormat::Rgba8Unorm,
        8,
        8,
        12,
        3,
        wgt::TextureUses::RESOURCE | wgt::TextureUses::COPY_SRC | wgt::TextureUses::COPY_DST,
    )?;
    expect_err(
        "Cube view with 12 layers",
        create_view(
            device,
            &wide_cube,
            "wide cube 12-layer view",
            wgt::TextureFormat::Rgba8Unorm,
            wgt::TextureViewDimension::Cube,
            wgt::TextureUses::RESOURCE,
            subresources(wgt::TextureAspect::All, 0, Some(1), 0, Some(12)),
        ),
    )?;
    expect_err(
        "Cube view starting at layer 6",
        create_view(
            device,
            &wide_cube,
            "wide cube offset view",
            wgt::TextureFormat::Rgba8Unorm,
            wgt::TextureViewDimension::Cube,
            wgt::TextureUses::RESOURCE,
            subresources(wgt::TextureAspect::All, 0, Some(1), 6, Some(6)),
        ),
    )?;
    let second_cube_srv = create_view(
        device,
        &wide_cube,
        "wide cube second cube-array SRV",
        wgt::TextureFormat::Rgba8Unorm,
        wgt::TextureViewDimension::CubeArray,
        wgt::TextureUses::RESOURCE,
        subresources(wgt::TextureAspect::All, 0, Some(1), 6, Some(6)),
    )?;
    if second_cube_srv.raw_srv().is_none() {
        return Err("cube-array SRV at layer 6 has no SRV".to_owned());
    }
    println!("  12-layer cube: Cube rejected, CubeArray at layer 6 accepted");

    expect_err(
        "CubeArray view with 3 layers",
        create_view(
            device,
            &cube,
            "cube array 3-layer view",
            wgt::TextureFormat::Rgba8Unorm,
            wgt::TextureViewDimension::CubeArray,
            wgt::TextureUses::RESOURCE,
            subresources(wgt::TextureAspect::All, 0, Some(1), 0, Some(3)),
        ),
    )?;

    let non_square = create_texture(
        device,
        "non-square cube candidate",
        wgt::TextureFormat::Rgba8Unorm,
        8,
        4,
        6,
        1,
        wgt::TextureUses::RESOURCE | wgt::TextureUses::COPY_SRC | wgt::TextureUses::COPY_DST,
    )?;
    expect_err(
        "Cube view over a non-square texture",
        create_view(
            device,
            &non_square,
            "non-square cube view",
            wgt::TextureFormat::Rgba8Unorm,
            wgt::TextureViewDimension::Cube,
            wgt::TextureUses::RESOURCE,
            subresources(wgt::TextureAspect::All, 0, Some(1), 0, Some(6)),
        ),
    )?;
    println!("  non-square cube and non-multiple-of-six cube-array rejected");

    unsafe {
        device.destroy_texture(non_square);
        device.destroy_texture_view(second_cube_srv);
        device.destroy_texture(wide_cube);
        device.destroy_buffer(readback);
        device.destroy_buffer(upload);
        device.destroy_texture_view(cube_array_srv);
        device.destroy_texture_view(cube_srv);
        device.destroy_texture(cube);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 7. Format allowlist matrix
// ---------------------------------------------------------------------------

fn section_format_matrix(
    adapter: &wgpu_hal::dx11::Adapter,
    device: &wgpu_hal::dx11::Device,
) -> Result<(), String> {
    println!("\n=== 7. format allowlist matrix ===");
    for &format in ALLOWLIST_FORMATS {
        let caps = unsafe { adapter.texture_format_capabilities(format) };
        if caps.is_empty() {
            return Err(format!(
                "{format:?} is in the contract §6.1 allowlist but the adapter reports no capabilities"
            ));
        }

        let mut usage = wgt::TextureUses::COPY_SRC | wgt::TextureUses::COPY_DST;
        if caps.contains(Tfc::SAMPLED) {
            usage |= wgt::TextureUses::RESOURCE;
        }
        if caps.contains(Tfc::COLOR_ATTACHMENT) {
            usage |= wgt::TextureUses::COLOR_TARGET;
        }
        if caps.contains(Tfc::DEPTH_STENCIL_ATTACHMENT) {
            usage |= wgt::TextureUses::DEPTH_STENCIL_READ | wgt::TextureUses::DEPTH_STENCIL_WRITE;
        }

        let texture = create_texture(
            device,
            &format!("format matrix {format:?}"),
            format,
            4,
            4,
            1,
            1,
            usage,
        )?;
        let aspect = if format.has_depth_aspect() {
            wgt::TextureAspect::DepthOnly
        } else {
            wgt::TextureAspect::All
        };
        let mut views = Vec::new();

        if caps.contains(Tfc::SAMPLED) {
            let view = create_view(
                device,
                &texture,
                &format!("format matrix {format:?} SRV"),
                format,
                wgt::TextureViewDimension::D2,
                wgt::TextureUses::RESOURCE,
                subresources(aspect, 0, Some(1), 0, Some(1)),
            )?;
            if view.raw_srv().is_none() {
                return Err(format!("{format:?} RESOURCE view has no SRV"));
            }
            views.push(view);
        }
        if caps.contains(Tfc::COLOR_ATTACHMENT) {
            let view = create_view(
                device,
                &texture,
                &format!("format matrix {format:?} RTV"),
                format,
                wgt::TextureViewDimension::D2,
                wgt::TextureUses::COLOR_TARGET,
                subresources(wgt::TextureAspect::All, 0, Some(1), 0, Some(1)),
            )?;
            if view.raw_rtv().is_none() {
                return Err(format!("{format:?} COLOR_TARGET view has no RTV"));
            }
            views.push(view);
        }
        if caps.contains(Tfc::DEPTH_STENCIL_ATTACHMENT) {
            let view = create_view(
                device,
                &texture,
                &format!("format matrix {format:?} DSV"),
                format,
                wgt::TextureViewDimension::D2,
                wgt::TextureUses::DEPTH_STENCIL_READ | wgt::TextureUses::DEPTH_STENCIL_WRITE,
                subresources(wgt::TextureAspect::DepthOnly, 0, Some(1), 0, Some(1)),
            )?;
            if view.raw_dsv().is_none() {
                return Err(format!("{format:?} depth-stencil view has no DSV"));
            }
            views.push(view);
        }

        println!("  {format:?} => {caps:?} ({usage:?})");
        for view in views {
            unsafe { device.destroy_texture_view(view) };
        }
        unsafe { device.destroy_texture(texture) };
    }

    for format in [
        wgt::TextureFormat::Bc1RgbaUnorm,
        wgt::TextureFormat::Etc2Rgb8Unorm,
        wgt::TextureFormat::Astc {
            block: wgt::AstcBlock::B4x4,
            channel: wgt::AstcChannel::Unorm,
        },
        wgt::TextureFormat::Stencil8,
        wgt::TextureFormat::Rgb9e5Ufloat,
    ] {
        expect_err(
            &format!("create_texture({format:?})"),
            create_texture(
                device,
                "unsupported format",
                format,
                4,
                4,
                1,
                1,
                wgt::TextureUses::COPY_SRC | wgt::TextureUses::COPY_DST,
            ),
        )?;
    }

    for usage in [
        wgt::TextureUses::STORAGE_READ_ONLY,
        wgt::TextureUses::STORAGE_WRITE_ONLY,
        wgt::TextureUses::STORAGE_READ_WRITE,
    ] {
        expect_err(
            &format!("create_texture(storage usage {usage:?})"),
            create_texture(
                device,
                "storage texture",
                wgt::TextureFormat::Rgba8Unorm,
                4,
                4,
                1,
                1,
                usage | wgt::TextureUses::COPY_SRC,
            ),
        )?;
    }

    // `view_formats` must be validated at texture creation time.
    let mut incompatible_view_formats = texture_desc(
        "incompatible view formats",
        wgt::TextureFormat::Rgba8Unorm,
        4,
        4,
        1,
        1,
        wgt::TextureUses::RESOURCE | wgt::TextureUses::COPY_SRC | wgt::TextureUses::COPY_DST,
    );
    incompatible_view_formats.view_formats = vec![wgt::TextureFormat::Bgra8Unorm];
    expect_err(
        "create_texture(view_formats = [Bgra8Unorm]) over Rgba8Unorm",
        unsafe { device.create_texture(&incompatible_view_formats) },
    )?;

    let mut srgb_view_formats = texture_desc(
        "sRGB view formats",
        wgt::TextureFormat::Rgba8Unorm,
        4,
        4,
        1,
        1,
        wgt::TextureUses::RESOURCE | wgt::TextureUses::COPY_SRC | wgt::TextureUses::COPY_DST,
    );
    srgb_view_formats.view_formats = vec![wgt::TextureFormat::Rgba8UnormSrgb];
    let texture = unsafe { device.create_texture(&srgb_view_formats) }
        .map_err(|e| format!("create_texture(view_formats = [Rgba8UnormSrgb]): {e}"))?;
    expect_err(
        "create_texture_view(Rgba8UnormSrgb over Rgba8Unorm)",
        create_view(
            device,
            &texture,
            "sRGB view over Rgba8Unorm",
            wgt::TextureFormat::Rgba8UnormSrgb,
            wgt::TextureViewDimension::D2,
            wgt::TextureUses::RESOURCE,
            full_range(),
        ),
    )?;
    unsafe { device.destroy_texture(texture) };
    println!("  view_formats: Bgra8Unorm and Rgba8UnormSrgb rejected (not advertised)");

    let zero_and_bad_mips: [(&str, u32, u32, u32, u32); 5] = [
        ("width = 0", 0, 4, 1, 1),
        ("height = 0", 4, 0, 1, 1),
        ("array layers = 0", 4, 4, 0, 1),
        ("mip levels = 0", 4, 4, 1, 0),
        ("mip levels beyond an 8x8 texture (5)", 8, 8, 1, 5),
    ];
    for (label, width, height, layers, mips) in zero_and_bad_mips {
        expect_err(
            &format!("create_texture({label})"),
            create_texture(
                device,
                label,
                wgt::TextureFormat::Rgba8Unorm,
                width,
                height,
                layers,
                mips,
                wgt::TextureUses::COPY_SRC | wgt::TextureUses::COPY_DST,
            ),
        )?;
    }
    expect_err(
        "create_texture(mip levels beyond a 4x4 texture (4))",
        create_texture(
            device,
            "too many mips",
            wgt::TextureFormat::Rgba8Unorm,
            4,
            4,
            1,
            4,
            wgt::TextureUses::COPY_SRC | wgt::TextureUses::COPY_DST,
        ),
    )?;

    // The largest valid mip chain for a 4x4 texture must be accepted.
    let mipmapped = create_texture(
        device,
        "4x4 three mips",
        wgt::TextureFormat::Rgba8Unorm,
        4,
        4,
        1,
        3,
        wgt::TextureUses::COPY_SRC | wgt::TextureUses::COPY_DST,
    )?;
    unsafe { device.destroy_texture(mipmapped) };

    println!("  format matrix: positives and negatives OK");
    Ok(())
}

// ---------------------------------------------------------------------------
// 8. Negative texture views
// ---------------------------------------------------------------------------

fn section_view_negatives(device: &wgpu_hal::dx11::Device) -> Result<(), String> {
    println!("\n=== 8. negative texture views ===");

    let color = create_texture(
        device,
        "view negative color",
        wgt::TextureFormat::Rgba8Unorm,
        8,
        8,
        2,
        2,
        wgt::TextureUses::RESOURCE
            | wgt::TextureUses::COLOR_TARGET
            | wgt::TextureUses::COPY_SRC
            | wgt::TextureUses::COPY_DST,
    )?;
    let depth = create_texture(
        device,
        "view negative depth",
        wgt::TextureFormat::Depth32Float,
        4,
        4,
        1,
        1,
        wgt::TextureUses::DEPTH_STENCIL_READ
            | wgt::TextureUses::DEPTH_STENCIL_WRITE
            | wgt::TextureUses::COPY_SRC,
    )?;
    let cube = create_texture(
        device,
        "view negative cube",
        wgt::TextureFormat::Rgba8Unorm,
        8,
        8,
        6,
        1,
        wgt::TextureUses::RESOURCE | wgt::TextureUses::COLOR_TARGET | wgt::TextureUses::COPY_SRC,
    )?;

    expect_err(
        "create_texture_view(storage usage)",
        create_view(
            device,
            &color,
            "storage view",
            wgt::TextureFormat::Rgba8Unorm,
            wgt::TextureViewDimension::D2,
            wgt::TextureUses::STORAGE_READ_ONLY,
            full_range(),
        ),
    )?;
    expect_err(
        "create_texture_view(D1)",
        create_view(
            device,
            &color,
            "D1 view",
            wgt::TextureFormat::Rgba8Unorm,
            wgt::TextureViewDimension::D1,
            wgt::TextureUses::RESOURCE,
            full_range(),
        ),
    )?;
    expect_err(
        "create_texture_view(D3)",
        create_view(
            device,
            &color,
            "D3 view",
            wgt::TextureFormat::Rgba8Unorm,
            wgt::TextureViewDimension::D3,
            wgt::TextureUses::RESOURCE,
            full_range(),
        ),
    )?;
    expect_err(
        "create_texture_view(RTV over depth)",
        create_view(
            device,
            &depth,
            "depth RTV",
            wgt::TextureFormat::Depth32Float,
            wgt::TextureViewDimension::D2,
            wgt::TextureUses::COLOR_TARGET,
            subresources(wgt::TextureAspect::All, 0, Some(1), 0, Some(1)),
        ),
    )?;
    expect_err(
        "create_texture_view(DSV over color)",
        create_view(
            device,
            &color,
            "color DSV",
            wgt::TextureFormat::Rgba8Unorm,
            wgt::TextureViewDimension::D2,
            wgt::TextureUses::DEPTH_STENCIL_READ,
            full_range(),
        ),
    )?;
    expect_err(
        "create_texture_view(incompatible format)",
        create_view(
            device,
            &color,
            "Rgba8Uint view",
            wgt::TextureFormat::Rgba8Uint,
            wgt::TextureViewDimension::D2,
            wgt::TextureUses::RESOURCE,
            full_range(),
        ),
    )?;
    expect_err(
        "create_texture_view(base_mip out of range)",
        create_view(
            device,
            &color,
            "out-of-range mip view",
            wgt::TextureFormat::Rgba8Unorm,
            wgt::TextureViewDimension::D2,
            wgt::TextureUses::RESOURCE,
            subresources(wgt::TextureAspect::All, 2, Some(1), 0, Some(1)),
        ),
    )?;
    expect_err(
        "create_texture_view(base_layer out of range)",
        create_view(
            device,
            &color,
            "out-of-range layer view",
            wgt::TextureFormat::Rgba8Unorm,
            wgt::TextureViewDimension::D2,
            wgt::TextureUses::RESOURCE,
            subresources(wgt::TextureAspect::All, 0, Some(1), 2, Some(1)),
        ),
    )?;
    expect_err(
        "create_texture_view(layer range overflow)",
        create_view(
            device,
            &color,
            "layer overflow view",
            wgt::TextureFormat::Rgba8Unorm,
            wgt::TextureViewDimension::D2Array,
            wgt::TextureUses::RESOURCE,
            subresources(wgt::TextureAspect::All, 0, Some(1), 1, Some(2)),
        ),
    )?;
    expect_err(
        "create_texture_view(no SRV/RTV/DSV usage)",
        create_view(
            device,
            &color,
            "copy-only view",
            wgt::TextureFormat::Rgba8Unorm,
            wgt::TextureViewDimension::D2,
            wgt::TextureUses::COPY_SRC,
            full_range(),
        ),
    )?;
    expect_err(
        "create_texture_view(Cube as RTV)",
        create_view(
            device,
            &cube,
            "cube RTV",
            wgt::TextureFormat::Rgba8Unorm,
            wgt::TextureViewDimension::Cube,
            wgt::TextureUses::COLOR_TARGET,
            subresources(wgt::TextureAspect::All, 0, Some(1), 0, Some(6)),
        ),
    )?;

    unsafe {
        device.destroy_texture(cube);
        device.destroy_texture(depth);
        device.destroy_texture(color);
    }
    println!("  all unsupported view requests were rejected");
    Ok(())
}

// ---------------------------------------------------------------------------
// 9. Samplers
// ---------------------------------------------------------------------------

fn section_samplers(
    device: &wgpu_hal::dx11::Device,
    downlevel_flags: wgt::DownlevelFlags,
) -> Result<(), String> {
    println!("\n=== 9. samplers ===");
    if downlevel_flags.contains(wgt::DownlevelFlags::ANISOTROPIC_FILTERING) {
        return Err(
            "adapter downlevel flags advertise ANISOTROPIC_FILTERING; the backend must not \
             gate feature support on anisotropy"
                .to_owned(),
        );
    }

    let mut samplers = Vec::new();
    let clamp = [
        wgt::AddressMode::ClampToEdge,
        wgt::AddressMode::ClampToEdge,
        wgt::AddressMode::ClampToEdge,
    ];

    samplers.push(create_sampler(
        device,
        "nearest",
        clamp,
        wgt::FilterMode::Nearest,
        wgt::FilterMode::Nearest,
        wgt::MipmapFilterMode::Nearest,
        0.0..32.0,
        None,
        1,
        None,
    )?);
    samplers.push(create_sampler(
        device,
        "linear",
        clamp,
        wgt::FilterMode::Linear,
        wgt::FilterMode::Linear,
        wgt::MipmapFilterMode::Linear,
        0.0..32.0,
        None,
        1,
        None,
    )?);
    samplers.push(create_sampler(
        device,
        "linear with nearest mipmap filter",
        clamp,
        wgt::FilterMode::Linear,
        wgt::FilterMode::Linear,
        wgt::MipmapFilterMode::Nearest,
        0.0..32.0,
        None,
        1,
        None,
    )?);
    samplers.push(create_sampler(
        device,
        "nearest with linear mipmap filter",
        clamp,
        wgt::FilterMode::Nearest,
        wgt::FilterMode::Nearest,
        wgt::MipmapFilterMode::Linear,
        0.0..32.0,
        None,
        1,
        None,
    )?);

    for mode in [
        wgt::AddressMode::ClampToEdge,
        wgt::AddressMode::Repeat,
        wgt::AddressMode::MirrorRepeat,
        wgt::AddressMode::ClampToBorder,
    ] {
        samplers.push(create_sampler(
            device,
            &format!("address mode {mode:?}"),
            [mode; 3],
            wgt::FilterMode::Linear,
            wgt::FilterMode::Linear,
            wgt::MipmapFilterMode::Nearest,
            0.0..32.0,
            None,
            1,
            None,
        )?);
    }

    for border_color in [
        wgt::SamplerBorderColor::TransparentBlack,
        wgt::SamplerBorderColor::OpaqueBlack,
        wgt::SamplerBorderColor::OpaqueWhite,
    ] {
        samplers.push(create_sampler(
            device,
            &format!("border color {border_color:?}"),
            [wgt::AddressMode::ClampToBorder; 3],
            wgt::FilterMode::Linear,
            wgt::FilterMode::Linear,
            wgt::MipmapFilterMode::Nearest,
            0.0..32.0,
            None,
            1,
            Some(border_color),
        )?);
    }

    for compare in [
        wgt::CompareFunction::Never,
        wgt::CompareFunction::Less,
        wgt::CompareFunction::Equal,
        wgt::CompareFunction::LessEqual,
        wgt::CompareFunction::Greater,
        wgt::CompareFunction::NotEqual,
        wgt::CompareFunction::GreaterEqual,
        wgt::CompareFunction::Always,
    ] {
        samplers.push(create_sampler(
            device,
            &format!("comparison {compare:?}"),
            clamp,
            wgt::FilterMode::Linear,
            wgt::FilterMode::Linear,
            wgt::MipmapFilterMode::Nearest,
            0.0..32.0,
            Some(compare),
            1,
            None,
        )?);
    }

    samplers.push(create_sampler(
        device,
        "lod clamp",
        clamp,
        wgt::FilterMode::Linear,
        wgt::FilterMode::Linear,
        wgt::MipmapFilterMode::Nearest,
        0.5..8.0,
        None,
        1,
        None,
    )?);
    samplers.push(create_sampler(
        device,
        "anisotropy 16",
        clamp,
        wgt::FilterMode::Linear,
        wgt::FilterMode::Linear,
        wgt::MipmapFilterMode::Linear,
        0.0..32.0,
        None,
        16,
        None,
    )?);
    println!("  created {} samplers", samplers.len());

    expect_err(
        "create_sampler(anisotropy_clamp = 0)",
        create_sampler(
            device,
            "anisotropy 0",
            clamp,
            wgt::FilterMode::Linear,
            wgt::FilterMode::Linear,
            wgt::MipmapFilterMode::Linear,
            0.0..32.0,
            None,
            0,
            None,
        ),
    )?;
    expect_err(
        "create_sampler(anisotropy_clamp = 17)",
        create_sampler(
            device,
            "anisotropy 17",
            clamp,
            wgt::FilterMode::Linear,
            wgt::FilterMode::Linear,
            wgt::MipmapFilterMode::Linear,
            0.0..32.0,
            None,
            17,
            None,
        ),
    )?;

    for sampler in samplers {
        unsafe { device.destroy_sampler(sampler) };
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn texture_desc<'a>(
    label: &'a str,
    format: wgt::TextureFormat,
    width: u32,
    height: u32,
    layers: u32,
    mip_level_count: u32,
    usage: wgt::TextureUses,
) -> wgpu_hal::TextureDescriptor<'a> {
    wgpu_hal::TextureDescriptor {
        label: Some(label),
        size: wgt::Extent3d {
            width,
            height,
            depth_or_array_layers: layers,
        },
        mip_level_count,
        sample_count: 1,
        dimension: wgt::TextureDimension::D2,
        format,
        usage,
        memory_flags: wgpu_hal::MemoryFlags::empty(),
        view_formats: Vec::new(),
    }
}

fn create_texture(
    device: &wgpu_hal::dx11::Device,
    label: &str,
    format: wgt::TextureFormat,
    width: u32,
    height: u32,
    layers: u32,
    mips: u32,
    usage: wgt::TextureUses,
) -> Result<wgpu_hal::dx11::Texture, String> {
    let desc = texture_desc(label, format, width, height, layers, mips, usage);
    unsafe { device.create_texture(&desc) }.map_err(|e| format!("create_texture({label}): {e}"))
}

fn subresources(
    aspect: wgt::TextureAspect,
    base_mip_level: u32,
    mip_level_count: Option<u32>,
    base_array_layer: u32,
    array_layer_count: Option<u32>,
) -> wgt::ImageSubresourceRange {
    wgt::ImageSubresourceRange {
        aspect,
        base_mip_level,
        mip_level_count,
        base_array_layer,
        array_layer_count,
    }
}

fn full_range() -> wgt::ImageSubresourceRange {
    subresources(wgt::TextureAspect::All, 0, Some(1), 0, Some(1))
}

#[allow(clippy::too_many_arguments)]
fn create_view(
    device: &wgpu_hal::dx11::Device,
    texture: &wgpu_hal::dx11::Texture,
    label: &str,
    format: wgt::TextureFormat,
    dimension: wgt::TextureViewDimension,
    usage: wgt::TextureUses,
    range: wgt::ImageSubresourceRange,
) -> Result<wgpu_hal::dx11::TextureView, String> {
    let desc = wgpu_hal::TextureViewDescriptor {
        label: Some(label),
        format,
        dimension,
        usage,
        range,
    };
    unsafe { device.create_texture_view(texture, &desc) }
        .map_err(|e| format!("create_texture_view({label}): {e}"))
}

fn create_buffer(
    device: &wgpu_hal::dx11::Device,
    label: &str,
    size: u64,
    usage: wgt::BufferUses,
) -> Result<wgpu_hal::dx11::Buffer, String> {
    let desc = wgpu_hal::BufferDescriptor {
        label: Some(label),
        size,
        usage,
        memory_flags: wgpu_hal::MemoryFlags::empty(),
    };
    unsafe { device.create_buffer(&desc) }.map_err(|e| format!("create_buffer({label}): {e}"))
}

#[allow(clippy::too_many_arguments)]
fn create_sampler(
    device: &wgpu_hal::dx11::Device,
    label: &str,
    address_modes: [wgt::AddressMode; 3],
    mag_filter: wgt::FilterMode,
    min_filter: wgt::FilterMode,
    mipmap_filter: wgt::MipmapFilterMode,
    lod_clamp: std::ops::Range<f32>,
    compare: Option<wgt::CompareFunction>,
    anisotropy_clamp: u16,
    border_color: Option<wgt::SamplerBorderColor>,
) -> Result<wgpu_hal::dx11::Sampler, String> {
    let desc = wgpu_hal::SamplerDescriptor {
        label: Some(label),
        address_modes,
        mag_filter,
        min_filter,
        mipmap_filter,
        lod_clamp,
        compare,
        anisotropy_clamp,
        border_color,
    };
    unsafe { device.create_sampler(&desc) }.map_err(|e| format!("create_sampler({label}): {e}"))
}

fn write_buffer(
    device: &wgpu_hal::dx11::Device,
    buffer: &wgpu_hal::dx11::Buffer,
    data: &[u8],
) -> Result<(), String> {
    let size = data.len() as u64;
    let mapping = unsafe { device.map_buffer(buffer, 0..size) }
        .map_err(|e| format!("map_buffer(write {size} bytes): {e}"))?;
    unsafe {
        core::ptr::copy_nonoverlapping(data.as_ptr(), mapping.ptr.as_ptr(), data.len());
        device.unmap_buffer(buffer);
    }
    Ok(())
}

fn map_read(
    device: &wgpu_hal::dx11::Device,
    buffer: &wgpu_hal::dx11::Buffer,
    size: u64,
) -> Result<Vec<u8>, String> {
    let mapping = unsafe { device.map_buffer(buffer, 0..size) }
        .map_err(|e| format!("map_buffer(read {size} bytes): {e}"))?;
    let bytes = unsafe { core::slice::from_raw_parts(mapping.ptr.as_ptr(), size as usize) };
    let data = bytes.to_vec();
    unsafe { device.unmap_buffer(buffer) };
    Ok(data)
}

fn with_encoder<F>(
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
    .map_err(|e| format!("create_command_encoder({label}): {e}"))?;

    unsafe { encoder.begin_encoding(Some(label)) }
        .map_err(|e| format!("begin_encoding({label}): {e}"))?;

    if let Err(error) = encode(&mut encoder) {
        unsafe { encoder.discard_encoding() };
        return Err(error);
    }

    let command_buffer =
        unsafe { encoder.end_encoding() }.map_err(|e| format!("end_encoding({label}): {e}"))?;

    let fence =
        unsafe { device.create_fence() }.map_err(|e| format!("create_fence({label}): {e}"))?;

    if let Err(error) = unsafe { queue.submit(&[&command_buffer], &[], (&fence, 1)) } {
        unsafe { device.destroy_fence(fence) };
        return Err(format!("queue.submit({label}): {error}"));
    }

    let reached =
        unsafe { device.wait(&fence, 1, None) }.map_err(|e| format!("device.wait({label}): {e}"));
    unsafe { device.destroy_fence(fence) };
    if !reached? {
        return Err(format!(
            "D3D11 fence did not reach the submitted value for '{label}'"
        ));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn upload_to_texture(
    device: &wgpu_hal::dx11::Device,
    queue: &wgpu_hal::dx11::Queue,
    label: &str,
    buffer: &wgpu_hal::dx11::Buffer,
    texture: &wgpu_hal::dx11::Texture,
    buffer_offset: u64,
    pitch: u32,
    rows: u32,
    mip_level: u32,
    array_layer: u32,
    origin: wgt::Origin3d,
    width: u32,
    height: u32,
    aspect: wgpu_hal::FormatAspects,
) -> Result<(), String> {
    with_encoder(device, queue, label, |encoder| {
        unsafe {
            encoder.copy_buffer_to_texture(
                buffer,
                texture,
                [wgpu_hal::BufferTextureCopy {
                    buffer_layout: wgt::TexelCopyBufferLayout {
                        offset: buffer_offset,
                        bytes_per_row: Some(pitch),
                        rows_per_image: Some(rows),
                    },
                    texture_base: wgpu_hal::TextureCopyBase {
                        mip_level,
                        array_layer,
                        origin,
                        aspect,
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
    })
}

#[allow(clippy::too_many_arguments)]
fn download_from_texture(
    device: &wgpu_hal::dx11::Device,
    queue: &wgpu_hal::dx11::Queue,
    label: &str,
    texture: &wgpu_hal::dx11::Texture,
    buffer: &wgpu_hal::dx11::Buffer,
    buffer_offset: u64,
    pitch: u32,
    rows: u32,
    mip_level: u32,
    array_layer: u32,
    origin: wgt::Origin3d,
    width: u32,
    height: u32,
    aspect: wgpu_hal::FormatAspects,
) -> Result<(), String> {
    with_encoder(device, queue, label, |encoder| {
        unsafe {
            encoder.copy_texture_to_buffer(
                texture,
                wgt::TextureUses::COPY_SRC,
                buffer,
                [wgpu_hal::BufferTextureCopy {
                    buffer_layout: wgt::TexelCopyBufferLayout {
                        offset: buffer_offset,
                        bytes_per_row: Some(pitch),
                        rows_per_image: Some(rows),
                    },
                    texture_base: wgpu_hal::TextureCopyBase {
                        mip_level,
                        array_layer,
                        origin,
                        aspect,
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
    })
}

#[allow(clippy::too_many_arguments)]
fn copy_texture_region(
    device: &wgpu_hal::dx11::Device,
    queue: &wgpu_hal::dx11::Queue,
    label: &str,
    src: &wgpu_hal::dx11::Texture,
    src_mip_level: u32,
    src_array_layer: u32,
    src_origin: wgt::Origin3d,
    dst: &wgpu_hal::dx11::Texture,
    dst_mip_level: u32,
    dst_array_layer: u32,
    dst_origin: wgt::Origin3d,
    width: u32,
    height: u32,
    aspect: wgpu_hal::FormatAspects,
) -> Result<(), String> {
    with_encoder(device, queue, label, |encoder| {
        unsafe {
            encoder.copy_texture_to_texture(
                src,
                wgt::TextureUses::COPY_SRC,
                dst,
                [wgpu_hal::TextureCopy {
                    src_base: wgpu_hal::TextureCopyBase {
                        mip_level: src_mip_level,
                        array_layer: src_array_layer,
                        origin: src_origin,
                        aspect,
                    },
                    dst_base: wgpu_hal::TextureCopyBase {
                        mip_level: dst_mip_level,
                        array_layer: dst_array_layer,
                        origin: dst_origin,
                        aspect,
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
    })
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
    with_encoder(device, queue, label, |encoder| {
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

fn pattern_bytes(len: usize) -> Vec<u8> {
    (0..len)
        .map(|index| ((index * 37 + 11) % 251) as u8)
        .collect()
}

fn rgba_pattern(x: u32, y: u32) -> [u8; 4] {
    let red = x.wrapping_mul(41).wrapping_add(y.wrapping_mul(7));
    let green = x
        .wrapping_mul(3)
        .wrapping_add(y.wrapping_mul(53))
        .wrapping_add(0x20);
    let blue = (x ^ y).wrapping_mul(29).wrapping_add(0x11);
    let alpha = 0x80 | (x.wrapping_mul(5).wrapping_add(y.wrapping_mul(11)) & 0x7f);
    [red as u8, green as u8, blue as u8, alpha as u8]
}

/// Fills `data` with RGBA8 texels laid out with `pitch` bytes per row and
/// returns the same texels tightly packed for comparison.
fn fill_rgba(
    data: &mut [u8],
    offset: usize,
    pitch: u32,
    width: u32,
    height: u32,
    pixel: impl Fn(u32, u32) -> [u8; 4],
) -> Vec<u8> {
    let mut expected = Vec::with_capacity((width * 4 * height) as usize);
    for y in 0..height {
        for x in 0..width {
            let value = pixel(x, y);
            let start = offset + (y * pitch + x * 4) as usize;
            data[start..start + 4].copy_from_slice(&value);
            expected.extend_from_slice(&value);
        }
    }
    expected
}

/// Fills `data` with Depth32Float texels laid out with `pitch` bytes per row
/// and returns the same texels tightly packed for comparison.
fn fill_depth(data: &mut [u8], offset: usize, pitch: u32, width: u32, height: u32) -> Vec<u8> {
    let mut expected = Vec::with_capacity((width * 4 * height) as usize);
    for y in 0..height {
        for x in 0..width {
            let value = (y * width + x) as f32 / 16.0;
            let bytes = value.to_ne_bytes();
            let start = offset + (y * pitch + x * 4) as usize;
            data[start..start + 4].copy_from_slice(&bytes);
            expected.extend_from_slice(&bytes);
        }
    }
    expected
}

fn compare_region(
    context: &str,
    actual: &[u8],
    actual_offset: usize,
    actual_pitch: usize,
    expected: &[u8],
    row_bytes: usize,
    rows: usize,
) -> Result<(), String> {
    for row in 0..rows {
        let actual_start = actual_offset + row * actual_pitch;
        let expected_start = row * row_bytes;
        if actual_start + row_bytes > actual.len() {
            return Err(format!(
                "{context}: read-back buffer is too short for row {row}"
            ));
        }
        if expected_start + row_bytes > expected.len() {
            return Err(format!(
                "{context}: expected data is too short for row {row}"
            ));
        }
        let actual_row = &actual[actual_start..actual_start + row_bytes];
        let expected_row = &expected[expected_start..expected_start + row_bytes];
        if actual_row != expected_row {
            let index = actual_row
                .iter()
                .zip(expected_row.iter())
                .position(|(actual, expected)| actual != expected);
            return match index {
                Some(index) => Err(format!(
                    "{context}: byte mismatch at row {row}, byte {index}: got 0x{:02x}, \
                     expected 0x{:02x} (row got {actual_row:?}, expected {expected_row:?})",
                    actual_row[index], expected_row[index]
                )),
                None => Err(format!("{context}: row {row} differs in length")),
            };
        }
    }
    Ok(())
}

//! D3D11 resource creation and upload smoke test.
//!
//! Opens the experimental `wgpu-hal` Direct3D 11 backend without a window,
//! creates and maps a vertex buffer, creates a 2D texture with RTV/SRV views
//! plus a depth texture with a DSV, creates a sampler, uploads pixels with
//! `copy_buffer_to_texture`, reads them back with `copy_texture_to_buffer`,
//! and checks that the data survived the round trip.
//!
//! Run on Windows with:
//!
//! ```text
//! cargo run -p wgpu-hal --example raw-dx11-resources --features dx11
//! ```

#[cfg(target_os = "windows")]
use wgpu_hal::{Adapter as _, CommandEncoder as _, Device as _, Instance as _, Queue as _};
#[cfg(target_os = "windows")]
use wgpu_types as wgt;

#[cfg(target_os = "windows")]
fn main() {
    env_logger::init();

    if let Err(e) = run() {
        eprintln!("raw-dx11-resources failed: {e}");
        std::process::exit(1);
    }

    println!("raw-dx11-resources completed successfully");
}

#[cfg(target_os = "windows")]
fn run() -> Result<(), Box<dyn std::error::Error>> {
    const TEXTURE_WIDTH: u32 = 4;
    const TEXTURE_HEIGHT: u32 = 4;
    const ROW_PITCH: u32 = 256;

    let instance_desc = wgpu_hal::InstanceDescriptor {
        name: "raw-dx11-resources",
        flags: wgt::InstanceFlags::default(),
        memory_budget_thresholds: wgt::MemoryBudgetThresholds::default(),
        backend_options: wgt::BackendOptions::default(),
        telemetry: None,
        display: None,
    };

    let instance =
        unsafe { <wgpu_hal::dx11::Instance as wgpu_hal::Instance>::init(&instance_desc)? };
    let adapters = unsafe { instance.enumerate_adapters(None) };
    let exposed = adapters.first().ok_or("no D3D11 adapters were found")?;
    println!(
        "using D3D11 adapter '{}' ({:?})",
        exposed.info.name, exposed.info.device_type
    );

    let wgpu_hal::OpenDevice { device, queue } = unsafe {
        exposed.adapter.open(
            wgt::Features::empty(),
            &exposed.capabilities.limits,
            &wgt::MemoryHints::default(),
        )?
    };
    println!("opened D3D11 device");

    let vertex_data: [f32; 12] = [
        0.0, 0.5, -0.5, -0.5, 0.5, -0.5, 0.0, 0.75, 0.25, 0.75, 0.75, 0.25,
    ];
    let vertex_bytes = unsafe {
        core::slice::from_raw_parts(
            vertex_data.as_ptr().cast::<u8>(),
            core::mem::size_of_val(&vertex_data),
        )
    };

    let vertex_buffer = unsafe {
        device.create_buffer(&wgpu_hal::BufferDescriptor {
            label: Some("vertices"),
            size: vertex_bytes.len() as u64,
            usage: wgt::BufferUses::VERTEX | wgt::BufferUses::COPY_DST | wgt::BufferUses::MAP_WRITE,
            memory_flags: wgpu_hal::MemoryFlags::empty(),
        })?
    };
    {
        let mapping = unsafe { device.map_buffer(&vertex_buffer, 0..vertex_bytes.len() as u64)? };
        unsafe {
            core::ptr::copy_nonoverlapping(
                vertex_bytes.as_ptr(),
                mapping.ptr.as_ptr(),
                vertex_bytes.len(),
            );
            device.unmap_buffer(&vertex_buffer);
        }
    }
    println!(
        "created and mapped vertex buffer ({} bytes)",
        vertex_bytes.len()
    );

    let copied_buffer = unsafe {
        device.create_buffer(&wgpu_hal::BufferDescriptor {
            label: Some("buffer copy destination"),
            size: vertex_bytes.len() as u64,
            usage: wgt::BufferUses::COPY_SRC | wgt::BufferUses::COPY_DST,
            memory_flags: wgpu_hal::MemoryFlags::empty(),
        })?
    };
    let buffer_readback = unsafe {
        device.create_buffer(&wgpu_hal::BufferDescriptor {
            label: Some("buffer readback"),
            size: vertex_bytes.len() as u64,
            usage: wgt::BufferUses::MAP_READ | wgt::BufferUses::COPY_DST,
            memory_flags: wgpu_hal::MemoryFlags::empty(),
        })?
    };

    let texture = unsafe {
        device.create_texture(&wgpu_hal::TextureDescriptor {
            label: Some("uploaded texture"),
            size: wgt::Extent3d {
                width: TEXTURE_WIDTH,
                height: TEXTURE_HEIGHT,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgt::TextureDimension::D2,
            format: wgt::TextureFormat::Rgba8Unorm,
            usage: wgt::TextureUses::RESOURCE
                | wgt::TextureUses::COLOR_TARGET
                | wgt::TextureUses::COPY_SRC
                | wgt::TextureUses::COPY_DST,
            memory_flags: wgpu_hal::MemoryFlags::empty(),
            view_formats: Vec::new(),
        })?
    };
    let texture_copy = unsafe {
        device.create_texture(&wgpu_hal::TextureDescriptor {
            label: Some("texture copy destination"),
            size: wgt::Extent3d {
                width: TEXTURE_WIDTH,
                height: TEXTURE_HEIGHT,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgt::TextureDimension::D2,
            format: wgt::TextureFormat::Rgba8Unorm,
            usage: wgt::TextureUses::RESOURCE
                | wgt::TextureUses::COLOR_TARGET
                | wgt::TextureUses::COPY_SRC
                | wgt::TextureUses::COPY_DST,
            memory_flags: wgpu_hal::MemoryFlags::empty(),
            view_formats: Vec::new(),
        })?
    };
    let depth_texture = unsafe {
        device.create_texture(&wgpu_hal::TextureDescriptor {
            label: Some("depth texture"),
            size: wgt::Extent3d {
                width: TEXTURE_WIDTH,
                height: TEXTURE_HEIGHT,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgt::TextureDimension::D2,
            format: wgt::TextureFormat::Depth32Float,
            usage: wgt::TextureUses::DEPTH_STENCIL_READ
                | wgt::TextureUses::DEPTH_STENCIL_WRITE
                | wgt::TextureUses::COPY_SRC,
            memory_flags: wgpu_hal::MemoryFlags::empty(),
            view_formats: Vec::new(),
        })?
    };
    println!("created upload, copy and depth textures");

    let view_range = wgt::ImageSubresourceRange {
        aspect: wgt::TextureAspect::All,
        base_mip_level: 0,
        mip_level_count: Some(1),
        base_array_layer: 0,
        array_layer_count: Some(1),
    };
    let rtv = unsafe {
        device.create_texture_view(
            &texture,
            &wgpu_hal::TextureViewDescriptor {
                label: Some("texture RTV"),
                format: wgt::TextureFormat::Rgba8Unorm,
                dimension: wgt::TextureViewDimension::D2,
                usage: wgt::TextureUses::COLOR_TARGET,
                range: view_range.clone(),
            },
        )?
    };
    let srv = unsafe {
        device.create_texture_view(
            &texture,
            &wgpu_hal::TextureViewDescriptor {
                label: Some("texture SRV"),
                format: wgt::TextureFormat::Rgba8Unorm,
                dimension: wgt::TextureViewDimension::D2,
                usage: wgt::TextureUses::RESOURCE,
                range: view_range.clone(),
            },
        )?
    };
    let dsv = unsafe {
        device.create_texture_view(
            &depth_texture,
            &wgpu_hal::TextureViewDescriptor {
                label: Some("depth DSV"),
                format: wgt::TextureFormat::Depth32Float,
                dimension: wgt::TextureViewDimension::D2,
                usage: wgt::TextureUses::DEPTH_STENCIL_READ | wgt::TextureUses::DEPTH_STENCIL_WRITE,
                range: wgt::ImageSubresourceRange {
                    aspect: wgt::TextureAspect::DepthOnly,
                    ..view_range.clone()
                },
            },
        )?
    };
    if rtv.raw_rtv().is_none() || srv.raw_srv().is_none() || dsv.raw_dsv().is_none() {
        return Err("one of the D3D11 texture views has no underlying handle".into());
    }
    println!("created RTV, SRV and DSV views");

    let sampler = unsafe {
        device.create_sampler(&wgpu_hal::SamplerDescriptor {
            label: Some("linear repeat sampler"),
            address_modes: [
                wgt::AddressMode::Repeat,
                wgt::AddressMode::Repeat,
                wgt::AddressMode::ClampToEdge,
            ],
            mag_filter: wgt::FilterMode::Linear,
            min_filter: wgt::FilterMode::Linear,
            mipmap_filter: wgt::MipmapFilterMode::Linear,
            lod_clamp: 0.0..32.0,
            compare: None,
            anisotropy_clamp: 1,
            border_color: None,
        })?
    };
    println!("created sampler");

    let pixel_data = |x: u32, y: u32| [(x * 40) as u8, (y * 40) as u8, ((x + y) * 20) as u8, 255];
    let mut upload_data = vec![0u8; (ROW_PITCH * TEXTURE_HEIGHT) as usize];
    for y in 0..TEXTURE_HEIGHT {
        for x in 0..TEXTURE_WIDTH {
            let offset = (y * ROW_PITCH + x * 4) as usize;
            upload_data[offset..offset + 4].copy_from_slice(&pixel_data(x, y));
        }
    }

    let upload_buffer = unsafe {
        device.create_buffer(&wgpu_hal::BufferDescriptor {
            label: Some("texture upload"),
            size: upload_data.len() as u64,
            usage: wgt::BufferUses::MAP_WRITE | wgt::BufferUses::COPY_SRC,
            memory_flags: wgpu_hal::MemoryFlags::empty(),
        })?
    };
    {
        let mapping = unsafe { device.map_buffer(&upload_buffer, 0..upload_data.len() as u64)? };
        unsafe {
            core::ptr::copy_nonoverlapping(
                upload_data.as_ptr(),
                mapping.ptr.as_ptr(),
                upload_data.len(),
            );
            device.unmap_buffer(&upload_buffer);
        }
    }
    println!("uploaded {} bytes into a staging buffer", upload_data.len());

    let texture_readback_size = (ROW_PITCH * TEXTURE_HEIGHT) as u64;
    let texture_readback = unsafe {
        device.create_buffer(&wgpu_hal::BufferDescriptor {
            label: Some("texture readback"),
            size: texture_readback_size,
            usage: wgt::BufferUses::MAP_READ | wgt::BufferUses::COPY_DST,
            memory_flags: wgpu_hal::MemoryFlags::empty(),
        })?
    };

    let fence = unsafe { device.create_fence()? };
    let mut encoder = unsafe {
        device.create_command_encoder(&wgpu_hal::CommandEncoderDescriptor {
            label: Some("raw-dx11-resources upload"),
            queue: &queue,
        })?
    };

    unsafe {
        encoder.begin_encoding(Some("raw-dx11-resources upload"))?;

        encoder.copy_buffer_to_buffer(
            &vertex_buffer,
            &copied_buffer,
            [wgpu_hal::BufferCopy {
                src_offset: 0,
                dst_offset: 0,
                size: wgt::BufferSize::new(vertex_bytes.len() as u64).unwrap(),
            }]
            .into_iter(),
        );
        encoder.copy_buffer_to_buffer(
            &copied_buffer,
            &buffer_readback,
            [wgpu_hal::BufferCopy {
                src_offset: 0,
                dst_offset: 0,
                size: wgt::BufferSize::new(vertex_bytes.len() as u64).unwrap(),
            }]
            .into_iter(),
        );

        encoder.copy_buffer_to_texture(
            &upload_buffer,
            &texture,
            [wgpu_hal::BufferTextureCopy {
                buffer_layout: wgt::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(ROW_PITCH),
                    rows_per_image: Some(TEXTURE_HEIGHT),
                },
                texture_base: wgpu_hal::TextureCopyBase {
                    mip_level: 0,
                    array_layer: 0,
                    origin: wgt::Origin3d::ZERO,
                    aspect: wgpu_hal::FormatAspects::COLOR,
                },
                size: wgpu_hal::CopyExtent {
                    width: TEXTURE_WIDTH,
                    height: TEXTURE_HEIGHT,
                    depth: 1,
                },
            }]
            .into_iter(),
        );

        encoder.copy_texture_to_texture(
            &texture,
            wgt::TextureUses::COPY_SRC,
            &texture_copy,
            [wgpu_hal::TextureCopy {
                src_base: wgpu_hal::TextureCopyBase {
                    mip_level: 0,
                    array_layer: 0,
                    origin: wgt::Origin3d::ZERO,
                    aspect: wgpu_hal::FormatAspects::COLOR,
                },
                dst_base: wgpu_hal::TextureCopyBase {
                    mip_level: 0,
                    array_layer: 0,
                    origin: wgt::Origin3d::ZERO,
                    aspect: wgpu_hal::FormatAspects::COLOR,
                },
                size: wgpu_hal::CopyExtent {
                    width: TEXTURE_WIDTH,
                    height: TEXTURE_HEIGHT,
                    depth: 1,
                },
            }]
            .into_iter(),
        );

        encoder.copy_texture_to_buffer(
            &texture_copy,
            wgt::TextureUses::COPY_SRC,
            &texture_readback,
            [wgpu_hal::BufferTextureCopy {
                buffer_layout: wgt::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(ROW_PITCH),
                    rows_per_image: Some(TEXTURE_HEIGHT),
                },
                texture_base: wgpu_hal::TextureCopyBase {
                    mip_level: 0,
                    array_layer: 0,
                    origin: wgt::Origin3d::ZERO,
                    aspect: wgpu_hal::FormatAspects::COLOR,
                },
                size: wgpu_hal::CopyExtent {
                    width: TEXTURE_WIDTH,
                    height: TEXTURE_HEIGHT,
                    depth: 1,
                },
            }]
            .into_iter(),
        );
    }

    let command_buffer = unsafe { encoder.end_encoding()? };
    unsafe {
        queue.submit(&[&command_buffer], &[], (&fence, 1))?;
        queue.wait_for_idle()?;
    }
    if !unsafe { device.wait(&fence, 1, None)? } {
        return Err("D3D11 fence did not reach the submitted value".into());
    }
    println!("submitted copies and waited for the device");

    {
        let mapping = unsafe { device.map_buffer(&buffer_readback, 0..vertex_bytes.len() as u64)? };
        let readback =
            unsafe { core::slice::from_raw_parts(mapping.ptr.as_ptr(), vertex_bytes.len()) };
        if readback != vertex_bytes {
            unsafe { device.unmap_buffer(&buffer_readback) };
            return Err("buffer copy round trip returned different vertex data".into());
        }
        unsafe { device.unmap_buffer(&buffer_readback) };
    }
    println!("buffer copy round trip verified");

    {
        let mapping = unsafe { device.map_buffer(&texture_readback, 0..texture_readback_size)? };
        let readback = unsafe {
            core::slice::from_raw_parts(mapping.ptr.as_ptr(), texture_readback_size as usize)
        };
        for y in 0..TEXTURE_HEIGHT {
            for x in 0..TEXTURE_WIDTH {
                let offset = (y * ROW_PITCH + x * 4) as usize;
                let expected = pixel_data(x, y);
                if readback[offset..offset + 4] != expected {
                    unsafe { device.unmap_buffer(&texture_readback) };
                    return Err(format!(
                        "texture readback mismatch at ({x}, {y}): {:?} != {:?}",
                        &readback[offset..offset + 4],
                        expected
                    )
                    .into());
                }
            }
        }
        unsafe { device.unmap_buffer(&texture_readback) };
    }
    println!("texture upload/copy/read-back verified");

    unsafe {
        device.destroy_fence(fence);
        device.destroy_sampler(sampler);
        device.destroy_texture_view(dsv);
        device.destroy_texture_view(srv);
        device.destroy_texture_view(rtv);
        device.destroy_buffer(texture_readback);
        device.destroy_buffer(upload_buffer);
        device.destroy_texture(depth_texture);
        device.destroy_texture(texture_copy);
        device.destroy_texture(texture);
        device.destroy_buffer(buffer_readback);
        device.destroy_buffer(copied_buffer);
        device.destroy_buffer(vertex_buffer);
    }

    println!("destroyed all D3D11 resources");
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn main() {
    eprintln!("the dx11 backend is only available on Windows");
    std::process::exit(1);
}

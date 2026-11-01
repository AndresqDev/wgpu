//! D3D11 render pipeline and draw smoke test.
//!
//! Opens the experimental `wgpu-hal` Direct3D 11 backend without a window,
//! creates an offscreen render target, a vertex buffer and a uniform buffer,
//! compiles a small WGSL shader to HLSL/FXC, draws one triangle through a real
//! `RenderPipeline`, and reads the center pixel back to the CPU.
//!
//! This example is headless and runs on the Windows CI runners:
//!
//! ```text
//! cargo run -p wgpu-hal --example raw-dx11-triangle --features dx11
//! ```

#[cfg(target_os = "windows")]
use std::borrow::Cow;

#[cfg(target_os = "windows")]
use wgpu_hal::{Adapter as _, CommandEncoder as _, Device as _, Instance as _, Queue as _};
#[cfg(target_os = "windows")]
use wgpu_types as wgt;

#[cfg(target_os = "windows")]
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

#[cfg(target_os = "windows")]
fn main() {
    env_logger::init();

    if let Err(e) = run() {
        eprintln!("raw-dx11-triangle failed: {e}");
        std::process::exit(1);
    }

    println!("raw-dx11-triangle completed successfully");
}

#[cfg(target_os = "windows")]
fn run() -> Result<(), Box<dyn std::error::Error>> {
    const TEXTURE_WIDTH: u32 = 64;
    const TEXTURE_HEIGHT: u32 = 64;
    const ROW_PITCH: u32 = TEXTURE_WIDTH * 4;
    const TEXTURE_FORMAT: wgt::TextureFormat = wgt::TextureFormat::Bgra8Unorm;
    const UNIFORM_SIZE: u64 = 32;

    let instance_desc = wgpu_hal::InstanceDescriptor {
        name: "raw-dx11-triangle",
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

    let vertices: [[f32; 2]; 3] = [[0.0, 0.8], [-0.8, -0.8], [0.8, -0.8]];
    let vertex_bytes = unsafe {
        core::slice::from_raw_parts(
            vertices.as_ptr().cast::<u8>(),
            core::mem::size_of_val(&vertices),
        )
    };
    let vertex_buffer = unsafe {
        device.create_buffer(&wgpu_hal::BufferDescriptor {
            label: Some("triangle vertices"),
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
        "created and filled vertex buffer ({} bytes)",
        vertex_bytes.len()
    );

    // `Globals { color: vec4<f32>, scale: f32 }` is 32 bytes with WGSL's
    // 16-byte struct alignment. The shader multiplies by scale (1.0) in the
    // vertex stage and returns color (red) in the fragment stage, so both
    // pipeline stages consume the same constant buffer.
    let globals: [f32; 8] = [1.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 0.0];
    let globals_bytes = unsafe {
        core::slice::from_raw_parts(
            globals.as_ptr().cast::<u8>(),
            core::mem::size_of_val(&globals),
        )
    };
    let uniform_buffer = unsafe {
        device.create_buffer(&wgpu_hal::BufferDescriptor {
            label: Some("triangle uniforms"),
            size: UNIFORM_SIZE,
            usage: wgt::BufferUses::UNIFORM | wgt::BufferUses::MAP_WRITE,
            memory_flags: wgpu_hal::MemoryFlags::empty(),
        })?
    };
    {
        let mapping = unsafe { device.map_buffer(&uniform_buffer, 0..UNIFORM_SIZE)? };
        unsafe {
            core::ptr::copy_nonoverlapping(
                globals_bytes.as_ptr(),
                mapping.ptr.as_ptr(),
                globals_bytes.len(),
            );
            device.unmap_buffer(&uniform_buffer);
        }
    }
    println!("created and filled uniform buffer");

    let texture = unsafe {
        device.create_texture(&wgpu_hal::TextureDescriptor {
            label: Some("triangle render target"),
            size: wgt::Extent3d {
                width: TEXTURE_WIDTH,
                height: TEXTURE_HEIGHT,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgt::TextureDimension::D2,
            format: TEXTURE_FORMAT,
            usage: wgt::TextureUses::COLOR_TARGET
                | wgt::TextureUses::RESOURCE
                | wgt::TextureUses::COPY_SRC,
            memory_flags: wgpu_hal::MemoryFlags::empty(),
            view_formats: Vec::new(),
        })?
    };
    let view_range = wgt::ImageSubresourceRange {
        aspect: wgt::TextureAspect::All,
        base_mip_level: 0,
        mip_level_count: Some(1),
        base_array_layer: 0,
        array_layer_count: Some(1),
    };
    let texture_view = unsafe {
        device.create_texture_view(
            &texture,
            &wgpu_hal::TextureViewDescriptor {
                label: Some("triangle RTV/SRV"),
                format: TEXTURE_FORMAT,
                dimension: wgt::TextureViewDimension::D2,
                usage: wgt::TextureUses::COLOR_TARGET | wgt::TextureUses::RESOURCE,
                range: view_range,
            },
        )?
    };
    if texture_view.raw_rtv().is_none() || texture_view.raw_srv().is_none() {
        return Err("triangle texture view did not create both RTV and SRV".into());
    }
    println!("created offscreen texture with RTV and SRV");

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
            label: Some("triangle bind group layout"),
            flags: wgpu_hal::BindGroupLayoutFlags::empty(),
            entries: &layout_entries,
        })?
    };
    let pipeline_layout = unsafe {
        device.create_pipeline_layout(&wgpu_hal::PipelineLayoutDescriptor {
            label: Some("triangle pipeline layout"),
            flags: wgpu_hal::PipelineLayoutFlags::empty(),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        })?
    };

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
            label: Some("triangle bind group"),
            layout: &bind_group_layout,
            buffers: &[buffer_binding],
            samplers: &[],
            textures: &[],
            entries: &bind_entries,
            acceleration_structures: &[],
            external_textures: &[],
        })?
    };
    println!("created bind group and pipeline layout");

    let module = naga::front::wgsl::parse_str(SHADER)?;
    let info = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&module)?;
    let shader_module = unsafe {
        device.create_shader_module(
            &wgpu_hal::ShaderModuleDescriptor {
                label: Some("triangle shader"),
                runtime_checks: wgt::ShaderRuntimeChecks::checked(),
            },
            wgpu_hal::ShaderInput::Naga(wgpu_hal::NagaShader {
                module: Cow::Owned(module),
                info,
                debug_source: None,
            }),
        )?
    };

    let constants = naga::back::PipelineConstants::default();
    let pipeline = unsafe {
        device.create_render_pipeline(&wgpu_hal::RenderPipelineDescriptor {
            label: Some("triangle pipeline"),
            layout: &pipeline_layout,
            vertex_processor: wgpu_hal::VertexProcessor::Standard {
                vertex_buffers: &[Some(wgpu_hal::VertexBufferLayout {
                    array_stride: core::mem::size_of::<[f32; 2]>() as u64,
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
                format: TEXTURE_FORMAT,
                blend: None,
                write_mask: wgt::ColorWrites::ALL,
            })],
            multiview_mask: None,
            cache: None,
        })?
    };
    println!(
        "created render pipeline with topology {:?}",
        pipeline.topology()
    );

    let readback_size = u64::from(ROW_PITCH) * u64::from(TEXTURE_HEIGHT);
    let readback = unsafe {
        device.create_buffer(&wgpu_hal::BufferDescriptor {
            label: Some("triangle readback"),
            size: readback_size,
            usage: wgt::BufferUses::MAP_READ | wgt::BufferUses::COPY_DST,
            memory_flags: wgpu_hal::MemoryFlags::empty(),
        })?
    };

    let fence = unsafe { device.create_fence()? };
    let mut encoder = unsafe {
        device.create_command_encoder(&wgpu_hal::CommandEncoderDescriptor {
            label: Some("raw-dx11-triangle"),
            queue: &queue,
        })?
    };

    unsafe {
        encoder.begin_encoding(Some("raw-dx11-triangle"))?;

        let color_attachment = wgpu_hal::ColorAttachment {
            target: wgpu_hal::Attachment {
                view: &texture_view,
                usage: wgt::TextureUses::COLOR_TARGET,
            },
            depth_slice: None,
            resolve_target: None,
            ops: wgpu_hal::AttachmentOps::LOAD_CLEAR | wgpu_hal::AttachmentOps::STORE,
            clear_value: wgt::Color {
                r: 0.0,
                g: 0.0,
                b: 1.0,
                a: 1.0,
            },
        };
        let render_pass = wgpu_hal::RenderPassDescriptor {
            label: Some("triangle pass"),
            extent: wgt::Extent3d {
                width: TEXTURE_WIDTH,
                height: TEXTURE_HEIGHT,
                depth_or_array_layers: 1,
            },
            sample_count: 1,
            color_attachments: &[Some(color_attachment)],
            depth_stencil_attachment: None,
            multiview_mask: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        };

        encoder.begin_render_pass(&render_pass)?;
        encoder.set_viewport(
            &wgpu_hal::Rect {
                x: 0.0,
                y: 0.0,
                w: TEXTURE_WIDTH as f32,
                h: TEXTURE_HEIGHT as f32,
            },
            0.0..1.0,
        );
        encoder.set_scissor_rect(&wgpu_hal::Rect {
            x: 0,
            y: 0,
            w: TEXTURE_WIDTH,
            h: TEXTURE_HEIGHT,
        });
        encoder.set_render_pipeline(&pipeline);
        encoder.set_bind_group(&pipeline_layout, 0, &bind_group, &[]);
        encoder.set_vertex_buffer(
            0,
            wgpu_hal::BufferBinding::new_unchecked(
                &vertex_buffer,
                0,
                wgt::BufferSize::new(vertex_bytes.len() as u64),
            ),
        );
        encoder.draw(0, 3, 0, 1);
        encoder.end_render_pass();

        encoder.copy_texture_to_buffer(
            &texture,
            wgt::TextureUses::COPY_SRC,
            &readback,
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
    println!("submitted triangle draw and readback copy");

    {
        let mapping = unsafe { device.map_buffer(&readback, 0..readback_size)? };
        let data =
            unsafe { core::slice::from_raw_parts(mapping.ptr.as_ptr(), readback_size as usize) };

        let center_offset = ((TEXTURE_HEIGHT / 2) * ROW_PITCH + (TEXTURE_WIDTH / 2) * 4) as usize;
        let center = [
            data[center_offset],
            data[center_offset + 1],
            data[center_offset + 2],
            data[center_offset + 3],
        ];
        let corner = [data[0], data[1], data[2], data[3]];
        unsafe { device.unmap_buffer(&readback) };

        println!("center pixel (BGRA): {center:?}");
        println!("corner pixel (BGRA): {corner:?}");

        if center != [0, 0, 255, 255] {
            return Err(format!(
                "expected a red center pixel [0, 0, 255, 255] in BGRA, got {center:?}"
            )
            .into());
        }
        if corner != [255, 0, 0, 255] {
            return Err(format!(
                "expected the cleared corner pixel to be blue [255, 0, 0, 255] in BGRA, got {corner:?}"
            )
            .into());
        }
    }
    println!("verified red triangle and blue clear color by readback");

    unsafe {
        device.destroy_fence(fence);
        device.destroy_render_pipeline(pipeline);
        device.destroy_bind_group(bind_group);
        device.destroy_shader_module(shader_module);
        device.destroy_pipeline_layout(pipeline_layout);
        device.destroy_texture_view(texture_view);
        device.destroy_texture(texture);
        device.destroy_buffer(readback);
        device.destroy_buffer(uniform_buffer);
        device.destroy_buffer(vertex_buffer);
        device.destroy_bind_group_layout(bind_group_layout);
    }

    println!("destroyed all D3D11 triangle resources");
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn main() {
    eprintln!("the dx11 backend is only available on Windows");
    std::process::exit(1);
}

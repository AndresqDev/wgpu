//! Pipeline and render-pass coverage for the experimental Direct3D 11 backend.
//!
//! This test is the executable form of the pipeline part of the P4 packet and
//! exercises render targets, depth attachments, clears, viewport/scissor
//! state, indexed and indirect draws, per-instance vertex stepping, color
//! write masks, blending and face culling through the real `wgpu-hal` API.
//! It also checks that operations which the backend cannot implement fail
//! with an explicit encoder or device error instead of panicking or silently
//! becoming a no-op.
//!
//! The test is headless: it creates offscreen textures and never opens a
//! window. It uses `Features::empty()` and the adapter's own limits, so it
//! runs unchanged on WARP and on hardware.
//!
//! Run on Windows with:
//!
//! ```text
//! cargo test -p wgpu-hal --features dx11 --test dx11_pipelines -- --nocapture
//! ```

#![cfg(all(target_os = "windows", feature = "dx11"))]

use std::borrow::Cow;
use std::num::NonZeroU32;

use wgpu_hal::{Adapter as _, CommandEncoder as _, Device as _, Instance as _, Queue as _};
use wgpu_types as wgt;

/// Extent of every render target used by the positive sections.
const WIDTH: u32 = 64;
const HEIGHT: u32 = 64;

/// Real row pitch of a four-byte-per-texel, `WIDTH`-wide image. It is also a
/// multiple of 256, as D3D11 texture copies require.
const PITCH: u32 = WIDTH * 4;

// Clear values chosen so every UNORM channel converts without a rounding tie:
// 0.0 -> 0, 0.25 -> 64, 0.5 -> 128, 0.75 -> 191, 1.0 -> 255.
const CLEAR_BLACK: wgt::Color = wgt::Color::BLACK;
const CLEAR_GRAY: wgt::Color = wgt::Color {
    r: 0.75,
    g: 0.75,
    b: 0.75,
    a: 1.0,
};
const CLEAR_VIEWPORT: wgt::Color = wgt::Color {
    r: 0.25,
    g: 0.75,
    b: 0.0,
    a: 1.0,
};
const CLEAR_WRITE_MASK: wgt::Color = wgt::Color {
    r: 0.0,
    g: 0.75,
    b: 0.75,
    a: 1.0,
};
const CLEAR_BLEND: wgt::Color = wgt::Color {
    r: 0.75,
    g: 0.75,
    b: 0.75,
    a: 1.0,
};
const CLEAR_CULL: wgt::Color = wgt::Color {
    r: 0.25,
    g: 0.75,
    b: 0.0,
    a: 1.0,
};

// Exact RGBA8 read-back values for the clear/draw colors above.
const BLACK: [u8; 4] = [0, 0, 0, 255];
const GRAY: [u8; 4] = [191, 191, 191, 255];
const GREEN: [u8; 4] = [0, 255, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];
const YELLOW: [u8; 4] = [255, 255, 0, 255];
const RED: [u8; 4] = [255, 0, 0, 255];
const VIEWPORT_CLEAR: [u8; 4] = [64, 191, 0, 255];
const VIEWPORT_DRAW: [u8; 4] = [191, 64, 0, 255];
const WRITE_MASK_DRAW: [u8; 4] = [255, 191, 191, 255];
const BLEND_DRAW: [u8; 4] = [255, 191, 191, 255];

/// `Bgra8Unorm` read-back stores blue as `[B, G, R, A]`.
const BGRA_BLUE: [u8; 4] = [255, 0, 0, 255];

/// Every entry point used by the test lives in one module so the shader is
/// parsed and validated once.
const SHADER: &str = r#"
struct VertexOutput {
    @builtin(position) position: vec4<f32>,
}

@vertex
fn vs_position(@location(0) position: vec2<f32>) -> VertexOutput {
    var output: VertexOutput;
    output.position = vec4<f32>(position, 0.0, 1.0);
    return output;
}

@vertex
fn vs_depth(@location(0) position: vec2<f32>) -> VertexOutput {
    var output: VertexOutput;
    output.position = vec4<f32>(position, 0.5, 1.0);
    return output;
}

@fragment
fn fs_solid() -> @location(0) vec4<f32> {
    return vec4<f32>(0.75, 0.25, 0.0, 1.0);
}

struct TwoTargetOutput {
    @location(0) target0: vec4<f32>,
    @location(1) target1: vec4<f32>,
}

@fragment
fn fs_two_targets() -> TwoTargetOutput {
    var output: TwoTargetOutput;
    output.target0 = vec4<f32>(0.0, 1.0, 0.0, 1.0);
    output.target1 = vec4<f32>(0.0, 0.0, 1.0, 1.0);
    return output;
}

struct FourTargetOutput {
    @location(0) target0: vec4<f32>,
    @location(1) target1: vec4<f32>,
    @location(2) target2: vec4<f32>,
    @location(3) target3: vec4<f32>,
}

@fragment
fn fs_four_targets() -> FourTargetOutput {
    var output: FourTargetOutput;
    output.target0 = vec4<f32>(1.0, 0.0, 0.0, 1.0);
    output.target1 = vec4<f32>(0.0, 1.0, 0.0, 1.0);
    output.target2 = vec4<f32>(0.0, 0.0, 1.0, 1.0);
    output.target3 = vec4<f32>(1.0, 1.0, 0.0, 1.0);
    return output;
}

struct ColorVertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec4<f32>,
}

@vertex
fn vs_color(
    @location(0) position: vec2<f32>,
    @location(1) color: vec4<f32>,
) -> ColorVertexOutput {
    var output: ColorVertexOutput;
    output.position = vec4<f32>(position, 0.0, 1.0);
    output.color = color;
    return output;
}

@fragment
fn fs_color(input: ColorVertexOutput) -> @location(0) vec4<f32> {
    return input.color;
}

struct InstanceOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) @interpolate(flat) instance: u32,
}

@vertex
fn vs_instance(
    @location(0) position: vec2<f32>,
    @location(1) offset: vec2<f32>,
    @builtin(instance_index) instance: u32,
) -> InstanceOutput {
    var output: InstanceOutput;
    output.position = vec4<f32>(position + offset, 0.0, 1.0);
    output.instance = instance;
    return output;
}

@fragment
fn fs_instance(input: InstanceOutput) -> @location(0) vec4<f32> {
    if input.instance == 0u {
        return vec4<f32>(1.0, 0.0, 0.0, 1.0);
    }
    return vec4<f32>(0.0, 1.0, 0.0, 1.0);
}

@fragment
fn fs_write_mask() -> @location(0) vec4<f32> {
    return vec4<f32>(1.0, 0.0, 1.0, 1.0);
}

@fragment
fn fs_blend() -> @location(0) vec4<f32> {
    return vec4<f32>(0.75, 0.0, 0.0, 0.0);
}

@compute @workgroup_size(1)
fn cs_main() {}
"#;

#[test]
fn dx11_pipelines() {
    if let Err(error) = run() {
        panic!("DX11 pipelines test failed: {error}");
    }
    println!("dx11_pipelines: all pipeline sections passed");
}

fn run() -> Result<(), String> {
    let instance_desc = wgpu_hal::InstanceDescriptor {
        name: "wgpu-hal-dx11-pipelines",
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
        "dx11_pipelines: enumerated {} DXGI adapter(s)",
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
                opened = Some(open);
                break;
            }
            Err(error) => println!("  adapter[{index}] could not be opened: {error:?}"),
        }
    }
    let wgpu_hal::OpenDevice { device, queue } =
        opened.ok_or_else(|| "no enumerated D3D11 adapter could open a device".to_owned())?;
    println!("opened D3D11 device with Features::empty()");

    let shader = create_test_shader(&device)?;
    let layout = create_empty_pipeline_layout(&device, "dx11 pipelines")?;

    section_two_targets_and_depth(&device, &queue, &shader, &layout)?;
    section_four_color_targets(&device, &queue, &shader, &layout)?;
    section_viewport_and_scissor(&device, &queue, &shader, &layout)?;
    section_indexed_and_base_vertex(&device, &queue, &shader, &layout)?;
    section_indirect_draws(&device, &queue, &shader, &layout)?;
    section_instance_step_mode(&device, &queue, &shader, &layout)?;
    section_write_mask_blend_and_culling(&device, &queue, &shader, &layout)?;
    section_topology_and_depth_bias(&device, &shader, &layout)?;
    section_negative_begin_pass(&device, &queue)?;
    section_negative_pipeline_creation(&device, &shader, &layout)?;
    section_negative_encoders(&device, &queue, &shader, &layout)?;

    unsafe {
        device.destroy_pipeline_layout(layout);
        device.destroy_shader_module(shader);
    }

    println!("dx11_pipelines: destroying the queue and device");
    drop(queue);
    drop(device);
    Ok(())
}

// ---------------------------------------------------------------------------
// 1. Two color targets plus a depth attachment
// ---------------------------------------------------------------------------

fn section_two_targets_and_depth(
    device: &wgpu_hal::dx11::Device,
    queue: &wgpu_hal::dx11::Queue,
    shader: &wgpu_hal::dx11::ShaderModule,
    layout: &wgpu_hal::dx11::PipelineLayout,
) -> Result<(), String> {
    println!("\n=== 1. two color targets + depth ===");

    let (rt0, rt0_view) = create_color_target(device, "rt0", wgt::TextureFormat::Rgba8Unorm)?;
    let (rt1, rt1_view) = create_color_target(device, "rt1", wgt::TextureFormat::Bgra8Unorm)?;
    let (depth, depth_view) = create_depth_target(device, "depth")?;

    let attributes = [wgt::VertexAttribute {
        format: wgt::VertexFormat::Float32x2,
        offset: 0,
        shader_location: 0,
    }];
    let vertex_buffers = [Some(wgpu_hal::VertexBufferLayout {
        array_stride: 8,
        step_mode: wgt::VertexStepMode::Vertex,
        attributes: &attributes,
    })];
    let color_targets = [
        Some(wgt::ColorTargetState {
            format: wgt::TextureFormat::Rgba8Unorm,
            blend: None,
            write_mask: wgt::ColorWrites::ALL,
        }),
        Some(wgt::ColorTargetState {
            format: wgt::TextureFormat::Bgra8Unorm,
            blend: None,
            write_mask: wgt::ColorWrites::ALL,
        }),
    ];
    let depth_state = wgt::DepthStencilState {
        format: wgt::TextureFormat::Depth32Float,
        depth_write_enabled: Some(true),
        depth_compare: Some(wgt::CompareFunction::LessEqual),
        stencil: wgt::StencilState::default(),
        bias: wgt::DepthBiasState::default(),
    };
    let pipeline = create_test_pipeline(
        device,
        "two targets + depth",
        layout,
        shader,
        "vs_depth",
        "fs_two_targets",
        &vertex_buffers,
        triangle_primitive(),
        Some(depth_state),
        &color_targets,
    )?;

    // The triangle only covers the middle of the target, so the corner keeps
    // the clear value and the center receives the shader output.
    let vertices: [[f32; 2]; 3] = [[-0.5, -0.5], [0.5, -0.5], [0.0, 0.5]];
    let vertex_bytes = as_bytes(&vertices);
    let vertex_buffer = create_upload_buffer(
        device,
        "target vertices",
        vertex_bytes,
        wgt::BufferUses::VERTEX | wgt::BufferUses::MAP_WRITE,
    )?;

    submit_encoder(device, queue, "two targets + depth", |encoder| {
        let color_attachments = [
            Some(make_color_attachment(&rt0_view, CLEAR_BLACK)),
            Some(make_color_attachment(&rt1_view, CLEAR_GRAY)),
        ];
        let depth_attachment = Some(make_depth_attachment(&depth_view, 1.0));
        let pass = render_pass_descriptor(
            "two targets + depth",
            extent(),
            1,
            &color_attachments,
            depth_attachment,
            None,
        );
        unsafe {
            encoder
                .begin_render_pass(&pass)
                .map_err(|e| format!("begin_render_pass(two targets): {e:?}"))?;
            encoder.set_render_pipeline(&pipeline);
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
        }
        Ok(())
    })?;

    let rt0_data = read_texture(
        device,
        queue,
        "rt0 readback",
        &rt0,
        wgpu_hal::FormatAspects::COLOR,
        WIDTH,
        HEIGHT,
        PITCH,
    )?;
    expect_pixel("RT0 center", &rt0_data, 32, 32, GREEN)?;
    expect_pixel("RT0 corner", &rt0_data, 0, 0, BLACK)?;

    let rt1_data = read_texture(
        device,
        queue,
        "rt1 readback",
        &rt1,
        wgpu_hal::FormatAspects::COLOR,
        WIDTH,
        HEIGHT,
        PITCH,
    )?;
    // RT1 is BGRA8: a blue fragment is [255, 0, 0, 255] and the gray clear is
    // [128, 128, 128, 255].
    expect_pixel("RT1 center", &rt1_data, 32, 32, BGRA_BLUE)?;
    expect_pixel("RT1 corner", &rt1_data, 0, 0, GRAY)?;

    let depth_data = read_texture(
        device,
        queue,
        "depth readback",
        &depth,
        wgpu_hal::FormatAspects::DEPTH,
        WIDTH,
        HEIGHT,
        PITCH,
    )?;
    expect_depth("depth center", &depth_data, 32, 32, 0.5)?;
    expect_depth("depth corner", &depth_data, 0, 0, 1.0)?;

    unsafe {
        device.destroy_render_pipeline(pipeline);
        device.destroy_buffer(vertex_buffer);
        device.destroy_texture_view(rt0_view);
        device.destroy_texture(rt0);
        device.destroy_texture_view(rt1_view);
        device.destroy_texture(rt1);
        device.destroy_texture_view(depth_view);
        device.destroy_texture(depth);
    }
    println!("  two targets, exact color and exact depth verified");
    Ok(())
}

// ---------------------------------------------------------------------------
// 1b. Four color targets (the advertised max_color_attachments limit)
// ---------------------------------------------------------------------------

fn section_four_color_targets(
    device: &wgpu_hal::dx11::Device,
    queue: &wgpu_hal::dx11::Queue,
    shader: &wgpu_hal::dx11::ShaderModule,
    layout: &wgpu_hal::dx11::PipelineLayout,
) -> Result<(), String> {
    println!("\n=== 1b. four color targets ===");

    let (rt0, rt0_view) = create_color_target(device, "four rt0", wgt::TextureFormat::Rgba8Unorm)?;
    let (rt1, rt1_view) = create_color_target(device, "four rt1", wgt::TextureFormat::Rgba8Unorm)?;
    let (rt2, rt2_view) = create_color_target(device, "four rt2", wgt::TextureFormat::Rgba8Unorm)?;
    let (rt3, rt3_view) = create_color_target(device, "four rt3", wgt::TextureFormat::Rgba8Unorm)?;

    let attributes = [wgt::VertexAttribute {
        format: wgt::VertexFormat::Float32x2,
        offset: 0,
        shader_location: 0,
    }];
    let vertex_buffers = [Some(wgpu_hal::VertexBufferLayout {
        array_stride: 8,
        step_mode: wgt::VertexStepMode::Vertex,
        attributes: &attributes,
    })];
    let color_targets = [
        Some(wgt::ColorTargetState {
            format: wgt::TextureFormat::Rgba8Unorm,
            blend: None,
            write_mask: wgt::ColorWrites::ALL,
        }),
        Some(wgt::ColorTargetState {
            format: wgt::TextureFormat::Rgba8Unorm,
            blend: None,
            write_mask: wgt::ColorWrites::ALL,
        }),
        Some(wgt::ColorTargetState {
            format: wgt::TextureFormat::Rgba8Unorm,
            blend: None,
            write_mask: wgt::ColorWrites::ALL,
        }),
        Some(wgt::ColorTargetState {
            format: wgt::TextureFormat::Rgba8Unorm,
            blend: None,
            write_mask: wgt::ColorWrites::ALL,
        }),
    ];
    let pipeline = create_test_pipeline(
        device,
        "four targets",
        layout,
        shader,
        "vs_position",
        "fs_four_targets",
        &vertex_buffers,
        triangle_primitive(),
        None,
        &color_targets,
    )?;

    let vertices: [[f32; 2]; 3] = [[-1.0, -1.0], [3.0, -1.0], [-1.0, 3.0]];
    let vertex_bytes = as_bytes(&vertices);
    let vertex_buffer = create_upload_buffer(
        device,
        "four target vertices",
        vertex_bytes,
        wgt::BufferUses::VERTEX | wgt::BufferUses::MAP_WRITE,
    )?;

    submit_encoder(device, queue, "four targets", |encoder| {
        let color_attachments = [
            Some(make_color_attachment(&rt0_view, CLEAR_BLACK)),
            Some(make_color_attachment(&rt1_view, CLEAR_BLACK)),
            Some(make_color_attachment(&rt2_view, CLEAR_BLACK)),
            Some(make_color_attachment(&rt3_view, CLEAR_BLACK)),
        ];
        let pass =
            render_pass_descriptor("four targets", extent(), 1, &color_attachments, None, None);
        unsafe {
            encoder
                .begin_render_pass(&pass)
                .map_err(|e| format!("begin_render_pass(four targets): {e:?}"))?;
            encoder.set_render_pipeline(&pipeline);
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
        }
        Ok(())
    })?;

    for (label, texture, expected) in [
        ("four rt0", &rt0, RED),
        ("four rt1", &rt1, GREEN),
        ("four rt2", &rt2, BLUE),
        ("four rt3", &rt3, YELLOW),
    ] {
        let data = read_texture(
            device,
            queue,
            label,
            texture,
            wgpu_hal::FormatAspects::COLOR,
            WIDTH,
            HEIGHT,
            PITCH,
        )?;
        expect_pixel(label, &data, 32, 32, expected)?;
    }
    println!("  four color attachments (the advertised limit) read back exactly");

    unsafe {
        device.destroy_render_pipeline(pipeline);
        device.destroy_buffer(vertex_buffer);
        device.destroy_texture_view(rt3_view);
        device.destroy_texture(rt3);
        device.destroy_texture_view(rt2_view);
        device.destroy_texture(rt2);
        device.destroy_texture_view(rt1_view);
        device.destroy_texture(rt1);
        device.destroy_texture_view(rt0_view);
        device.destroy_texture(rt0);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 2. Viewport and scissor
// ---------------------------------------------------------------------------

fn section_viewport_and_scissor(
    device: &wgpu_hal::dx11::Device,
    queue: &wgpu_hal::dx11::Queue,
    shader: &wgpu_hal::dx11::ShaderModule,
    layout: &wgpu_hal::dx11::PipelineLayout,
) -> Result<(), String> {
    println!("\n=== 2. viewport and scissor ===");

    let (target, target_view) =
        create_color_target(device, "viewport target", wgt::TextureFormat::Rgba8Unorm)?;
    let (pipeline, vertex_buffer) =
        create_fullscreen_pipeline(device, shader, layout, "viewport pipeline", "fs_solid")?;

    submit_encoder(device, queue, "viewport and scissor", |encoder| {
        let color_attachments = [Some(make_color_attachment(&target_view, CLEAR_VIEWPORT))];
        let pass = render_pass_descriptor(
            "viewport and scissor",
            extent(),
            1,
            &color_attachments,
            None,
            None,
        );
        unsafe {
            encoder
                .begin_render_pass(&pass)
                .map_err(|e| format!("begin_render_pass(viewport pass 1): {e:?}"))?;
            encoder.set_viewport(
                &wgpu_hal::Rect {
                    x: 16.0,
                    y: 16.0,
                    w: 16.0,
                    h: 16.0,
                },
                0.0..1.0,
            );
            encoder.set_scissor_rect(&wgpu_hal::Rect {
                x: 16,
                y: 16,
                w: 16,
                h: 16,
            });
            encoder.set_render_pipeline(&pipeline);
            encoder.set_vertex_buffer(
                0,
                wgpu_hal::BufferBinding::new_unchecked(
                    &vertex_buffer,
                    0,
                    wgt::BufferSize::new(FULLSCREEN_VERTEX_BYTES),
                ),
            );
            encoder.draw(0, 3, 0, 1);
            encoder.end_render_pass();
        }
        Ok(())
    })?;

    let data = read_texture(
        device,
        queue,
        "viewport pass 1 readback",
        &target,
        wgpu_hal::FormatAspects::COLOR,
        WIDTH,
        HEIGHT,
        PITCH,
    )?;
    expect_pixel(
        "pass 1 inside viewport/scissor",
        &data,
        20,
        20,
        VIEWPORT_DRAW,
    )?;
    expect_pixel("pass 1 outside near corner", &data, 8, 8, VIEWPORT_CLEAR)?;
    expect_pixel("pass 1 outside far corner", &data, 40, 40, VIEWPORT_CLEAR)?;

    submit_encoder(device, queue, "scissor only", |encoder| {
        let color_attachments = [Some(make_color_attachment(&target_view, CLEAR_VIEWPORT))];
        let pass =
            render_pass_descriptor("scissor only", extent(), 1, &color_attachments, None, None);
        unsafe {
            encoder
                .begin_render_pass(&pass)
                .map_err(|e| format!("begin_render_pass(viewport pass 2): {e:?}"))?;
            encoder.set_viewport(
                &wgpu_hal::Rect {
                    x: 0.0,
                    y: 0.0,
                    w: WIDTH as f32,
                    h: HEIGHT as f32,
                },
                0.0..1.0,
            );
            encoder.set_scissor_rect(&wgpu_hal::Rect {
                x: 20,
                y: 20,
                w: 20,
                h: 20,
            });
            encoder.set_render_pipeline(&pipeline);
            encoder.set_vertex_buffer(
                0,
                wgpu_hal::BufferBinding::new_unchecked(
                    &vertex_buffer,
                    0,
                    wgt::BufferSize::new(FULLSCREEN_VERTEX_BYTES),
                ),
            );
            encoder.draw(0, 3, 0, 1);
            encoder.end_render_pass();
        }
        Ok(())
    })?;

    let data = read_texture(
        device,
        queue,
        "viewport pass 2 readback",
        &target,
        wgpu_hal::FormatAspects::COLOR,
        WIDTH,
        HEIGHT,
        PITCH,
    )?;
    expect_pixel("pass 2 inside scissor", &data, 24, 24, VIEWPORT_DRAW)?;
    expect_pixel("pass 2 outside near corner", &data, 8, 8, VIEWPORT_CLEAR)?;
    expect_pixel("pass 2 outside scissor", &data, 40, 40, VIEWPORT_CLEAR)?;

    unsafe {
        device.destroy_render_pipeline(pipeline);
        device.destroy_buffer(vertex_buffer);
        device.destroy_texture_view(target_view);
        device.destroy_texture(target);
    }
    println!("  viewport-only and scissor-only clipping verified at exact pixels");
    Ok(())
}

// ---------------------------------------------------------------------------
// 3. Indexed draw plus base_vertex
// ---------------------------------------------------------------------------

fn section_indexed_and_base_vertex(
    device: &wgpu_hal::dx11::Device,
    queue: &wgpu_hal::dx11::Queue,
    shader: &wgpu_hal::dx11::ShaderModule,
    layout: &wgpu_hal::dx11::PipelineLayout,
) -> Result<(), String> {
    println!("\n=== 3. indexed draw + base_vertex ===");

    let (target, target_view) =
        create_color_target(device, "indexed target", wgt::TextureFormat::Rgba8Unorm)?;
    let (pipeline, vertex_buffer, index_buffer) = create_color_scene(device, shader, layout)?;

    // Draw the second triangle with `base_vertex = 3`. `first_index = 0` and
    // the index buffer only contains [0, 1, 2].
    submit_encoder(device, queue, "indexed base_vertex", |encoder| {
        let color_attachments = [Some(make_color_attachment(&target_view, CLEAR_BLACK))];
        let pass = render_pass_descriptor(
            "indexed base_vertex",
            extent(),
            1,
            &color_attachments,
            None,
            None,
        );
        unsafe {
            encoder
                .begin_render_pass(&pass)
                .map_err(|e| format!("begin_render_pass(indexed): {e:?}"))?;
            encoder.set_render_pipeline(&pipeline);
            encoder.set_vertex_buffer(
                0,
                wgpu_hal::BufferBinding::new_unchecked(
                    &vertex_buffer,
                    0,
                    wgt::BufferSize::new(COLOR_SCENE_VERTEX_BYTES),
                ),
            );
            encoder.set_index_buffer(
                wgpu_hal::BufferBinding::new_unchecked(
                    &index_buffer,
                    0,
                    wgt::BufferSize::new(3 * core::mem::size_of::<u16>() as u64),
                ),
                wgt::IndexFormat::Uint16,
            );
            encoder.draw_indexed(0, 3, 3, 0, 1);
            encoder.end_render_pass();
        }
        Ok(())
    })?;
    let indexed_data = read_texture(
        device,
        queue,
        "indexed base_vertex readback",
        &target,
        wgpu_hal::FormatAspects::COLOR,
        WIDTH,
        HEIGHT,
        PITCH,
    )?;

    // The direct draw fetches vertices 3, 4 and 5 without an index buffer.
    submit_encoder(device, queue, "direct base_vertex", |encoder| {
        let color_attachments = [Some(make_color_attachment(&target_view, CLEAR_BLACK))];
        let pass = render_pass_descriptor(
            "direct base_vertex",
            extent(),
            1,
            &color_attachments,
            None,
            None,
        );
        unsafe {
            encoder
                .begin_render_pass(&pass)
                .map_err(|e| format!("begin_render_pass(direct): {e:?}"))?;
            encoder.set_render_pipeline(&pipeline);
            encoder.set_vertex_buffer(
                0,
                wgpu_hal::BufferBinding::new_unchecked(
                    &vertex_buffer,
                    0,
                    wgt::BufferSize::new(COLOR_SCENE_VERTEX_BYTES),
                ),
            );
            encoder.draw(3, 3, 0, 1);
            encoder.end_render_pass();
        }
        Ok(())
    })?;
    let direct_data = read_texture(
        device,
        queue,
        "direct base_vertex readback",
        &target,
        wgpu_hal::FormatAspects::COLOR,
        WIDTH,
        HEIGHT,
        PITCH,
    )?;

    if indexed_data != direct_data {
        return Err(
            "draw_indexed(base_vertex = 3) did not match draw(first_vertex = 3)".to_owned(),
        );
    }
    expect_pixel("indexed base_vertex center", &indexed_data, 32, 32, BLUE)?;
    expect_pixel("indexed base_vertex corner", &indexed_data, 0, 0, BLUE)?;

    unsafe {
        device.destroy_render_pipeline(pipeline);
        device.destroy_buffer(index_buffer);
        device.destroy_buffer(vertex_buffer);
        device.destroy_texture_view(target_view);
        device.destroy_texture(target);
    }
    println!("  indexed base_vertex readback is byte-identical to the direct draw");
    Ok(())
}

// ---------------------------------------------------------------------------
// 4. Indirect draws
// ---------------------------------------------------------------------------

fn section_indirect_draws(
    device: &wgpu_hal::dx11::Device,
    queue: &wgpu_hal::dx11::Queue,
    shader: &wgpu_hal::dx11::ShaderModule,
    layout: &wgpu_hal::dx11::PipelineLayout,
) -> Result<(), String> {
    println!("\n=== 4. indirect draws ===");

    let (target, target_view) =
        create_color_target(device, "indirect target", wgt::TextureFormat::Rgba8Unorm)?;
    let (pipeline, vertex_buffer, index_buffer) = create_color_scene(device, shader, layout)?;

    // 16 bytes: vertex_count, instance_count, first_vertex, first_instance.
    let draw_args: [u32; 4] = [3, 1, 0, 0];
    let draw_staging = create_upload_buffer(
        device,
        "draw args staging",
        as_bytes(&draw_args),
        wgt::BufferUses::MAP_WRITE | wgt::BufferUses::COPY_SRC,
    )?;
    // The indirect buffer is 32 bytes and the arguments are copied at offset
    // 16, leaving a 16-byte padding area before them.
    let draw_indirect_buffer = create_buffer(
        device,
        "draw args",
        32,
        wgt::BufferUses::INDIRECT | wgt::BufferUses::COPY_DST,
    )?;
    copy_buffer_region(
        device,
        queue,
        "upload draw args",
        &draw_staging,
        &draw_indirect_buffer,
        0,
        16,
        as_bytes(&draw_args).len() as u64,
    )?;

    // Direct draw baseline.
    submit_encoder(device, queue, "direct draw baseline", |encoder| {
        let color_attachments = [Some(make_color_attachment(&target_view, CLEAR_BLACK))];
        let pass = render_pass_descriptor(
            "direct draw baseline",
            extent(),
            1,
            &color_attachments,
            None,
            None,
        );
        unsafe {
            encoder
                .begin_render_pass(&pass)
                .map_err(|e| format!("begin_render_pass(direct draw): {e:?}"))?;
            encoder.set_render_pipeline(&pipeline);
            encoder.set_vertex_buffer(
                0,
                wgpu_hal::BufferBinding::new_unchecked(
                    &vertex_buffer,
                    0,
                    wgt::BufferSize::new(COLOR_SCENE_VERTEX_BYTES),
                ),
            );
            encoder.draw(0, 3, 0, 1);
            encoder.end_render_pass();
        }
        Ok(())
    })?;
    let direct_draw_data = read_texture(
        device,
        queue,
        "direct draw readback",
        &target,
        wgpu_hal::FormatAspects::COLOR,
        WIDTH,
        HEIGHT,
        PITCH,
    )?;

    submit_encoder(device, queue, "indirect draw", |encoder| {
        let color_attachments = [Some(make_color_attachment(&target_view, CLEAR_BLACK))];
        let pass =
            render_pass_descriptor("indirect draw", extent(), 1, &color_attachments, None, None);
        unsafe {
            encoder
                .begin_render_pass(&pass)
                .map_err(|e| format!("begin_render_pass(indirect draw): {e:?}"))?;
            encoder.set_render_pipeline(&pipeline);
            encoder.set_vertex_buffer(
                0,
                wgpu_hal::BufferBinding::new_unchecked(
                    &vertex_buffer,
                    0,
                    wgt::BufferSize::new(COLOR_SCENE_VERTEX_BYTES),
                ),
            );
            encoder.draw_indirect(&draw_indirect_buffer, 16, 1);
            encoder.end_render_pass();
        }
        Ok(())
    })?;
    let indirect_draw_data = read_texture(
        device,
        queue,
        "indirect draw readback",
        &target,
        wgpu_hal::FormatAspects::COLOR,
        WIDTH,
        HEIGHT,
        PITCH,
    )?;

    if direct_draw_data != indirect_draw_data {
        return Err("draw_indirect readback did not match the direct draw".to_owned());
    }
    expect_pixel("draw_indirect center", &indirect_draw_data, 32, 32, RED)?;

    // 20 bytes: index_count, instance_count, first_index, base_vertex,
    // first_instance.
    let indexed_args: [u32; 5] = [3, 1, 0, 0, 0];
    let indexed_staging = create_upload_buffer(
        device,
        "indexed args staging",
        as_bytes(&indexed_args),
        wgt::BufferUses::MAP_WRITE | wgt::BufferUses::COPY_SRC,
    )?;
    let indexed_indirect_buffer = create_buffer(
        device,
        "indexed args",
        64,
        wgt::BufferUses::INDIRECT | wgt::BufferUses::COPY_DST,
    )?;
    copy_buffer_region(
        device,
        queue,
        "upload indexed args",
        &indexed_staging,
        &indexed_indirect_buffer,
        0,
        16,
        as_bytes(&indexed_args).len() as u64,
    )?;

    // Direct indexed baseline.
    submit_encoder(device, queue, "direct indexed baseline", |encoder| {
        let color_attachments = [Some(make_color_attachment(&target_view, CLEAR_BLACK))];
        let pass = render_pass_descriptor(
            "direct indexed baseline",
            extent(),
            1,
            &color_attachments,
            None,
            None,
        );
        unsafe {
            encoder
                .begin_render_pass(&pass)
                .map_err(|e| format!("begin_render_pass(direct indexed): {e:?}"))?;
            encoder.set_render_pipeline(&pipeline);
            encoder.set_vertex_buffer(
                0,
                wgpu_hal::BufferBinding::new_unchecked(
                    &vertex_buffer,
                    0,
                    wgt::BufferSize::new(COLOR_SCENE_VERTEX_BYTES),
                ),
            );
            encoder.set_index_buffer(
                wgpu_hal::BufferBinding::new_unchecked(
                    &index_buffer,
                    0,
                    wgt::BufferSize::new(3 * core::mem::size_of::<u16>() as u64),
                ),
                wgt::IndexFormat::Uint16,
            );
            encoder.draw_indexed(0, 3, 0, 0, 1);
            encoder.end_render_pass();
        }
        Ok(())
    })?;
    let direct_indexed_data = read_texture(
        device,
        queue,
        "direct indexed readback",
        &target,
        wgpu_hal::FormatAspects::COLOR,
        WIDTH,
        HEIGHT,
        PITCH,
    )?;

    submit_encoder(device, queue, "indexed indirect draw", |encoder| {
        let color_attachments = [Some(make_color_attachment(&target_view, CLEAR_BLACK))];
        let pass = render_pass_descriptor(
            "indexed indirect draw",
            extent(),
            1,
            &color_attachments,
            None,
            None,
        );
        unsafe {
            encoder
                .begin_render_pass(&pass)
                .map_err(|e| format!("begin_render_pass(indexed indirect): {e:?}"))?;
            encoder.set_render_pipeline(&pipeline);
            encoder.set_vertex_buffer(
                0,
                wgpu_hal::BufferBinding::new_unchecked(
                    &vertex_buffer,
                    0,
                    wgt::BufferSize::new(COLOR_SCENE_VERTEX_BYTES),
                ),
            );
            encoder.set_index_buffer(
                wgpu_hal::BufferBinding::new_unchecked(
                    &index_buffer,
                    0,
                    wgt::BufferSize::new(3 * core::mem::size_of::<u16>() as u64),
                ),
                wgt::IndexFormat::Uint16,
            );
            encoder.draw_indexed_indirect(&indexed_indirect_buffer, 16, 1);
            encoder.end_render_pass();
        }
        Ok(())
    })?;
    let indexed_indirect_data = read_texture(
        device,
        queue,
        "indexed indirect readback",
        &target,
        wgpu_hal::FormatAspects::COLOR,
        WIDTH,
        HEIGHT,
        PITCH,
    )?;

    if direct_indexed_data != indexed_indirect_data {
        return Err(
            "draw_indexed_indirect readback did not match the direct indexed draw".to_owned(),
        );
    }
    expect_pixel(
        "draw_indexed_indirect center",
        &indexed_indirect_data,
        32,
        32,
        RED,
    )?;

    unsafe {
        device.destroy_render_pipeline(pipeline);
        device.destroy_buffer(indexed_indirect_buffer);
        device.destroy_buffer(draw_indirect_buffer);
        device.destroy_buffer(indexed_staging);
        device.destroy_buffer(draw_staging);
        device.destroy_buffer(index_buffer);
        device.destroy_buffer(vertex_buffer);
        device.destroy_texture_view(target_view);
        device.destroy_texture(target);
    }
    println!("  direct, indirect, indexed and indexed-indirect draws agree byte-for-byte");
    Ok(())
}

// ---------------------------------------------------------------------------
// 5. Per-instance vertex step mode
// ---------------------------------------------------------------------------

fn section_instance_step_mode(
    device: &wgpu_hal::dx11::Device,
    queue: &wgpu_hal::dx11::Queue,
    shader: &wgpu_hal::dx11::ShaderModule,
    layout: &wgpu_hal::dx11::PipelineLayout,
) -> Result<(), String> {
    println!("\n=== 5. per-instance vertex step mode ===");

    let (target, target_view) =
        create_color_target(device, "instance target", wgt::TextureFormat::Rgba8Unorm)?;

    let vertex_attributes = [wgt::VertexAttribute {
        format: wgt::VertexFormat::Float32x2,
        offset: 0,
        shader_location: 0,
    }];
    let instance_attributes = [wgt::VertexAttribute {
        format: wgt::VertexFormat::Float32x2,
        offset: 0,
        shader_location: 1,
    }];
    let vertex_buffers = [
        Some(wgpu_hal::VertexBufferLayout {
            array_stride: 8,
            step_mode: wgt::VertexStepMode::Vertex,
            attributes: &vertex_attributes,
        }),
        Some(wgpu_hal::VertexBufferLayout {
            array_stride: 8,
            step_mode: wgt::VertexStepMode::Instance,
            attributes: &instance_attributes,
        }),
    ];
    let color_targets = [Some(wgt::ColorTargetState {
        format: wgt::TextureFormat::Rgba8Unorm,
        blend: None,
        write_mask: wgt::ColorWrites::ALL,
    })];
    let pipeline = create_test_pipeline(
        device,
        "instance pipeline",
        layout,
        shader,
        "vs_instance",
        "fs_instance",
        &vertex_buffers,
        triangle_primitive(),
        None,
        &color_targets,
    )?;

    // A small centered triangle translated to the left by instance 0 and to
    // the right by instance 1.
    let vertices: [[f32; 2]; 3] = [[-0.25, -0.25], [0.25, -0.25], [0.0, 0.25]];
    let instances: [[f32; 2]; 2] = [[-0.5, 0.0], [0.5, 0.0]];
    let vertex_bytes = as_bytes(&vertices);
    let instance_bytes = as_bytes(&instances);
    let vertex_buffer = create_upload_buffer(
        device,
        "instance vertices",
        vertex_bytes,
        wgt::BufferUses::VERTEX | wgt::BufferUses::MAP_WRITE,
    )?;
    let instance_buffer = create_upload_buffer(
        device,
        "instance offsets",
        instance_bytes,
        wgt::BufferUses::VERTEX | wgt::BufferUses::MAP_WRITE,
    )?;

    submit_encoder(device, queue, "instance step", |encoder| {
        let color_attachments = [Some(make_color_attachment(&target_view, CLEAR_BLACK))];
        let pass =
            render_pass_descriptor("instance step", extent(), 1, &color_attachments, None, None);
        unsafe {
            encoder
                .begin_render_pass(&pass)
                .map_err(|e| format!("begin_render_pass(instance): {e:?}"))?;
            encoder.set_render_pipeline(&pipeline);
            encoder.set_vertex_buffer(
                0,
                wgpu_hal::BufferBinding::new_unchecked(
                    &vertex_buffer,
                    0,
                    wgt::BufferSize::new(vertex_bytes.len() as u64),
                ),
            );
            encoder.set_vertex_buffer(
                1,
                wgpu_hal::BufferBinding::new_unchecked(
                    &instance_buffer,
                    0,
                    wgt::BufferSize::new(instance_bytes.len() as u64),
                ),
            );
            encoder.draw(0, 3, 0, 2);
            encoder.end_render_pass();
        }
        Ok(())
    })?;

    let data = read_texture(
        device,
        queue,
        "instance readback",
        &target,
        wgpu_hal::FormatAspects::COLOR,
        WIDTH,
        HEIGHT,
        PITCH,
    )?;
    expect_pixel("instance 0 region", &data, 16, 32, RED)?;
    expect_pixel("instance 1 region", &data, 48, 32, GREEN)?;
    expect_pixel("instance background", &data, 0, 0, BLACK)?;

    unsafe {
        device.destroy_render_pipeline(pipeline);
        device.destroy_buffer(instance_buffer);
        device.destroy_buffer(vertex_buffer);
        device.destroy_texture_view(target_view);
        device.destroy_texture(target);
    }
    println!("  per-instance offsets and @builtin(instance_index) colors verified");
    Ok(())
}

// ---------------------------------------------------------------------------
// 6. Write mask, blending and culling
// ---------------------------------------------------------------------------

fn section_write_mask_blend_and_culling(
    device: &wgpu_hal::dx11::Device,
    queue: &wgpu_hal::dx11::Queue,
    shader: &wgpu_hal::dx11::ShaderModule,
    layout: &wgpu_hal::dx11::PipelineLayout,
) -> Result<(), String> {
    println!("\n=== 6. write mask, blend and culling ===");

    let (target, target_view) =
        create_color_target(device, "state target", wgt::TextureFormat::Rgba8Unorm)?;

    let attributes = [wgt::VertexAttribute {
        format: wgt::VertexFormat::Float32x2,
        offset: 0,
        shader_location: 0,
    }];
    let vertex_buffers = [Some(wgpu_hal::VertexBufferLayout {
        array_stride: 8,
        step_mode: wgt::VertexStepMode::Vertex,
        attributes: &attributes,
    })];

    let write_mask_targets = [Some(wgt::ColorTargetState {
        format: wgt::TextureFormat::Rgba8Unorm,
        blend: None,
        write_mask: wgt::ColorWrites::RED,
    })];
    let write_mask_pipeline = create_test_pipeline(
        device,
        "write mask pipeline",
        layout,
        shader,
        "vs_position",
        "fs_write_mask",
        &vertex_buffers,
        triangle_primitive(),
        None,
        &write_mask_targets,
    )?;

    let blend_targets = [Some(wgt::ColorTargetState {
        format: wgt::TextureFormat::Rgba8Unorm,
        blend: Some(wgt::BlendState::ADDITIVE),
        write_mask: wgt::ColorWrites::ALL,
    })];
    let blend_pipeline = create_test_pipeline(
        device,
        "blend pipeline",
        layout,
        shader,
        "vs_position",
        "fs_blend",
        &vertex_buffers,
        triangle_primitive(),
        None,
        &blend_targets,
    )?;

    let solid_targets = [Some(wgt::ColorTargetState {
        format: wgt::TextureFormat::Rgba8Unorm,
        blend: None,
        write_mask: wgt::ColorWrites::ALL,
    })];
    let mut cull_front_primitive = triangle_primitive();
    cull_front_primitive.front_face = wgt::FrontFace::Ccw;
    cull_front_primitive.cull_mode = Some(wgt::Face::Front);
    let cull_front_pipeline = create_test_pipeline(
        device,
        "cull front pipeline",
        layout,
        shader,
        "vs_position",
        "fs_solid",
        &vertex_buffers,
        cull_front_primitive,
        None,
        &solid_targets,
    )?;
    let mut cull_back_primitive = triangle_primitive();
    cull_back_primitive.front_face = wgt::FrontFace::Ccw;
    cull_back_primitive.cull_mode = Some(wgt::Face::Back);
    let cull_back_pipeline = create_test_pipeline(
        device,
        "cull back pipeline",
        layout,
        shader,
        "vs_position",
        "fs_solid",
        &vertex_buffers,
        cull_back_primitive,
        None,
        &solid_targets,
    )?;

    let fullscreen: [[f32; 2]; 3] = [[-1.0, -1.0], [3.0, -1.0], [-1.0, 3.0]];
    let fullscreen_bytes = as_bytes(&fullscreen);
    let fullscreen_buffer = create_upload_buffer(
        device,
        "state fullscreen vertices",
        fullscreen_bytes,
        wgt::BufferUses::VERTEX | wgt::BufferUses::MAP_WRITE,
    )?;
    // The culling triangle covers the center but not the corner.
    let centered: [[f32; 2]; 3] = [[-0.5, -0.5], [0.5, -0.5], [0.0, 0.5]];
    let centered_bytes = as_bytes(&centered);
    let centered_buffer = create_upload_buffer(
        device,
        "state centered vertices",
        centered_bytes,
        wgt::BufferUses::VERTEX | wgt::BufferUses::MAP_WRITE,
    )?;

    // Write mask: the fragment writes (1, 0, 1, 1) but only RED is enabled, so
    // green and blue keep the clear values.
    submit_encoder(device, queue, "write mask pass", |encoder| {
        let color_attachments = [Some(make_color_attachment(&target_view, CLEAR_WRITE_MASK))];
        let pass = render_pass_descriptor(
            "write mask pass",
            extent(),
            1,
            &color_attachments,
            None,
            None,
        );
        unsafe {
            encoder
                .begin_render_pass(&pass)
                .map_err(|e| format!("begin_render_pass(write mask): {e:?}"))?;
            encoder.set_render_pipeline(&write_mask_pipeline);
            encoder.set_vertex_buffer(
                0,
                wgpu_hal::BufferBinding::new_unchecked(
                    &fullscreen_buffer,
                    0,
                    wgt::BufferSize::new(fullscreen_bytes.len() as u64),
                ),
            );
            encoder.draw(0, 3, 0, 1);
            encoder.end_render_pass();
        }
        Ok(())
    })?;
    let data = read_texture(
        device,
        queue,
        "write mask readback",
        &target,
        wgpu_hal::FormatAspects::COLOR,
        WIDTH,
        HEIGHT,
        PITCH,
    )?;
    expect_pixel("write mask center", &data, 32, 32, WRITE_MASK_DRAW)?;

    // Additive blending over the 0.75 gray clear. The shader writes red 0.75, so
    // the red channel saturates to 1.0 while green and blue keep the clear
    // value. This is independent of the exact UNORM8 blend rounding.
    submit_encoder(device, queue, "blend pass", |encoder| {
        let color_attachments = [Some(make_color_attachment(&target_view, CLEAR_BLEND))];
        let pass =
            render_pass_descriptor("blend pass", extent(), 1, &color_attachments, None, None);
        unsafe {
            encoder
                .begin_render_pass(&pass)
                .map_err(|e| format!("begin_render_pass(blend): {e:?}"))?;
            encoder.set_render_pipeline(&blend_pipeline);
            encoder.set_vertex_buffer(
                0,
                wgpu_hal::BufferBinding::new_unchecked(
                    &fullscreen_buffer,
                    0,
                    wgt::BufferSize::new(fullscreen_bytes.len() as u64),
                ),
            );
            encoder.draw(0, 3, 0, 1);
            encoder.end_render_pass();
        }
        Ok(())
    })?;
    let data = read_texture(
        device,
        queue,
        "blend readback",
        &target,
        wgpu_hal::FormatAspects::COLOR,
        WIDTH,
        HEIGHT,
        PITCH,
    )?;
    expect_pixel("blend center", &data, 32, 32, BLEND_DRAW)?;

    // Culling: exactly one of front/back culling must leave the triangle
    // visible. Which one depends on the winding convention, so the test only
    // requires an XOR instead of hard-coding an orientation.
    let front_pixel = render_culling_case(
        device,
        queue,
        "cull front pass",
        &target,
        &target_view,
        &cull_front_pipeline,
        &centered_buffer,
        centered_bytes.len() as u64,
    )?;
    let back_pixel = render_culling_case(
        device,
        queue,
        "cull back pass",
        &target,
        &target_view,
        &cull_back_pipeline,
        &centered_buffer,
        centered_bytes.len() as u64,
    )?;
    let front_drawn = front_pixel == VIEWPORT_DRAW;
    let back_drawn = back_pixel == VIEWPORT_DRAW;
    if front_drawn == back_drawn {
        return Err(format!(
            "culling with front_face Ccw must keep exactly one orientation visible, \
             but front culling read {front_pixel:?} and back culling read {back_pixel:?}"
        ));
    }
    let culled_pixel = if front_drawn { back_pixel } else { front_pixel };
    if culled_pixel != VIEWPORT_CLEAR {
        return Err(format!(
            "the culled orientation must leave the clear value {VIEWPORT_CLEAR:?}, \
             got {culled_pixel:?}"
        ));
    }
    println!(
        "  culling: front-face case {} the triangle, back-face case {} it",
        if front_drawn { "drew" } else { "culled" },
        if back_drawn { "drew" } else { "culled" },
    );

    unsafe {
        device.destroy_render_pipeline(cull_back_pipeline);
        device.destroy_render_pipeline(cull_front_pipeline);
        device.destroy_render_pipeline(blend_pipeline);
        device.destroy_render_pipeline(write_mask_pipeline);
        device.destroy_buffer(centered_buffer);
        device.destroy_buffer(fullscreen_buffer);
        device.destroy_texture_view(target_view);
        device.destroy_texture(target);
    }
    println!("  write mask, additive blend and XOR face culling verified");
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn render_culling_case(
    device: &wgpu_hal::dx11::Device,
    queue: &wgpu_hal::dx11::Queue,
    label: &str,
    target: &wgpu_hal::dx11::Texture,
    target_view: &wgpu_hal::dx11::TextureView,
    pipeline: &wgpu_hal::dx11::RenderPipeline,
    vertex_buffer: &wgpu_hal::dx11::Buffer,
    vertex_size: u64,
) -> Result<[u8; 4], String> {
    submit_encoder(device, queue, label, |encoder| {
        let color_attachments = [Some(make_color_attachment(target_view, CLEAR_CULL))];
        let pass = render_pass_descriptor(label, extent(), 1, &color_attachments, None, None);
        unsafe {
            encoder
                .begin_render_pass(&pass)
                .map_err(|e| format!("begin_render_pass({label}): {e:?}"))?;
            encoder.set_render_pipeline(pipeline);
            encoder.set_vertex_buffer(
                0,
                wgpu_hal::BufferBinding::new_unchecked(
                    vertex_buffer,
                    0,
                    wgt::BufferSize::new(vertex_size),
                ),
            );
            encoder.draw(0, 3, 0, 1);
            encoder.end_render_pass();
        }
        Ok(())
    })?;
    let data = read_texture(
        device,
        queue,
        label,
        target,
        wgpu_hal::FormatAspects::COLOR,
        WIDTH,
        HEIGHT,
        PITCH,
    )?;
    pixel(&data, 32, 32)
}

// ---------------------------------------------------------------------------
// 6b. Topology coverage and depth bias/clamp state
// ---------------------------------------------------------------------------

fn section_topology_and_depth_bias(
    device: &wgpu_hal::dx11::Device,
    shader: &wgpu_hal::dx11::ShaderModule,
    layout: &wgpu_hal::dx11::PipelineLayout,
) -> Result<(), String> {
    println!("\n=== 6b. topology and depth bias ===");

    let attributes = [wgt::VertexAttribute {
        format: wgt::VertexFormat::Float32x2,
        offset: 0,
        shader_location: 0,
    }];
    let vertex_buffers = [Some(wgpu_hal::VertexBufferLayout {
        array_stride: 8,
        step_mode: wgt::VertexStepMode::Vertex,
        attributes: &attributes,
    })];
    let color_targets = [Some(wgt::ColorTargetState {
        format: wgt::TextureFormat::Rgba8Unorm,
        blend: None,
        write_mask: wgt::ColorWrites::ALL,
    })];

    // D3D11 primitive topology values: POINTLIST = 1, LINELIST = 2,
    // LINESTRIP = 3, TRIANGLELIST = 4, TRIANGLESTRIP = 5.
    for (topology, expected) in [
        (wgt::PrimitiveTopology::PointList, 1),
        (wgt::PrimitiveTopology::LineList, 2),
        (wgt::PrimitiveTopology::LineStrip, 3),
        (wgt::PrimitiveTopology::TriangleList, 4),
        (wgt::PrimitiveTopology::TriangleStrip, 5),
    ] {
        let mut primitive = triangle_primitive();
        primitive.topology = topology;
        let pipeline = create_test_pipeline(
            device,
            "topology pipeline",
            layout,
            shader,
            "vs_position",
            "fs_solid",
            &vertex_buffers,
            primitive,
            None,
            &color_targets,
        )?;
        if pipeline.topology().0 != expected {
            return Err(format!(
                "topology {topology:?} mapped to {}, expected {expected}",
                pipeline.topology().0
            ));
        }
        unsafe { device.destroy_render_pipeline(pipeline) };
    }
    println!("  all five WebGPU topologies mapped to D3D11");

    let depth_state = wgt::DepthStencilState {
        format: wgt::TextureFormat::Depth32Float,
        depth_write_enabled: Some(true),
        depth_compare: Some(wgt::CompareFunction::LessEqual),
        stencil: wgt::StencilState::default(),
        bias: wgt::DepthBiasState {
            constant: 2,
            slope_scale: 1.5,
            clamp: 0.01,
        },
    };
    let biased_pipeline = create_test_pipeline(
        device,
        "depth bias pipeline",
        layout,
        shader,
        "vs_position",
        "fs_solid",
        &vertex_buffers,
        wgt::PrimitiveState {
            topology: wgt::PrimitiveTopology::TriangleList,
            cull_mode: Some(wgt::Face::Back),
            ..Default::default()
        },
        Some(depth_state),
        &color_targets,
    )?;
    unsafe { device.destroy_render_pipeline(biased_pipeline) };
    println!("  depth bias/clamp, depth compare and culling state accepted");
    Ok(())
}

// ---------------------------------------------------------------------------
// 7a. begin_render_pass negatives
// ---------------------------------------------------------------------------

fn section_negative_begin_pass(
    device: &wgpu_hal::dx11::Device,
    queue: &wgpu_hal::dx11::Queue,
) -> Result<(), String> {
    println!("\n=== 7a. begin_render_pass negatives ===");

    let (target, target_view) = create_color_target(
        device,
        "negative pass target",
        wgt::TextureFormat::Rgba8Unorm,
    )?;
    let (resolve, resolve_view) = create_color_target(
        device,
        "negative pass resolve",
        wgt::TextureFormat::Rgba8Unorm,
    )?;

    let normal_attachments = [Some(make_color_attachment(&target_view, CLEAR_BLACK))];

    let pass = render_pass_descriptor(
        "sample count 4",
        extent(),
        4,
        &normal_attachments,
        None,
        None,
    );
    expect_begin_pass_error(device, queue, "begin_render_pass(sample_count = 4)", &pass)?;

    let pass = render_pass_descriptor(
        "multiview",
        extent(),
        1,
        &normal_attachments,
        None,
        Some(NonZeroU32::new(2).unwrap()),
    );
    expect_begin_pass_error(
        device,
        queue,
        "begin_render_pass(multiview_mask = Some(2))",
        &pass,
    )?;

    let resolve_attachment = wgpu_hal::ColorAttachment {
        target: wgpu_hal::Attachment {
            view: &target_view,
            usage: wgt::TextureUses::COLOR_TARGET,
        },
        depth_slice: None,
        resolve_target: Some(wgpu_hal::Attachment {
            view: &resolve_view,
            usage: wgt::TextureUses::COLOR_TARGET,
        }),
        ops: wgpu_hal::AttachmentOps::LOAD_CLEAR | wgpu_hal::AttachmentOps::STORE,
        clear_value: CLEAR_BLACK,
    };
    let resolve_attachments = [Some(resolve_attachment)];
    let pass = render_pass_descriptor(
        "resolve target",
        extent(),
        1,
        &resolve_attachments,
        None,
        None,
    );
    expect_begin_pass_error(
        device,
        queue,
        "begin_render_pass(resolve_target = Some(..))",
        &pass,
    )?;

    let depth_slice_attachment = wgpu_hal::ColorAttachment {
        target: wgpu_hal::Attachment {
            view: &target_view,
            usage: wgt::TextureUses::COLOR_TARGET,
        },
        depth_slice: Some(0),
        resolve_target: None,
        ops: wgpu_hal::AttachmentOps::LOAD_CLEAR | wgpu_hal::AttachmentOps::STORE,
        clear_value: CLEAR_BLACK,
    };
    let depth_slice_attachments = [Some(depth_slice_attachment)];
    let pass = render_pass_descriptor(
        "depth slice",
        extent(),
        1,
        &depth_slice_attachments,
        None,
        None,
    );
    expect_begin_pass_error(
        device,
        queue,
        "begin_render_pass(depth_slice = Some(0))",
        &pass,
    )?;

    unsafe {
        device.destroy_texture_view(resolve_view);
        device.destroy_texture(resolve);
        device.destroy_texture_view(target_view);
        device.destroy_texture(target);
    }
    println!("  every unsupported begin_render_pass shape returned Err immediately");
    Ok(())
}

// ---------------------------------------------------------------------------
// 7b. Device-level pipeline creation negatives
// ---------------------------------------------------------------------------

fn section_negative_pipeline_creation(
    device: &wgpu_hal::dx11::Device,
    shader: &wgpu_hal::dx11::ShaderModule,
    layout: &wgpu_hal::dx11::PipelineLayout,
) -> Result<(), String> {
    println!("\n=== 7b. pipeline creation negatives ===");

    let attributes = [wgt::VertexAttribute {
        format: wgt::VertexFormat::Float32x2,
        offset: 0,
        shader_location: 0,
    }];
    let vertex_buffers = [Some(wgpu_hal::VertexBufferLayout {
        array_stride: 8,
        step_mode: wgt::VertexStepMode::Vertex,
        attributes: &attributes,
    })];
    let color_targets = [Some(wgt::ColorTargetState {
        format: wgt::TextureFormat::Rgba8Unorm,
        blend: None,
        write_mask: wgt::ColorWrites::ALL,
    })];
    let constants = naga::back::PipelineConstants::default();

    let mut descriptor = negative_pipeline_base(
        "MSAA pipeline",
        layout,
        shader,
        &constants,
        &vertex_buffers,
        &color_targets,
    );
    descriptor.multisample = wgt::MultisampleState {
        count: 4,
        ..Default::default()
    };
    expect_err(
        "create_render_pipeline(multisample.count = 4)",
        unsafe { device.create_render_pipeline(&descriptor) }.map(|_| ()),
    )?;

    let mut descriptor = negative_pipeline_base(
        "multiview pipeline",
        layout,
        shader,
        &constants,
        &vertex_buffers,
        &color_targets,
    );
    descriptor.multiview_mask = Some(NonZeroU32::new(2).unwrap());
    expect_err(
        "create_render_pipeline(multiview_mask = Some(2))",
        unsafe { device.create_render_pipeline(&descriptor) }.map(|_| ()),
    )?;

    let mut descriptor = negative_pipeline_base(
        "mesh pipeline",
        layout,
        shader,
        &constants,
        &vertex_buffers,
        &color_targets,
    );
    descriptor.vertex_processor = wgpu_hal::VertexProcessor::Mesh {
        task_stage: None,
        mesh_stage: wgpu_hal::ProgrammableStage {
            module: shader,
            entry_point: "vs_position",
            constants: &constants,
            zero_initialize_workgroup_memory: false,
        },
    };
    expect_err(
        "create_render_pipeline(VertexProcessor::Mesh)",
        unsafe { device.create_render_pipeline(&descriptor) }.map(|_| ()),
    )?;

    let compute_descriptor = wgpu_hal::ComputePipelineDescriptor {
        label: Some("compute pipeline"),
        layout,
        stage: wgpu_hal::ProgrammableStage {
            module: shader,
            entry_point: "cs_main",
            constants: &constants,
            zero_initialize_workgroup_memory: false,
        },
        cache: None,
    };
    expect_err(
        "create_compute_pipeline",
        unsafe { device.create_compute_pipeline(&compute_descriptor) }.map(|_| ()),
    )?;

    println!("  unsupported render/compute pipelines returned explicit errors");
    Ok(())
}

// ---------------------------------------------------------------------------
// 7c. Encoder-level negatives
// ---------------------------------------------------------------------------

fn section_negative_encoders(
    device: &wgpu_hal::dx11::Device,
    queue: &wgpu_hal::dx11::Queue,
    shader: &wgpu_hal::dx11::ShaderModule,
    layout: &wgpu_hal::dx11::PipelineLayout,
) -> Result<(), String> {
    println!("\n=== 7c. encoder negatives ===");

    let (target, target_view) = create_color_target(
        device,
        "negative encoder target",
        wgt::TextureFormat::Rgba8Unorm,
    )?;
    let (depth, depth_view) = create_depth_target(device, "negative encoder depth")?;

    let attributes = [wgt::VertexAttribute {
        format: wgt::VertexFormat::Float32x2,
        offset: 0,
        shader_location: 0,
    }];
    let vertex_buffers = [Some(wgpu_hal::VertexBufferLayout {
        array_stride: 8,
        step_mode: wgt::VertexStepMode::Vertex,
        attributes: &attributes,
    })];
    let single_target = [Some(wgt::ColorTargetState {
        format: wgt::TextureFormat::Rgba8Unorm,
        blend: None,
        write_mask: wgt::ColorWrites::ALL,
    })];
    let two_targets = [
        Some(wgt::ColorTargetState {
            format: wgt::TextureFormat::Rgba8Unorm,
            blend: None,
            write_mask: wgt::ColorWrites::ALL,
        }),
        Some(wgt::ColorTargetState {
            format: wgt::TextureFormat::Rgba8Unorm,
            blend: None,
            write_mask: wgt::ColorWrites::ALL,
        }),
    ];

    let single_pipeline = create_test_pipeline(
        device,
        "negative single target",
        layout,
        shader,
        "vs_position",
        "fs_solid",
        &vertex_buffers,
        triangle_primitive(),
        None,
        &single_target,
    )?;
    let mismatch_pipeline = create_test_pipeline(
        device,
        "negative two targets",
        layout,
        shader,
        "vs_position",
        "fs_two_targets",
        &vertex_buffers,
        triangle_primitive(),
        None,
        &two_targets,
    )?;

    let fullscreen: [[f32; 2]; 3] = [[-1.0, -1.0], [3.0, -1.0], [-1.0, 3.0]];
    let fullscreen_bytes = as_bytes(&fullscreen);
    let vertex_buffer = create_upload_buffer(
        device,
        "negative vertices",
        fullscreen_bytes,
        wgt::BufferUses::VERTEX | wgt::BufferUses::MAP_WRITE,
    )?;
    let vertex_size = fullscreen_bytes.len() as u64;

    let indirect_buffer = create_buffer(
        device,
        "negative indirect args",
        32,
        wgt::BufferUses::INDIRECT | wgt::BufferUses::COPY_DST,
    )?;
    let count_buffer = create_buffer(
        device,
        "negative count buffer",
        16,
        wgt::BufferUses::INDIRECT | wgt::BufferUses::COPY_DST,
    )?;

    // A pipeline with two color targets against a pass with one attachment:
    // set_render_pipeline and the following draw must record errors. The draw
    // must not run, which is proven by reading the pass clear value back.
    expect_encoder_error(
        device,
        queue,
        "pipeline/pass color target mismatch",
        |encoder| {
            let color_attachments = [Some(make_color_attachment(&target_view, CLEAR_VIEWPORT))];
            let pass = render_pass_descriptor(
                "pipeline/pass color target mismatch",
                extent(),
                1,
                &color_attachments,
                None,
                None,
            );
            unsafe {
                encoder
                    .begin_render_pass(&pass)
                    .map_err(|e| format!("begin_render_pass(mismatch): {e:?}"))?;
                encoder.set_render_pipeline(&mismatch_pipeline);
                encoder.set_vertex_buffer(
                    0,
                    wgpu_hal::BufferBinding::new_unchecked(
                        &vertex_buffer,
                        0,
                        wgt::BufferSize::new(vertex_size),
                    ),
                );
                encoder.draw(0, 3, 0, 1);
                encoder.end_render_pass();
            }
            Ok(())
        },
    )?;
    let data = read_texture(
        device,
        queue,
        "mismatch readback",
        &target,
        wgpu_hal::FormatAspects::COLOR,
        WIDTH,
        HEIGHT,
        PITCH,
    )?;
    expect_pixel(
        "mismatched pipeline did not draw",
        &data,
        32,
        32,
        VIEWPORT_CLEAR,
    )?;

    // A pipeline without a depth attachment against a pass that has one.
    expect_encoder_error(device, queue, "pipeline/pass depth mismatch", |encoder| {
        let color_attachments = [Some(make_color_attachment(&target_view, CLEAR_BLACK))];
        let depth_attachment = Some(make_depth_attachment(&depth_view, 1.0));
        let pass = render_pass_descriptor(
            "pipeline/pass depth mismatch",
            extent(),
            1,
            &color_attachments,
            depth_attachment,
            None,
        );
        unsafe {
            encoder
                .begin_render_pass(&pass)
                .map_err(|e| format!("begin_render_pass(depth mismatch): {e:?}"))?;
            encoder.set_render_pipeline(&single_pipeline);
            encoder.set_vertex_buffer(
                0,
                wgpu_hal::BufferBinding::new_unchecked(
                    &vertex_buffer,
                    0,
                    wgt::BufferSize::new(vertex_size),
                ),
            );
            encoder.draw(0, 3, 0, 1);
            encoder.end_render_pass();
        }
        Ok(())
    })?;

    // draw_indirect_count and draw_indexed_indirect_count are not implemented.
    expect_encoder_error(device, queue, "draw_indirect_count", |encoder| {
        let color_attachments = [Some(make_color_attachment(&target_view, CLEAR_BLACK))];
        let pass = render_pass_descriptor(
            "draw_indirect_count",
            extent(),
            1,
            &color_attachments,
            None,
            None,
        );
        unsafe {
            encoder
                .begin_render_pass(&pass)
                .map_err(|e| format!("begin_render_pass(draw_indirect_count): {e:?}"))?;
            encoder.set_render_pipeline(&single_pipeline);
            encoder.set_vertex_buffer(
                0,
                wgpu_hal::BufferBinding::new_unchecked(
                    &vertex_buffer,
                    0,
                    wgt::BufferSize::new(vertex_size),
                ),
            );
            encoder.draw_indirect_count(&indirect_buffer, 0, &count_buffer, 0, 1);
            encoder.end_render_pass();
        }
        Ok(())
    })?;

    expect_encoder_error(device, queue, "draw_indexed_indirect_count", |encoder| {
        let color_attachments = [Some(make_color_attachment(&target_view, CLEAR_BLACK))];
        let pass = render_pass_descriptor(
            "draw_indexed_indirect_count",
            extent(),
            1,
            &color_attachments,
            None,
            None,
        );
        unsafe {
            encoder
                .begin_render_pass(&pass)
                .map_err(|e| format!("begin_render_pass(draw_indexed_indirect_count): {e:?}"))?;
            encoder.set_render_pipeline(&single_pipeline);
            encoder.set_vertex_buffer(
                0,
                wgpu_hal::BufferBinding::new_unchecked(
                    &vertex_buffer,
                    0,
                    wgt::BufferSize::new(vertex_size),
                ),
            );
            encoder.draw_indexed_indirect_count(&indirect_buffer, 0, &count_buffer, 0, 1);
            encoder.end_render_pass();
        }
        Ok(())
    })?;

    // Mesh shader draws are not implemented.
    expect_encoder_error(device, queue, "draw_mesh_tasks", |encoder| {
        let color_attachments = [Some(make_color_attachment(&target_view, CLEAR_BLACK))];
        let pass = render_pass_descriptor(
            "draw_mesh_tasks",
            extent(),
            1,
            &color_attachments,
            None,
            None,
        );
        unsafe {
            encoder
                .begin_render_pass(&pass)
                .map_err(|e| format!("begin_render_pass(draw_mesh_tasks): {e:?}"))?;
            encoder.set_render_pipeline(&single_pipeline);
            encoder.draw_mesh_tasks(1, 1, 1);
            encoder.end_render_pass();
        }
        Ok(())
    })?;

    // Compute passes and dispatches are not implemented.
    expect_encoder_error(device, queue, "compute pass", |encoder| {
        let descriptor = wgpu_hal::ComputePassDescriptor {
            label: Some("compute pass"),
            timestamp_writes: None,
        };
        unsafe {
            encoder.begin_compute_pass(&descriptor);
            encoder.dispatch_workgroups([1, 1, 1]);
            encoder.end_compute_pass();
        }
        Ok(())
    })?;

    // set_render_pipeline is only valid inside a render pass.
    expect_encoder_error(
        device,
        queue,
        "set_render_pipeline outside pass",
        |encoder| {
            unsafe { encoder.set_render_pipeline(&single_pipeline) };
            Ok(())
        },
    )?;

    // An indexed draw without an index buffer must fail the encoder.
    expect_encoder_error(
        device,
        queue,
        "draw_indexed without index buffer",
        |encoder| {
            let color_attachments = [Some(make_color_attachment(&target_view, CLEAR_BLACK))];
            let pass = render_pass_descriptor(
                "draw_indexed without index buffer",
                extent(),
                1,
                &color_attachments,
                None,
                None,
            );
            unsafe {
                encoder
                    .begin_render_pass(&pass)
                    .map_err(|e| format!("begin_render_pass(missing index buffer): {e:?}"))?;
                encoder.set_render_pipeline(&single_pipeline);
                encoder.set_vertex_buffer(
                    0,
                    wgpu_hal::BufferBinding::new_unchecked(
                        &vertex_buffer,
                        0,
                        wgt::BufferSize::new(vertex_size),
                    ),
                );
                encoder.draw_indexed(0, 3, 0, 0, 1);
                encoder.end_render_pass();
            }
            Ok(())
        },
    )?;

    unsafe {
        device.destroy_render_pipeline(mismatch_pipeline);
        device.destroy_render_pipeline(single_pipeline);
        device.destroy_buffer(count_buffer);
        device.destroy_buffer(indirect_buffer);
        device.destroy_buffer(vertex_buffer);
        device.destroy_texture_view(depth_view);
        device.destroy_texture(depth);
        device.destroy_texture_view(target_view);
        device.destroy_texture(target);
    }
    println!("  every unsupported encoder operation failed end_encoding");
    Ok(())
}

// ---------------------------------------------------------------------------
// Shared shader, pipeline and resource helpers
// ---------------------------------------------------------------------------

fn create_test_shader(
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
                label: Some("dx11 pipelines shader"),
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

fn create_empty_pipeline_layout(
    device: &wgpu_hal::dx11::Device,
    label: &str,
) -> Result<wgpu_hal::dx11::PipelineLayout, String> {
    unsafe {
        device.create_pipeline_layout(&wgpu_hal::PipelineLayoutDescriptor {
            label: Some(label),
            flags: wgpu_hal::PipelineLayoutFlags::empty(),
            bind_group_layouts: &[],
            immediate_size: 0,
        })
    }
    .map_err(|e| format!("create_pipeline_layout({label}): {e:?}"))
}

#[allow(clippy::too_many_arguments)]
fn create_test_pipeline(
    device: &wgpu_hal::dx11::Device,
    label: &str,
    layout: &wgpu_hal::dx11::PipelineLayout,
    shader: &wgpu_hal::dx11::ShaderModule,
    vertex_entry: &str,
    fragment_entry: &str,
    vertex_buffers: &[Option<wgpu_hal::VertexBufferLayout<'_>>],
    primitive: wgt::PrimitiveState,
    depth_stencil: Option<wgt::DepthStencilState>,
    color_targets: &[Option<wgt::ColorTargetState>],
) -> Result<wgpu_hal::dx11::RenderPipeline, String> {
    let constants = naga::back::PipelineConstants::default();
    unsafe {
        device.create_render_pipeline(&wgpu_hal::RenderPipelineDescriptor {
            label: Some(label),
            layout,
            vertex_processor: wgpu_hal::VertexProcessor::Standard {
                vertex_buffers,
                vertex_stage: wgpu_hal::ProgrammableStage {
                    module: shader,
                    entry_point: vertex_entry,
                    constants: &constants,
                    zero_initialize_workgroup_memory: false,
                },
            },
            primitive,
            depth_stencil,
            multisample: wgt::MultisampleState::default(),
            fragment_stage: Some(wgpu_hal::ProgrammableStage {
                module: shader,
                entry_point: fragment_entry,
                constants: &constants,
                zero_initialize_workgroup_memory: false,
            }),
            color_targets,
            multiview_mask: None,
            cache: None,
        })
    }
    .map_err(|e| format!("create_render_pipeline({label}): {e:?}"))
}

type TestRenderPipelineDescriptor<'a> = wgpu_hal::RenderPipelineDescriptor<
    'a,
    wgpu_hal::dx11::PipelineLayout,
    wgpu_hal::dx11::ShaderModule,
    wgpu_hal::dx11::PipelineCache,
>;

fn negative_pipeline_base<'a>(
    label: &'a str,
    layout: &'a wgpu_hal::dx11::PipelineLayout,
    shader: &'a wgpu_hal::dx11::ShaderModule,
    constants: &'a naga::back::PipelineConstants,
    vertex_buffers: &'a [Option<wgpu_hal::VertexBufferLayout<'a>>],
    color_targets: &'a [Option<wgt::ColorTargetState>],
) -> TestRenderPipelineDescriptor<'a> {
    wgpu_hal::RenderPipelineDescriptor {
        label: Some(label),
        layout,
        vertex_processor: wgpu_hal::VertexProcessor::Standard {
            vertex_buffers,
            vertex_stage: wgpu_hal::ProgrammableStage {
                module: shader,
                entry_point: "vs_position",
                constants,
                zero_initialize_workgroup_memory: false,
            },
        },
        primitive: triangle_primitive(),
        depth_stencil: None,
        multisample: wgt::MultisampleState::default(),
        fragment_stage: Some(wgpu_hal::ProgrammableStage {
            module: shader,
            entry_point: "fs_solid",
            constants,
            zero_initialize_workgroup_memory: false,
        }),
        color_targets,
        multiview_mask: None,
        cache: None,
    }
}

fn triangle_primitive() -> wgt::PrimitiveState {
    wgt::PrimitiveState {
        topology: wgt::PrimitiveTopology::TriangleList,
        cull_mode: None,
        ..Default::default()
    }
}

fn extent() -> wgt::Extent3d {
    wgt::Extent3d {
        width: WIDTH,
        height: HEIGHT,
        depth_or_array_layers: 1,
    }
}

fn render_pass_descriptor<'a>(
    label: &'a str,
    extent: wgt::Extent3d,
    sample_count: u32,
    color_attachments: &'a [Option<wgpu_hal::ColorAttachment<'a, wgpu_hal::dx11::TextureView>>],
    depth_stencil_attachment: Option<
        wgpu_hal::DepthStencilAttachment<'a, wgpu_hal::dx11::TextureView>,
    >,
    multiview_mask: Option<NonZeroU32>,
) -> wgpu_hal::RenderPassDescriptor<'a, wgpu_hal::dx11::Resource, wgpu_hal::dx11::TextureView> {
    wgpu_hal::RenderPassDescriptor {
        label: Some(label),
        extent,
        sample_count,
        color_attachments,
        depth_stencil_attachment,
        multiview_mask,
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

fn make_depth_attachment<'a>(
    view: &'a wgpu_hal::dx11::TextureView,
    clear_value: f32,
) -> wgpu_hal::DepthStencilAttachment<'a, wgpu_hal::dx11::TextureView> {
    wgpu_hal::DepthStencilAttachment {
        target: wgpu_hal::Attachment {
            view,
            usage: wgt::TextureUses::DEPTH_STENCIL_WRITE,
        },
        depth_ops: wgpu_hal::AttachmentOps::LOAD_CLEAR | wgpu_hal::AttachmentOps::STORE,
        stencil_ops: wgpu_hal::AttachmentOps::empty(),
        clear_value: (clear_value, 0),
    }
}

fn create_color_target(
    device: &wgpu_hal::dx11::Device,
    label: &str,
    format: wgt::TextureFormat,
) -> Result<(wgpu_hal::dx11::Texture, wgpu_hal::dx11::TextureView), String> {
    let texture = create_texture(
        device,
        label,
        format,
        wgt::TextureUses::COLOR_TARGET | wgt::TextureUses::COPY_SRC,
    )?;
    let view = create_view(
        device,
        &texture,
        label,
        format,
        wgt::TextureUses::COLOR_TARGET,
        wgt::TextureAspect::All,
    )?;
    Ok((texture, view))
}

fn create_depth_target(
    device: &wgpu_hal::dx11::Device,
    label: &str,
) -> Result<(wgpu_hal::dx11::Texture, wgpu_hal::dx11::TextureView), String> {
    let format = wgt::TextureFormat::Depth32Float;
    let texture = create_texture(
        device,
        label,
        format,
        wgt::TextureUses::DEPTH_STENCIL_WRITE | wgt::TextureUses::COPY_SRC,
    )?;
    let view = create_view(
        device,
        &texture,
        label,
        format,
        wgt::TextureUses::DEPTH_STENCIL_WRITE,
        wgt::TextureAspect::All,
    )?;
    Ok((texture, view))
}

fn create_texture(
    device: &wgpu_hal::dx11::Device,
    label: &str,
    format: wgt::TextureFormat,
    usage: wgt::TextureUses,
) -> Result<wgpu_hal::dx11::Texture, String> {
    unsafe {
        device.create_texture(&wgpu_hal::TextureDescriptor {
            label: Some(label),
            size: extent(),
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgt::TextureDimension::D2,
            format,
            usage,
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
    format: wgt::TextureFormat,
    usage: wgt::TextureUses,
    aspect: wgt::TextureAspect,
) -> Result<wgpu_hal::dx11::TextureView, String> {
    unsafe {
        device.create_texture_view(
            texture,
            &wgpu_hal::TextureViewDescriptor {
                label: Some(label),
                format,
                dimension: wgt::TextureViewDimension::D2,
                usage,
                range: wgt::ImageSubresourceRange {
                    aspect,
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

fn create_upload_buffer(
    device: &wgpu_hal::dx11::Device,
    label: &str,
    data: &[u8],
    usage: wgt::BufferUses,
) -> Result<wgpu_hal::dx11::Buffer, String> {
    let buffer = create_buffer(device, label, data.len() as u64, usage)?;
    if let Err(error) = write_buffer(device, label, &buffer, data) {
        unsafe { device.destroy_buffer(buffer) };
        return Err(error);
    }
    Ok(buffer)
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

const FULLSCREEN_VERTEX_BYTES: u64 = 24;
const COLOR_SCENE_VERTEX_BYTES: u64 = 6 * 6 * 4;

fn create_fullscreen_pipeline(
    device: &wgpu_hal::dx11::Device,
    shader: &wgpu_hal::dx11::ShaderModule,
    layout: &wgpu_hal::dx11::PipelineLayout,
    label: &str,
    fragment_entry: &str,
) -> Result<(wgpu_hal::dx11::RenderPipeline, wgpu_hal::dx11::Buffer), String> {
    let attributes = [wgt::VertexAttribute {
        format: wgt::VertexFormat::Float32x2,
        offset: 0,
        shader_location: 0,
    }];
    let vertex_buffers = [Some(wgpu_hal::VertexBufferLayout {
        array_stride: 8,
        step_mode: wgt::VertexStepMode::Vertex,
        attributes: &attributes,
    })];
    let color_targets = [Some(wgt::ColorTargetState {
        format: wgt::TextureFormat::Rgba8Unorm,
        blend: None,
        write_mask: wgt::ColorWrites::ALL,
    })];
    let pipeline = create_test_pipeline(
        device,
        label,
        layout,
        shader,
        "vs_position",
        fragment_entry,
        &vertex_buffers,
        triangle_primitive(),
        None,
        &color_targets,
    )?;

    let vertices: [[f32; 2]; 3] = [[-1.0, -1.0], [3.0, -1.0], [-1.0, 3.0]];
    let buffer = match create_upload_buffer(
        device,
        label,
        as_bytes(&vertices),
        wgt::BufferUses::VERTEX | wgt::BufferUses::MAP_WRITE,
    ) {
        Ok(buffer) => buffer,
        Err(error) => {
            unsafe { device.destroy_render_pipeline(pipeline) };
            return Err(error);
        }
    };
    Ok((pipeline, buffer))
}

/// Two fullscreen triangles in one vertex buffer: vertices 0..=2 are red and
/// vertices 3..=5 are blue. The index buffer only contains `[0, 1, 2]`, so a
/// draw over vertices 3..=5 needs `base_vertex = 3`.
fn create_color_scene(
    device: &wgpu_hal::dx11::Device,
    shader: &wgpu_hal::dx11::ShaderModule,
    layout: &wgpu_hal::dx11::PipelineLayout,
) -> Result<
    (
        wgpu_hal::dx11::RenderPipeline,
        wgpu_hal::dx11::Buffer,
        wgpu_hal::dx11::Buffer,
    ),
    String,
> {
    let attributes = [
        wgt::VertexAttribute {
            format: wgt::VertexFormat::Float32x2,
            offset: 0,
            shader_location: 0,
        },
        wgt::VertexAttribute {
            format: wgt::VertexFormat::Float32x4,
            offset: 8,
            shader_location: 1,
        },
    ];
    let vertex_buffers = [Some(wgpu_hal::VertexBufferLayout {
        array_stride: 24,
        step_mode: wgt::VertexStepMode::Vertex,
        attributes: &attributes,
    })];
    let color_targets = [Some(wgt::ColorTargetState {
        format: wgt::TextureFormat::Rgba8Unorm,
        blend: None,
        write_mask: wgt::ColorWrites::ALL,
    })];
    let pipeline = create_test_pipeline(
        device,
        "position + color pipeline",
        layout,
        shader,
        "vs_color",
        "fs_color",
        &vertex_buffers,
        triangle_primitive(),
        None,
        &color_targets,
    )?;

    let vertices: [[f32; 6]; 6] = [
        [-1.0, -1.0, 1.0, 0.0, 0.0, 1.0],
        [3.0, -1.0, 1.0, 0.0, 0.0, 1.0],
        [-1.0, 3.0, 1.0, 0.0, 0.0, 1.0],
        [-1.0, -1.0, 0.0, 0.0, 1.0, 1.0],
        [3.0, -1.0, 0.0, 0.0, 1.0, 1.0],
        [-1.0, 3.0, 0.0, 0.0, 1.0, 1.0],
    ];
    let vertex_buffer = match create_upload_buffer(
        device,
        "color scene vertices",
        as_bytes(&vertices),
        wgt::BufferUses::VERTEX | wgt::BufferUses::MAP_WRITE,
    ) {
        Ok(buffer) => buffer,
        Err(error) => {
            unsafe { device.destroy_render_pipeline(pipeline) };
            return Err(error);
        }
    };

    // Six meaningful index bytes plus two bytes of padding: D3D11 buffer
    // widths are rounded to four bytes anyway.
    let indices: [u16; 4] = [0, 1, 2, 0];
    let index_buffer = match create_upload_buffer(
        device,
        "color scene indices",
        as_bytes(&indices),
        wgt::BufferUses::INDEX | wgt::BufferUses::MAP_WRITE,
    ) {
        Ok(buffer) => buffer,
        Err(error) => {
            unsafe {
                device.destroy_buffer(vertex_buffer);
                device.destroy_render_pipeline(pipeline);
            }
            return Err(error);
        }
    };

    Ok((pipeline, vertex_buffer, index_buffer))
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

/// Accepts only a failure recorded by the encoder. The encoder is always
/// discarded and nothing is submitted, so a rejected command cannot leak into
/// a later test section.
fn expect_encoder_error<F>(
    device: &wgpu_hal::dx11::Device,
    queue: &wgpu_hal::dx11::Queue,
    description: &str,
    encode: F,
) -> Result<(), String>
where
    F: FnOnce(&mut wgpu_hal::dx11::CommandBuffer) -> Result<(), String>,
{
    let mut encoder = unsafe {
        device.create_command_encoder(&wgpu_hal::CommandEncoderDescriptor {
            label: Some(description),
            queue,
        })
    }
    .map_err(|e| format!("create_command_encoder({description}): {e:?}"))?;
    unsafe { encoder.begin_encoding(Some(description)) }
        .map_err(|e| format!("begin_encoding({description}): {e:?}"))?;

    if let Err(error) = encode(&mut encoder) {
        unsafe { encoder.discard_encoding() };
        return Err(error);
    }

    let result = unsafe { encoder.end_encoding() };
    unsafe { encoder.discard_encoding() };
    match result {
        Ok(_) => Err(format!(
            "{description}: expected end_encoding to fail, but it succeeded"
        )),
        Err(error) => {
            println!("  rejected as expected: {description} (end_encoding: {error:?})");
            Ok(())
        }
    }
}

fn expect_begin_pass_error(
    device: &wgpu_hal::dx11::Device,
    queue: &wgpu_hal::dx11::Queue,
    description: &str,
    descriptor: &wgpu_hal::RenderPassDescriptor<
        '_,
        wgpu_hal::dx11::Resource,
        wgpu_hal::dx11::TextureView,
    >,
) -> Result<(), String> {
    let mut encoder = unsafe {
        device.create_command_encoder(&wgpu_hal::CommandEncoderDescriptor {
            label: Some(description),
            queue,
        })
    }
    .map_err(|e| format!("create_command_encoder({description}): {e:?}"))?;
    unsafe { encoder.begin_encoding(Some(description)) }
        .map_err(|e| format!("begin_encoding({description}): {e:?}"))?;
    let result = unsafe { encoder.begin_render_pass(descriptor) };
    unsafe { encoder.discard_encoding() };
    match result {
        Ok(()) => Err(format!(
            "{description}: expected begin_render_pass to fail, but it succeeded"
        )),
        Err(error) => {
            println!("  rejected as expected: {description} ({error:?})");
            Ok(())
        }
    }
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

fn read_texture(
    device: &wgpu_hal::dx11::Device,
    queue: &wgpu_hal::dx11::Queue,
    label: &str,
    texture: &wgpu_hal::dx11::Texture,
    aspect: wgpu_hal::FormatAspects,
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
    })?;
    let data = map_read(device, &readback, size)?;
    unsafe { device.destroy_buffer(readback) };
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

fn depth_at(data: &[u8], x: u32, y: u32) -> Result<f32, String> {
    let offset = (y * PITCH + x * 4) as usize;
    let bytes = data
        .get(offset..offset + 4)
        .ok_or_else(|| format!("read-back buffer is too short for depth ({x}, {y})"))?;
    Ok(f32::from_ne_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

fn expect_depth(context: &str, data: &[u8], x: u32, y: u32, expected: f32) -> Result<(), String> {
    let actual = depth_at(data, x, y)?;
    if actual != expected {
        return Err(format!(
            "{context}: depth ({x}, {y}) is {actual:?}, expected {expected:?}"
        ));
    }
    println!("  {context}: depth ({x}, {y}) = {actual:?}");
    Ok(())
}

fn as_bytes<T>(data: &[T]) -> &[u8] {
    unsafe { core::slice::from_raw_parts(data.as_ptr().cast::<u8>(), core::mem::size_of_val(data)) }
}

//! D3D11 bind group and shader compilation smoke test.
//!
//! Opens the experimental `wgpu-hal` Direct3D 11 backend without a window,
//! creates a bind group layout, pipeline layout and CPU-side bind group,
//! translates a small WGSL shader to HLSL, and compiles its vertex and fragment
//! entry points with FXC for Shader Model 5.0. No pipeline or draw is created.
//!
//! Run on Windows with:
//!
//! ```text
//! cargo run -p wgpu-hal --example raw-dx11-shader --features dx11
//! ```

#[cfg(target_os = "windows")]
use std::borrow::Cow;

#[cfg(target_os = "windows")]
use wgpu_hal::{Adapter as _, Device as _, Instance as _};
#[cfg(target_os = "windows")]
use wgpu_types as wgt;

#[cfg(target_os = "windows")]
const SHADER: &str = r#"
struct Globals {
    color: vec4<f32>,
    scale: f32,
}

@group(0) @binding(0) var<uniform> globals: Globals;
@group(0) @binding(1) var albedo: texture_2d<f32>;
@group(0) @binding(2) var albedo_sampler: sampler;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> VertexOutput {
    var positions = array<vec2<f32>, 3>(
        vec2<f32>(0.0, 0.5),
        vec2<f32>(-0.5, -0.5),
        vec2<f32>(0.5, -0.5),
    );
    let position = positions[vertex_index];

    var output: VertexOutput;
    output.position = vec4<f32>(position * globals.scale, 0.0, 1.0);
    output.uv = position + vec2<f32>(0.5);
    return output;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    return textureSample(albedo, albedo_sampler, input.uv) * globals.color;
}
"#;

#[cfg(target_os = "windows")]
fn main() {
    env_logger::init();

    if let Err(e) = run() {
        eprintln!("raw-dx11-shader failed: {e}");
        std::process::exit(1);
    }

    println!("raw-dx11-shader completed successfully");
}

#[cfg(target_os = "windows")]
fn run() -> Result<(), Box<dyn std::error::Error>> {
    let instance_desc = wgpu_hal::InstanceDescriptor {
        name: "raw-dx11-shader",
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

    let wgpu_hal::OpenDevice { device, queue: _ } = unsafe {
        exposed.adapter.open(
            wgt::Features::empty(),
            &exposed.capabilities.limits,
            &wgt::MemoryHints::default(),
        )?
    };
    println!("opened D3D11 device");

    let layout_entries = [
        wgt::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgt::ShaderStages::VERTEX_FRAGMENT,
            ty: wgt::BindingType::Buffer {
                ty: wgt::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: wgt::BufferSize::new(32),
            },
            count: None,
        },
        wgt::BindGroupLayoutEntry {
            binding: 1,
            visibility: wgt::ShaderStages::FRAGMENT,
            ty: wgt::BindingType::Texture {
                sample_type: wgt::TextureSampleType::Float { filterable: true },
                view_dimension: wgt::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        },
        wgt::BindGroupLayoutEntry {
            binding: 2,
            visibility: wgt::ShaderStages::FRAGMENT,
            ty: wgt::BindingType::Sampler(wgt::SamplerBindingType::Filtering),
            count: None,
        },
    ];

    let bind_group_layout = unsafe {
        device.create_bind_group_layout(&wgpu_hal::BindGroupLayoutDescriptor {
            label: Some("shader smoke bind group layout"),
            flags: wgpu_hal::BindGroupLayoutFlags::empty(),
            entries: &layout_entries,
        })?
    };
    println!(
        "created bind group layout with {} entries",
        bind_group_layout.entries().len()
    );

    let pipeline_layout = unsafe {
        device.create_pipeline_layout(&wgpu_hal::PipelineLayoutDescriptor {
            label: Some("shader smoke pipeline layout"),
            flags: wgpu_hal::PipelineLayoutFlags::empty(),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        })?
    };
    for (group, binding) in [(0, 0), (0, 1), (0, 2)] {
        let target = pipeline_layout
            .bind_target(group, binding)
            .ok_or("pipeline layout has no register for a binding")?;
        println!(
            "binding group {group} binding {binding} -> register {} (space {})",
            target.register, target.space
        );
    }
    if pipeline_layout.naga_options().sampler_binding_mode
        != naga::back::hlsl::SamplerBindingMode::Direct
    {
        return Err("pipeline layout did not select direct sampler bindings".into());
    }

    let uniform_buffer = unsafe {
        device.create_buffer(&wgpu_hal::BufferDescriptor {
            label: Some("shader smoke uniform buffer"),
            size: 32,
            usage: wgt::BufferUses::UNIFORM | wgt::BufferUses::COPY_DST,
            memory_flags: wgpu_hal::MemoryFlags::empty(),
        })?
    };
    let sampler = unsafe {
        device.create_sampler(&wgpu_hal::SamplerDescriptor {
            label: Some("shader smoke sampler"),
            address_modes: [wgt::AddressMode::ClampToEdge; 3],
            mag_filter: wgt::FilterMode::Linear,
            min_filter: wgt::FilterMode::Linear,
            mipmap_filter: wgt::MipmapFilterMode::Nearest,
            lod_clamp: 0.0..1.0,
            compare: None,
            anisotropy_clamp: 1,
            border_color: None,
        })?
    };
    let texture = unsafe {
        device.create_texture(&wgpu_hal::TextureDescriptor {
            label: Some("shader smoke texture"),
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
        })?
    };
    let texture_view = unsafe {
        device.create_texture_view(
            &texture,
            &wgpu_hal::TextureViewDescriptor {
                label: Some("shader smoke texture view"),
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
        )?
    };

    let buffer_binding =
        wgpu_hal::BufferBinding::new_unchecked(&uniform_buffer, 0, wgt::BufferSize::new(32));
    let texture_binding = wgpu_hal::TextureBinding {
        view: &texture_view,
        usage: wgt::TextureUses::RESOURCE,
    };
    let samplers = [&sampler];
    let bind_entries = [
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
    let bind_group = unsafe {
        device.create_bind_group(&wgpu_hal::BindGroupDescriptor {
            label: Some("shader smoke bind group"),
            layout: &bind_group_layout,
            buffers: &[buffer_binding],
            samplers: &samplers,
            textures: &[texture_binding],
            entries: &bind_entries,
            acceleration_structures: &[],
            external_textures: &[],
        })?
    };
    println!(
        "created bind group with {} buffers, {} textures and {} samplers",
        bind_group.buffers().len(),
        bind_group.textures().len(),
        bind_group.samplers().len()
    );

    let module = naga::front::wgsl::parse_str(SHADER)?;
    let info = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&module)?;
    let shader_module = unsafe {
        device.create_shader_module(
            &wgpu_hal::ShaderModuleDescriptor {
                label: Some("shader smoke module"),
                runtime_checks: wgt::ShaderRuntimeChecks::checked(),
            },
            wgpu_hal::ShaderInput::Naga(wgpu_hal::NagaShader {
                module: Cow::Owned(module),
                info,
                debug_source: None,
            }),
        )?
    };
    println!(
        "created shader module with entry points: {:?}",
        shader_module.entry_points()
    );

    let constants = naga::back::PipelineConstants::default();
    let vertex = shader_module.compile(
        "vs_main",
        naga::ShaderStage::Vertex,
        &pipeline_layout,
        &constants,
    )?;
    let fragment = shader_module.compile(
        "fs_main",
        naga::ShaderStage::Fragment,
        &pipeline_layout,
        &constants,
    )?;

    for shader in [&vertex, &fragment] {
        if shader.bytecode.is_empty() {
            return Err(format!("{} produced empty bytecode", shader.entry_point).into());
        }
        println!(
            "compiled {} ({:?}): {} bytes of HLSL, {} bytes of bytecode",
            shader.entry_point,
            shader.stage,
            shader.hlsl.len(),
            shader.bytecode.len()
        );
    }

    if !vertex.hlsl.contains("register(b") {
        return Err("vertex HLSL has no uniform register".into());
    }
    if !fragment.hlsl.contains("register(t") {
        return Err("fragment HLSL has no texture register".into());
    }
    if !fragment.hlsl.contains("register(s") {
        return Err("fragment HLSL has no direct sampler register".into());
    }
    if vertex.hlsl.contains("nagaSamplerHeap") || fragment.hlsl.contains("nagaSamplerHeap") {
        return Err("HLSL contains a D3D12 sampler heap".into());
    }

    println!("shader HLSL uses direct sampler registers and no sampler heap");

    unsafe {
        device.destroy_bind_group(bind_group);
        device.destroy_shader_module(shader_module);
        device.destroy_pipeline_layout(pipeline_layout);
        device.destroy_texture_view(texture_view);
        device.destroy_texture(texture);
        device.destroy_sampler(sampler);
        device.destroy_buffer(uniform_buffer);
        device.destroy_bind_group_layout(bind_group_layout);
    }

    println!("destroyed bind group and shader objects");
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn main() {
    eprintln!("the dx11 backend is only available on Windows");
    std::process::exit(1);
}

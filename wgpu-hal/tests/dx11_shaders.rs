//! Shader compilation coverage for the experimental Direct3D 11 backend.
//!
//! This test is the executable form of the shader part of the P3 packet in
//! `dx11-task-packets.md` and of sections 4, 5 and 8 of the Cinnabar DX11
//! capability contract. It opens the D3D11 backend offscreen, translates WGSL
//! to HLSL through Naga, compiles the result with FXC for Shader Model 5.0 and
//! checks that:
//!
//! - vertex and fragment entry points produce non-empty bytecode;
//! - HLSL uses direct samplers, flat `b`/`t`/`s` registers and no register
//!   spaces or sampler heap;
//! - a more demanding fragment shader (loop, derivatives, array indexing and
//!   sampling) still compiles;
//! - compiling the same entry point twice is deterministic through the cache;
//! - direct HLSL input compiles;
//! - syntax errors, missing entry points, wrong stages and bindings absent
//!   from the pipeline layout fail with actionable compilation errors.
//!
//! The test uses `Features::empty()` and the adapter's own limits, so it runs
//! unchanged on WARP and on hardware. It needs a Windows session because the
//! D3D11 backend is Windows-only; it does not create a window.
//!
//! Run on Windows CI with:
//!
//! ```text
//! cargo test -p wgpu-hal --features dx11 --test dx11_shaders -- --nocapture
//! ```

#![cfg(all(target_os = "windows", feature = "dx11"))]

use std::borrow::Cow;

use wgpu_hal::{Adapter as _, Device as _, Instance as _};
use wgpu_types as wgt;

/// WGSL with a uniform buffer, a texture and a sampler shared by a vertex and
/// a fragment entry point.
const SCENE_SHADER: &str = r#"
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

/// A fragment shader that is closer to production code: a bounded loop,
/// explicit derivatives, array indexing, selection and texture sampling.
const CORPUS_SHADER: &str = r#"
struct Globals {
    color: vec4<f32>,
    scale: f32,
}

@group(0) @binding(0) var<uniform> globals: Globals;
@group(0) @binding(1) var albedo: texture_2d<f32>;
@group(0) @binding(2) var albedo_sampler: sampler;

struct FragmentInput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@fragment
fn fs_corpus(input: FragmentInput) -> @location(0) vec4<f32> {
    var offsets: array<vec2<f32>, 4> = array<vec2<f32>, 4>(
        vec2<f32>(0.0, 0.0),
        vec2<f32>(0.125, 0.0),
        vec2<f32>(0.0, 0.125),
        vec2<f32>(0.125, 0.125),
    );

    var accum = vec4<f32>(0.0, 0.0, 0.0, 0.0);
    var index: u32 = 0u;
    loop {
        if index >= 4u {
            break;
        }

        let uv = input.uv + offsets[index];
        let sample = textureSampleLevel(albedo, albedo_sampler, uv, 0.0);
        let edge = dpdx(sample.r) + dpdy(sample.g);
        let tinted = sample * globals.color;
        if edge > 0.001 {
            accum = accum + tinted;
        } else {
            accum = accum + sample;
        }
        index = index + 1u;
    }

    return accum * globals.scale;
}
"#;

/// Direct HLSL input for the positive ShaderInput::Hlsl path.
const DIRECT_HLSL: &str = r#"
struct VertexInput {
    float2 position : POSITION;
    float2 uv : TEXCOORD0;
};

struct VertexOutput {
    float4 position : SV_POSITION;
    float2 uv : TEXCOORD0;
};

VertexOutput vs_direct(VertexInput input) {
    VertexOutput output;
    output.position = float4(input.position, 0.0, 1.0);
    output.uv = input.uv;
    return output;
}
"#;

/// Deliberately malformed HLSL: the assignment is missing its semicolon.
const BROKEN_HLSL: &str = r#"
struct PixelOutput {
    float4 color : SV_Target;
};

PixelOutput ps_broken() {
    PixelOutput output;
    output.color = float4(1.0, 0.0, 0.0, 1.0)
    return output;
}
"#;

/// WGSL whose uniform binding is absent from the empty pipeline layout used by
/// the negative test.
const MISSING_BINDING_SHADER: &str = r#"
struct Globals {
    color: vec4<f32>,
}

@group(0) @binding(3) var<uniform> globals: Globals;

@fragment
fn fs_missing_binding() -> @location(0) vec4<f32> {
    return globals.color;
}
"#;

#[test]
fn dx11_shaders() {
    if let Err(error) = run() {
        panic!("DX11 shader test failed: {error}");
    }
    println!("dx11_shaders: all shader sections passed");
}

fn run() -> Result<(), String> {
    let instance_desc = wgpu_hal::InstanceDescriptor {
        name: "wgpu-hal-dx11-shaders",
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
        "dx11_shaders: enumerated {} DXGI adapter(s)",
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
    println!(
        "opened D3D11 device on '{}' with Features::empty()",
        adapters[adapter_index].info.name
    );

    let (bind_group_layout, pipeline_layout) = create_scene_layout(&device)?;

    section_wgsl_positive(&device, &pipeline_layout)?;
    section_corpus(&device, &pipeline_layout)?;
    section_cache(&device, &pipeline_layout)?;
    section_hlsl_positive(&device, &pipeline_layout)?;
    section_hlsl_syntax_error(&device, &pipeline_layout)?;
    section_missing_entry_point(&device, &pipeline_layout)?;
    section_wrong_stage(&device, &pipeline_layout)?;
    section_missing_binding(&device)?;

    unsafe {
        device.destroy_pipeline_layout(pipeline_layout);
        device.destroy_bind_group_layout(bind_group_layout);
    }

    println!("dx11_shaders: destroying the queue and device");
    drop(queue);
    drop(device);
    Ok(())
}

fn create_scene_layout(
    device: &wgpu_hal::dx11::Device,
) -> Result<
    (
        wgpu_hal::dx11::BindGroupLayout,
        wgpu_hal::dx11::PipelineLayout,
    ),
    String,
> {
    let entries = [
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
            label: Some("shader test bind group layout"),
            flags: wgpu_hal::BindGroupLayoutFlags::empty(),
            entries: &entries,
        })
    }
    .map_err(|error| format!("creating the shader test bind group layout failed: {error:?}"))?;

    let pipeline_layout = unsafe {
        device.create_pipeline_layout(&wgpu_hal::PipelineLayoutDescriptor {
            label: Some("shader test pipeline layout"),
            flags: wgpu_hal::PipelineLayoutFlags::empty(),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        })
    }
    .map_err(|error| format!("creating the shader test pipeline layout failed: {error:?}"))?;

    Ok((bind_group_layout, pipeline_layout))
}

fn create_naga_module(
    device: &wgpu_hal::dx11::Device,
    label: &str,
    source: &str,
) -> Result<wgpu_hal::dx11::ShaderModule, String> {
    let module = naga::front::wgsl::parse_str(source)
        .map_err(|error| format!("WGSL parsing failed for shader '{label}': {error}"))?;
    let info = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .map_err(|error| format!("Naga validation failed for shader '{label}': {error}"))?;

    unsafe {
        device.create_shader_module(
            &wgpu_hal::ShaderModuleDescriptor {
                label: Some(label),
                runtime_checks: wgt::ShaderRuntimeChecks::checked(),
            },
            wgpu_hal::ShaderInput::Naga(wgpu_hal::NagaShader {
                module: Cow::Owned(module),
                info,
                debug_source: None,
            }),
        )
    }
    .map_err(|error| format!("creating Naga shader module '{label}' failed: {error}"))
}

fn create_hlsl_module(
    device: &wgpu_hal::dx11::Device,
    label: &str,
    source: &str,
) -> Result<wgpu_hal::dx11::ShaderModule, String> {
    unsafe {
        device.create_shader_module(
            &wgpu_hal::ShaderModuleDescriptor {
                label: Some(label),
                runtime_checks: wgt::ShaderRuntimeChecks::checked(),
            },
            wgpu_hal::ShaderInput::Hlsl { shader: source },
        )
    }
    .map_err(|error| format!("creating HLSL shader module '{label}' failed: {error}"))
}

fn check_compiled(
    label: &str,
    compiled: &wgpu_hal::dx11::CompiledShader,
    entry_point: &str,
    stage: naga::ShaderStage,
) -> Result<(), String> {
    if compiled.entry_point != entry_point {
        return Err(format!(
            "shader '{label}': expected entry point '{entry_point}', got '{}'",
            compiled.entry_point
        ));
    }
    if compiled.stage != stage {
        return Err(format!(
            "shader '{label}' entry point '{entry_point}': expected stage {stage:?}, got {:?}",
            compiled.stage
        ));
    }
    if compiled.bytecode.is_empty() {
        return Err(format!(
            "shader '{label}' entry point '{entry_point}' ({stage:?}) produced empty bytecode"
        ));
    }

    println!(
        "compiled shader '{label}' entry point '{entry_point}' ({stage:?}): {} bytes of HLSL, {} bytes of bytecode",
        compiled.hlsl.len(),
        compiled.bytecode.len()
    );
    Ok(())
}

fn expect_compilation_error(context: &str, error: wgpu_hal::ShaderError) -> Result<String, String> {
    match error {
        wgpu_hal::ShaderError::Compilation(message) => Ok(message),
        other => Err(format!(
            "{context}: expected ShaderError::Compilation, got {other:?}"
        )),
    }
}

fn require_message_contains(context: &str, message: &str, expected: &[&str]) -> Result<(), String> {
    for needle in expected {
        if !message.contains(needle) {
            return Err(format!(
                "{context}: error message does not contain {needle:?}: {message}"
            ));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Positive coverage
// ---------------------------------------------------------------------------

fn section_wgsl_positive(
    device: &wgpu_hal::dx11::Device,
    layout: &wgpu_hal::dx11::PipelineLayout,
) -> Result<(), String> {
    println!("\n=== 1. Naga WGSL vertex and fragment ===");
    let module = create_naga_module(device, "scene-shader", SCENE_SHADER)?;
    let constants = naga::back::PipelineConstants::default();

    let vertex = module
        .compile("vs_main", naga::ShaderStage::Vertex, layout, &constants)
        .map_err(|error| format!("compiling vertex entry point failed: {error}"))?;
    check_compiled(
        "scene-shader",
        &vertex,
        "vs_main",
        naga::ShaderStage::Vertex,
    )?;

    let fragment = module
        .compile("fs_main", naga::ShaderStage::Fragment, layout, &constants)
        .map_err(|error| format!("compiling fragment entry point failed: {error}"))?;
    check_compiled(
        "scene-shader",
        &fragment,
        "fs_main",
        naga::ShaderStage::Fragment,
    )?;

    if !vertex.hlsl.contains("register(b") {
        return Err("vertex HLSL has no uniform register".to_owned());
    }
    if !fragment.hlsl.contains("register(t") {
        return Err("fragment HLSL has no texture register".to_owned());
    }
    if !fragment.hlsl.contains("register(s") {
        return Err("fragment HLSL has no direct sampler register".to_owned());
    }
    if !fragment.hlsl.contains("SamplerState") {
        return Err("fragment HLSL has no direct sampler state".to_owned());
    }

    for (name, compiled) in [("vertex", &vertex), ("fragment", &fragment)] {
        for forbidden in ["nagaSamplerHeap", "SamplerIndexArray", "space0"] {
            if compiled.hlsl.contains(forbidden) {
                return Err(format!(
                    "{name} HLSL contains forbidden sampler-heap or register-space text {forbidden:?}"
                ));
            }
        }
    }

    unsafe { device.destroy_shader_module(module) };
    println!("scene HLSL uses direct samplers and flat registers without a sampler heap");
    Ok(())
}

fn section_corpus(
    device: &wgpu_hal::dx11::Device,
    layout: &wgpu_hal::dx11::PipelineLayout,
) -> Result<(), String> {
    println!("\n=== 2. production-like fragment corpus ===");
    let module = create_naga_module(device, "corpus-shader", CORPUS_SHADER)?;
    let constants = naga::back::PipelineConstants::default();

    let fragment = module
        .compile("fs_corpus", naga::ShaderStage::Fragment, layout, &constants)
        .map_err(|error| format!("compiling corpus fragment entry point failed: {error}"))?;
    check_compiled(
        "corpus-shader",
        &fragment,
        "fs_corpus",
        naga::ShaderStage::Fragment,
    )?;

    unsafe { device.destroy_shader_module(module) };
    println!("corpus fragment with loops, derivatives, arrays and sampling compiled");
    Ok(())
}

fn section_cache(
    device: &wgpu_hal::dx11::Device,
    layout: &wgpu_hal::dx11::PipelineLayout,
) -> Result<(), String> {
    println!("\n=== 3. bytecode cache determinism ===");
    let module = create_naga_module(device, "cache-shader", SCENE_SHADER)?;
    let constants = naga::back::PipelineConstants::default();

    let first = module
        .compile("fs_main", naga::ShaderStage::Fragment, layout, &constants)
        .map_err(|error| format!("first cached compilation failed: {error}"))?;
    check_compiled(
        "cache-shader",
        &first,
        "fs_main",
        naga::ShaderStage::Fragment,
    )?;

    let second = module
        .compile("fs_main", naga::ShaderStage::Fragment, layout, &constants)
        .map_err(|error| format!("second cached compilation failed: {error}"))?;
    check_compiled(
        "cache-shader",
        &second,
        "fs_main",
        naga::ShaderStage::Fragment,
    )?;

    if first.hlsl != second.hlsl {
        return Err("compiling the same entry point twice produced different HLSL".to_owned());
    }
    if first.bytecode != second.bytecode {
        return Err("compiling the same entry point twice produced different bytecode".to_owned());
    }

    unsafe { device.destroy_shader_module(module) };
    println!("repeated compilation returned identical HLSL and bytecode");
    Ok(())
}

fn section_hlsl_positive(
    device: &wgpu_hal::dx11::Device,
    layout: &wgpu_hal::dx11::PipelineLayout,
) -> Result<(), String> {
    println!("\n=== 4. direct HLSL input ===");
    let module = create_hlsl_module(device, "direct-hlsl", DIRECT_HLSL)?;
    let constants = naga::back::PipelineConstants::default();

    let vertex = module
        .compile("vs_direct", naga::ShaderStage::Vertex, layout, &constants)
        .map_err(|error| format!("compiling direct HLSL failed: {error}"))?;
    check_compiled(
        "direct-hlsl",
        &vertex,
        "vs_direct",
        naga::ShaderStage::Vertex,
    )?;

    if vertex.hlsl != DIRECT_HLSL {
        return Err("direct HLSL input was modified before compilation".to_owned());
    }

    unsafe { device.destroy_shader_module(module) };
    println!("direct HLSL input compiled through FXC");
    Ok(())
}

// ---------------------------------------------------------------------------
// Negative coverage
// ---------------------------------------------------------------------------

fn section_hlsl_syntax_error(
    device: &wgpu_hal::dx11::Device,
    layout: &wgpu_hal::dx11::PipelineLayout,
) -> Result<(), String> {
    println!("\n=== 5. HLSL syntax error diagnostics ===");
    let module = create_hlsl_module(device, "broken-hlsl", BROKEN_HLSL)?;
    let constants = naga::back::PipelineConstants::default();

    let error = module
        .compile("ps_broken", naga::ShaderStage::Fragment, layout, &constants)
        .expect_err("malformed HLSL must not compile");
    let message = expect_compilation_error("HLSL syntax error", error)?;
    require_message_contains(
        "HLSL syntax error",
        &message,
        &["broken-hlsl", "ps_broken", "Fragment", "ps_5_0", "error X"],
    )?;

    unsafe { device.destroy_shader_module(module) };
    println!("syntax error reported with shader label, entry point, stage and FXC diagnostic");
    Ok(())
}

fn section_missing_entry_point(
    device: &wgpu_hal::dx11::Device,
    layout: &wgpu_hal::dx11::PipelineLayout,
) -> Result<(), String> {
    println!("\n=== 6. missing entry point ===");
    let module = create_naga_module(device, "missing-entry-shader", SCENE_SHADER)?;
    let constants = naga::back::PipelineConstants::default();

    let error = module
        .compile(
            "not_an_entry_point",
            naga::ShaderStage::Fragment,
            layout,
            &constants,
        )
        .expect_err("a missing entry point must not compile");
    let message = expect_compilation_error("missing entry point", error)?;
    require_message_contains(
        "missing entry point",
        &message,
        &[
            "missing-entry-shader",
            "not_an_entry_point",
            "Fragment",
            "ps_5_0",
        ],
    )?;

    unsafe { device.destroy_shader_module(module) };
    println!("missing entry point reported with shader label, entry point and stage");
    Ok(())
}

fn section_wrong_stage(
    device: &wgpu_hal::dx11::Device,
    layout: &wgpu_hal::dx11::PipelineLayout,
) -> Result<(), String> {
    println!("\n=== 7. wrong entry point stage ===");
    let module = create_naga_module(device, "wrong-stage-shader", SCENE_SHADER)?;
    let constants = naga::back::PipelineConstants::default();

    let error = module
        .compile("fs_main", naga::ShaderStage::Vertex, layout, &constants)
        .expect_err("a fragment entry point must not compile as a vertex shader");
    let message = expect_compilation_error("wrong stage", error)?;
    require_message_contains(
        "wrong stage",
        &message,
        &["wrong-stage-shader", "fs_main", "Vertex", "vs_5_0"],
    )?;

    unsafe { device.destroy_shader_module(module) };
    println!("wrong stage reported with shader label, entry point and stage/profile");
    Ok(())
}

fn section_missing_binding(device: &wgpu_hal::dx11::Device) -> Result<(), String> {
    println!("\n=== 8. binding absent from the pipeline layout ===");
    let empty_layout = unsafe {
        device.create_pipeline_layout(&wgpu_hal::PipelineLayoutDescriptor {
            label: Some("empty shader test pipeline layout"),
            flags: wgpu_hal::PipelineLayoutFlags::empty(),
            bind_group_layouts: &[],
            immediate_size: 0,
        })
    }
    .map_err(|error| format!("creating the empty pipeline layout failed: {error:?}"))?;

    let module = create_naga_module(device, "missing-binding-shader", MISSING_BINDING_SHADER)?;
    let constants = naga::back::PipelineConstants::default();

    let error = module
        .compile(
            "fs_missing_binding",
            naga::ShaderStage::Fragment,
            &empty_layout,
            &constants,
        )
        .expect_err("a binding absent from the pipeline layout must not compile");
    let message = expect_compilation_error("missing binding", error)?;
    require_message_contains(
        "missing binding",
        &message,
        &[
            "missing-binding-shader",
            "fs_missing_binding",
            "Fragment",
            "ps_5_0",
            "binding",
        ],
    )?;

    unsafe {
        device.destroy_shader_module(module);
        device.destroy_pipeline_layout(empty_layout);
    }
    println!("missing binding reported with shader label, entry point, stage and binding");
    Ok(())
}

//! Tests for the HLSL backend's sampler binding modes.

use alloc::string::String;

use super::{BindTarget, Options, SamplerBindingMode, Writer};
use crate::ResourceBinding;

const SAMPLERS: &str = r#"
@group(0) @binding(0) var color_texture: texture_2d<f32>;
@group(0) @binding(1) var color_sampler: sampler;
@group(0) @binding(2) var depth_texture: texture_depth_2d;
@group(0) @binding(3) var depth_sampler: sampler_comparison;

@fragment
fn main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    let color = textureSampleLevel(color_texture, color_sampler, uv, 0.0);
    let depth = textureSampleCompareLevel(depth_texture, depth_sampler, uv, 0.5);
    return color * depth;
}
"#;

fn validate(source: &str) -> (crate::Module, crate::valid::ModuleInfo) {
    let module = crate::front::wgsl::parse_str(source).expect("test WGSL should parse");
    let info = crate::valid::Validator::new(
        crate::valid::ValidationFlags::all(),
        crate::valid::Capabilities::all(),
    )
    .validate(&module)
    .expect("test WGSL should validate");
    (module, info)
}

fn write_hlsl(
    module: &crate::Module,
    info: &crate::valid::ModuleInfo,
    options: &Options,
) -> Result<String, super::Error> {
    let mut text = String::new();
    let pipeline_options = super::PipelineOptions::default();
    Writer::new(&mut text, options, &pipeline_options).write(module, info, None)?;
    Ok(text)
}

fn direct_options() -> Options {
    let mut options = Options {
        sampler_binding_mode: SamplerBindingMode::Direct,
        fake_missing_bindings: false,
        ..Options::default()
    };

    let bindings = [
        (0, 2), // color_texture
        (1, 5), // color_sampler
        (2, 3), // depth_texture
        (3, 7), // depth_sampler
    ];
    for (binding, register) in bindings {
        options.binding_map.insert(
            ResourceBinding { group: 0, binding },
            BindTarget {
                register,
                ..BindTarget::default()
            },
        );
    }

    options
}

#[test]
fn heap_is_default_and_keeps_sampler_heap() {
    assert_eq!(
        Options::default().sampler_binding_mode,
        SamplerBindingMode::Heap
    );

    let (module, info) = validate(SAMPLERS);
    let text = write_hlsl(&module, &info, &Options::default()).expect("HLSL should be generated");

    assert!(text.contains("SamplerState nagaSamplerHeap[2048]: register(s0, space0);"));
    assert!(text
        .contains("SamplerComparisonState nagaComparisonSamplerHeap[2048]: register(s0, space1);"));
    assert!(text.contains("StructuredBuffer<uint> nagaGroup0SamplerIndexArray"));
    assert!(text.contains("nagaSamplerHeap[nagaGroup0SamplerIndexArray["));
    assert!(text.contains("nagaComparisonSamplerHeap[nagaGroup0SamplerIndexArray["));
}

#[test]
fn direct_declares_samplers_at_their_registers() {
    let (module, info) = validate(SAMPLERS);
    let text = write_hlsl(&module, &info, &direct_options()).expect("HLSL should be generated");

    assert!(text.contains("SamplerState color_sampler : register(s5);"));
    assert!(text.contains("SamplerComparisonState depth_sampler : register(s7);"));

    assert!(!text.contains("nagaSamplerHeap"));
    assert!(!text.contains("nagaComparisonSamplerHeap"));
    assert!(!text.contains("SamplerIndexArray"));
    assert!(!text.contains("space0"));
    assert!(!text.contains("space1"));
}

#[test]
fn direct_rejects_nonzero_sampler_space() {
    let (module, info) = validate(SAMPLERS);
    let mut options = direct_options();
    options
        .binding_map
        .get_mut(&ResourceBinding {
            group: 0,
            binding: 1,
        })
        .unwrap()
        .space = 1;

    let error = write_hlsl(&module, &info, &options).expect_err("nonzero space should fail");
    assert!(matches!(
        error,
        super::Error::DirectSamplerNonZeroSpace(ref name, 1) if name.as_str() == "color_sampler"
    ));
}
#[test]
fn direct_rejects_sampler_binding_arrays() {
    let (module, info) = validate(
        r#"
        enable wgpu_binding_array;

        @group(0) @binding(0) var samplers: binding_array<sampler, 4>;
        @group(0) @binding(1) var color_texture: texture_2d<f32>;

        @fragment
        fn main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
            return textureSampleLevel(color_texture, samplers[0], uv, 0.0);
        }
        "#,
    );
    let options = Options {
        sampler_binding_mode: SamplerBindingMode::Direct,
        ..Options::default()
    };

    let error = write_hlsl(&module, &info, &options).expect_err("binding arrays should fail");
    assert!(matches!(
        error,
        super::Error::SamplerBindingArrayUnsupported(ref name) if name.as_str() == "samplers"
    ));
}

# DX11 P3 shaders and descriptors on Intel HD Graphics 4400

| Field | Value |
| --- | --- |
| Date | 2026-11-01 |
| Branch | `dx11-hal-shaders` |
| Base | `97d0007c2` |
| Machine | Windows 10 IoT Enterprise LTSC 21H2, build 19044 |
| GPU | Intel(R) HD Graphics 4400 (`0x8086:0x041e`), feature level 11_0 |
| Backend | D3D11 |
| Results | shaders **1 passed**, descriptors **1 passed**, unit tests **9 passed** |

## Validation route

The tests were cross-compiled on the ARM64 Linux VPS with Podman and copied to
the Windows machine over SSH. They were executed offscreen; no GitHub Actions
run was used for this validation.

## Shader coverage

`dx11_shaders` passed:

- WGSL → Naga IR → HLSL → FXC SM 5.0 for vertex and fragment entry points;
- direct samplers and flat `b`/`t`/`s` registers without a sampler heap;
- a fragment corpus with a loop, derivatives, arrays and texture sampling;
- deterministic bytecode caching;
- direct HLSL input;
- actionable errors for syntax errors, missing entry points, wrong stages and
  bindings absent from the pipeline layout;
- rejection of empty or null FXC bytecode.

## Descriptor coverage

`dx11_descriptors` passed:

- static UBO + texture + sampler bindings;
- dynamic uniform offsets 0 and 256;
- rejection of storage, arrays, 1D/3D, MSAA and unsupported dimensions;
- bind-group count, immediate and binding-count limits;
- invalid dynamic offsets (alignment and range);
- push constants rejected through an explicit encoder error;
- no partial binding state after a failed bind.

## Limit correction

The original contract advertised `max_uniform_buffers_per_shader_stage = 12`
and `max_samplers_per_shader_stage = 16`. Naga/HLSL declares module-level
globals for each entry point, and D3D11 exposes only 14 constant-buffer slots
and 16 sampler slots globally. The contract now advertises:

```text
max_uniform_buffers_per_shader_stage = 7
max_samplers_per_shader_stage = 8
max_dynamic_uniform_buffers_per_pipeline_layout = 8
```

This keeps a layout accepted by `wgpu-core` bindable in both stages without
silently failing at draw time.

## Limitations

- This is not a Cinnabar menu or gameplay run.
- Storage buffers/textures, compute, push constants, binding arrays and MSAA
  remain outside P3.
- Windowed/swapchain tests require an interactive Windows session.

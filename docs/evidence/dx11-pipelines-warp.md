# DX11 P4 pipelines and render passes on WARP

| Field | Value |
| --- | --- |
| Date | 2026-11-01 |
| Branch | `dx11-hal-pipelines` |
| Commit | `e47254f06` (first fully green run); evidence commit adds this file |
| Runner | GitHub Actions `windows-2025` |
| Adapter | Microsoft Basic Render Driver (`0x1414:0x008c`), software/WARP, feature level 11_0 |
| Workflow | `DX11 pipelines check` (`.github/workflows/dx11-pipelines-check.yml`) |
| Runs | Red `41328658155` (compile error); green `41328768270` |
| Tests | `dx11_pipelines`, `dx11_contract`, `dx11_resources`, `dx11_shaders`, `dx11_descriptors` all passed |

## Validation route

The workflow compiles `wgpu-hal --features dx11` and runs the new P4 test
together with the P1-P3 DX11 regression tests on a hosted Windows runner. The
new test is offscreen; no interactive desktop or swapchain is involved.

## P4 coverage (`wgpu-hal/tests/dx11_pipelines.rs`)

- Two color targets and a `Depth32Float` attachment in one pass, with exact
  `Rgba8Unorm`, `Bgra8Unorm` and depth read-back.
- Four color attachments, the advertised `max_color_attachments` limit.
- Clears, viewport-only and scissor-only clipping at exact pixels.
- Direct, indexed (`base_vertex = 3`) and indirect direct/indexed draws,
  compared byte-for-byte with their direct baselines.
- Per-instance `VertexStepMode::Instance` offsets and
  `@builtin(instance_index)` colors.
- Color write mask, additive blending and front/back culling as an XOR.
- All five primitive topologies and depth bias/clamp state.
- Explicit rejection of MSAA, multiview, resolve targets, 3D render-target
  slices, mesh/task pipelines, compute pipelines/passes, indirect count,
  pipeline/render-pass color and depth mismatches, `set_render_pipeline`
  outside a pass and indexed draws without an index buffer.
- The pipeline/pass mismatch cases prove that the failed encoder does not draw:
  a later read-back contains only the render-pass clear value.

## Fixes found by the runs

- `41328658155` failed the `cargo check` step because `CommandBuffer` called
  `vertex_strides()` on its own `PipelineState` instead of reading the
  `vertex_strides` field. Fixed in `e47254f06`.
- During test review, the BGRA8 expectation was corrected to its raw
  `[B, G, R, A]` byte order and the clear/blend values were moved off exact
  UNORM ties so the assertions test behavior rather than rounding policy.

## Limitations

- This is WARP/software only. Hardware parity (for example the Intel HD 4400)
  is still pending and is not claimed by this evidence.
- MSAA, multiview, resolve, mesh/task, compute, storage resources and 3D
  slices remain unsupported and are rejected explicitly.
- D3D11 records on the immediate context: a rejected command fails
  `end_encoding` instead of being submitted.
- The test covers four color attachments, not the eight simultaneous D3D11
  targets; four is the limit advertised by this backend.

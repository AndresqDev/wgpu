# DX11 Gate 0 contract on Intel HD Graphics 4400

| Field | Value |
| --- | --- |
| Date | 2026-11-01 |
| Backend commit | `9bc4fc056` (`dx11-stabilization`) |
| Test binary | `dx11_contract` cross-compiled for `x86_64-pc-windows-gnu` |
| Machine | Windows 10 IoT Enterprise LTSC 21H2, build 19044 |
| CPU | Intel Core i3, 4th generation, 2 cores / 4 threads |
| RAM | 16 GB |
| GPU | Intel(R) HD Graphics 4400, vendor `0x8086`, device `0x041e` |
| Feature level | 11_0 |
| Result | **1 passed; 0 failed; 0 ignored** |

## How it was run

The test binary was cross-compiled on an ARM64 Linux host using:

```text
cargo test -p wgpu-hal --features dx11 --test dx11_contract \
  --no-run --target x86_64-pc-windows-gnu
```

The resulting executable was copied to the Windows machine and executed from
an SSH session:

```powershell
cd C:\cinnabar-test
.\dx11_contract.exe --nocapture
```

The test enumerated two adapters:

1. `Intel(R) HD Graphics 4400` (`IntegratedGpu`, hardware, feature level 11_0)
2. `Microsoft Basic Render Driver` (`Cpu`, software, feature level 11_0)

Both adapters passed the contract. The hardware adapter is the one relevant to
the target machine.

## Result

```text
test dx11_gate0_contract ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

The captured output is stored alongside this document as
`dx11-contract-hardware-intel-hd4400.log`.

## Contract values observed on the Intel adapter

- Features: `TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES`, `DEPTH_CLIP_CONTROL`.
- Downlevel: `BASE_VERTEX`, `NON_POWER_OF_TWO_MIPMAPPED_TEXTURES`,
  `CUBE_ARRAY_TEXTURES`, `COMPARISON_SAMPLERS`, `INDEPENDENT_BLEND`,
  `FULL_DRAW_INDEX_UINT32`, `DEPTH_BIAS_CLAMP`.
- Surface: `Bgra8UnormSrgb` / `Bgra8Unorm`, `Fifo` / `Immediate`, `Opaque`.
- Positive smoke: buffers, 2D texture, view, sampler, uniform bind group and
  render pipeline created successfully; native shader compile and draw path
  reached.
- All negative cases returned explicit errors, including BC, MSAA, 1D/3D,
  storage, immediate-size, compute, query set, ray tracing, acceleration
  structures, invalid surface format and `Mailbox`.
- `surface.configure` rejected invalid configurations without touching the
  swap chain.

## Limitations

- The test was executed from an SSH session. It reached the hardware Intel
  adapter, but this is not a visual menu/game test.
- Gate 0 is closed on this machine; Gate 1 still requires debug layer,
  device-loss recovery, additional render-target coverage and hardware
  validation of the remaining examples.
- No user-facing build or artifact was produced.

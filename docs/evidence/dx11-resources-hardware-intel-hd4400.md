# DX11 resources test on Intel HD Graphics 4400

| Field | Value |
| --- | --- |
| Date | 2026-11-01 |
| Branch | `dx11-hal-resources` |
| Commits | `4d3f94e6f`, `ad4dc2868` |
| Machine | Windows 10 IoT Enterprise LTSC 21H2, build 19044 |
| CPU | Intel Core i3, 4th generation, 2 cores / 4 threads |
| RAM | 16 GB |
| GPU | Intel(R) HD Graphics 4400 (`0x8086:0x041e`), feature level 11_0 |
| Result | **1 passed; 0 failed** |

## How it was run

The test binary was cross-compiled on the ARM64 Linux VPS with Podman and
copied to the Windows machine over SSH. It was executed from an SSH session
(no interactive desktop needed for this offscreen test):

```powershell
cd C:\cinnabar-test
.\dx11_resources.exe --nocapture
```

No GitHub Actions run was used for this validation.

## Coverage

The test passed all sections:

1. buffer creation, usage validation and storage rejection;
2. buffer mapping and read/write/range errors;
3. buffer copies, including the `CopyResource` path;
4. 2D color textures, direct upload at an offset, and texture-to-texture copy;
5. `Depth32Float` upload and exact read-back;
6. arrays, mip levels and cubemaps;
7. the complete format allowlist, plus negative formats;
8. negative texture-view cases;
9. sampler creation and anisotropy validation.

The final result was:

```text
dx11_resources: destroying the queue and device
dx11_resources: all resource sections passed
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

The complete output is stored as
`dx11-resources-hardware-intel-hd4400.log`.

## Fixes discovered by the hardware run

- Depth textures are created with typeless DXGI formats so typed DSV/SRV views
  are valid.
- Depth-stencil uploads use a writable staging texture followed by a
  whole-subresource copy.
- Full depth read-back uses `CopyResource`, because `CopySubresourceRegion`
  cannot apply a source box to a depth-stencil resource.
- `Depth32FloatStencil8` depth-only DSVs use the packed DSV format instead of
  `D32_FLOAT`, which is incompatible with `R32G8X24_TYPELESS`.
- sRGB view formats are explicitly rejected because the backend does not
  advertise `VIEW_FORMATS`.

## Limitations

- This is the resource test, not a Cinnabar menu or gameplay test.
- MSAA, storage/UAV, 1D/3D and compute remain outside this delivery.
- The windowed examples still need an interactive Windows session for the
  swap chain.

# DX11 HAL core examples on Intel HD Graphics 4400

| Field | Value |
| --- | --- |
| Date | 2026-11-01 |
| Backend commit | `9bc4fc056` |
| Machine | Windows 10 IoT Enterprise LTSC 21H2, build 19044 |
| CPU | Intel Core i3, 4th generation, 2 cores / 4 threads |
| RAM | 16 GB |
| GPU | Intel(R) HD Graphics 4400 (`0x8086:0x041e`), feature level 11_0 |
| Runner | Local interactive Windows desktop |

## Result

All examples exited with code 0:

| Example | Result | Evidence |
| --- | --- | --- |
| `raw-dx11` | PASS | enumerated Intel HD 4400 and Microsoft Basic Render Driver |
| `raw-dx11-resources` | PASS | buffer copy round trip and texture upload/readback verified |
| `raw-dx11-triangle` | PASS | triangle center `[0,0,255,255]`, corner `[255,0,0,255]` |
| `raw-dx11-resize` | PASS | 320x240 and 640x360 frames presented |
| `raw-dx11-window` | PASS | 16 frames presented |
| `dx11_contract` | PASS | 1 passed; 0 failed on both adapters |

The complete captured output is in
`dx11-hal-core-hardware-intel-hd4400.log`.

## Swap-chain fallback observed

Both windowed examples logged:

```text
[ERROR wgpu_hal::dx11::surface] failed to get swap-chain buffer 1:
DXGI_ERROR_INVALID_CALL (0x887A0001)
```

The flip-model attempt then fell back to `DXGI_SWAP_EFFECT_DISCARD` with one
buffer. The examples completed and presented frames successfully, so this is a
functional fallback, not a test failure.

It does mean the Intel path is currently using the legacy discard swap chain
instead of the intended flip-discard chain. This remains a Gate 1 follow-up:
determine whether the Intel driver rejects the second flip buffer because of
the swap-chain description or because of the Windows/desktop configuration.

## Limitations

- This is a hardware example run, not a Cinnabar menu or gameplay run.
- Debug layer, device-loss recovery, multiple render targets and arrays/cubemap
  coverage remain pending.
- The fallback to discard is functional but should be investigated for
  performance and frame-latency correctness.

# DX11 fence query pool on WARP

| Field | Value |
| --- | --- |
| Date | 2026-11-01 |
| Branch | `dx11-query-pool` |
| Base | `389966d34` (P1 stability + P4 pipelines integrated) |
| Code commit | `356aeec1f` |
| Runner | GitHub Actions `windows-2025` |
| Adapter | Microsoft Basic Render Driver (WARP), feature level 11_0 |
| Workflow | `DX11 stability check` (`.github/workflows/dx11-stability-check.yml`) |
| Runs | Red `41334983554` (unit test used an invalid never-ended query); green `41335241093` |
| Tests | `dx11_query_pool` (4/4), `dx11_device_loss`, contract/resources/shaders/descriptors/pipelines/debug and `--lib` all passed |

## Change under test

Each `Fence` now owns eight `D3D11_QUERY_EVENT` slots. `Queue::submit` arms a
free slot instead of re-arming the single fence query; a slot is only reused
after its marker was observed. When all eight slots are in flight, `submit`
waits at most 100 ms for the oldest marker and then rotates that slot to a
fresh query, so a marker that does not retire cannot block the next submission.
`wait` and `get_fence_value` poll the armed slots; `completed_value` only
advances to values whose marker the GPU reached. Device removal still surfaces
as `DeviceError::Lost`, and the capability contract is unchanged (no query
sets, no compute, no new limits).

## Coverage

`wgpu-hal/tests/dx11_query_pool.rs`:

- `dx11_query_pool_overlapping_submissions_and_out_of_order_waits`: eight
  submissions armed without intermediate waits, waits for values 4, 8 and 1,
  a lower-value submit that must be ignored, and a second batch (9..=16) that
  reuses the slots freed by the first batch.
- `dx11_query_pool_rotates_under_pressure`: 32 back-to-back submissions with a
  pool of 8, then a bounded wait for value 32. The run submitted all 32 values
  in 2.77 ms and reached completed value 32.
- `dx11_wait_for_idle_after_overlapping_submissions`: 12 submissions with
  slots in flight, `Queue::wait_for_idle`, then fence value 12.
- `dx11_fence_wait_timeout_is_reported`: a never-submitted value returns
  `false`, a zero-timeout wait on value 8 is tolerated, a later wait reaches
  value 8, and the device stays healthy.

`wgpu-hal/src/dx11/queue.rs` unit test:

- `fence_wait_times_out_when_no_marker_is_armed`: publishes a pending value
  with no armed slot and verifies `Fence::wait` returns `false` at a 20 ms
  deadline without advancing the completed value. This deterministically covers
  the timeout branch without depending on WARP speed.

## Log extract

```text
running 4 tests
wait(8) with a zero timeout returned false
submitted fence values 1..=8 without intermediate waits
values 1..=8 completed; completed value = 8
lower-value submit 5 ignored; completed value still 8
second batch 9..=16 reused the pool; completed value = 16
submitted fence values 1..=32 back-to-back in 2.7732ms (pool size is 8; exhausted pools rotate instead of failing)
value 32 completed after rotation; completed value = 32
queue idle marker observed; fence completed value = 12
device is healthy; completed value = 8
test result: ok. 4 passed; 0 failed
```

## Limitations

- WARP retires empty submissions faster than the 100 ms rotation timeout, so
  the integration run did not have to rotate a slot under real GPU pressure.
  The timeout/deadline path is covered by the unit test above; the rotation
  branch itself is exercised only by code review and by the bounded submit
  path, not by a forced 100 ms stall.
- The run is WARP only. Hardware parity for the fence pool is not claimed by
  this evidence.
- The window/resize examples in the workflow still cover the swap-chain path;
  this evidence is about offscreen queue fences only.

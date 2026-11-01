//! D3D11 device-loss and fence-state hardening.
//!
//! A real D3D11 device removal (TDR, driver reset, GPU hang) cannot be forced
//! through the public D3D11 API: there is no `RemoveDevice` equivalent to
//! D3D12's, and `ID3D11Device::GetDeviceRemovedReason` is read-only. This test
//! therefore covers what can be verified headlessly on WARP and hardware:
//!
//! - the healthy-device classification (`GetDeviceRemovedReason` is `S_OK`);
//! - the controlled `Fence::wait` paths, including values that were never
//!   submitted and waits after several overlapping submissions;
//! - repeated submissions on one fence (the backend retires the previous
//!   `D3D11_QUERY_EVENT` marker before re-arming it);
//! - the no-op behaviour of a lower-than-pending fence value;
//! - dropping a fence whose markers were never observed, without a panic.
//!
//! Removal, reset and hang HRESULTs are classified by the unit test in
//! `wgpu-hal/src/dx11/queue.rs`. Actual removal recovery remains a hardware
//! validation item and is not claimed here.
//!
//! Run on Windows with:
//!
//! ```text
//! cargo test -p wgpu-hal --features dx11 --test dx11_device_loss -- --nocapture
//! ```

#![cfg(all(target_os = "windows", feature = "dx11"))]

use core::time::Duration;

use wgpu_hal::{Adapter as _, Device as _, Instance as _, Queue as _};
use wgpu_types as wgt;

const WAIT_TIMEOUT: Duration = Duration::from_secs(10);

struct Opened {
    _instance: wgpu_hal::dx11::Instance,
    device: wgpu_hal::dx11::Device,
    queue: wgpu_hal::dx11::Queue,
}

fn open_device() -> Result<Opened, String> {
    let descriptor = wgpu_hal::InstanceDescriptor {
        name: "wgpu-hal-dx11-device-loss",
        flags: wgt::InstanceFlags::default(),
        memory_budget_thresholds: wgt::MemoryBudgetThresholds::default(),
        backend_options: wgt::BackendOptions::default(),
        telemetry: None,
        display: None,
    };

    let instance = unsafe { <wgpu_hal::dx11::Instance as wgpu_hal::Instance>::init(&descriptor) }
        .map_err(|e| format!("D3D11 instance initialization failed: {e}"))?;

    let adapters = unsafe { instance.enumerate_adapters(None) };
    let exposed = adapters
        .into_iter()
        .next()
        .ok_or_else(|| "no D3D11 adapters were found".to_owned())?;
    println!(
        "adapter: '{}' (software={})",
        exposed.info.name,
        matches!(exposed.info.device_type, wgt::DeviceType::Cpu)
    );

    let opened = unsafe {
        exposed.adapter.open(
            exposed.features,
            &exposed.capabilities.limits,
            &wgt::MemoryHints::default(),
        )
    }
    .map_err(|e| format!("Adapter::open failed: {e:?}"))?;

    Ok(Opened {
        _instance: instance,
        device: opened.device,
        queue: opened.queue,
    })
}

#[test]
fn dx11_fence_waits_are_bounded_and_ordered() {
    println!("=== D3D11 fence wait semantics ===");
    let Opened { device, queue, .. } = open_device().expect("device setup failed");

    assert!(
        !device.is_device_lost(),
        "a freshly opened device reported as removed: {:?}",
        device.device_removed_reason()
    );
    device
        .check_device_state()
        .expect("healthy device must pass check_device_state");

    let fence = unsafe { device.create_fence() }.expect("create_fence failed");

    assert!(
        unsafe { device.wait(&fence, 0, None) }.expect("wait(0) failed"),
        "wait(0) must be immediately true"
    );
    assert!(
        !unsafe { device.wait(&fence, 1, Some(Duration::from_millis(100))) }
            .expect("never-submitted wait failed"),
        "waiting for a value that was never submitted must return false"
    );

    for value in 1..=12 {
        unsafe { queue.submit(&[], &[], (&fence, value)) }
            .unwrap_or_else(|e| panic!("submit({value}) failed: {e:?}"));
    }
    println!("submitted fence values 1..=12 without waiting in between");

    assert!(
        unsafe { device.wait(&fence, 12, Some(WAIT_TIMEOUT)) }.expect("wait(12) failed"),
        "the fence did not reach the last submitted value"
    );
    for value in [1, 2, 5, 11, 12] {
        assert!(
            unsafe { device.wait(&fence, value, None) }
                .unwrap_or_else(|e| panic!("wait({value}) after completion failed: {e:?}")),
            "fence value {value} was not reported as reached after value 12 completed"
        );
    }
    let completed = unsafe { device.get_fence_value(&fence) }.expect("get_fence_value failed");
    assert!(
        completed >= 12,
        "get_fence_value returned {completed} after value 12 completed"
    );
    println!("all overlapping markers completed; completed value = {completed}");

    // A lower value must not re-arm a marker for a submission that already
    // happened: wgpu-hal says a lower fence value has no effect.
    unsafe { queue.submit(&[], &[], (&fence, 5)) }.expect("lower-value submit must be a no-op");
    assert!(
        unsafe { device.wait(&fence, 12, None) }.expect("wait(12) after lower submit failed"),
        "lower-value submit corrupted the fence state"
    );
    let completed_after =
        unsafe { device.get_fence_value(&fence) }.expect("get_fence_value failed");
    assert!(
        completed_after >= 12,
        "lower-value submit moved the completed value backwards ({completed_after})"
    );
    println!("lower-value submit ignored; completed value = {completed_after}");

    unsafe { device.destroy_fence(fence) };

    // A fence with markers that were never observed must be droppable.
    let abandoned = unsafe { device.create_fence() }.expect("create_fence failed");
    for value in 1..=3 {
        unsafe { queue.submit(&[], &[], (&abandoned, value)) }.expect("submit failed");
    }
    drop(abandoned);
    println!("dropped a fence with unobserved markers without panicking");

    println!(
        "NOTE: forced device removal is not available through the public D3D11 API; \
         actual removal recovery must be validated on hardware."
    );
}

#[test]
fn dx11_removed_device_probe_is_explicit() {
    println!("=== D3D11 device-removal probe ===");
    let Opened { device, .. } = open_device().expect("device setup failed");

    assert_eq!(device.device_removed_reason(), None);
    assert!(!device.is_device_lost());
    println!(
        "GetDeviceRemovedReason reports a healthy device; `check_device_state()` and the \
         queue/submit/present paths classify DXGI_ERROR_DEVICE_REMOVED/RESET/HUNG as Lost"
    );
    println!(
        "NOTE: this WARP run cannot induce a TDR/reset, so only the healthy and controlled \
         error paths are verified here."
    );
}

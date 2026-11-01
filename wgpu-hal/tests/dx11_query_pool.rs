//! D3D11 fence query-pool behaviour.
//!
//! Each fence owns a pool of `D3D11_QUERY_EVENT` markers. These tests verify
//! that overlapping submissions arm several slots, that waits may be issued
//! out of order, that the pool rotates under pressure instead of blocking the
//! next submission, that `wait_for_idle` still works with per-fence slots in
//! flight, and that a zero/short wait timeout is reported without losing the
//! markers for a later wait.
//!
//! Run on Windows with:
//!
//! ```text
//! cargo test -p wgpu-hal --features dx11 --test dx11_query_pool -- --nocapture
//! ```

#![cfg(all(target_os = "windows", feature = "dx11"))]

use std::time::{Duration, Instant};

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
        name: "wgpu-hal-dx11-query-pool",
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
fn dx11_query_pool_overlapping_submissions_and_out_of_order_waits() {
    println!("=== D3D11 fence query pool: overlapping submissions and out-of-order waits ===");
    let Opened { device, queue, .. } = open_device().expect("device setup failed");
    let fence = unsafe { device.create_fence() }.expect("create_fence failed");

    for value in 1..=8 {
        unsafe { queue.submit(&[], &[], (&fence, value)) }
            .unwrap_or_else(|e| panic!("submit({value}) failed: {e:?}"));
    }
    println!("submitted fence values 1..=8 without intermediate waits");

    assert!(
        unsafe { device.wait(&fence, 4, Some(WAIT_TIMEOUT)) }.expect("wait(4) failed"),
        "the fence did not reach value 4"
    );
    assert!(
        unsafe { device.wait(&fence, 8, Some(WAIT_TIMEOUT)) }.expect("wait(8) failed"),
        "the fence did not reach value 8"
    );
    assert!(
        unsafe { device.wait(&fence, 1, None) }.expect("wait(1) failed"),
        "value 1 must already be reached after value 8 completed"
    );

    let completed = unsafe { device.get_fence_value(&fence) }.expect("get_fence_value failed");
    assert!(
        completed >= 8,
        "get_fence_value returned {completed} after value 8 completed"
    );
    println!("values 1..=8 completed; completed value = {completed}");

    // A lower value must not arm another marker or move the fence state:
    // wgpu-hal says a lower-than-pending fence value has no effect.
    unsafe { queue.submit(&[], &[], (&fence, 5)) }.expect("lower-value submit must be a no-op");
    assert!(
        unsafe { device.wait(&fence, 8, None) }.expect("wait(8) after lower submit failed"),
        "the lower-value submit corrupted the completed markers"
    );
    let completed_after_lower =
        unsafe { device.get_fence_value(&fence) }.expect("get_fence_value failed");
    assert_eq!(
        completed_after_lower, completed,
        "a lower-value submit changed the completed fence value"
    );
    println!("lower-value submit 5 ignored; completed value still {completed_after_lower}");

    // A second batch must reuse the slots that completed in the first batch.
    for value in 9..=16 {
        unsafe { queue.submit(&[], &[], (&fence, value)) }
            .unwrap_or_else(|e| panic!("submit({value}) failed: {e:?}"));
    }
    assert!(
        unsafe { device.wait(&fence, 16, Some(WAIT_TIMEOUT)) }.expect("wait(16) failed"),
        "the fence did not reach value 16"
    );
    let completed =
        unsafe { device.get_fence_value(&fence) }.expect("get_fence_value after reuse failed");
    assert!(
        completed >= 16,
        "get_fence_value returned {completed} after value 16 completed"
    );
    println!("second batch 9..=16 reused the pool; completed value = {completed}");

    unsafe { device.destroy_fence(fence) };
}

#[test]
fn dx11_query_pool_rotates_under_pressure() {
    println!("=== D3D11 fence query pool: rotation under pressure ===");
    let Opened { device, queue, .. } = open_device().expect("device setup failed");
    let fence = unsafe { device.create_fence() }.expect("create_fence failed");

    let start = Instant::now();
    for value in 1..=32 {
        unsafe { queue.submit(&[], &[], (&fence, value)) }
            .unwrap_or_else(|e| panic!("submit({value}) failed under pool pressure: {e:?}"));
    }
    let submit_elapsed = start.elapsed();
    println!(
        "submitted fence values 1..=32 back-to-back in {submit_elapsed:?} (pool size is 8; \
         exhausted pools rotate instead of failing)"
    );
    assert!(
        submit_elapsed < Duration::from_secs(20),
        "32 submissions took {submit_elapsed:?}; rotation must not block for seconds"
    );

    assert!(
        unsafe { device.wait(&fence, 32, Some(Duration::from_secs(20))) }.expect("wait(32) failed"),
        "the fence did not reach value 32 after rotation pressure"
    );
    let completed =
        unsafe { device.get_fence_value(&fence) }.expect("get_fence_value after pressure failed");
    assert!(
        completed >= 32,
        "get_fence_value returned {completed} after value 32 completed"
    );
    println!("value 32 completed after rotation; completed value = {completed}");

    unsafe { device.destroy_fence(fence) };
}

#[test]
fn dx11_wait_for_idle_after_overlapping_submissions() {
    println!("=== D3D11 queue idle marker with fence slots in flight ===");
    let Opened { device, queue, .. } = open_device().expect("device setup failed");
    let fence = unsafe { device.create_fence() }.expect("create_fence failed");

    for value in 1..=12 {
        unsafe { queue.submit(&[], &[], (&fence, value)) }
            .unwrap_or_else(|e| panic!("submit({value}) failed: {e:?}"));
    }
    println!("submitted fence values 1..=12; waiting for the queue idle marker");

    unsafe { queue.wait_for_idle() }.expect("wait_for_idle failed while fence slots were armed");
    assert!(
        unsafe { device.wait(&fence, 12, Some(WAIT_TIMEOUT)) }.expect("wait(12) after idle failed"),
        "the fence did not reach value 12 after wait_for_idle"
    );
    let completed =
        unsafe { device.get_fence_value(&fence) }.expect("get_fence_value after idle failed");
    assert!(
        completed >= 12,
        "get_fence_value returned {completed} after idle and value 12"
    );
    println!("queue idle marker observed; fence completed value = {completed}");

    unsafe { device.destroy_fence(fence) };
}

#[test]
fn dx11_fence_wait_timeout_is_reported() {
    println!("=== D3D11 fence wait timeouts ===");
    let Opened { device, queue, .. } = open_device().expect("device setup failed");
    let fence = unsafe { device.create_fence() }.expect("create_fence failed");

    assert!(
        !unsafe { device.wait(&fence, 1, Some(Duration::from_millis(50))) }
            .expect("wait on a never-submitted value failed"),
        "waiting for a value that was never submitted must return false"
    );
    println!("wait for never-submitted value 1 returned false");

    for value in 1..=8 {
        unsafe { queue.submit(&[], &[], (&fence, value)) }
            .unwrap_or_else(|e| panic!("submit({value}) failed: {e:?}"));
    }

    // With no timeout the wait must either observe the marker immediately or
    // hit the timeout; both are correct. A false result must not corrupt the
    // fence: a later wait has to make progress.
    let zero_timeout =
        unsafe { device.wait(&fence, 8, Some(Duration::ZERO)) }.expect("wait(8, zero) failed");
    println!("wait(8) with a zero timeout returned {zero_timeout}");

    if !zero_timeout {
        assert!(
            unsafe { device.wait(&fence, 8, Some(WAIT_TIMEOUT)) }
                .expect("wait(8) after zero-timeout failed"),
            "a zero-timeout wait left the fence unable to reach value 8"
        );
        println!("a later wait with a 10 s timeout reached value 8");
    }

    assert!(
        !device.is_device_lost(),
        "the device was lost after exercising the wait timeout path: {:?}",
        device.device_removed_reason()
    );
    let completed =
        unsafe { device.get_fence_value(&fence) }.expect("get_fence_value after timeouts failed");
    assert!(
        completed >= 8,
        "get_fence_value returned {completed} after value 8 completed"
    );
    println!("device is healthy; completed value = {completed}");

    unsafe { device.destroy_fence(fence) };
}

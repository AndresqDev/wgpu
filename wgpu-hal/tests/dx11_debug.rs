//! D3D11 debug-layer coverage.
//!
//! This test requests `D3D11_CREATE_DEVICE_DEBUG` through
//! `InstanceFlags::DEBUG`, proves that `ID3D11InfoQueue` is attached, triggers
//! a deliberate API error and checks that the structured message is surfaced
//! through `Device::take_debug_messages` instead of being silently dropped.
//!
//! The D3D11 debug layer is an optional Windows component
//! (`d3d11sdklayers.dll`, installed with the SDK's Graphics Tools). When it is
//! absent, device creation falls back to a normal device and the test prints an
//! explicit `SKIP` line with the limitation instead of failing: the production
//! path must keep working on machines without the SDK layer.
//!
//! Run on Windows with:
//!
//! ```text
//! cargo test -p wgpu-hal --features dx11 --test dx11_debug -- --nocapture
//! ```

#![cfg(all(target_os = "windows", feature = "dx11"))]

use wgpu_hal::{Adapter as _, Device as _, Instance as _, Queue as _};
use wgpu_types as wgt;

fn instance_descriptor(flags: wgt::InstanceFlags) -> wgpu_hal::InstanceDescriptor<'static> {
    wgpu_hal::InstanceDescriptor {
        name: "wgpu-hal-dx11-debug",
        flags,
        memory_budget_thresholds: wgt::MemoryBudgetThresholds::default(),
        backend_options: wgt::BackendOptions::default(),
        telemetry: None,
        display: None,
    }
}

fn open_first_device(
    flags: wgt::InstanceFlags,
) -> Result<
    (
        wgpu_hal::dx11::Instance,
        wgpu_hal::ExposedAdapter<wgpu_hal::dx11::Api>,
        wgpu_hal::OpenDevice<wgpu_hal::dx11::Api>,
    ),
    String,
> {
    let descriptor = instance_descriptor(flags);
    let instance = unsafe { <wgpu_hal::dx11::Instance as wgpu_hal::Instance>::init(&descriptor) }
        .map_err(|e| format!("D3D11 instance initialization failed: {e}"))?;

    let adapters = unsafe { instance.enumerate_adapters(None) };
    let exposed = adapters
        .into_iter()
        .next()
        .ok_or_else(|| "no D3D11 adapters were found".to_owned())?;

    let opened = unsafe {
        exposed.adapter.open(
            exposed.features,
            &exposed.capabilities.limits,
            &wgt::MemoryHints::default(),
        )
    }
    .map_err(|e| format!("Adapter::open failed: {e:?}"))?;

    Ok((instance, exposed, opened))
}

#[test]
fn dx11_debug_layer_reports_errors() {
    println!("=== D3D11 debug layer ===");

    let (_instance, exposed, opened) = match open_first_device(wgt::InstanceFlags::DEBUG) {
        Ok(opened) => opened,
        Err(error) => panic!("debug-layer instance setup failed: {error}"),
    };

    println!(
        "adapter: '{}' (software={})",
        exposed.info.name,
        matches!(exposed.info.device_type, wgt::DeviceType::Cpu)
    );

    if !exposed.adapter.debug_layer_available() {
        println!(
            "SKIP: the D3D11 debug layer (d3d11sdklayers.dll) is not installed on this machine."
        );
        println!(
            "SKIP: D3D11CreateDevice with D3D11_CREATE_DEVICE_DEBUG failed and the backend \
             retried without it, as designed."
        );
        println!(
            "SKIP: install the Windows SDK Graphics Tools optional feature to exercise this test."
        );
        return;
    }

    let wgpu_hal::OpenDevice { device, queue } = opened;
    assert!(
        device.debug_layer_available(),
        "the adapter reported a debug layer but the opened device has no ID3D11InfoQueue"
    );
    println!("debug layer attached: yes");

    // Drain messages produced by device creation so the assertion below only
    // sees messages caused by the deliberate error.
    let initial = device.take_debug_messages();
    println!("drained {} device-creation debug message(s)", initial.len());

    // Deliberate API error: a zero-byte D3D11 buffer is invalid. The D3D11
    // debug layer logs an error for it even though the HAL would reject the
    // descriptor before reaching the runtime.
    {
        use windows::Win32::Graphics::Direct3D11;

        let invalid = Direct3D11::D3D11_BUFFER_DESC {
            ByteWidth: 0,
            Usage: Direct3D11::D3D11_USAGE_DEFAULT,
            BindFlags: 0,
            CPUAccessFlags: 0,
            MiscFlags: 0,
            StructureByteStride: 0,
        };
        let mut raw: Option<Direct3D11::ID3D11Buffer> = None;
        let result = unsafe {
            device
                .raw_device()
                .CreateBuffer(&invalid, None, Some(&mut raw))
        };
        assert!(
            result.is_err(),
            "CreateBuffer(ByteWidth = 0) unexpectedly succeeded"
        );
        println!(
            "deliberate invalid CreateBuffer failed as expected: {}",
            result.unwrap_err()
        );
    }

    let messages = device.take_debug_messages();
    println!("captured {} debug message(s):", messages.len());
    for message in &messages {
        println!("  {message}");
    }

    assert!(
        messages.iter().any(|message| message.severity.is_error()),
        "the D3D11 debug layer stored no error/corruption message for an invalid call"
    );
    assert!(
        messages
            .iter()
            .filter(|message| message.severity.is_error())
            .all(|message| !message.description.trim().is_empty()),
        "a captured debug error has an empty description"
    );

    let drained = device.take_debug_messages();
    assert!(
        drained.is_empty(),
        "take_debug_messages did not drain the queue; {} message(s) remained",
        drained.len()
    );
    println!("drain semantics verified: the second take returned no messages");

    drop(queue);
    drop(device);
}

#[test]
fn dx11_normal_device_path_is_unaffected() {
    println!("=== D3D11 normal device path (no debug flags) ===");

    let (_instance, _exposed, opened) = match open_first_device(wgt::InstanceFlags::default()) {
        Ok(opened) => opened,
        Err(error) => panic!("normal instance setup failed: {error}"),
    };
    let wgpu_hal::OpenDevice { device, queue } = opened;

    let buffer = unsafe {
        device.create_buffer(&wgpu_hal::BufferDescriptor {
            label: Some("debug test normal-path buffer"),
            size: 16,
            usage: wgt::BufferUses::VERTEX | wgt::BufferUses::COPY_DST,
            memory_flags: wgpu_hal::MemoryFlags::empty(),
        })
    }
    .expect("normal device must still create buffers");
    println!("normal device created a 16-byte buffer");

    let fence = unsafe { device.create_fence() }.expect("normal device must create fences");
    unsafe { queue.submit(&[], &[], (&fence, 1)) }.expect("normal submission must succeed");
    assert!(
        unsafe { device.wait(&fence, 1, None) }.expect("normal wait must not fail"),
        "normal device fence did not reach its submitted value"
    );
    println!("normal device submitted and completed empty work");

    unsafe {
        device.destroy_fence(fence);
        device.destroy_buffer(buffer);
    }
    println!("normal device path completed without a debug layer");
}

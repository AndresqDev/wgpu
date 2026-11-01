//! Minimal D3D11 backend smoke test.
//!
//! Initializes the experimental `wgpu-hal` Direct3D 11 backend, enumerates
//! DXGI adapters, prints their name / feature level / VRAM, and opens the first
//! adapter. It exits with status 0 only if at least one adapter was found and a
//! D3D11 device could be created.
//!
//! Run on Windows with:
//!
//! ```text
//! cargo run -p wgpu-hal --example raw-dx11 --features dx11
//! ```

#[cfg(target_os = "windows")]
use wgpu_hal::{Adapter as _, Instance as _};
#[cfg(target_os = "windows")]
use wgpu_types as wgt;

#[cfg(target_os = "windows")]
fn main() {
    env_logger::init();

    let instance_desc = wgpu_hal::InstanceDescriptor {
        name: "raw-dx11",
        flags: wgt::InstanceFlags::default(),
        memory_budget_thresholds: wgt::MemoryBudgetThresholds::default(),
        backend_options: wgt::BackendOptions::default(),
        telemetry: None,
        display: None,
    };

    let instance =
        match unsafe { <wgpu_hal::dx11::Instance as wgpu_hal::Instance>::init(&instance_desc) } {
            Ok(instance) => instance,
            Err(e) => {
                eprintln!("failed to initialize the D3D11 backend: {e}");
                std::process::exit(1);
            }
        };

    let adapters = unsafe { instance.enumerate_adapters(None) };
    if adapters.is_empty() {
        eprintln!("no D3D11 adapters found");
        std::process::exit(1);
    }

    println!("found {} D3D11 adapter(s):", adapters.len());
    for exposed in &adapters {
        println!(
            "  - {} | feature level {} | dedicated VRAM {} MiB | shared {} MiB | {:?}",
            exposed.info.name,
            exposed.adapter.feature_level_name(),
            exposed.adapter.dedicated_video_memory() / (1024 * 1024),
            exposed.adapter.shared_system_memory() / (1024 * 1024),
            exposed.info.device_type,
        );
    }

    let exposed = &adapters[0];
    let open = unsafe {
        exposed.adapter.open(
            wgt::Features::empty(),
            &exposed.capabilities.limits,
            &wgt::MemoryHints::default(),
        )
    };

    match open {
        Ok(open) => {
            println!(
                "opened D3D11 device on '{}' ({:?})",
                exposed.info.name, open.device
            );
        }
        Err(e) => {
            eprintln!("failed to open the first D3D11 adapter: {e}");
            std::process::exit(1);
        }
    }
}

#[cfg(not(target_os = "windows"))]
fn main() {
    eprintln!("the dx11 backend is only available on Windows");
    std::process::exit(1);
}

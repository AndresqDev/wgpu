//! Windowed D3D11 swap-chain resize test.
//!
//! Creates a hidden Win32 window, initializes the experimental `wgpu-hal`
//! Direct3D 11 backend, configures a DXGI swap chain at 320x240, renders and
//! presents one cleared frame, then reconfigures the same surface at 640x360
//! and repeats. Before each frame the example asserts that the acquired
//! back-buffer texture really has the extent requested from
//! `Surface::configure`, which is what makes this a resize test rather than a
//! plain windowed smoke test.
//!
//! The window is never shown and messages are pumped non-blockingly, so the
//! example can run unattended on a CI runner.
//!
//! Run on Windows with:
//!
//! ```text
//! cargo run -p wgpu-hal --example raw-dx11-resize --features dx11
//! ```

#[cfg(target_os = "windows")]
fn main() {
    env_logger::init();

    if let Err(e) = run() {
        eprintln!("raw-dx11-resize failed: {e}");
        std::process::exit(1);
    }

    println!("raw-dx11-resize completed successfully");
}

#[cfg(target_os = "windows")]
fn pump_messages() -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, PeekMessageW, TranslateMessage, MSG, PM_REMOVE, WM_QUIT,
    };

    let mut message = MSG::default();
    loop {
        let has_message = unsafe { PeekMessageW(&mut message, None, 0, 0, PM_REMOVE) }.as_bool();
        if !has_message {
            return true;
        }
        if message.message == WM_QUIT {
            return false;
        }
        unsafe {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
}

/// Acquires one back buffer, verifies its extent, clears and presents it.
///
/// The acquired texture and the command buffer are dropped before this
/// function returns, which is required before `Surface::unconfigure` and
/// before the next `Surface::configure`: no surface texture may outlive its
/// swap chain.
#[cfg(target_os = "windows")]
fn render_and_present(
    device: &wgpu_hal::dx11::Device,
    queue: &wgpu_hal::dx11::Queue,
    surface: &wgpu_hal::dx11::Surface,
    fence: &wgpu_hal::dx11::Fence,
    extent: wgpu_types::Extent3d,
    clear_color: wgpu_types::Color,
    fence_value: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    use wgpu_hal::{CommandEncoder as _, Device as _, Queue as _, Surface as _};
    use wgpu_types as wgt;

    if !pump_messages() {
        return Err("the window message loop received WM_QUIT".into());
    }

    let acquired = unsafe { surface.acquire_texture(None, fence)? };

    // The public accessor is on `wgpu_hal::dx11::Texture`; the acquired
    // surface texture exposes it through `Borrow<Texture>`. Verify the real
    // back-buffer extent instead of trusting the requested configuration.
    {
        let texture: &wgpu_hal::dx11::Texture = std::borrow::Borrow::borrow(&acquired.texture);
        let acquired_extent = texture.extent();
        println!(
            "acquired {}x{} back buffer (depth_or_array_layers = {})",
            acquired_extent.width, acquired_extent.height, acquired_extent.depth_or_array_layers
        );
        if acquired_extent.width != extent.width
            || acquired_extent.height != extent.height
            || acquired_extent.depth_or_array_layers != 1
        {
            return Err(format!(
                "acquired swap-chain texture is {}x{}x{}, expected {}x{}x1",
                acquired_extent.width,
                acquired_extent.height,
                acquired_extent.depth_or_array_layers,
                extent.width,
                extent.height
            )
            .into());
        }
    }

    let mut encoder = unsafe {
        device.create_command_encoder(&wgpu_hal::CommandEncoderDescriptor {
            label: Some("raw-dx11-resize frame"),
            queue,
        })?
    };

    unsafe {
        encoder.begin_encoding(Some("raw-dx11-resize frame"))?;

        let color_attachment = wgpu_hal::ColorAttachment {
            target: wgpu_hal::Attachment {
                view: acquired.texture.view(),
                usage: wgt::TextureUses::COLOR_TARGET,
            },
            depth_slice: None,
            resolve_target: None,
            ops: wgpu_hal::AttachmentOps::LOAD_CLEAR | wgpu_hal::AttachmentOps::STORE,
            clear_value: clear_color,
        };
        let render_pass = wgpu_hal::RenderPassDescriptor {
            label: Some("clear the resized back buffer"),
            extent,
            sample_count: 1,
            color_attachments: &[Some(color_attachment)],
            depth_stencil_attachment: None,
            multiview_mask: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        };

        encoder.begin_render_pass(&render_pass)?;
        encoder.end_render_pass();
    }

    let command_buffer = unsafe { encoder.end_encoding()? };

    unsafe {
        queue.submit(
            &[&command_buffer],
            &[&acquired.texture],
            (fence, fence_value),
        )?;
        queue.present(surface, acquired.texture)?;
    }

    println!(
        "cleared and presented a {}x{} frame",
        extent.width, extent.height
    );
    // `command_buffer` and `acquired` are dropped here, and `acquired.texture`
    // was consumed by `present`; nothing may reference the swap chain when
    // the caller reconfigures or unconfigures the surface.
    Ok(())
}

#[cfg(target_os = "windows")]
fn run() -> Result<(), Box<dyn std::error::Error>> {
    use core::num::NonZeroIsize;

    use raw_window_handle::{
        RawDisplayHandle, RawWindowHandle, Win32WindowHandle, WindowsDisplayHandle,
    };
    use wgpu_hal::{Adapter as _, Device as _, Instance as _, Queue as _, Surface as _};
    use wgpu_types as wgt;
    use windows::{
        core::{w, PCWSTR},
        Win32::{
            Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM},
            Graphics::Gdi::HBRUSH,
            System::LibraryLoader::GetModuleHandleW,
            UI::WindowsAndMessaging::{
                CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassW, UnregisterClassW,
                CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, HCURSOR, HICON, WINDOW_EX_STYLE, WNDCLASSW,
                WS_OVERLAPPEDWINDOW,
            },
        },
    };

    const FIRST_WIDTH: u32 = 320;
    const FIRST_HEIGHT: u32 = 240;
    const SECOND_WIDTH: u32 = 640;
    const SECOND_HEIGHT: u32 = 360;

    const FIRST_CLEAR_COLOR: wgt::Color = wgt::Color {
        r: 0.05,
        g: 0.35,
        b: 0.8,
        a: 1.0,
    };
    const SECOND_CLEAR_COLOR: wgt::Color = wgt::Color {
        r: 0.8,
        g: 0.25,
        b: 0.1,
        a: 1.0,
    };

    struct Window {
        hwnd: HWND,
        hinstance: HINSTANCE,
        class_name: PCWSTR,
    }

    impl Drop for Window {
        fn drop(&mut self) {
            unsafe {
                let _ = DestroyWindow(self.hwnd);
                let _ = UnregisterClassW(self.class_name, Some(self.hinstance));
            }
        }
    }

    unsafe extern "system" fn window_proc(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
    }

    // Create a hidden Win32 window. Using `windows` directly instead of winit
    // avoids a blocking event loop, which keeps the CI run deterministic.
    let hmodule = unsafe { GetModuleHandleW(PCWSTR::null()) }?;
    let hinstance = HINSTANCE(hmodule.0);
    let class_name = w!("wgpu_hal_dx11_resize_window");
    let window_class = WNDCLASSW {
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(window_proc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: hinstance,
        hIcon: HICON(core::ptr::null_mut()),
        hCursor: HCURSOR(core::ptr::null_mut()),
        hbrBackground: HBRUSH(core::ptr::null_mut()),
        lpszMenuName: PCWSTR::null(),
        lpszClassName: class_name,
    };
    if unsafe { RegisterClassW(&window_class) } == 0 {
        return Err("RegisterClassW failed".into());
    }

    let hwnd = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class_name,
            w!("wgpu-hal D3D11 resize"),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            FIRST_WIDTH as i32,
            FIRST_HEIGHT as i32,
            None,
            None,
            Some(hinstance),
            None,
        )
    }?;
    let window = Window {
        hwnd,
        hinstance,
        class_name,
    };

    let hwnd =
        NonZeroIsize::new(window.hwnd.0 as isize).ok_or("CreateWindowExW returned a null HWND")?;
    let mut window_handle = Win32WindowHandle::new(hwnd);
    window_handle.hinstance = NonZeroIsize::new(hinstance.0 as isize);

    let instance_desc = wgpu_hal::InstanceDescriptor {
        name: "raw-dx11-resize",
        flags: wgt::InstanceFlags::default(),
        memory_budget_thresholds: wgt::MemoryBudgetThresholds::default(),
        backend_options: wgt::BackendOptions::default(),
        telemetry: None,
        display: None,
    };

    let instance =
        unsafe { <wgpu_hal::dx11::Instance as wgpu_hal::Instance>::init(&instance_desc)? };
    let surface = unsafe {
        instance.create_surface(
            RawDisplayHandle::Windows(WindowsDisplayHandle::new()),
            RawWindowHandle::Win32(window_handle),
        )?
    };

    let adapters = unsafe { instance.enumerate_adapters(Some(&surface)) };
    let exposed = adapters.first().ok_or("no D3D11 adapters were found")?;
    println!(
        "using D3D11 adapter '{}' ({:?})",
        exposed.info.name, exposed.info.device_type
    );

    let wgpu_hal::OpenDevice { device, queue } = unsafe {
        exposed.adapter.open(
            wgt::Features::empty(),
            &exposed.capabilities.limits,
            &wgt::MemoryHints::default(),
        )?
    };

    let mut config = wgpu_hal::SurfaceConfiguration {
        maximum_frame_latency: 1,
        present_mode: wgt::PresentMode::Fifo,
        composite_alpha_mode: wgt::CompositeAlphaMode::Opaque,
        format: wgt::TextureFormat::Bgra8UnormSrgb,
        color_space: wgt::SurfaceColorSpace::Srgb,
        extent: wgt::Extent3d {
            width: FIRST_WIDTH,
            height: FIRST_HEIGHT,
            depth_or_array_layers: 1,
        },
        usage: wgt::TextureUses::COLOR_TARGET,
        view_formats: Vec::new(),
    };

    unsafe { surface.configure(&device, &config)? };
    println!(
        "configured {}x{} {:?} surface",
        config.extent.width, config.extent.height, config.format
    );

    let fence = unsafe { device.create_fence()? };

    render_and_present(
        &device,
        &queue,
        &surface,
        &fence,
        config.extent,
        FIRST_CLEAR_COLOR,
        1,
    )?;
    unsafe { queue.wait_for_idle()? };
    println!(
        "first frame at {}x{} reached GPU idle",
        config.extent.width, config.extent.height
    );

    // Reconfigure the same surface at a different extent. The previous swap
    // chain and all of its render-target views have already been released:
    // `render_and_present` dropped its command buffer and the presented
    // surface texture before returning.
    config.extent = wgt::Extent3d {
        width: SECOND_WIDTH,
        height: SECOND_HEIGHT,
        depth_or_array_layers: 1,
    };
    unsafe { surface.configure(&device, &config)? };
    println!(
        "reconfigured to {}x{} {:?} surface",
        config.extent.width, config.extent.height, config.format
    );

    render_and_present(
        &device,
        &queue,
        &surface,
        &fence,
        config.extent,
        SECOND_CLEAR_COLOR,
        2,
    )?;
    unsafe { queue.wait_for_idle()? };
    println!(
        "second frame at {}x{} reached GPU idle",
        config.extent.width, config.extent.height
    );

    // Teardown order: no acquired surface texture or command buffer is alive at
    // this point, so the swap chain can be released before the device and
    // queue.
    unsafe { surface.unconfigure(&device) };
    unsafe { device.destroy_fence(fence) };
    println!("unconfigured the surface");

    drop(queue);
    drop(device);
    drop(surface);
    drop(window);
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn main() {
    eprintln!("the dx11 backend is only available on Windows");
    std::process::exit(1);
}

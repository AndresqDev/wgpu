//! Minimal windowed D3D11 backend smoke test.
//!
//! Creates a hidden Win32 window, initializes the experimental `wgpu-hal`
//! Direct3D 11 backend, configures a flip-model DXGI swap chain, clears each
//! back buffer to a known color, presents it, and exits after a bounded number
//! of frames. The window is never shown and messages are pumped
//! non-blockingly, so the example can run unattended on a CI runner.
//!
//! Run on Windows with:
//!
//! ```text
//! cargo run -p wgpu-hal --example raw-dx11-window --features dx11
//! ```

#[cfg(target_os = "windows")]
fn main() {
    env_logger::init();

    if let Err(e) = run() {
        eprintln!("raw-dx11-window failed: {e}");
        std::process::exit(1);
    }

    println!("raw-dx11-window completed successfully");
}

#[cfg(target_os = "windows")]
fn run() -> Result<(), Box<dyn std::error::Error>> {
    use core::num::NonZeroIsize;

    use raw_window_handle::{
        RawDisplayHandle, RawWindowHandle, Win32WindowHandle, WindowsDisplayHandle,
    };
    use wgpu_hal::{
        Adapter as _, CommandEncoder as _, Device as _, Instance as _, Queue as _, Surface as _,
    };
    use wgpu_types as wgt;
    use windows::{
        core::{w, PCWSTR},
        Win32::{
            Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM},
            Graphics::Gdi::HBRUSH,
            System::LibraryLoader::GetModuleHandleW,
            UI::WindowsAndMessaging::{
                CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, PeekMessageW,
                RegisterClassW, TranslateMessage, UnregisterClassW, CS_HREDRAW, CS_VREDRAW,
                CW_USEDEFAULT, HCURSOR, HICON, MSG, PM_REMOVE, WINDOW_EX_STYLE, WM_QUIT, WNDCLASSW,
                WS_OVERLAPPEDWINDOW,
            },
        },
    };

    const WINDOW_WIDTH: u32 = 320;
    const WINDOW_HEIGHT: u32 = 240;
    const FRAME_COUNT: u32 = 16;
    const CLEAR_COLOR: wgt::Color = wgt::Color {
        r: 0.05,
        g: 0.35,
        b: 0.8,
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

    fn pump_messages() -> bool {
        let mut message = MSG::default();
        loop {
            let has_message =
                unsafe { PeekMessageW(&mut message, None, 0, 0, PM_REMOVE) }.as_bool();
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

    // Create a hidden Win32 window. Using `windows` directly instead of winit
    // avoids a blocking event loop, which keeps the CI run deterministic.
    let hmodule = unsafe { GetModuleHandleW(PCWSTR::null()) }?;
    let hinstance = HINSTANCE(hmodule.0);
    let class_name = w!("wgpu_hal_dx11_surface_window");
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
            w!("wgpu-hal D3D11 surface"),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            WINDOW_WIDTH as i32,
            WINDOW_HEIGHT as i32,
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
        name: "raw-dx11-window",
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

    let capabilities = unsafe { exposed.adapter.surface_capabilities(&surface) }
        .ok_or("the D3D11 adapter does not support the created surface")?;
    println!(
        "surface formats: {:?}; present modes: {:?}",
        capabilities.texture_formats().collect::<Vec<_>>(),
        capabilities.present_modes
    );

    let wgpu_hal::OpenDevice { device, queue } = unsafe {
        exposed.adapter.open(
            wgt::Features::empty(),
            &exposed.capabilities.limits,
            &wgt::MemoryHints::default(),
        )?
    };

    let config = wgpu_hal::SurfaceConfiguration {
        maximum_frame_latency: 1,
        present_mode: wgt::PresentMode::Fifo,
        composite_alpha_mode: wgt::CompositeAlphaMode::Opaque,
        format: wgt::TextureFormat::Bgra8UnormSrgb,
        color_space: wgt::SurfaceColorSpace::Srgb,
        extent: wgt::Extent3d {
            width: WINDOW_WIDTH,
            height: WINDOW_HEIGHT,
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
    for frame in 0..FRAME_COUNT {
        if !pump_messages() {
            return Err("the window message loop received WM_QUIT".into());
        }

        let acquired = unsafe { surface.acquire_texture(None, &fence)? };
        let mut encoder = unsafe {
            device.create_command_encoder(&wgpu_hal::CommandEncoderDescriptor {
                label: Some("raw-dx11-window frame"),
                queue: &queue,
            })?
        };

        unsafe {
            encoder.begin_encoding(Some("raw-dx11-window frame"))?;

            let color_attachment = wgpu_hal::ColorAttachment {
                target: wgpu_hal::Attachment {
                    view: acquired.texture.view(),
                    usage: wgt::TextureUses::COLOR_TARGET,
                },
                depth_slice: None,
                resolve_target: None,
                ops: wgpu_hal::AttachmentOps::LOAD_CLEAR | wgpu_hal::AttachmentOps::STORE,
                clear_value: CLEAR_COLOR,
            };
            let render_pass = wgpu_hal::RenderPassDescriptor {
                label: Some("clear to a known color"),
                extent: config.extent,
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
                (&fence, u64::from(frame) + 1),
            )?;
            queue.present(&surface, acquired.texture)?;
        }

        println!("presented frame {}/{FRAME_COUNT}", frame + 1);
    }

    unsafe {
        queue.wait_for_idle()?;
        surface.unconfigure(&device);
        device.destroy_fence(fence);
    }

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

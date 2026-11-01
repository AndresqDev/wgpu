//! D3D11 queue.
//!
//! D3D11 has no command queue object. Commands are recorded into an
//! `ID3D11DeviceContext` and submitted when the context is flushed. The queue
//! owns the immediate context and signals fences on the CPU after submission;
//! real GPU-completion tracking will use an `ID3D11Query` event.

use core::{fmt, sync::atomic::Ordering};

use windows::Win32::Graphics::Direct3D11;

use super::{Api, CommandBuffer, Fence, SurfaceTexture};

/// The D3D11 submission queue.
pub struct Queue {
    context: Direct3D11::ID3D11DeviceContext,
    _device: Direct3D11::ID3D11Device,
    last_submitted: crate::AtomicFenceValue,
}

impl fmt::Debug for Queue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Queue")
            .field(
                "last_submitted",
                &self.last_submitted.load(Ordering::Relaxed),
            )
            .finish_non_exhaustive()
    }
}

impl Queue {
    pub(crate) fn new(
        context: Direct3D11::ID3D11DeviceContext,
        device: Direct3D11::ID3D11Device,
    ) -> Self {
        Self {
            context,
            _device: device,
            last_submitted: Default::default(),
        }
    }

    /// Returns the immediate device context used for submission.
    pub fn raw_context(&self) -> &Direct3D11::ID3D11DeviceContext {
        &self.context
    }
}

impl crate::Queue for Queue {
    type A = Api;

    unsafe fn submit(
        &self,
        command_buffers: &[&CommandBuffer],
        surface_textures: &[&SurfaceTexture],
        (fence, fence_value): (&Fence, crate::FenceValue),
    ) -> Result<(), crate::DeviceError> {
        // Render-pass clears are executed immediately on the shared immediate
        // context, so submission only has to flush the context. Deferred
        // contexts and ordered command-list execution are the next milestone.
        let _ = (command_buffers, surface_textures);

        unsafe { self.context.Flush() };
        self.last_submitted.fetch_max(fence_value, Ordering::AcqRel);
        fence.signal(fence_value);
        Ok(())
    }

    unsafe fn present(
        &self,
        surface: &super::Surface,
        texture: SurfaceTexture,
    ) -> Result<(), crate::SurfaceError> {
        surface.present_texture(&texture)
    }

    unsafe fn wait_for_idle(&self) -> Result<(), crate::DeviceError> {
        // Flush pushes pending commands to the GPU. Waiting for completion
        // requires the fence query implemented in the next step.
        unsafe { self.context.Flush() };
        Ok(())
    }

    unsafe fn get_timestamp_period(&self) -> f32 {
        1.0
    }
}

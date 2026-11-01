//! D3D11 queue.
//!
//! D3D11 has no command queue object. Commands are recorded into an
//! `ID3D11DeviceContext` and submitted when the context is flushed, so the
//! queue owns the immediate context.
//!
//! ## Completion tracking
//!
//! The queue owns one `ID3D11Query` of type `D3D11_QUERY_EVENT`. Every
//! [`crate::Queue::submit`] ends the query before flushing, which places a
//! marker in the command stream, and [`crate::Queue::wait_for_idle`] flushes
//! again and polls `ID3D11DeviceContext::GetData` on that marker until the GPU
//! has processed it. This is a real GPU-completion wait, not a CPU-side
//! approximation.
//!
//! The `Fence` value handed to `submit` is still signalled on the CPU and
//! [`crate::Device::wait`] still compares against that CPU value: a per-fence
//! `ID3D11_QUERY_EVENT` living in [`super::Fence`] is phase 2 and requires
//! changes to `types.rs`/`device.rs`, which this delivery intentionally avoids.

use core::{
    fmt,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

use windows::{core::BOOL, Win32::Graphics::Direct3D11};

use super::{Api, CommandBuffer, Fence, SurfaceTexture};

/// Upper bound for a single GPU-completion wait.
///
/// A hung GPU or a driver that never retires the marker must not block the
/// caller forever, so `wait_for_idle` gives up and reports an unexpected
/// error instead of spinning indefinitely.
const GPU_WAIT_TIMEOUT: Duration = Duration::from_secs(5);

/// How often the completion marker is polled while waiting.
const GPU_WAIT_POLL_INTERVAL: Duration = Duration::from_millis(1);

/// The D3D11 submission queue.
pub struct Queue {
    context: Direct3D11::ID3D11DeviceContext,
    device: Direct3D11::ID3D11Device,
    /// Completion marker shared by all submissions.
    ///
    /// A single query is enough because every marker covers all work that was
    /// recorded before it: the queue only needs to know that the GPU reached
    /// the most recent marker, not which submission a marker belongs to.
    event_query: Option<Direct3D11::ID3D11Query>,
    /// Whether `event_query` already holds a marker that has not been observed
    /// by a successful `GetData` yet.
    ///
    /// D3D11 does not allow the same event query to be re-armed while its
    /// previous marker is still in flight, so `submit` and `wait_for_idle`
    /// retire the outstanding marker before calling `End` again.
    marker_pending: AtomicBool,
    last_submitted: crate::AtomicFenceValue,
}

impl fmt::Debug for Queue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Queue")
            .field(
                "last_submitted",
                &self.last_submitted.load(Ordering::Relaxed),
            )
            .field("has_event_query", &self.event_query.is_some())
            .field("marker_pending", &self.marker_pending.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

impl Queue {
    pub(crate) fn new(
        context: Direct3D11::ID3D11DeviceContext,
        device: Direct3D11::ID3D11Device,
    ) -> Self {
        let event_query = create_event_query(&device);
        if event_query.is_none() {
            log::error!(
                "failed to create the D3D11_QUERY_EVENT completion query; \
                 submit and wait_for_idle will fail on this queue"
            );
        }

        Self {
            context,
            device,
            event_query,
            marker_pending: AtomicBool::new(false),
            last_submitted: Default::default(),
        }
    }

    /// Returns the immediate device context used for submission.
    pub fn raw_context(&self) -> &Direct3D11::ID3D11DeviceContext {
        &self.context
    }

    /// Returns the completion query, or an explicit error when it could not be
    /// created when the queue was opened.
    fn event_query(&self) -> Result<&Direct3D11::ID3D11Query, crate::DeviceError> {
        self.event_query.as_ref().ok_or_else(|| {
            log::error!(
                "the D3D11 queue has no D3D11_QUERY_EVENT completion query, so GPU completion \
                 cannot be tracked"
            );
            crate::DeviceError::Unexpected
        })
    }
}

/// Creates the `D3D11_QUERY_EVENT` marker used for GPU-completion waits.
fn create_event_query(device: &Direct3D11::ID3D11Device) -> Option<Direct3D11::ID3D11Query> {
    let desc = Direct3D11::D3D11_QUERY_DESC {
        Query: Direct3D11::D3D11_QUERY_EVENT,
        MiscFlags: 0,
    };

    let mut query: Option<Direct3D11::ID3D11Query> = None;
    match unsafe { device.CreateQuery(&desc, Some(&mut query)) } {
        Ok(()) => query,
        Err(error) => {
            log::error!("ID3D11Device::CreateQuery(D3D11_QUERY_EVENT) failed: {error}");
            None
        }
    }
}

/// Returns the removal reason when D3D11 no longer considers the device valid.
///
/// `ID3D11Device::GetDeviceRemovedReason` returns `S_OK` while the device is
/// healthy and the removal/reset reason otherwise, so an error here means any
/// following call would fail or produce undefined results.
fn device_removed(device: &Direct3D11::ID3D11Device) -> Option<windows::core::Error> {
    match unsafe { device.GetDeviceRemovedReason() } {
        Ok(()) => None,
        Err(error) => Some(error),
    }
}

/// Waits for the GPU to reach the marker enqueued by `EndQuery`.
///
/// `D3D11_QUERY_EVENT` writes a `BOOL` that becomes `TRUE` once the GPU has
/// processed the marker. The typed `windows` wrapper maps the "not ready yet"
/// `S_FALSE` result to `Ok(())`, so the payload is the only reliable completion
/// signal: `GetData` leaves it untouched while the query is still in flight.
/// Device loss is reported as `Lost`, a poll failure as `Unexpected` and a
/// timeout as `Unexpected` with an explicit log line; the function never
/// panics.
fn wait_for_event_query(
    context: &Direct3D11::ID3D11DeviceContext,
    device: &Direct3D11::ID3D11Device,
    query: &Direct3D11::ID3D11Query,
) -> Result<(), crate::DeviceError> {
    let start = std::time::Instant::now();

    loop {
        if let Some(error) = device_removed(device) {
            log::error!("D3D11 device removed while waiting for GPU completion: {error}");
            return Err(crate::DeviceError::Lost);
        }

        let mut complete = BOOL::from(false);
        let result = unsafe {
            context.GetData(
                query,
                Some(&mut complete as *mut BOOL as *mut core::ffi::c_void),
                core::mem::size_of::<BOOL>() as u32,
                0,
            )
        };

        match result {
            Ok(()) if complete.as_bool() => return Ok(()),
            // `S_FALSE`: the marker has not been reached yet, keep polling.
            Ok(()) => {}
            Err(error) => {
                if let Some(reason) = device_removed(device) {
                    log::error!(
                        "ID3D11DeviceContext::GetData(D3D11_QUERY_EVENT) failed because the \
                         device was removed: {reason}"
                    );
                    return Err(crate::DeviceError::Lost);
                }
                log::error!("ID3D11DeviceContext::GetData(D3D11_QUERY_EVENT) failed: {error}");
                return Err(crate::DeviceError::Unexpected);
            }
        }

        if start.elapsed() >= GPU_WAIT_TIMEOUT {
            log::error!(
                "timed out after {GPU_WAIT_TIMEOUT:?} waiting for the GPU to reach a \
                 D3D11_QUERY_EVENT marker"
            );
            return Err(crate::DeviceError::Unexpected);
        }

        std::thread::sleep(GPU_WAIT_POLL_INTERVAL);
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

        let query = self.event_query()?;

        if let Some(error) = device_removed(&self.device) {
            log::error!("cannot submit to the D3D11 device because it was removed: {error}");
            return Err(crate::DeviceError::Lost);
        }

        // Place a marker after everything recorded so far, then hand the
        // command stream to the GPU. The marker is not observed here: `submit`
        // must not claim GPU completion, only that the work was queued.
        //
        // The same query cannot be re-armed while a marker is still in flight,
        // so retire the outstanding one first. This is bounded by
        // `GPU_WAIT_TIMEOUT` and never reports completion early.
        if self.marker_pending.swap(false, Ordering::AcqRel) {
            if let Err(error) = wait_for_event_query(&self.context, &self.device, query) {
                self.marker_pending.store(true, Ordering::Release);
                return Err(error);
            }
        }

        unsafe { self.context.End(query) };
        self.marker_pending.store(true, Ordering::Release);
        unsafe { self.context.Flush() };

        if let Some(error) = device_removed(&self.device) {
            log::error!("the D3D11 device was removed while submitting work: {error}");
            return Err(crate::DeviceError::Lost);
        }

        self.last_submitted.fetch_max(fence_value, Ordering::AcqRel);

        // Fence values are still CPU-side bookkeeping in this delivery.
        // `Device::wait` compares against this value and therefore only proves
        // that the CPU submitted the work; a per-fence query that proves GPU
        // completion is phase 2.
        fence.signal(fence_value);

        Ok(())
    }

    unsafe fn present(
        &self,
        surface: &super::Surface,
        texture: SurfaceTexture,
    ) -> Result<(), crate::SurfaceError> {
        if let Some(error) = device_removed(&self.device) {
            log::error!("cannot present with a removed D3D11 device: {error}");
            return Err(crate::SurfaceError::Lost);
        }

        let result = surface.present_texture(&texture);

        // A present can be the call that discovers the removal (for example a
        // driver reset between frames), so check again afterwards.
        if let Some(error) = device_removed(&self.device) {
            log::error!("the D3D11 device was removed while presenting: {error}");
            return Err(crate::SurfaceError::Lost);
        }

        result
    }

    unsafe fn wait_for_idle(&self) -> Result<(), crate::DeviceError> {
        let query = self.event_query()?;

        if let Some(error) = device_removed(&self.device) {
            log::error!("cannot wait for the D3D11 queue because the device was removed: {error}");
            return Err(crate::DeviceError::Lost);
        }

        // Retire any marker left by a previous submit, then place a fresh one
        // that covers every command recorded before this call. The marker is
        // kept armed until `GetData` observes it so a timeout never allows the
        // query to be silently re-armed.
        if self.marker_pending.swap(false, Ordering::AcqRel) {
            if let Err(error) = wait_for_event_query(&self.context, &self.device, query) {
                self.marker_pending.store(true, Ordering::Release);
                return Err(error);
            }
        }

        unsafe { self.context.End(query) };
        self.marker_pending.store(true, Ordering::Release);
        unsafe { self.context.Flush() };

        if let Some(error) = device_removed(&self.device) {
            log::error!("the D3D11 device was removed while flushing for idle: {error}");
            return Err(crate::DeviceError::Lost);
        }

        let result = wait_for_event_query(&self.context, &self.device, query);
        if result.is_ok() {
            self.marker_pending.store(false, Ordering::Release);
        }
        result
    }

    unsafe fn get_timestamp_period(&self) -> f32 {
        1.0
    }
}

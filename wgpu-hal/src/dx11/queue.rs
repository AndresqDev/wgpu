//! D3D11 queue.
//!
//! D3D11 has no command queue object. Commands are recorded into an
//! `ID3D11DeviceContext` and submitted when the context is flushed, so the
//! queue owns the immediate context.
//!
//! ## Completion tracking
//!
//! The queue owns one `ID3D11Query` of type `D3D11_QUERY_EVENT` for
//! [`crate::Queue::wait_for_idle`], and every [`super::Fence`] owns its own
//! `D3D11_QUERY_EVENT` for [`crate::Device::wait`]. Submission ends the
//! relevant query before flushing, which places a marker in the command
//! stream; the wait entry points then poll `ID3D11DeviceContext::GetData` on
//! that marker until the GPU has processed it. Both are real GPU-completion
//! waits, not CPU-side approximations.
//!
//! D3D11 does not allow an event query to be re-armed while its previous
//! marker is still in flight, so a query is retired (its marker observed, or
//! the bounded queue timeout hit) before `EndQuery` is issued again. Fence
//! values are only published as pending on submission; they become completed
//! when the per-fence marker is observed, never earlier.

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
    /// Idle marker used by [`crate::Queue::wait_for_idle`].
    ///
    /// A single queue-owned query is enough for idleness because every marker
    /// covers all work recorded before it: the queue only needs to know that
    /// the GPU reached the most recent marker. Per-submission completion is
    /// tracked by the query owned by [`super::Fence`].
    event_query: Option<Direct3D11::ID3D11Query>,
    /// Whether `event_query` already holds a marker that has not been observed
    /// by a successful `GetData` yet.
    ///
    /// D3D11 does not allow the same event query to be re-armed while its
    /// previous marker is still in flight, so `wait_for_idle` retires the
    /// outstanding marker before calling `End` again.
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
                "failed to create the D3D11_QUERY_EVENT completion query; wait_for_idle will \
                 fail on this queue"
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

    /// Returns the queue's idle marker, or an explicit error when it could not
    /// be created when the queue was opened.
    fn event_query(&self) -> Result<&Direct3D11::ID3D11Query, crate::DeviceError> {
        self.event_query.as_ref().ok_or_else(|| {
            log::error!(
                "the D3D11 queue has no D3D11_QUERY_EVENT idle marker, so wait_for_idle cannot \
                 track GPU completion"
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

/// Performs one non-blocking completion check on a `D3D11_QUERY_EVENT` marker.
///
/// `D3D11_QUERY_EVENT` writes a `BOOL` that becomes `TRUE` once the GPU has
/// processed the marker. The typed `windows` wrapper maps the "not ready yet"
/// `S_FALSE` result to `Ok(())`, so the payload is the only reliable completion
/// signal: `GetData` leaves it untouched while the query is still in flight.
pub(crate) fn query_complete(
    context: &Direct3D11::ID3D11DeviceContext,
    device: &Direct3D11::ID3D11Device,
    query: &Direct3D11::ID3D11Query,
) -> Result<bool, crate::DeviceError> {
    if let Some(error) = device_removed(device) {
        log::error!("D3D11 device removed while polling GPU completion: {error}");
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
        Ok(()) => Ok(complete.as_bool()),
        Err(error) => {
            if let Some(reason) = device_removed(device) {
                log::error!(
                    "ID3D11DeviceContext::GetData(D3D11_QUERY_EVENT) failed because the \
                     device was removed: {reason}"
                );
                return Err(crate::DeviceError::Lost);
            }
            log::error!("ID3D11DeviceContext::GetData(D3D11_QUERY_EVENT) failed: {error}");
            Err(crate::DeviceError::Unexpected)
        }
    }
}

/// Waits for the GPU to reach the marker most recently enqueued on `query`.
///
/// Returns `Ok(true)` when the marker completed, `Ok(false)` when `timeout`
/// expired with the marker still in flight, `Err(DeviceError::Lost)` when the
/// device was removed and `Err(DeviceError::Unexpected)` when `GetData` fails.
/// `None` means "no deadline": the loop keeps polling until the marker
/// completes or the device goes away. This function never panics.
pub(crate) fn wait_for_query(
    context: &Direct3D11::ID3D11DeviceContext,
    device: &Direct3D11::ID3D11Device,
    query: &Direct3D11::ID3D11Query,
    timeout: Option<Duration>,
) -> Result<bool, crate::DeviceError> {
    let start = std::time::Instant::now();

    loop {
        if query_complete(context, device, query)? {
            return Ok(true);
        }

        if let Some(limit) = timeout {
            if start.elapsed() >= limit {
                log::warn!(
                    "timed out after {limit:?} waiting for a D3D11_QUERY_EVENT marker; the \
                     marker is still in flight"
                );
                return Ok(false);
            }
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

        if let Some(error) = device_removed(&self.device) {
            log::error!("cannot submit to the D3D11 device because it was removed: {error}");
            return Err(crate::DeviceError::Lost);
        }

        // The same event query cannot be re-armed while a marker is still in
        // flight, so retire the marker of the previous submission on this
        // fence first. This observes real GPU work and only advances
        // `completed_value` up to what the GPU has actually reached.
        let pending = fence.pending();
        if pending > fence.completed() {
            let retired = wait_for_query(
                &self.context,
                &self.device,
                fence.query(),
                Some(GPU_WAIT_TIMEOUT),
            )?;
            if retired {
                // Publish only the value this marker was armed for. A newer
                // submission may have advanced `pending` meanwhile.
                fence.note_completed(pending);
            } else {
                log::error!(
                    "the previous submission on this D3D11 fence did not complete within \
                     {GPU_WAIT_TIMEOUT:?}; refusing to re-arm its completion query"
                );
                return Err(crate::DeviceError::Unexpected);
            }
        }

        // Place the marker after everything recorded so far, then hand the
        // command stream to the GPU. `submit` must not claim GPU completion:
        // it only publishes the pending value, and `Device::wait` observes the
        // marker before moving the completed value.
        unsafe { self.context.End(fence.query()) };
        fence.note_pending(fence_value);
        unsafe { self.context.Flush() };

        if let Some(error) = device_removed(&self.device) {
            log::error!("the D3D11 device was removed while submitting work: {error}");
            return Err(crate::DeviceError::Lost);
        }

        self.last_submitted.fetch_max(fence_value, Ordering::AcqRel);

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

        // Retire any marker left by a previous `submit` or `wait_for_idle`,
        // then place a fresh one that covers every command recorded before
        // this call. The marker is kept armed until `GetData` observes it, so
        // a timeout never allows the query to be silently re-armed.
        if self.marker_pending.swap(false, Ordering::AcqRel) {
            let retired = wait_for_query(
                &self.context,
                &self.device,
                query,
                Some(GPU_WAIT_TIMEOUT),
            );
            match retired {
                Ok(true) => {}
                Ok(false) => {
                    self.marker_pending.store(true, Ordering::Release);
                    log::error!(
                        "timed out waiting for the outstanding D3D11 queue marker; the queue \
                         is not idle"
                    );
                    return Err(crate::DeviceError::Unexpected);
                }
                Err(error) => {
                    self.marker_pending.store(true, Ordering::Release);
                    return Err(error);
                }
            }
        }

        unsafe { self.context.End(query) };
        self.marker_pending.store(true, Ordering::Release);
        unsafe { self.context.Flush() };

        if let Some(error) = device_removed(&self.device) {
            log::error!("the D3D11 device was removed while flushing for idle: {error}");
            return Err(crate::DeviceError::Lost);
        }

        let result = wait_for_query(&self.context, &self.device, query, Some(GPU_WAIT_TIMEOUT));
        match result {
            Ok(true) => {
                self.marker_pending.store(false, Ordering::Release);
                Ok(())
            }
            Ok(false) => {
                log::error!(
                    "timed out after {GPU_WAIT_TIMEOUT:?} waiting for the D3D11 queue to \
                     become idle"
                );
                Err(crate::DeviceError::Unexpected)
            }
            Err(error) => Err(error),
        }
    }

    unsafe fn get_timestamp_period(&self) -> f32 {
        1.0
    }
}

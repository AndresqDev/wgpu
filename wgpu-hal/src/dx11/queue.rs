//! D3D11 queue.
//!
//! D3D11 has no command queue object. Commands are recorded into an
//! `ID3D11DeviceContext` and submitted when the context is flushed, so the
//! queue owns the immediate context.
//!
//! ## Completion tracking
//!
//! The queue owns one `ID3D11Query` of type `D3D11_QUERY_EVENT` for
//! [`crate::Queue::wait_for_idle`], and every [`super::Fence`] owns a pool of
//! eight such queries for [`crate::Device::wait`]. Submission arms a free
//! per-fence slot before flushing, which places a marker in the command
//! stream; the wait entry points then poll `ID3D11DeviceContext::GetData` on
//! the armed markers until the GPU has processed them. Both are real
//! GPU-completion waits, not CPU-side approximations.
//!
//! D3D11 does not allow an event query to be re-armed while its previous
//! marker is still in flight, so a fence slot is only reused once its marker
//! has been observed, or is rotated to a fresh query when it remains in flight
//! past the slot-rotation timeout. The queue's idle marker follows the same
//! rule. Fence values are only published as pending on submission; they become
//! completed when a marker that covers them is observed, never earlier.

use core::{
    fmt,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

use windows::{
    core::BOOL,
    Win32::{
        Foundation,
        Graphics::{Direct3D11, Dxgi},
    },
};

use super::{debug::DebugLayer, Api, CommandBuffer, Fence, SurfaceTexture};

/// Upper bound for a single GPU-completion wait.
///
/// A hung GPU or a driver that never retires the marker must not block the
/// caller forever, so `wait_for_idle` gives up and reports an unexpected
/// error instead of spinning indefinitely.
const GPU_WAIT_TIMEOUT: Duration = Duration::from_secs(5);

/// How often the completion marker is polled while waiting.
pub(crate) const GPU_WAIT_POLL_INTERVAL: Duration = Duration::from_millis(1);

/// The D3D11 submission queue.
pub struct Queue {
    context: Direct3D11::ID3D11DeviceContext,
    device: Direct3D11::ID3D11Device,
    /// Idle marker used by [`crate::Queue::wait_for_idle`].
    ///
    /// A single queue-owned query is enough for idleness because every marker
    /// covers all work recorded before it: the queue only needs to know that
    /// the GPU reached the most recent marker. Per-submission completion is
    /// tracked by the query pool owned by [`super::Fence`].
    event_query: Option<Direct3D11::ID3D11Query>,
    /// Whether `event_query` already holds a marker that has not been observed
    /// by a successful `GetData` yet.
    ///
    /// D3D11 does not allow the same event query to be re-armed while its
    /// previous marker is still in flight, so `wait_for_idle` retires the
    /// outstanding marker before calling `End` again.
    marker_pending: AtomicBool,
    last_submitted: crate::AtomicFenceValue,
    /// Debug-layer message queue of the device this queue submits to.
    ///
    /// `None` for devices created without `D3D11_CREATE_DEVICE_DEBUG`, so the
    /// normal path is unchanged. When present, messages are drained into the
    /// `log` crate after submission, presentation and idle waits instead of
    /// being left unread in the runtime's bounded queue.
    debug_layer: Option<DebugLayer>,
}

impl fmt::Debug for Queue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Queue")
            .field(
                "last_submitted",
                &self.last_submitted.load(Ordering::Relaxed),
            )
            .field("has_event_query", &self.event_query.is_some())
            .field(
                "marker_pending",
                &self.marker_pending.load(Ordering::Relaxed),
            )
            .field("has_debug_layer", &self.debug_layer.is_some())
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

        // Only debug devices expose `ID3D11InfoQueue`; probing here keeps the
        // normal path unchanged while letting `submit`/`present` surface
        // debug-layer errors.
        let debug_layer = DebugLayer::new(&device);

        Self {
            context,
            device,
            event_query,
            marker_pending: AtomicBool::new(false),
            last_submitted: Default::default(),
            debug_layer,
        }
    }

    /// Returns the immediate device context used for submission.
    pub fn raw_context(&self) -> &Direct3D11::ID3D11DeviceContext {
        &self.context
    }

    /// Drains the debug layer, if this queue has one, and logs the messages.
    ///
    /// No-op on devices created without the debug flag.
    fn report_debug_messages(&self) {
        if let Some(debug_layer) = &self.debug_layer {
            debug_layer.report();
        }
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
pub(crate) fn create_event_query(
    device: &Direct3D11::ID3D11Device,
) -> Option<Direct3D11::ID3D11Query> {
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

/// Returns whether `code` is a DXGI failure that means the device is gone.
///
/// D3D11 reports removal, reset and hang with distinct HRESULTs; all three
/// invalidate the device and must surface as [`crate::DeviceError::Lost`] (or
/// [`crate::SurfaceError::Lost`]) instead of a generic unexpected error.
pub(crate) fn is_device_loss_hresult(code: windows::core::HRESULT) -> bool {
    code == Dxgi::DXGI_ERROR_DEVICE_REMOVED
        || code == Dxgi::DXGI_ERROR_DEVICE_RESET
        || code == Dxgi::DXGI_ERROR_DEVICE_HUNG
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
            if is_device_loss_hresult(error.code()) {
                log::error!(
                    "ID3D11DeviceContext::GetData(D3D11_QUERY_EVENT) failed because the \
                     device was removed, reset or hung: {error}"
                );
                return Err(crate::DeviceError::Lost);
            }
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
        // The immediate context is shared with uploads and presentation. Hold
        // the same lock while executing command lists and arming markers.
        let _context_guard = super::command::lock_context();
        let _ = surface_textures;

        if let Some(error) = device_removed(&self.device) {
            log::error!("cannot submit to the D3D11 device because it was removed: {error}");
            self.report_debug_messages();
            return Err(crate::DeviceError::Lost);
        }

        // The HAL contract says a fence value lower than the highest submitted
        // value has no effect. Flush already-submitted work, but do not arm
        // another marker: reusing an out-of-order value would make `wait`
        // publish completion for a value that this marker does not cover.
        if fence_value <= fence.pending() {
            log::warn!(
                "ignoring D3D11 fence value {fence_value}: the fence already has a higher \
                 submitted value {}",
                fence.pending()
            );
            unsafe { self.context.Flush() };
            if let Some(error) = device_removed(&self.device) {
                log::error!("the D3D11 device was removed while flushing ignored work: {error}");
                self.report_debug_messages();
                return Err(crate::DeviceError::Lost);
            }
            self.report_debug_messages();
            return Ok(());
        }

        for command_buffer in command_buffers {
            let Some(command_list) = command_buffer.command_list() else {
                continue;
            };
            if let Err(error) = unsafe {
                self.context
                    .ExecuteCommandList(command_list, Foundation::BOOL::from(false))
            } {
                log::error!("ID3D11DeviceContext::ExecuteCommandList failed: {error}");
                self.report_debug_messages();
                if device_removed(&self.device).is_some() {
                    return Err(crate::DeviceError::Lost);
                }
                return Err(crate::DeviceError::Unexpected);
            }
        }

        // Arm a per-fence marker after everything recorded so far, then hand
        // the command stream to the GPU. The fence reuses a free pool slot or
        // rotates the oldest in-flight marker; it publishes the pending value
        // before returning. `submit` must not claim GPU completion: only
        // `Device::wait` observes a marker and moves the completed value.
        fence.arm(&self.context, &self.device, fence_value)?;
        unsafe { self.context.Flush() };

        if let Some(error) = device_removed(&self.device) {
            log::error!("the D3D11 device was removed while submitting work: {error}");
            self.report_debug_messages();
            return Err(crate::DeviceError::Lost);
        }

        self.last_submitted.fetch_max(fence_value, Ordering::AcqRel);
        self.report_debug_messages();

        Ok(())
    }

    unsafe fn present(
        &self,
        surface: &super::Surface,
        texture: SurfaceTexture,
    ) -> Result<(), crate::SurfaceError> {
        if let Some(error) = device_removed(&self.device) {
            log::error!("cannot present with a removed D3D11 device: {error}");
            self.report_debug_messages();
            return Err(crate::SurfaceError::Lost);
        }

        let result = surface.present_texture(&texture);

        // A present can be the call that discovers the removal (for example a
        // driver reset between frames), so check again afterwards.
        if let Some(error) = device_removed(&self.device) {
            log::error!("the D3D11 device was removed while presenting: {error}");
            self.report_debug_messages();
            return Err(crate::SurfaceError::Lost);
        }

        self.report_debug_messages();
        result
    }

    unsafe fn wait_for_idle(&self) -> Result<(), crate::DeviceError> {
        let _context_guard = super::command::lock_context();
        let query = self.event_query()?;

        if let Some(error) = device_removed(&self.device) {
            log::error!("cannot wait for the D3D11 queue because the device was removed: {error}");
            self.report_debug_messages();
            return Err(crate::DeviceError::Lost);
        }

        // Retire any marker left by a previous `submit` or `wait_for_idle`,
        // then place a fresh one that covers every command recorded before
        // this call. The marker is kept armed until `GetData` observes it, so
        // a timeout never allows the query to be silently re-armed.
        if self.marker_pending.swap(false, Ordering::AcqRel) {
            let retired =
                wait_for_query(&self.context, &self.device, query, Some(GPU_WAIT_TIMEOUT));
            match retired {
                Ok(true) => {}
                Ok(false) => {
                    self.marker_pending.store(true, Ordering::Release);
                    log::error!(
                        "timed out waiting for the outstanding D3D11 queue marker; the queue \
                         is not idle"
                    );
                    self.report_debug_messages();
                    return Err(crate::DeviceError::Unexpected);
                }
                Err(error) => {
                    self.marker_pending.store(true, Ordering::Release);
                    self.report_debug_messages();
                    return Err(error);
                }
            }
        }

        unsafe { self.context.End(query) };
        self.marker_pending.store(true, Ordering::Release);
        unsafe { self.context.Flush() };

        if let Some(error) = device_removed(&self.device) {
            log::error!("the D3D11 device was removed while flushing for idle: {error}");
            self.report_debug_messages();
            return Err(crate::DeviceError::Lost);
        }

        let result = wait_for_query(&self.context, &self.device, query, Some(GPU_WAIT_TIMEOUT));
        self.report_debug_messages();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_device_loss_hresults() {
        for code in [
            Dxgi::DXGI_ERROR_DEVICE_REMOVED,
            Dxgi::DXGI_ERROR_DEVICE_RESET,
            Dxgi::DXGI_ERROR_DEVICE_HUNG,
        ] {
            assert!(
                is_device_loss_hresult(code),
                "{code:?} must be classified as device loss"
            );
        }

        for code in [
            Dxgi::DXGI_ERROR_INVALID_CALL,
            Foundation::E_INVALIDARG,
            Foundation::E_FAIL,
        ] {
            assert!(
                !is_device_loss_hresult(code),
                "{code:?} must not be classified as device loss"
            );
        }
    }

    /// Checks that `Fence::wait` honours a timeout when a value is pending but
    /// no marker can be polled.
    ///
    /// `force_pending_for_test` publishes the value without arming a pool slot,
    /// so the wait loop has no GPU marker to observe and must return
    /// `Ok(false)` at the deadline. This exercises the same timeout branch that
    /// a marker stuck behind a hung GPU would hit, without depending on WARP
    /// speed or a real TDR.
    #[test]
    #[cfg(target_os = "windows")]
    fn fence_wait_times_out_when_no_marker_is_armed() {
        use windows::Win32::{Foundation::HMODULE, Graphics::Direct3D};

        let feature_levels = [Direct3D::D3D_FEATURE_LEVEL_11_0];
        let mut device: Option<Direct3D11::ID3D11Device> = None;
        let mut context: Option<Direct3D11::ID3D11DeviceContext> = None;

        unsafe {
            Direct3D11::D3D11CreateDevice(
                None::<&Dxgi::IDXGIAdapter>,
                Direct3D::D3D_DRIVER_TYPE_WARP,
                HMODULE::default(),
                Direct3D11::D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&feature_levels),
                Direct3D11::D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )
        }
        .expect("D3D11CreateDevice(WARP) failed while setting up the fence-timeout test");

        let device = device.expect("D3D11CreateDevice succeeded without returning a device");
        let context = context.expect("D3D11CreateDevice succeeded without returning a context");
        let fence = Fence::new(&device).expect("failed to create the fence for the timeout test");

        fence.force_pending_for_test(1);

        let start = std::time::Instant::now();
        let reached = fence
            .wait(&context, &device, 1, Some(Duration::from_millis(20)))
            .expect("Fence::wait failed on the timeout test fence");
        let elapsed = start.elapsed();

        assert!(
            !reached,
            "Fence::wait reported completion although no marker was armed"
        );
        assert!(
            elapsed >= Duration::from_millis(20),
            "Fence::wait returned before its 20 ms timeout ({elapsed:?})"
        );
        assert!(
            elapsed < Duration::from_secs(5),
            "Fence::wait blocked for {elapsed:?} although it had a 20 ms timeout"
        );
        assert_eq!(
            fence.completed(),
            0,
            "a timeout must not advance the completed fence value"
        );
        assert_eq!(
            fence
                .get_completed_value(&context, &device)
                .expect("get_completed_value failed on the timeout test fence"),
            0,
            "polling after a timeout must not invent a completed value"
        );
    }
}

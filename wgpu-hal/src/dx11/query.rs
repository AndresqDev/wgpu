//! Per-fence GPU-completion markers.
//!
//! Each [`Fence`] owns a small pool of `D3D11_QUERY_EVENT` queries. A
//! submission arms a free slot with `End` immediately before the queue flushes
//! the immediate context, so the slot records a point in the command stream
//! and the fence value submitted there. Waits poll the armed slots with
//! `ID3D11DeviceContext::GetData`; only a marker the GPU has actually reached
//! advances the fence's completed value.
//!
//! ## Pool and rotation
//!
//! D3D11 does not allow an event query to be re-armed while its previous
//! marker is still in flight: polling would only observe the newest marker and
//! older values would lose their tracking. A fence therefore never re-arms an
//! armed slot. When all [`QUERY_POOL_SIZE`] slots are in flight, `submit`
//! waits briefly for the oldest marker ([`SLOT_ROTATION_TIMEOUT`], 100 ms). If
//! the marker completes, the slot is freed and reused. If it does not, the
//! slot is rotated: a fresh query replaces the old one and the in-flight marker
//! is abandoned. Releasing the old COM interface is safe because the D3D11
//! device keeps its own reference until the GPU retires the query. A rotation
//! timeout never fails a submission; the next marker is armed normally.
//!
//! ## Why a completed marker covers earlier values
//!
//! The immediate context executes commands in order. When the marker armed for
//! value `v` is observed complete, every command recorded before it, including
//! every earlier marker of the same fence, has completed as well. It is
//! therefore correct to advance `completed` to `v`. The same reasoning covers
//! rotation: a replacement query was armed later in the same stream, so when
//! its marker completes it also implies the abandoned marker's value has
//! completed. `completed` is never advanced to a value that no observed marker
//! covers.

use alloc::vec::Vec;
use core::{fmt, sync::atomic::Ordering, time::Duration};

use windows::Win32::Graphics::Direct3D11;

/// Number of `D3D11_QUERY_EVENT` slots owned by one fence.
pub(crate) const QUERY_POOL_SIZE: usize = 8;

/// How long a fence waits for its oldest in-flight marker before rotating the
/// slot to a fresh query.
///
/// Rotation only happens when all slots are armed, so the wait is bounded: a
/// hung marker must not block the next submission forever. The abandoned
/// marker is still covered by any later marker in the same command stream.
pub(crate) const SLOT_ROTATION_TIMEOUT: Duration = Duration::from_millis(100);

/// One armed (or free) `D3D11_QUERY_EVENT` slot.
struct Slot {
    /// Event query used to place the marker and poll for completion.
    query: Direct3D11::ID3D11Query,
    /// Fence value whose marker is armed on `query`, or `None` when the slot is
    /// free.
    value: Option<u64>,
    /// Order in which the slot was last armed. Used to find the oldest marker
    /// when the pool is exhausted.
    sequence: u64,
}

/// Mutable state protected by the fence's mutex.
struct FenceSlots {
    slots: Vec<Slot>,
    next_sequence: u64,
}

/// A GPU-completion fence backed by a pool of `D3D11_QUERY_EVENT` queries.
///
/// [`crate::Queue::submit`] arms a free slot immediately before flushing the
/// immediate context and [`crate::Device::wait`] polls the armed slots. Two
/// atomic values track progress:
///
/// - `pending_value` is the highest value submitted on this fence;
/// - `completed_value` is the highest value the GPU has been observed to
///   reach.
///
/// The atomics are safe to read from any thread. The query pool is protected
/// by a `parking_lot::Mutex`, and wait loops do not hold that lock while
/// sleeping. Under the HAL's external-synchronization contract a fence's
/// submit and wait calls are not issued concurrently by the caller.
pub struct Fence {
    slots: parking_lot::Mutex<FenceSlots>,
    pending_value: crate::AtomicFenceValue,
    completed_value: crate::AtomicFenceValue,
}

impl Fence {
    /// Creates a fence with exactly [`QUERY_POOL_SIZE`] event queries.
    ///
    /// Passing a partially created pool to the caller would leave submits
    /// unable to arm a marker, so any query creation failure is reported as
    /// [`crate::DeviceError::Unexpected`] and the already created queries are
    /// released when the local vector is dropped.
    pub(crate) fn new(device: &Direct3D11::ID3D11Device) -> Result<Self, crate::DeviceError> {
        let mut slots = Vec::with_capacity(QUERY_POOL_SIZE);
        for index in 0..QUERY_POOL_SIZE {
            let query = super::queue::create_event_query(device).ok_or_else(|| {
                log::error!(
                    "failed to create D3D11_QUERY_EVENT slot {index} of {QUERY_POOL_SIZE} for a \
                     D3D11 fence"
                );
                crate::DeviceError::Unexpected
            })?;
            slots.push(Slot {
                query,
                value: None,
                sequence: 0,
            });
        }

        Ok(Self {
            slots: parking_lot::Mutex::new(FenceSlots {
                slots,
                next_sequence: 0,
            }),
            pending_value: Default::default(),
            completed_value: Default::default(),
        })
    }

    /// Highest fence value submitted on this fence.
    pub(crate) fn pending(&self) -> u64 {
        self.pending_value.load(Ordering::Acquire)
    }

    /// Highest fence value the GPU has been observed to complete.
    pub(crate) fn completed(&self) -> u64 {
        self.completed_value.load(Ordering::Acquire)
    }

    /// Publishes a pending value without arming a marker.
    ///
    /// Only used by the timeout unit test: it lets the wait loop run against a
    /// fence whose slots are all free, which deterministically exercises the
    /// timeout branch without depending on how fast a real GPU drains markers.
    #[cfg(test)]
    pub(crate) fn force_pending_for_test(&self, value: u64) {
        self.pending_value.fetch_max(value, Ordering::AcqRel);
    }

    /// Polls every armed slot without blocking.
    ///
    /// Completed slots advance `completed_value` to the value their marker was
    /// armed for and become free. This never re-arms a slot that is still in
    /// flight.
    fn poll_all(
        &self,
        context: &Direct3D11::ID3D11DeviceContext,
        device: &Direct3D11::ID3D11Device,
    ) -> Result<(), crate::DeviceError> {
        let mut slots = self.slots.lock();
        poll_slots(&self.completed_value, &mut slots, context, device)
    }

    /// Arms a marker for `value`, reusing or rotating a pool slot.
    ///
    /// A free slot is used directly. When the pool is exhausted, the oldest
    /// armed slot is retired with [`SLOT_ROTATION_TIMEOUT`]; if its marker does
    /// not complete, the slot is rotated to a fresh query instead of failing
    /// the submission. The pending value is published before this function
    /// returns, so the caller's subsequent `Flush` cannot make a marker visible
    /// to a waiter before the fence knows the value it covers.
    pub(crate) fn arm(
        &self,
        context: &Direct3D11::ID3D11DeviceContext,
        device: &Direct3D11::ID3D11Device,
        value: u64,
    ) -> Result<(), crate::DeviceError> {
        let mut slots = self.slots.lock();

        poll_slots(&self.completed_value, &mut slots, context, device)?;

        let index = match slots.slots.iter().position(|slot| slot.value.is_none()) {
            Some(index) => index,
            None => {
                // Every slot is armed: retire the oldest marker first. Sequence
                // numbers are unique per arm, so this selection is stable.
                let index = slots
                    .slots
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, slot)| slot.sequence)
                    .map(|(index, _)| index)
                    .expect("a fence always owns at least one query slot");

                let retired = {
                    let query = &slots.slots[index].query;
                    super::queue::wait_for_query(
                        context,
                        device,
                        query,
                        Some(SLOT_ROTATION_TIMEOUT),
                    )?
                };

                if retired {
                    // This marker was observed, which also covers every value
                    // armed before it on this fence.
                    if let Some(retired_value) = slots.slots[index].value {
                        self.completed_value
                            .fetch_max(retired_value, Ordering::AcqRel);
                    }
                    slots.slots[index].value = None;
                } else {
                    log::warn!(
                        "the oldest D3D11 fence marker did not complete within \
                         {SLOT_ROTATION_TIMEOUT:?}; rotating slot {index} to a fresh query so the \
                         next submission is not blocked"
                    );

                    let replacement =
                        super::queue::create_event_query(device).ok_or_else(|| {
                            log::error!(
                                "failed to rotate D3D11 fence slot {index} to a fresh \
                             D3D11_QUERY_EVENT after a {SLOT_ROTATION_TIMEOUT:?} timeout"
                            );
                            crate::DeviceError::Unexpected
                        })?;
                    // Dropping the previous query is safe: the D3D11 device
                    // keeps its own reference while the marker is in flight,
                    // and the replacement marker is later in the same ordered
                    // stream, so its completion covers the abandoned value.
                    slots.slots[index].query = replacement;
                    slots.slots[index].value = None;
                }

                index
            }
        };

        let sequence = slots.next_sequence;
        slots.next_sequence = slots.next_sequence.wrapping_add(1);

        {
            let slot = &mut slots.slots[index];

            // Publish the pending value while the slots are still locked and
            // before `End`, so a concurrent waiter can never observe the new
            // marker (or the stale `pending` value) without knowing which
            // fence value it covers. Waiting blocks on the same mutex until the
            // slot has been armed.
            self.pending_value.fetch_max(value, Ordering::AcqRel);

            // `End` records the marker in the immediate context stream. The
            // slot stays armed until its marker is observed or rotated.
            unsafe { context.End(&slot.query) };
            slot.value = Some(value);
            slot.sequence = sequence;
        }

        Ok(())
    }

    /// Waits until the GPU reaches `value` or `timeout` expires.
    ///
    /// Returns `Ok(false)` for a value that was never submitted (waiting
    /// cannot make progress) and for a timeout. Device loss is propagated as
    /// [`crate::DeviceError::Lost`]. The pool mutex is only held while polling,
    /// never across the sleep between polls.
    pub(crate) fn wait(
        &self,
        context: &Direct3D11::ID3D11DeviceContext,
        device: &Direct3D11::ID3D11Device,
        value: u64,
        timeout: Option<Duration>,
    ) -> Result<bool, crate::DeviceError> {
        let start = std::time::Instant::now();

        loop {
            if self.completed() >= value {
                return Ok(true);
            }

            self.poll_all(context, device)?;

            if self.completed() >= value {
                return Ok(true);
            }

            if self.pending() < value {
                log::warn!(
                    "waiting for D3D11 fence value {value}, which was never submitted (pending \
                     value is {})",
                    self.pending()
                );
                return Ok(false);
            }

            if let Some(limit) = timeout {
                if start.elapsed() >= limit {
                    return Ok(false);
                }
            }

            std::thread::sleep(super::queue::GPU_WAIT_POLL_INTERVAL);
        }
    }

    /// Polls armed slots once and returns the highest completed value.
    pub(crate) fn get_completed_value(
        &self,
        context: &Direct3D11::ID3D11DeviceContext,
        device: &Direct3D11::ID3D11Device,
    ) -> Result<u64, crate::DeviceError> {
        self.poll_all(context, device)?;
        Ok(self.completed())
    }
}

/// Polls armed slots and frees the ones whose marker the GPU reached.
fn poll_slots(
    completed_value: &crate::AtomicFenceValue,
    slots: &mut FenceSlots,
    context: &Direct3D11::ID3D11DeviceContext,
    device: &Direct3D11::ID3D11Device,
) -> Result<(), crate::DeviceError> {
    for slot in &mut slots.slots {
        let Some(value) = slot.value else {
            continue;
        };

        let complete = {
            let query = &slot.query;
            super::queue::query_complete(context, device, query)?
        };

        if complete {
            // Observing this marker proves every earlier command (and every
            // earlier marker of this fence) completed.
            completed_value.fetch_max(value, Ordering::AcqRel);
            slot.value = None;
        }
    }

    Ok(())
}

impl fmt::Debug for Fence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let slots = self.slots.lock();
        let armed = slots
            .slots
            .iter()
            .filter(|slot| slot.value.is_some())
            .count();
        f.debug_struct("Fence")
            .field("pending", &self.pending())
            .field("completed", &self.completed())
            .field("slots", &slots.slots.len())
            .field("armed", &armed)
            .finish_non_exhaustive()
    }
}

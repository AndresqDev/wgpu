//! D3D11 debug-layer (`ID3D11InfoQueue`) plumbing.
//!
//! A device created with `D3D11_CREATE_DEVICE_DEBUG` exposes
//! [`ID3D11InfoQueue`](windows::Win32::Graphics::Direct3D11::ID3D11InfoQueue),
//! which stores the messages emitted by the D3D11 runtime and the SDK debug
//! layer. The queue is bounded when this module attaches to it, and every entry
//! point that can generate messages (`submit`, `present` and the idle wait)
//! drains the queue into the `log` crate. [`super::Device::take_debug_messages`]
//! exposes the same drain to callers that want the structured messages.
//!
//! The debug layer is optional: it requires `d3d11sdklayers.dll`, which is
//! installed with the Windows SDK's Graphics Tools. When it is missing,
//! `D3D11CreateDevice` fails with the debug flag and the adapter falls back to
//! a normal device; [`DebugLayer::new`] then returns `None` because the device
//! does not expose `ID3D11InfoQueue`.

use alloc::{string::String, vec, vec::Vec};
use core::fmt;

use windows::{core::Interface as _, Win32::Graphics::Direct3D11};

/// Severity of a D3D11 debug-layer message.
///
/// These mirror the `D3D11_MESSAGE_SEVERITY` values, from most to least severe.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DebugSeverity {
    /// The device or its driver state is corrupted.
    Corruption,
    /// An API call failed or was rejected.
    Error,
    /// An API call succeeded but used a suspicious argument or state.
    Warning,
    /// Informational message emitted by the runtime or driver.
    Info,
    /// Free-form message, including application-defined messages.
    Message,
}

impl DebugSeverity {
    /// Returns whether this severity represents a failed or corrupting call.
    pub fn is_error(&self) -> bool {
        matches!(self, Self::Corruption | Self::Error)
    }

    /// Returns a short lowercase label for this severity.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Corruption => "corruption",
            Self::Error => "error",
            Self::Warning => "warning",
            Self::Info => "info",
            Self::Message => "message",
        }
    }

    /// Maps a raw `D3D11_MESSAGE_SEVERITY` value to this enum.
    fn from_raw(value: i32) -> Option<Self> {
        match value {
            0 => Some(Self::Corruption),
            1 => Some(Self::Error),
            2 => Some(Self::Warning),
            3 => Some(Self::Info),
            4 => Some(Self::Message),
            _ => None,
        }
    }
}

impl fmt::Display for DebugSeverity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One message stored by the D3D11 debug layer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DebugMessage {
    /// Severity reported by the debug layer.
    pub severity: DebugSeverity,
    /// Numeric `D3D11_MESSAGE_ID` of the message.
    pub id: i32,
    /// Human-readable description, with the trailing NUL removed.
    pub description: String,
}

impl DebugMessage {
    /// Logs this message at the level implied by its severity.
    pub(crate) fn log(&self) {
        match self.severity {
            DebugSeverity::Corruption | DebugSeverity::Error => {
                log::error!(target: "wgpu_hal::dx11::debug", "{}", self);
            }
            DebugSeverity::Warning => {
                log::warn!(target: "wgpu_hal::dx11::debug", "{}", self);
            }
            DebugSeverity::Info => {
                log::info!(target: "wgpu_hal::dx11::debug", "{}", self);
            }
            DebugSeverity::Message => {
                log::debug!(target: "wgpu_hal::dx11::debug", "{}", self);
            }
        }
    }
}

impl fmt::Display for DebugMessage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} [{}]: {}", self.severity, self.id, self.description)
    }
}

/// A handle on the D3D11 debug layer's message queue.
///
/// Cloning this is cheap: it only clones the `ID3D11InfoQueue` COM reference.
/// Every clone drains the same underlying queue.
#[derive(Clone, Debug)]
pub(crate) struct DebugLayer {
    queue: Direct3D11::ID3D11InfoQueue,
}

impl DebugLayer {
    /// Attaches to the debug layer of `device`, if it has one.
    ///
    /// Devices created without `D3D11_CREATE_DEVICE_DEBUG` do not expose
    /// `ID3D11InfoQueue`, so this returns `None` for them. The message limit is
    /// bounded so a long-running device cannot accumulate messages without
    /// limit.
    pub(crate) fn new(device: &Direct3D11::ID3D11Device) -> Option<Self> {
        // `Interface::cast` performs `QueryInterface` internally and does not
        // require an unsafe block.
        let queue = device.cast::<Direct3D11::ID3D11InfoQueue>().ok()?;

        // SAFETY: the interface is valid and `SetMessageCountLimit` has no
        // pointer arguments.
        if let Err(error) = unsafe { queue.SetMessageCountLimit(1024) } {
            log::warn!(
                target: "wgpu_hal::dx11::debug",
                "failed to set the D3D11 debug-layer message count limit: {error}"
            );
        }

        Some(Self { queue })
    }

    /// Drains every stored message and clears the queue.
    ///
    /// Malformed entries reported by the debug layer are skipped rather than
    /// trusted: the description pointer and length come from the runtime and
    /// are validated against the buffer that `GetMessage` filled.
    pub(crate) fn take_messages(&self) -> Vec<DebugMessage> {
        // SAFETY: `GetNumStoredMessages` takes no arguments and the interface
        // is valid.
        let count = unsafe { self.queue.GetNumStoredMessages() };
        let mut messages = Vec::new();

        for index in 0..count {
            let mut message_len = 0usize;
            // First call: query the byte length required for this message. A
            // null message pointer is the documented size query.
            // SAFETY: passing `None` only writes the length out-parameter.
            if unsafe { self.queue.GetMessage(index, None, &mut message_len) }.is_err()
                || message_len == 0
            {
                continue;
            }

            // `D3D11_MESSAGE` contains pointer-sized fields, so the buffer must
            // be aligned for it. Allocating `u64`s keeps the buffer aligned on
            // every Windows target while still providing `message_len` bytes.
            let word_len = message_len.div_ceil(core::mem::size_of::<u64>());
            let mut storage = vec![0u64; word_len];
            let mut written = message_len;

            // Second call: fill the buffer with the message header followed by
            // the description bytes.
            // SAFETY: `storage` is at least `message_len` bytes long and
            // correctly aligned for `D3D11_MESSAGE`.
            if unsafe {
                self.queue.GetMessage(
                    index,
                    Some(storage.as_mut_ptr().cast::<Direct3D11::D3D11_MESSAGE>()),
                    &mut written,
                )
            }
            .is_err()
                || written < core::mem::size_of::<Direct3D11::D3D11_MESSAGE>()
            {
                continue;
            }

            // SAFETY: `written` bytes of `storage` were initialised by
            // `GetMessage` and the buffer is aligned for `D3D11_MESSAGE`.
            let message = unsafe { &*storage.as_ptr().cast::<Direct3D11::D3D11_MESSAGE>() };

            let Some(severity) = DebugSeverity::from_raw(message.Severity.0) else {
                continue;
            };

            if message.pDescription.is_null() || message.DescriptionByteLength == 0 {
                continue;
            }

            // Validate that the description lies inside the buffer returned by
            // `GetMessage` before constructing a slice from it.
            let buffer_start = storage.as_ptr() as usize;
            let description_start = message.pDescription as usize;
            let Some(description_offset) = description_start.checked_sub(buffer_start) else {
                continue;
            };
            if description_offset > written
                || message.DescriptionByteLength > written - description_offset
            {
                continue;
            }

            // SAFETY: the bounds check above proves that
            // `DescriptionByteLength` bytes starting at `pDescription` are
            // inside the initialised `storage` buffer.
            let description_bytes = unsafe {
                core::slice::from_raw_parts(message.pDescription, message.DescriptionByteLength)
            };
            let description = String::from_utf8_lossy(description_bytes);
            let description = description.trim_end_matches('\0');
            if description.is_empty() {
                continue;
            }

            messages.push(DebugMessage {
                severity,
                id: message.ID.0,
                description: String::from(description),
            });
        }

        // SAFETY: the queue is valid and `ClearStoredMessages` takes no
        // arguments. Clearing once after the loop avoids invalidating the
        // indices that `GetMessage` is still reading.
        unsafe { self.queue.ClearStoredMessages() };

        messages
    }

    /// Drains the queue and logs every message at its severity.
    pub(crate) fn report(&self) {
        for message in self.take_messages() {
            message.log();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_raw_d3d11_severities() {
        assert_eq!(
            DebugSeverity::from_raw(Direct3D11::D3D11_MESSAGE_SEVERITY_CORRUPTION.0),
            Some(DebugSeverity::Corruption)
        );
        assert_eq!(
            DebugSeverity::from_raw(Direct3D11::D3D11_MESSAGE_SEVERITY_ERROR.0),
            Some(DebugSeverity::Error)
        );
        assert_eq!(
            DebugSeverity::from_raw(Direct3D11::D3D11_MESSAGE_SEVERITY_WARNING.0),
            Some(DebugSeverity::Warning)
        );
        assert_eq!(
            DebugSeverity::from_raw(Direct3D11::D3D11_MESSAGE_SEVERITY_INFO.0),
            Some(DebugSeverity::Info)
        );
        assert_eq!(
            DebugSeverity::from_raw(Direct3D11::D3D11_MESSAGE_SEVERITY_MESSAGE.0),
            Some(DebugSeverity::Message)
        );
        assert_eq!(DebugSeverity::from_raw(5), None);

        assert!(DebugSeverity::Corruption.is_error());
        assert!(DebugSeverity::Error.is_error());
        assert!(!DebugSeverity::Warning.is_error());
        assert!(!DebugSeverity::Info.is_error());
        assert!(!DebugSeverity::Message.is_error());
    }
}

use core::sync::atomic::Ordering;

/// Placeholder for every GPU resource type that is not implemented yet.
///
/// The skeleton deliberately reuses one zero-sized type for buffers, textures,
/// views, samplers, bind groups, layouts, pipelines and query sets. Once the
/// corresponding D3D11 objects are implemented, each associated type in
/// [`super::Api`] should be replaced by its own concrete wrapper.
#[derive(Debug)]
pub struct Resource;

/// A CPU-visible fence value.
///
/// The real implementation will pair this with an `ID3D11Query`
/// (`D3D11_QUERY_EVENT`) so that [`crate::Device::wait`] can block on GPU
/// completion. For now the value is only updated by [`crate::Queue::submit`],
/// which is CPU-synchronous in this skeleton.
#[derive(Debug, Default)]
pub struct Fence {
    value: crate::AtomicFenceValue,
}

impl Fence {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn signal(&self, value: u64) {
        self.value.fetch_max(value, Ordering::Release);
    }

    pub(crate) fn value(&self) -> u64 {
        self.value.load(Ordering::Acquire)
    }
}

/// Dummy pipeline cache.
///
/// D3D11 has no API-level pipeline cache object; compiled shader blobs will
/// eventually be cached by `wgpu-core` or on disk.
#[derive(Debug, Default)]
pub struct PipelineCache;

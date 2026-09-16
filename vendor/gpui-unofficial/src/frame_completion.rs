//! Optional GPU completion observation for a dedicated benchmark window.
use std::time::Instant;

/// Work actually performed by the submitted renderer, separate from UI intent.
#[derive(Clone, Copy, Debug, Default)]
pub struct FrameRenderStats {
    /// Retained layers composited in this submission, including nested layers.
    pub retained_layers: u32,
    /// Cached content textures redrawn after content or resource invalidation.
    pub content_redraws: u32,
    /// Dynamic contour mask textures redrawn in this submission.
    pub mask_redraws: u32,
    /// The renderer expanded retained content to ordinary primitives.
    pub retained_fallback: bool,
}

/// A submitted frame's observed GPU completion, on the host monotonic clock.
pub struct FrameCompletion {
    /// Time when the completed command buffer callback ran.
    pub completed_at: Instant,
    /// Whether the renderer completed without a command-buffer error.
    pub succeeded: bool,
    /// Time spent acquiring the native drawable, including any presentation backpressure.
    pub drawable_wait: std::time::Duration,
    /// Renderer preparation and Metal encoding before command-buffer submission.
    pub encoding: std::time::Duration,
    /// Actual renderer path and cache work for this submitted frame.
    pub render: FrameRenderStats,
}

type Callback = Box<dyn FnOnce(FrameCompletion) + Send>;

/// The destination and completion observer of one benchmark submission.
pub struct FrameCompletionRequest {
    /// Reuse a private render target so display-buffer availability does not pace rendering.
    pub offscreen: bool,
    /// Called once the GPU command buffer has completed.
    pub callback: Box<dyn FnOnce(FrameCompletion) + Send>,
}

#[cfg(feature = "bench")]
thread_local! {
    static PENDING: std::cell::RefCell<Option<FrameCompletionRequest>> = Default::default();
}

/// Consume the observer for this synchronous submission, if a benchmark has
/// armed one. Ordinary production rendering has no completion observer.
pub fn take_frame_completion() -> Option<FrameCompletionRequest> {
    #[cfg(feature = "bench")]
    return PENDING.with_borrow_mut(Option::take);
    #[cfg(not(feature = "bench"))]
    None
}

#[cfg(feature = "bench")]
pub(crate) fn observe(callback: Callback, offscreen: bool, draw: impl FnOnce()) -> bool {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            PENDING.with_borrow_mut(|slot| {
                slot.take();
            });
        }
    }
    PENDING.with_borrow_mut(|slot| {
        assert!(slot.is_none(), "nested frame completion observer");
        *slot = Some(FrameCompletionRequest {
            callback,
            offscreen,
        });
    });
    let _reset = Reset;
    draw();
    PENDING.with_borrow(|slot| slot.is_none())
}

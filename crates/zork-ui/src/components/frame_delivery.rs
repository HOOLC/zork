//! Coalesce invalidations at the owner's next render. The owner decides which
//! read-only presentation sources to drain; hidden views catch up when shown.
use gpui::{AsyncApp, WeakEntity};

#[derive(Default)]
pub struct FrameDelivery {
    pending: bool,
}

impl FrameDelivery {
    /// Render entry is already a frame boundary. Do not defer a pending batch
    /// behind a platform callback that may not run on this rendering path.
    pub fn enter(&mut self) -> bool {
        std::mem::take(&mut self.pending)
    }

    pub fn request<T: 'static>(
        owner: &WeakEntity<T>,
        cx: &mut AsyncApp,
        access: fn(&mut T) -> &mut Self,
    ) -> bool {
        owner
            .update(cx, |view, cx| {
                if !std::mem::replace(&mut access(view).pending, true) {
                    cx.notify();
                }
            })
            .is_ok()
    }
}

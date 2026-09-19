//! Core subscriptions must retain their queued frame delivery across native reentry.
use gpui::{Context, Render, RequestFrameOptions, TestAppContext, Window};
use std::{cell::Cell, rc::Rc};

struct EmptyView;
impl Render for EmptyView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl gpui::IntoElement {
        gpui::Empty
    }
}

#[gpui::test]
fn next_frame_callbacks_survive_a_busy_app(cx: &mut TestAppContext) {
    let window = cx.add_window(|_, _| EmptyView);
    let test_window = cx.test_window(window.into());
    let delivered = Rc::new(Cell::new(0));
    window
        .update(cx, {
            let delivered = delivered.clone();
            move |_, window, _| {
                window.on_next_frame(move |_, _| delivered.set(delivered.get() + 1));
            }
        })
        .unwrap();

    // Native event pumps can request a frame during an ordinary app update,
    // even when no draw is in progress. Retry after the app becomes available.
    window
        .update(cx, |_, _, _| {
            test_window.simulate_frame_request(RequestFrameOptions {
                require_presentation: true,
                ..Default::default()
            });
        })
        .unwrap();
    assert_eq!(delivered.get(), 0);
    test_window.simulate_frame_request(RequestFrameOptions {
        require_presentation: true,
        ..Default::default()
    });
    assert_eq!(
        delivered.get(),
        1,
        "a busy app must not drop frame delivery"
    );
    test_window.simulate_frame_request(RequestFrameOptions {
        require_presentation: true,
        ..Default::default()
    });
    assert_eq!(delivered.get(), 1, "the retained callback runs only once");
}

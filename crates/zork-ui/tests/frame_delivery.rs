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

struct SubscriberView {
    delivery: zork_ui::components::frame_delivery::FrameDelivery,
    source: Rc<Cell<usize>>,
    shown: Rc<Cell<usize>>,
}
impl Render for SubscriberView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl gpui::IntoElement {
        if self.delivery.enter() {
            self.shown.set(self.source.get());
        }
        gpui::Empty
    }
}

#[gpui::test]
fn subscription_updates_reach_a_render_without_a_platform_frame_callback(cx: &mut TestAppContext) {
    use gpui::AppContext;
    use zork_ui::components::frame_delivery::FrameDelivery;
    let source = Rc::new(Cell::new(0));
    let shown = Rc::new(Cell::new(0));
    let window = cx.add_window(|_, _| SubscriberView {
        delivery: Default::default(),
        source: source.clone(),
        shown: shown.clone(),
    });
    let view = window.entity(cx).unwrap().downgrade();
    for value in [42, 99] {
        source.set(value);
        assert!(FrameDelivery::request(&view, &mut cx.to_async(), |view| {
            &mut view.delivery
        }));
        // A normal retained-view render must consume the batch even when no
        // native on_request_frame callback is supplied by the test platform.
        cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear(cx))
            .unwrap();
        assert_eq!(shown.get(), value);
        source.set(value + 1);
        cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear(cx))
            .unwrap();
        assert_eq!(
            shown.get(),
            value,
            "an idle render must not poll the source"
        );
    }
}

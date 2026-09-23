use super::*;

pub struct Story {
    available_width: f32,
    messages: Vec<MessageDocument>,
    text: crate::resources::Text,
    placeholder: Option<String>,
    composer: Option<Entity<crate::component_story::ComposerExample>>,
}
impl Story {
    pub fn new(state: &str, text: crate::resources::Text, cx: &mut Context<Self>) -> Self {
        let fixture = crate::stories::page_fixture();
        let messages = fixture["conversation"]["messages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|message| {
                let body = if state.starts_with("long") {
                    message["content"].as_str().unwrap().repeat(50)
                } else {
                    message["content"].as_str().unwrap().to_owned()
                };
                let document = if message["role"] == "user" {
                    MessageDocument::plain(&body)
                } else {
                    MessageDocument::parse(&body)
                };
                document
            })
            .collect::<Vec<_>>();
        Self {
            messages,
            available_width: 240.,
            text,
            placeholder: ["loading", "empty", "offline", "error"]
                .into_iter()
                .find(|name| state.starts_with(name))
                .map(str::to_owned),
            composer: state
                .starts_with("composer")
                .then(|| cx.new(crate::component_story::ComposerExample::new)),
        }
    }
}
impl Render for Story {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(state) = &self.placeholder {
            return crate::components::message_placeholder::render(
                crate::components::message_placeholder::Data {
                    loading: state == "loading",
                    message: self.text.text(match state.as_str() {
                        "loading" => "loading_messages",
                        "offline" => "device_no_cached_messages",
                        "error" => "messages_load_failed",
                        _ => "waiting_first_update",
                    }),
                    older: (state == "error").then(|| (self.text.text("load_earlier"), true)),
                },
                cx,
                |v, cx| {
                    v.placeholder = None;
                    cx.notify();
                },
            );
        }
        let width = self.available_width.max(1.);
        let rows = self
            .messages
            .iter()
            .enumerate()
            .map(|(index, document)| {
                Row {
                    index,
                    user: index == 0,
                    document,
                    content_width: width,
                    selection: None,
                    author_name: (index != 0).then(|| "产品领队".into()),
                    device: Some("演示设备".into()),
                    model: (index != 0).then(|| "gpt-6".into()),
                    time: Some("10:24".into()),
                }
                .render(window)
            })
            .collect::<Vec<_>>();
        let owner = cx.entity().downgrade();
        div()
            .relative()
            .min_w_0()
            .child(
                gpui::canvas(
                    move |bounds, _, cx| {
                        let _ = owner.update(cx, |v, cx| {
                            let next = bounds.size.width.as_f32();
                            if (v.available_width - next).abs() > 0.1 {
                                v.available_width = next;
                                cx.notify();
                            }
                        });
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .id("conversation-messages")
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .id("conversation-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .children(rows),
            )
            .when_some(self.composer.clone(), |v, composer| {
                v.child(div().h(px(400.)).child(composer))
            })
            .into_any_element()
    }
}

use super::*;

impl RootView {
    pub(crate) fn set_first_chat_welcome(&mut self, welcome: bool, cx: &mut Context<Self>) {
        if self.first_chat_welcome != welcome {
            self.first_chat_welcome = welcome;
            cx.notify();
        }
    }

    pub(crate) fn set_new_chat_devices(
        &mut self,
        choice: zork_client_core::new_chat::Choice,
        cx: &mut Context<Self>,
    ) {
        if self.new_chat_devices != choice {
            self.new_chat_devices = choice;
            zork_ui::components::region::invalidate_all(cx);
            cx.notify();
        }
    }

    pub(super) fn watch_new_chat(&mut self, cx: &mut Context<Self>) {
        let mut updates = self.core_device.new_chat().subscribe_view();
        updates.snapshot();
        self.new_chat_updates = Some(cx.spawn(async move |view, cx| {
            while updates.ready().await.is_ok() {
                let Some(batch) = updates.prepare() else {
                    continue;
                };
                if !updates.valid(batch.id) {
                    updates.discard(batch.id);
                    continue;
                }
                let applied = view
                    .update(cx, |view, cx| {
                        if view.selected_session.is_none() {
                            if let Some(chat) = &batch.snapshot.value.created {
                                let current = view.core_device.new_chat().presentation();
                                if current
                                    .created
                                    .as_ref()
                                    .is_some_and(|created| created.chat_id == chat.chat_id)
                                {
                                    // Apply the core catalog batch before opening its Chat.
                                    view.deliver_core_updates(cx);
                                    view.select_session(&chat.chat_id, cx);
                                }
                            }
                        }
                        zork_ui::components::region::invalidate_all(cx);
                    })
                    .is_ok();
                if applied {
                    updates.acknowledge(batch.id);
                } else {
                    updates.discard(batch.id);
                    break;
                }
            }
        }));
    }

    pub(super) fn render_new_chat(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<zork_ui::new_chat::Page> {
        let locale = self.locale;
        let text = zork_ui::resources::Text(Rc::new(move |key| locale.text(key).into()));
        let page = if let Some(page) = &self.new_chat_page {
            page.clone()
        } else {
            let page = cx.new(|cx| zork_ui::new_chat::Page::new(text.clone(), cx));
            cx.subscribe(
                &page,
                |view, _, event: &zork_ui::new_chat::Event, cx| match event {
                    zork_ui::new_chat::Event::Intent(action) => {
                        view.error = view
                            .core_device
                            .new_chat()
                            .apply(action.clone())
                            .err()
                            .map(|error| error.to_string());
                        zork_ui::components::region::invalidate_all(cx);
                    }
                    zork_ui::new_chat::Event::ConfigureModels => cx.emit(DesktopAction::ManageNode),
                    zork_ui::new_chat::Event::SelectDevice(id) => {
                        cx.emit(DesktopAction::SelectChatDevice(id.clone()))
                    }
                },
            )
            .detach();
            self.new_chat_page = Some(page.clone());
            page
        };
        let mut data = self.core_device.new_chat().presentation();
        data.device = self.new_chat_devices.clone();
        data.error = data.error.or_else(|| self.error.clone());
        page.update(cx, |v, cx| {
            v.set_welcome(self.first_chat_welcome, cx);
            v.configure(data, self.composer_surface_width, text, cx)
        });
        page
    }
}

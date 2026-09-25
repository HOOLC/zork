use super::*;
fn sample(start: usize, name: &str, model: &str, state: &str) -> Device {
    let chats = (start..start + 3)
        .map(|i| NavigationChat {
            chat_id: format!("chat-{i}"),
            title: [
                "评估 Qwen 方案",
                "评审组件库",
                "检查并清理冗余内容",
                "重构 Chat 创建与 Session",
                "查看推箱子训练",
                "绘制骑自行车的鹈鹕",
            ][i]
                .into(),
            description: "讨论界面与交互细节".into(),
            workspace: "产品项目".into(),
            model: model.into(),
            updated_at: if start == 0 {
                iso_days_ago(0).replace("T12:", &format!("T1{}:", 8 - (i - start)))
            } else {
                iso_days_ago(2).replace("T12:", &format!("T1{}:", 8 - (i - start)))
            },
            in_preview: true,
            unread: state == "unread" && i == start,
            ..Default::default()
        })
        .collect();
    Device {
        id: format!("device-{start}"),
        name: name.into(),
        machine: None,
        online: Some(state != "offline"),
        status: if state == "offline" {
            crate::device_name::DeviceStatus::Offline
        } else {
            crate::device_name::DeviceStatus::Direct
        },
        direct: true,
        public: false,
        chats: Arc::new(chats),
        selected_session: (start == 0).then(|| "chat-0".into()),
        chatting: start == 0,
    }
}
pub fn create(state: &str, text: Text, cx: &mut gpui::App) -> gpui::Entity<Navigation> {
    let devices = vec![
        sample(0, "本机", "deepseek-flash", state),
        sample(3, "Studio", "gpt-5.4", state),
    ];
    let view = cx.new(|cx| {
        let mut view = Navigation::new(text, cx);
        view.set_data(
            if state == "empty" { vec![] } else { devices },
            Some("device-0".into()),
            280.,
            cx,
        );
        view
    });
    cx.subscribe(&view, |view, event: &Action, cx| {
        if let Action::Archive {
            node,
            chat,
            archived,
            ..
        } = event
        {
            // Supply a confirmed mock response; archive policy belongs to core/Station.
            view.update(cx, |view, cx| {
                let mut devices = view.devices.clone();
                if let Some(device) = devices.iter_mut().find(|device| &device.id == node) {
                    if let Some(item) = Arc::make_mut(&mut device.chats)
                        .iter_mut()
                        .find(|item| &item.chat_id == chat)
                    {
                        item.archived = *archived;
                    }
                }
                view.set_data(devices, view.active.clone(), view.width, cx);
            });
        }
        if let Action::Navigate { node, destination } = event {
            view.update(cx, |view, cx| {
                let mut devices = view.devices.clone();
                for device in &mut devices {
                    let chosen = Some(&device.id) == node.as_ref();
                    match destination {
                        Destination::NewChat if chosen => {
                            device.selected_session = None;
                            device.chatting = false;
                        }
                        Destination::Conversation { ref session } if chosen => {
                            device.selected_session = Some(session.clone());
                            device.chatting = true;
                        }
                        Destination::Conversation { .. } | Destination::NewChat => {
                            device.chatting = false;
                        }
                        _ => {}
                    }
                }
                view.set_data(devices, node.clone(), view.width, cx);
            });
        }
    })
    .detach();
    view
}

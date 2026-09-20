use super::*;
pub fn create(state: &str, text: Text, cx: &mut gpui::App) -> gpui::Entity<Navigation> {
    let chats = (0..8)
        .map(|i| NavigationChat {
            chat_id: format!("chat-{i}"),
            title: format!("产品讨论 {}", i + 1),
            description: "讨论界面与交互细节".into(),
            workspace: "产品项目".into(),
            in_preview: i < 3,
            unread: state == "unread" && i == 0,
            ..Default::default()
        })
        .collect();
    let device = Device {
        id: "demo-device".into(),
        name: "工作设备".into(),
        online: Some(state != "offline"),
        direct: true,
        public: false,
        chats: Arc::new(chats),
        selected_session: Some("chat-0".into()),
        chatting: true,
    };
    let mut collapsed = HashSet::new();
    if state == "collapsed" {
        collapsed.insert("demo-device".into());
    }
    let view = cx.new(|cx| {
        let mut view = Navigation::new(collapsed, text, cx);
        view.set_data(
            if state == "empty" {
                vec![]
            } else {
                vec![device]
            },
            Some("demo-device".into()),
            false,
            280.,
            cx,
        );
        view
    });
    cx.subscribe(&view, |view, event: &Action, cx| {
        if let Action::Navigate { node, destination } = event {
            view.update(cx, |view, cx| {
                let mut devices = view.devices.clone();
                let shared = matches!(destination, Destination::SharedFiles);
                if let Some(device) = devices
                    .iter_mut()
                    .find(|device| Some(&device.id) == node.as_ref())
                {
                    match destination {
                        Destination::NewChat => device.selected_session = None,
                        Destination::Conversation { session, .. } => {
                            device.selected_session = Some(session.clone())
                        }
                        _ => {}
                    }
                }
                view.set_data(devices, node.clone(), shared, view.width, cx);
            });
        }
    })
    .detach();
    view
}

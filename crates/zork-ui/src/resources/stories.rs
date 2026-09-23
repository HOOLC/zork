//! Fixed resource inputs for the production list and inspector.
use super::*;
use zork_client_types::resources::*;

pub fn create(state: &str, text: Text, cx: &mut gpui::App) -> gpui::Entity<ResourcesView> {
    let mut data = ResourcesData::default();
    let items = vec![Resource::new(
        ResourceKind::Service,
        "preview".into(),
        "报告预览".into(),
        "running".into(),
        "shared".into(),
    )];
    if state != "empty" {
        data.devices.push(ResourceDevice {
            status: crate::device_name::DeviceStatus::Direct,
            id: "demo-device".into(),
            name: "工作设备".into(),
            catalog: (!matches!(state, "loading" | "error")).then_some(ResourceCatalog {
                items: items.clone(),
                ..Default::default()
            }),
            loading: state == "loading",
            error: (state == "error").then(|| "设备暂时不可用".into()),
        });
    }
    let details = ResourceDetails {
        title: "报告预览".into(),
        description: "查看当前设备的报告预览服务。".into(),
        document: Some(ResourceDocument {
            path: "README.md".into(),
            text: "# 报告预览\n\n服务运行正常。".into(),
            truncated: false,
        }),
        facts: vec![("status".into(), "ready".into())],
        ..Default::default()
    };
    let query = Inspection::Service {
        id: "preview".into(),
        log: None,
    };
    data.inspections.insert(
        ("demo-device".into(), query.clone()),
        InspectionState {
            content: Some(Arc::new(InspectionContent::Details(details.clone()))),
            ..Default::default()
        },
    );
    let mode = Mode::Services("demo-device".into());
    let view = cx.new(|cx| {
        let mut view = ResourcesView::new(Arc::new(data), text, mode, cx);
        if state == "detail" {
            view.select("demo-device".into(), query, cx);
        }
        view
    });
    cx.subscribe(&view, move |view, event: &Refresh, cx| {
        view.update(cx, |view, cx| {
            let mut next = (*view.data).clone();
            if next.devices.is_empty() {
                next.devices.push(ResourceDevice {
                    id: "demo-device".into(),
                    name: "工作设备".into(),
                    ..Default::default()
                });
            }
            for device in &mut next.devices {
                device.loading = false;
                device.error = None;
                device.catalog = Some(ResourceCatalog {
                    items: items.clone(),
                    ..Default::default()
                });
            }
            if let Some(query) = &event.query {
                next.inspections
                    .entry(query.clone())
                    .or_insert_with(|| InspectionState {
                        content: Some(Arc::new(InspectionContent::Details(details.clone()))),
                        ..Default::default()
                    });
            }
            view.set_data(Arc::new(next), cx);
        });
    })
    .detach();
    view
}

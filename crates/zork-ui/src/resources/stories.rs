//! Fixed resource inputs for the production list and inspector.
use super::*;
use zork_client_types::resources::*;

pub fn create(state: &str, text: Text, cx: &mut gpui::App) -> gpui::Entity<ResourcesView> {
    let mut data = ResourcesData::default();
    let items = vec![
        Resource::new(
            ResourceKind::Mcp,
            "knowledge".into(),
            "团队知识库".into(),
            "ready".into(),
            "selected".into(),
        ),
        Resource::new(
            ResourceKind::Service,
            "preview".into(),
            "报告预览".into(),
            "running".into(),
            "shared".into(),
        ),
    ];
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
        title: "团队知识库".into(),
        description: "按关键词查找团队文档。".into(),
        tools: vec![ResourceTool {
            name: "search".into(),
            description: "查找文档".into(),
            input_schema: serde_json::json!({"type":"object","properties":{"query":{"type":"string"}}}),
        }],
        document: Some(ResourceDocument {
            path: "README.md".into(),
            text: "# 团队知识库\n\n共享的项目文档与研究资料。".into(),
            truncated: false,
        }),
        facts: vec![("status".into(), "ready".into())],
        ..Default::default()
    };
    for query in [
        Inspection::Mcp("knowledge".into()),
        Inspection::Service {
            id: "preview".into(),
            log: None,
        },
        Inspection::Skill {
            agent: "leader".into(),
            skill: "research".into(),
            file: None,
        },
    ] {
        data.inspections.insert(
            ("demo-device".into(), query),
            InspectionState {
                content: Some(Arc::new(InspectionContent::Details(details.clone()))),
                ..Default::default()
            },
        );
    }
    data.inspections.insert(
        (
            "demo-device".into(),
            Inspection::AgentSkills("leader".into()),
        ),
        InspectionState {
            content: Some(Arc::new(InspectionContent::Skills(AgentSkills {
                skills: vec![SkillEntry {
                    id: "research".into(),
                    name: "研究手册".into(),
                    description: "整理资料并撰写报告".into(),
                    path: "research/SKILL.md".into(),
                    source: "团队技能".into(),
                    content_hash: "demo".into(),
                }],
                ..Default::default()
            }))),
            ..Default::default()
        },
    );
    let mode = match state {
        "services" => Mode::Services("demo-device".into()),
        "skills" => Mode::Skills {
            node: "demo-device".into(),
            agent: "leader".into(),
        },
        _ => Mode::Connections,
    };
    let view = cx.new(|cx| {
        let mut view = ResourcesView::new(Arc::new(data), text, mode, cx);
        if state == "detail" {
            view.select(
                "demo-device".into(),
                Inspection::Mcp("knowledge".into()),
                cx,
            );
        }
        if state == "skills" {
            view.select(
                "demo-device".into(),
                Inspection::AgentSkills("leader".into()),
                cx,
            );
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

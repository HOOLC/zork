//! Retained directory content; navigation owns its animation notifications.
use super::playground::GROUPS;
use super::*;
use crate::components::{liquid::controls as component, workbench as wb};

#[derive(Clone, PartialEq)]
pub(super) struct Props {
    pub width: f32,
    pub parent: u32,
    pub section: Section,
    pub group: usize,
    pub selected_kind: Option<Kind>,
    pub business_family: Option<String>,
    pub families: Vec<(String, String)>,
    pub enabled: bool,
    pub material: Material,
}
pub(super) struct Library {
    pub props: Props,
    pub parent: WeakEntity<Gallery>,
    pub navigation: liquid::navigation::Navigation,
}
impl Render for Library {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let width = self.props.width;
        let parent = self.props.parent;
        use liquid::navigation::{Item, Kind as NavigationKind, Style};
        let mut items = Vec::new();
        let mut destinations = Vec::new();
        let mut selected = 0;
        if self.props.section == Section::Scenarios {
            for (family, title) in self.props.families.clone() {
                if self.props.business_family.as_deref() == Some(&family) {
                    selected = items.len();
                }
                items.push(Item::new(format!("liquid-business-{family}"), title));
                destinations.push((2, None, Some(family)));
            }
        } else {
            for (group, (title, _)) in GROUPS.iter().enumerate().filter(|(group, _)| *group != 4) {
                let count = Kind::ALL
                    .iter()
                    .filter(|kind| kind.group() == group && kind.section() == self.props.section)
                    .count();
                if count == 0 {
                    continue;
                }
                if self.props.selected_kind.is_none() && self.props.group == group {
                    selected = items.len();
                }
                let mut item =
                    Item::new(format!("liquid-tab-{group}"), format!("{title} · {count}"));
                item.heading = true;
                item.gap_before = if items.is_empty() { 0. } else { 12. };
                items.push(item);
                destinations.push((group, None, None));
                for kind in Kind::ALL.into_iter().filter(|kind| kind.group() == group) {
                    if self.props.selected_kind == Some(kind) {
                        selected = items.len();
                    }
                    items.push(Item::new(
                        format!("liquid-inspect-{}", kind.key()),
                        kind.title(),
                    ));
                    destinations.push((group, Some(kind), None));
                }
            }
        }
        if self.props.group == 4 {
            selected = items.len();
        }
        let mut benchmark = Item::new("liquid-tab-4", "并发测试");
        benchmark.gap_before = 16.;
        items.push(benchmark);
        destinations.push((4, None, None));
        let navigation = self.navigation.render(
            "liquid-library-navigation",
            width,
            items,
            selected,
            self.props.enabled,
            Style {
                kind: NavigationKind::Sidebar,
                framed: false,
                parent,
                activate_on_arrow: false,
                row_radius: 14.,
            },
            self.props.material,
            window,
            cx,
            move |v, index, cx| {
                let (group, kind, family) = destinations[index].clone();
                let _ = v.parent.update(cx, |v, cx| {
                    if let Some(family) = family {
                        v.select_business(family, cx);
                    } else if let Some(kind) = kind {
                        v.select_kind(kind, cx);
                    } else {
                        v.set_group(group, cx);
                    }
                });
            },
        );
        wb::column(16.)
            .w(px(width))
            .child(component::segmented(
                "liquid-library-section",
                width,
                Section::ALL
                    .into_iter()
                    .map(|section| {
                        (
                            format!("liquid-section-{}", section.key()),
                            section.title().into(),
                        )
                    })
                    .collect(),
                usize::from(self.props.section == Section::Scenarios),
                self.props.enabled,
                parent,
                window,
                cx,
                |v, index, cx| {
                    let _ = v
                        .parent
                        .update(cx, |v, cx| v.set_section(Section::ALL[index], cx));
                },
            ))
            .child(navigation)
            .into_any_element()
    }
}

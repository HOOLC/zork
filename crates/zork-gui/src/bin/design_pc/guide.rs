//! Editable design source documents rendered by the native design browser.
use zork_ui::components::message::MessageDocument;

pub struct Guide {
    pub id: String,
    pub title: String,
    pub source: &'static str,
    pub document: MessageDocument,
}

pub struct DesignImage {
    pub path: String,
    pub title: String,
    pub category: String,
    pub status: String,
}

pub fn category_label(category: &str) -> &str {
    match category {
        "brand" => "品牌标志",
        "wordmark" => "字标",
        "svg/icons" => "产品图标",
        "gui/glyphs" => "界面图标",
        "gui/provider-icons" => "供应商图标",
        "state-symbol" => "状态图形",
        "interface-extension" => "扩展图标",
        "app-icon" => "App 图标",
        "historical-proposal" => "历史方案",
        "font" => "字体",
        "license" => "许可",
        "provenance" => "来源",
        "tokens" => "设计参数",
        other => other,
    }
}

fn status_label(status: &str) -> &str {
    match status {
        "current-direction" => "当前方向",
        "current-adopted-v2" => "当前采用",
        "normalized-native-resource" => "原生资源",
        "third-party-reference" | "third-party" => "第三方来源",
        "functional-svg-alias" => "功能图形",
        "historical-not-current" | "superseded" => "历史版本",
        "proposal" => "提案",
        "required-attribution" => "需保留署名",
        "reference" => "参考",
        "prototype-source" => "原型输入",
        other => other,
    }
}

pub fn images() -> Vec<DesignImage> {
    let manifest: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../../apps/zork-design-pc/assets/manifest.json"
    ))
    .expect("curated design manifest");
    let mut images: Vec<_> = manifest["items"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let path = item["path"].as_str()?;
            if item["category"] == "svg/avatars" {
                return None;
            }
            if !path.ends_with(".svg") && !path.ends_with(".png") {
                return None;
            }
            Some(DesignImage {
                path: format!("design/{path}"),
                title: path
                    .rsplit('/')
                    .next()?
                    .trim_end_matches(".svg")
                    .trim_end_matches(".png")
                    .into(),
                category: item["category"].as_str().unwrap_or("其他素材").into(),
                status: status_label(item["status"].as_str().unwrap_or_default()).into(),
            })
        })
        .collect();
    images.push(DesignImage {
        path: "design/mobile/zork-mobile-v1.png".into(),
        title: "移动端概念图".into(),
        category: "移动端".into(),
        status: "概念参考".into(),
    });
    for path in super::assets::REFERENCE_PATHS {
        let name = path.rsplit('/').next().unwrap_or(path);
        let group = name.split('-').next().unwrap_or("页面");
        images.push(DesignImage {
            path: format!("design/{path}"),
            title: name.trim_end_matches(".png").into(),
            category: format!("历史参考 · {group}"),
            status: "历史对照".into(),
        });
    }
    images
}

macro_rules! source {
    ($path:literal) => {
        (
            $path,
            include_str!(concat!("../../../../../apps/zork-design-pc/", $path)),
        )
    };
}

pub fn catalog() -> Vec<Guide> {
    let sources: &[(&str, &str)] = &[
        source!("docs/00-overview.md"),
        source!("docs/01-concepts.md"),
        source!("docs/02-principles.md"),
        source!("docs/03-visual-system.md"),
        source!("docs/04-components.md"),
        source!("docs/05-assets.md"),
        source!("docs/06-states-and-copy.md"),
        source!("docs/07-motion.md"),
        source!("docs/08-decisions.md"),
        source!("docs/09-handoff.md"),
        source!("docs/10-mobile.md"),
        source!("docs/12-icons-and-scenes.md"),
        source!("docs/14-controls-revision.md"),
        source!("docs/gui-approved-design.md"),
        source!("mobile/README.md"),
        source!("mobile/concept-v1.md"),
        source!("mobile/design-spec.md"),
        source!("implementation/README.md"),
    ];
    sources
        .iter()
        .map(|(source, text)| Guide {
            id: source.replace('/', "-").replace('.', "-"),
            title: text
                .lines()
                .find_map(|line| line.strip_prefix("# "))
                .unwrap_or(source)
                .into(),
            source,
            document: MessageDocument::parse(text),
        })
        .collect()
}

# 维护与交付

## 来源

当前设计源来自 mini1 的 `artifacts/brand-review-site/src`，以 nav34 后续布局与 brand36 为本次整理基线。

素材逐项来源、状态与 SHA-256 记录在 [manifest](../assets/manifest.json)。本目录是独立整理的设计资料；原工作目录保留，不覆盖现有产品改动。

移动端来源为任务 `01a07575-74bb-7591-aaa9-c2d3818f8392`，以 `mobile/design-spec.md` 与 nav7 HTML 为准；总手册链接该文档，不再维护另一份完整移动规范。

## 文件命名

- `mark*.svg`：品牌主体及尺寸/背景变体。
- `zork-wordmark-draft.svg`：当前待定制字标。
- `avatars/{stable-id}.svg`：保持既有 Agent 头像 ID。
- `icons/product/{meaning}.svg`：领域语义。
- `icons/interface/{action}.svg`：通用操作。
- `providers/{provider}.svg`：外部服务身份。
- `concepts/`：尚未确认使用方式的资源。
- `archive/`：已被替代或历史提案，不从这里直接取当前素材。制作脚本与字标探索图也保留在这里。

## 修改流程

1. 修改文档与对应 SVG/token 源文件。
2. 在决策记录中写明改变了什么、哪些界面受到影响。
3. 重建 HTML 手册、素材页和 PNG 检视图。
4. 检查实际 16/20/24/26 px 尺寸、hover/active/键盘焦点和较窄窗口。
5. 设计确认后再同步原生资源映射；不把预览图片当母稿修改。

## 原生实现映射

| 设计内容 | 原生目录 |
| --- | --- |
| 头像 | `crates/zork-gui/assets/avatars` |
| 产品与界面图标 | `crates/zork-gui/assets/icons` |
| 供应商图标 | `crates/zork-gui/assets/providers` |
| 标志与字标 | `crates/zork-gui/assets/brand` |
| 色彩与密度 | `crates/zork-gui/src/design.rs` |
| 设备与会话导航 | `crates/zork-gui/src/desktop/navigation.rs` |
| 模型连接 | `crates/zork-gui/src/desktop/profiles.rs` |
| 消息与评论 | `components/message.rs`、`components/selection.rs`、`views/comments.rs` |

## 原型说明

`prototype/gui.html` 可独立演示导航、评论队列、设置和连接流程，素材引用本目录的 assets。它使用示例数据，不发送真实授权、消息、安装或升级请求。

规范中的容量约束为严格小于上下文；已将整理副本中的早期校验同步修正。字体、动效和原生控件仍需按实际渲染检查，不能把原型截图当作原生验收结果。

## 第三方资源

Inter 字体附 OFL；7 个供应商品牌 SVG 来自 Lobe Icons 1.95.0，附 MIT 许可及来源。兼容接口符号与产品图形单独记录来源。保留这些记录以便后续更新或替换素材。

## 当前素材接入

`asset-application.json` 记录素材源与实际网页目标。运行 `scripts/apply_web_assets.py` 同步当前字标、功能图标和场景到桌面/移动原型及旧预览入口，再运行 build 与 sync-brand-preview。历史 archive 不覆盖。

原生当前使用的引用以全量清单中的“直接引用”为依据；Central Icons/Phosphor 原文件保留来源，不再把来源目录名当作 Zork 当前图标命名规范。

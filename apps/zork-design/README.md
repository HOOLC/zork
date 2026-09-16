# Zork Design

Zork 的设计工作目录：产品概念、交互规范、视觉规范、素材与可操作样稿。

**整理日期：2026-09-07。** 当前依据为本轮已确认的交互方向，以及 桌面 nav34 / brand36 与移动端 nav7 本地样稿。这里记录设计意图，不代表所有界面已经发布。

[打开 PC 组件库](index.html#/pc/button) · [打开设计手册](index.html) · [打开交互样稿](index.html#/product) · [浏览全部素材](assets/index.html) · [移动端设计](mobile/index.html) · [品牌字标](wordmark/index.html) · [四种动效](motion/index.html) · [原生接入记录](implementation/index.html)

## 从这里开始

| 文档                                          | 内容                                                      |
| --------------------------------------------- | --------------------------------------------------------- |
| [产品概念](docs/01-concepts.md)               | Zork 是什么；Device、Leader、Worker、Task、Profile 的关系 |
| [设计原则](docs/02-principles.md)             | 信息层级、真实状态、归属关系、稳定交互                    |
| [视觉规范](docs/03-visual-system.md)          | 色彩、字体、尺寸、间距、圆角、布局                        |
| [组件与交互](docs/04-components.md)           | 导航、群聊、评论、输入框、设置与模型连接                  |
| [图形与素材](docs/05-assets.md)               | 标志、头像、功能图标、供应商图标、插画的用途              |
| [状态与文案](docs/06-states-and-copy.md)      | 任务、连接、模型、投递和未读的独立语义                    |
| [动效](docs/07-motion.md)                     | 品牌联动、状态动效、减少动态效果                          |
| [决策与待定项](docs/08-decisions.md)          | 已确认、当前方向、待完善、旧方案                          |
| [头像生成规范](docs/11-avatars.md)            | 网格、五官、配色、生成提示词和验收                        |
| [功能图标与插画](docs/12-icons-and-scenes.md) | 圆角规则、原生全量清单和统一场景语言                      |
| [移动端规范](docs/10-mobile.md)               | 触摸、单屏导航、返回关系和桌面差异                        |
| [维护与交付](docs/09-handoff.md)              | 来源、命名、修改流程、原生实现映射                        |

## 目录

- `docs/`：可编辑 Markdown 规范；对应 HTML 为生成文件。
- `assets/`：整理后的 SVG 源文件及原生全量快照、字体与素材清单；按用途分组。
- `tokens/`：结构化设计参数与原型 CSS 色值。
- `mobile/`：移动端 nav7 规范、交互原型、概念图和验证记录。
- `prototype/`：本地交互样稿，使用本目录素材；数据是演示数据。
- `previews/`：从 SVG 生成的检视图。
- `archive/`：仅保留站点使用的原始产品参考与覆盖基线；其余早期提案和旧字标留在本地。
- `licenses/`：第三方素材来源及许可。
- `scripts/`：重建手册、素材页和预览图的工具。

## 当前最需要继续评审的地方

首字母大写 **Zork** 已确认。定制字标仍在探索，尤其是 Z 中的橙色“折角”：v2 将独立色块改为轮廓折口，并提供“折角 / 回转”两种候选，尚未定稿。宣传语、深色主题、App 图标的平台交付也都保持提案状态。

## 开发与构建

本项目是 Zork monorepo 的 `apps/zork-design` workspace 包，包名 `zork-design`。
依赖使用仓库根的 `pnpm-workspace.yaml` 与 `pnpm-lock.yaml`，不维护嵌套 Git 仓库或独立锁文件。
在仓库根运行：

```sh
npx --yes pnpm@10.33.0 install --frozen-lockfile
npx --yes pnpm@10.33.0 design:dev
npx --yes pnpm@10.33.0 design:check
npx --yes pnpm@10.33.0 design:test
npx --yes pnpm@10.33.0 design:build
```

Vite 开发入口为 `http://localhost:49186/design/`，构建输出在本包 `dist/`。
`public/` 由 `scripts/prepare-assets.mjs` 创建相对路径链接；所有源码和资源来自仓库内。
原始产品样稿保存在本包 `archive/reference-prototype/`，无需旧 worktree 或私人 artifacts 目录。
`components/web/` 是可重建的 GPUI/WASM 输出，不提交二进制。首次需要运行 Rust 组件时执行：

```sh
npx --yes pnpm@10.33.0 design:components:web
```

统一 UI 的最新规范直接引用仓库内文档，网页通过 `docs/` 中的仓库内链接展示同一份内容：
[界面规范](docs/gui-approved-design.md)。

`src/app/` 管理顶层导航；`src/pages/` 是设计内容页面；`src/workbench/` 管理组件导航、GPUI、HTML 参考与像素对照；`src/reference/` 是当前完整设计参考；旧 HTML 与 `src/bridge/reference-entry.ts` 保留为历史原型适配。

原生应用与 Web 画布共用 zork 仓库的 `crates/zork-ui/`。`page-fixture.json` 同时驱动两侧的设备、连接、Agent、模型、会话数据。Web 画布在切换页签时复用同一个 WASM 实例。已登记的 58 个页面均有设计参考，18 类公共组件共 64 个状态。会话整页的 GPUI 一侧显示实际原生快照，公共历史、评论和附件另有可交互 Web 示例。详见 [PC 设计覆盖](docs/13-design-coverage.md)。

在 zork 仓库重建 GPUI 与快照：

```sh
python3 scripts/storybook/build.py --base http://127.0.0.1:49186 --dev-reference
```

修改旧素材或文档生成脚本不会覆盖 React 入口；历史静态手册保存在 `archive/`。文档和素材仍可用以下命令重建：

```sh
uv run --with markdown==3.8.2 --with resvg-py==0.5.0 python scripts/build.py
```

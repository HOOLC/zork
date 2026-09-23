# 图形与素材

素材可在原生 `zork-design-pc` 的“设计素材”目录浏览。[机器可读清单](../assets/manifest.json) 与[实际原生 SVG 清单](../assets/native/inventory.json)保留来源和引用。

## 素材分类

| 目录 | 数量 | 用途与状态 |
| --- | --- | --- |
| `assets/brand` | 6 SVG | 主标志及变体 4 个；当前字标与组合标志各 1 个 |
| `assets/avatars` | 12 SVG | 历史头像素材，当前界面不使用 |
| `assets/icons/product` | 18 SVG | 设备、角色、任务、交付与状态图标 |
| `assets/icons/interface` | 34 SVG | 添加、关闭、发送、搜索、提及等通用控件 |
| `assets/providers` | 8 SVG | 7 个外部品牌图标与 1 个原创兼容接口符号 |
| `assets/concepts/app-icon` | 4 SVG | App 图标、深色与分层母稿，提案 |
| `assets/concepts/illustrations` | 8 SVG | 复用功能图标的 SVG 状态，无装饰插图 |
| `assets/fonts` | 1 TTF | Inter Variable |

## 品牌标志

主标志、橙色版、反白版、微型版按背景和尺寸选择。128×128 是标志母版；普通标志从 24 px 开始检视，16 px 使用微型版。最小尺寸和安全区是当前设计起点，仍需在原生 Dock、菜单与窗口中确认。

当前字标文件为 `zork-wordmark-draft.svg`：大写 Z，定制路径，v2 将折角放入 Z 底部轮廓；另有更柔和的回转候选。见 [字标比较](../wordmark/comparison.svg)。旧的 Inter 排版小写字标与组合标志已放入 archive，不用于新界面。

## 动物头像

历史系列包含猫、兔、熊、狐狸、熊猫、小鸡、狗、猫头鹰、考拉、企鹅、鹿、章鱼。素材保留原有 ID、浅色底和简明轮廓，供追溯旧设计。

历史头像仅供回看原设计；当前角色与状态用名称和独立标签/图形表达。
这 12 个原创 SVG 来自早期品牌评审原型 `artifacts/brand-review-site/src/svg/avatars`，原有 ID 保留在历史 Agent 数据中。桌面 Composer 曾使用去掉背景板的透明肖像，当前客户端不再打包这些副本。

## 功能图标

24×24 母版，约 1.7 描边，圆端点与简化隐喻；常见呈现尺寸 16 / 18 / 20 / 24 px。图标颜色由组件语义控制，不把 SVG 固定颜色作为状态系统。

当前功能图标是自绘 SVG。Apple 图标指南提供的是比例、简化和视觉重量参考；本目录没有将 Apple 导出的参考符号当作原创发布素材。

## 供应商图标

供应商品牌保持其真实名称与形状。它表示供应商身份，不表示账号已登录或某个模型已验证。

Lobe Icons 来源记录见 [sources](../licenses/provider-icons-sources.json)，许可见 [MIT 文本](../licenses/Lobe-Icons-MIT.txt)。OpenAI-compatible 符号为原创。Inter 许可见 [OFL 文本](../licenses/Inter-OFL.txt)。

## 使用优先级

SVG 为母稿，预览 PNG 为检视图。后续修改应先改 SVG，再重建预览。界面直接使用小尺寸 SVG；PNG 仅用于验证截图，不作为装饰素材。状态符号为已有功能图标的别名。

桌面 App 实际使用的图标资源位于 `crates/zork-ui/assets/app/`：正式版沿用橙底的 `icon.png`，Dev 为深灰底终端徽记，Design 为浅紫底设计闪光徽记。macOS 的 ICNS 由 `scripts/build/macos-icon.py` 生成；本目录的 `assets/concepts/app-icon` 保留原提案供对照。

历史头像的原始生成规则保存在 [头像设计资料](11-avatars.md)，当前场景与图标规范见 [图标与 SVG 状态](12-icons-and-scenes.md)。

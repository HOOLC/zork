//! Shared controls and business compositions have separate browsing sections.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Section {
    Components,
    Scenarios,
}
impl Section {
    pub const ALL: [Self; 2] = [Self::Components, Self::Scenarios];
    pub fn key(self) -> &'static str {
        match self {
            Self::Components => "components",
            Self::Scenarios => "scenarios",
        }
    }
    pub fn title(self) -> &'static str {
        match self {
            Self::Components => "基础组件",
            Self::Scenarios => "业务组件",
        }
    }
    pub fn first_group(self) -> usize {
        match self {
            Self::Components => 0,
            Self::Scenarios => 2,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    Actions,
    Fields,
    Choices,
    Switch,
    Navigation,
    Rows,
    Popover,
    Details,
    Disclosure,
    Composer,
    Attachments,
    Comments,
    Modal,
    Notice,
    Primitive(super::primitives::Example),
}
impl Kind {
    pub const BENCH: [Self; 14] = [
        Self::Composer,
        Self::Attachments,
        Self::Popover,
        Self::Modal,
        Self::Navigation,
        Self::Disclosure,
        Self::Comments,
        Self::Details,
        Self::Switch,
        Self::Choices,
        Self::Notice,
        Self::Rows,
        Self::Fields,
        Self::Actions,
    ];
    pub const ALL: [Self; 47] = [
        Self::Actions,
        Self::Fields,
        Self::Choices,
        Self::Switch,
        Self::Navigation,
        Self::Rows,
        Self::Popover,
        Self::Details,
        Self::Disclosure,
        Self::Composer,
        Self::Attachments,
        Self::Comments,
        Self::Modal,
        Self::Notice,
        Self::Primitive(super::primitives::Example::Checkbox),
        Self::Primitive(super::primitives::Example::CheckboxGroup),
        Self::Primitive(super::primitives::Example::CheckboxCards),
        Self::Primitive(super::primitives::Example::RadioCards),
        Self::Primitive(super::primitives::Example::Toggle),
        Self::Primitive(super::primitives::Example::ToggleGroup),
        Self::Primitive(super::primitives::Example::Slider),
        Self::Primitive(super::primitives::Example::Progress),
        Self::Primitive(super::primitives::Example::Toast),
        Self::Primitive(super::primitives::Example::Badge),
        Self::Primitive(super::primitives::Example::Skeleton),
        Self::Primitive(super::primitives::Example::Spinner),
        Self::Primitive(super::primitives::Example::Accordion),
        Self::Primitive(super::primitives::Example::Collapsible),
        Self::Primitive(super::primitives::Example::Dialog),
        Self::Primitive(super::primitives::Example::AlertDialog),
        Self::Primitive(super::primitives::Example::ContextMenu),
        Self::Primitive(super::primitives::Example::Menubar),
        Self::Primitive(super::primitives::Example::Popover),
        Self::Primitive(super::primitives::Example::Tooltip),
        Self::Primitive(super::primitives::Example::HoverCard),
        Self::Primitive(super::primitives::Example::Tabs),
        Self::Primitive(super::primitives::Example::Toolbar),
        Self::Primitive(super::primitives::Example::NavigationMenu),
        Self::Primitive(super::primitives::Example::Avatar),
        Self::Primitive(super::primitives::Example::DataList),
        Self::Primitive(super::primitives::Example::Table),
        Self::Primitive(super::primitives::Example::ScrollArea),
        Self::Primitive(super::primitives::Example::Layout),
        Self::Primitive(super::primitives::Example::Otp),
        Self::Primitive(super::primitives::Example::Password),
        Self::Primitive(super::primitives::Example::Form),
        Self::Primitive(super::primitives::Example::TextArea),
    ];
    pub fn key(self) -> &'static str {
        match self {
            Self::Primitive(example) => example.key(),
            Self::Actions => "actions",
            Self::Fields => "fields",
            Self::Choices => "choices",
            Self::Switch => "switch",
            Self::Navigation => "navigation",
            Self::Rows => "rows",
            Self::Popover => "popover",
            Self::Details => "details",
            Self::Disclosure => "disclosure",
            Self::Composer => "composer",
            Self::Attachments => "attachments",
            Self::Comments => "comments",
            Self::Modal => "modal",
            Self::Notice => "notice",
        }
    }
    pub fn title(self) -> &'static str {
        match self {
            Self::Primitive(example) => example.title(),
            Self::Actions => "按钮与图标按钮",
            Self::Fields => "文本输入",
            Self::Choices => "分段选择与单选",
            Self::Switch => "开关",
            Self::Navigation => "导航与标签切换",
            Self::Rows => "列表行与卡片",
            Self::Popover => "下拉菜单与选择器",
            Self::Details => "提示与悬停详情",
            Self::Disclosure => "执行历史与分组",
            Self::Composer => "消息编辑与发送",
            Self::Attachments => "附件与文件预览",
            Self::Comments => "选区评论与队列",
            Self::Modal => "模型连接表单",
            Self::Notice => "状态提示",
        }
    }
    pub fn group(self) -> usize {
        match self {
            Self::Primitive(example) => example.group(),
            Self::Actions | Self::Fields | Self::Choices | Self::Switch => 0,
            Self::Navigation | Self::Rows | Self::Popover | Self::Details | Self::Notice => 1,
            Self::Composer | Self::Attachments | Self::Comments => 2,
            Self::Modal | Self::Disclosure => 3,
        }
    }
    pub fn section(self) -> Section {
        match self.group() {
            2 | 3 => Section::Scenarios,
            _ => Section::Components,
        }
    }
    pub fn description(self) -> &'static str {
        match self {
            Self::Primitive(example) => example.description(),
            Self::Actions => "比较点击、按住、忙碌与禁用时的操作反馈。",
            Self::Fields => "输入文字，检查焦点、错误提示和禁用状态。",
            Self::Choices => "在分段、单选和头像选择中切换同一组选项。",
            Self::Switch => "切换选项，观察开启、关闭和禁用时的反馈。",
            Self::Navigation => "移动指针或切换条目，观察选中标识与悬停反馈。",
            Self::Rows => "比较整行选择与卡片操作的热区和状态。",
            Self::Popover => "展开菜单，试用单选、多选和即时操作。",
            Self::Details => "在锚点之间移动，检查提示的定位与连续过渡。",
            Self::Disclosure => "展开执行步骤或工作区分组，检查组合内容的进出与折叠。",
            Self::Composer => "输入多行文字、添加附件，试用发送和停止状态。",
            Self::Attachments => "展开附件、查看预览，再返回文件列表。",
            Self::Comments => "选择正文添加评论，再编辑待发送的草稿。",
            Self::Modal => "填写模型连接表单，检查焦点、字段错误与提交状态。",
            Self::Notice => "比较提示、加载、成功、警告与错误的语义。",
        }
    }
    pub fn coverage(self) -> &'static str {
        match self {
            Self::Primitive(example) => example.title(),
            Self::Actions => "Button · IconButton · QuietButton · BusyButton · ActionLink",
            Self::Fields => "Field · Password · Error · InputControl",
            Self::Choices => "ChoiceGroup · Segment · Choice · AvatarPicker",
            Self::Switch => "Switch · enabled / disabled",
            Self::Navigation => "TabGroup · hover / active marker · horizontal / vertical",
            Self::Rows => "InteractiveRow · settings row · selectable card",
            Self::Popover => "Dropdown · SelectorMenu · action menu · member multi-select",
            Self::Details => "Hint · DetailsTooltip · SlidingPopup",
            Self::Disclosure => "Collapse · ActivityHeader · history details · sidebar fold",
            Self::Composer => {
                "真实 ComposerInput · 三行增高 · core 发送／停止能力 · 成员活动与附件"
            }
            Self::Attachments => "AttachmentFan · attachment row / card · preview",
            Self::Comments => "selection · comment popover · queued / editing",
            Self::Modal => "Modal · DetailModal · Form · focus / error / busy",
            Self::Notice => "StatusNotice · info / loading / success / warning / error",
        }
    }
}

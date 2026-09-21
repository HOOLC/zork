//! Component-only fixtures; presentation values remain local and never call services.
mod render;
use super::*;
use crate::components::liquid::primitives as p;
use crate::components::text_input::ComposerEdited;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub enum Example {
    Checkbox,
    CheckboxGroup,
    CheckboxCards,
    RadioCards,
    Toggle,
    ToggleGroup,
    Slider,
    Progress,
    Toast,
    Badge,
    Skeleton,
    Spinner,
    Accordion,
    Collapsible,
    Dialog,
    AlertDialog,
    ContextMenu,
    Menubar,
    Popover,
    Tooltip,
    HoverCard,
    Tabs,
    Toolbar,
    NavigationMenu,
    Avatar,
    DataList,
    Table,
    ScrollArea,
    Layout,
    Otp,
    Password,
    Form,
    TextArea,
}
impl Example {
    pub fn key(self) -> &'static str {
        match self {
            Self::Checkbox => "checkbox",
            Self::CheckboxGroup => "checkboxgroup",
            Self::CheckboxCards => "checkboxcards",
            Self::RadioCards => "radiocards",
            Self::Toggle => "togglebutton",
            Self::ToggleGroup => "togglegroup",
            Self::Slider => "slider",
            Self::Progress => "progress",
            Self::Toast => "toast",
            Self::Badge => "badge",
            Self::Skeleton => "skeleton",
            Self::Spinner => "spinner",
            Self::Accordion => "accordion",
            Self::Collapsible => "collapsible",
            Self::Dialog => "dialog",
            Self::AlertDialog => "alertdialog",
            Self::ContextMenu => "contextmenu",
            Self::Menubar => "menubar",
            Self::Popover => "popovercontent",
            Self::Tooltip => "tooltip",
            Self::HoverCard => "hovercard",
            Self::Tabs => "tabs",
            Self::Toolbar => "toolbar",
            Self::NavigationMenu => "navigationmenu",
            Self::Avatar => "avatar",
            Self::DataList => "datalist",
            Self::Table => "table",
            Self::ScrollArea => "scrollarea",
            Self::Layout => "layout",
            Self::Otp => "otp",
            Self::Password => "password",
            Self::Form => "form",
            Self::TextArea => "textarea",
        }
    }
    pub fn title(self) -> &'static str {
        match self {
            Self::Checkbox => "复选框",
            Self::CheckboxGroup => "复选框组",
            Self::CheckboxCards => "多选卡片",
            Self::RadioCards => "单选卡片",
            Self::Toggle => "切换按钮",
            Self::ToggleGroup => "切换按钮组",
            Self::Slider => "滑块",
            Self::Progress => "进度条",
            Self::Toast => "临时通知",
            Self::Badge => "标记",
            Self::Skeleton => "加载占位",
            Self::Spinner => "加载指示器",
            Self::Accordion => "手风琴",
            Self::Collapsible => "折叠区",
            Self::Dialog => "对话框",
            Self::AlertDialog => "确认对话框",
            Self::ContextMenu => "上下文菜单",
            Self::Menubar => "菜单栏",
            Self::Popover => "交互浮层",
            Self::Tooltip => "文字提示",
            Self::HoverCard => "悬停卡片",
            Self::Tabs => "标签面板",
            Self::Toolbar => "工具栏",
            Self::NavigationMenu => "导航菜单",
            Self::Avatar => "头像",
            Self::DataList => "数据列表",
            Self::Table => "表格",
            Self::ScrollArea => "滚动区域",
            Self::Layout => "比例、分隔与边缘延展",
            Self::Otp => "一次性验证码",
            Self::Password => "密码显隐",
            Self::Form => "表单与标签",
            Self::TextArea => "多行输入",
        }
    }
    pub fn group(self) -> usize {
        match self {
            Self::Checkbox => 5,
            Self::CheckboxGroup => 5,
            Self::CheckboxCards => 5,
            Self::RadioCards => 5,
            Self::Toggle => 5,
            Self::ToggleGroup => 5,
            Self::Slider => 5,
            Self::Progress => 6,
            Self::Toast => 6,
            Self::Badge => 6,
            Self::Skeleton => 6,
            Self::Spinner => 6,
            Self::Accordion => 7,
            Self::Collapsible => 7,
            Self::Dialog => 7,
            Self::AlertDialog => 7,
            Self::ContextMenu => 7,
            Self::Menubar => 7,
            Self::Popover => 7,
            Self::Tooltip => 7,
            Self::HoverCard => 7,
            Self::Tabs => 8,
            Self::Toolbar => 8,
            Self::NavigationMenu => 8,
            Self::Avatar => 8,
            Self::DataList => 8,
            Self::Table => 8,
            Self::ScrollArea => 8,
            Self::Layout => 8,
            Self::Otp => 9,
            Self::Password => 9,
            Self::Form => 9,
            Self::TextArea => 9,
        }
    }
    pub fn description(self) -> &'static str {
        match self {
            Self::Checkbox => "切换勾选、半选与禁用状态",
            Self::CheckboxGroup => "独立多选与全选，保留部分选中的反馈",
            Self::CheckboxCards => "整张卡片可选择，包含说明和禁用选项",
            Self::RadioCards => "使用方向键切换唯一选择并跳过禁用项",
            Self::Toggle => "独立的按下状态，再次点击可以取消",
            Self::ToggleGroup => "比较可取消的单选与多选，以及方向键焦点移动",
            Self::Slider => "拖动、键盘步进、区间双滑块与纵向模式",
            Self::Progress => "确定与不确定进度，显示当前值与完成状态",
            Self::Toast => "关闭、执行操作、队列上限和计时暂停",
            Self::Badge => "用紧凑标记呈现中性、成功、警告和错误状态",
            Self::Skeleton => "保持内容尺寸，在加载完成后显示实际内容",
            Self::Spinner => "不同尺寸的等待指示，遵循减少动态效果设置",
            Self::Accordion => "单开、多开与不可全部收起，支持组内键盘移动",
            Self::Collapsible => "展开与收起实际内容，检查高度变化与焦点返回",
            Self::Dialog => "打开、关闭、焦点圈与回到触发按钮",
            Self::AlertDialog => "默认聚焦取消，确认后才执行样例操作",
            Self::ContextMenu => "右键、长按或 Shift F10，试用子菜单与快捷定位",
            Self::Menubar => "菜单之间用方向键移动，支持分组与子菜单",
            Self::Popover => "在锚定浮层中编辑与选择，检查 Tab、Escape 和外部点击",
            Self::Tooltip => "悬停或键盘聚焦显示紧凑提示，Escape 关闭",
            Self::HoverCard => "从触发器移动到只读详情卡，保留短暂跨越间隔",
            Self::Tabs => "比较自动、手动激活的标签面板与链接导航",
            Self::Toolbar => "在操作和切换按钮间用方向键移动焦点",
            Self::NavigationMenu => "展开导航分组，选择目的地后显示对应内容",
            Self::Avatar => "无框图像、字母回退与加载占位，比较不同尺寸",
            Self::DataList => "键值内容按宽度换行并保持标签对齐",
            Self::Table => "表头、行标题与数值列，窄窗口可横向滚动",
            Self::ScrollArea => "双轴滚动条、拖动滑块与键盘滚动",
            Self::Layout => "比较固定比例容器、横竖分隔线和边缘内容布局",
            Self::Otp => "分格输入、整段粘贴、光标移动与删除",
            Self::Password => "切换可见性时保留光标、选择范围和编辑历史",
            Self::Form => "点击标签聚焦字段，呈现宿主提供的校验反馈",
            Self::TextArea => "中文组合输入、换行、选择与只读状态",
        }
    }
}

pub(super) struct Specimen {
    example: Example,
    id: String,
    width: f32,
    variant: usize,
    disabled: bool,
    selected: Vec<usize>,
    checked: p::selection::Checked,
    values: Vec<f64>,
    actions: usize,
    commits: usize,
    open: bool,
    status: String,
    input: Entity<ComposerInput>,
    second: Entity<ComposerInput>,
    otp: Entity<p::input::OneTimeCode>,
    toasts: Entity<p::feedback::Toasts>,
    dialog: crate::modal::PlainDialog,
    alert: p::dialog::AlertDialog,
    flyout: p::dialog::Flyout,
    menu: p::menu::Menu,
    menubar: p::menu::Menubar,
    navigation_menu: p::dialog::NavigationMenu,
    scroll: p::data::ScrollArea,
    details: Entity<crate::components::tooltip::DetailsOverlay>,
}
impl Specimen {
    pub(super) fn new(example: Example, id: String, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| {
            let input = ComposerInput::new(
                if example == Example::TextArea {
                    "输入多行文字…"
                } else {
                    "输入示例内容…"
                },
                cx,
            );
            if example == Example::TextArea {
                input.multiline()
            } else {
                input.single_line()
            }
        });
        let second = cx.new(|cx| ComposerInput::new("补充内容…", cx).single_line());
        let otp = cx.new(|cx| p::input::OneTimeCode::new(6, cx));
        let toasts = cx.new(p::feedback::Toasts::new);
        cx.observe(&input, |_, _, cx| cx.notify()).detach();
        cx.observe(&second, |_, _, cx| cx.notify()).detach();
        cx.subscribe(&input, |v, _, _: &ComposerEdited, cx| {
            v.status = "内容已更新".into();
            cx.notify();
        })
        .detach();
        cx.subscribe(&otp, |v, _, event: &p::input::CodeEdited, cx| {
            v.status = if event.complete {
                "已输入完整验证码".into()
            } else {
                format!("已输入 {} 位", event.value.len())
            };
            cx.notify();
        })
        .detach();
        cx.subscribe(&toasts, |v, _, event: &p::feedback::ToastEvent, cx| {
            v.actions += 1;
            v.status = format!("已执行通知操作：{}", event.key);
            cx.notify();
        })
        .detach();
        if example == Example::Password {
            input.update(cx, |input, cx| {
                input.set_value("example-password", cx);
                input.set_secret(true, cx);
            });
        }
        if example == Example::AlertDialog {
            input.update(cx, |input, cx| input.set_value("可清空的样例内容", cx));
        }
        let details = cx.new(|_| crate::components::tooltip::DetailsOverlay::default());
        Self {
            example,
            id,
            width: 320.,
            variant: if example == Example::Checkbox { 2 } else { 0 },
            disabled: false,
            selected: if matches!(
                example,
                Example::CheckboxGroup
                    | Example::CheckboxCards
                    | Example::RadioCards
                    | Example::Tabs
            ) {
                vec![0]
            } else {
                vec![]
            },
            checked: p::selection::Checked::Mixed,
            values: vec![35.],
            actions: 0,
            commits: 0,
            open: false,
            status: String::new(),
            input,
            second,
            otp,
            toasts,
            dialog: crate::modal::PlainDialog::new(cx),
            alert: p::dialog::AlertDialog::new(cx),
            flyout: p::dialog::Flyout::new(cx),
            menu: p::menu::Menu::default(),
            menubar: p::menu::Menubar::default(),
            navigation_menu: p::dialog::NavigationMenu::default(),
            scroll: p::data::ScrollArea::default(),
            details,
        }
    }
    pub(super) fn set_width(&mut self, width: f32, cx: &mut Context<Self>) {
        if self.width != width {
            self.width = width;
            cx.notify();
        }
    }
    fn sid(&self, key: &str) -> String {
        format!("{}-{key}", self.id)
    }
    pub(super) fn inspect(&self, cx: &App) -> Value {
        json!({"example":self.example.key(),"variant":self.variant,"disabled":self.disabled,"selected":self.selected,"checked":self.checked,"values":self.values,"actions":self.actions,"commits":self.commits,"open":self.open,"input":self.input.read(cx).value(),"selection":self.input.read(cx).selection(cx),"status":self.status,"otp":self.otp.read(cx).inspect(cx),"toasts":self.toasts.read(cx).inspect(),"menu":self.menu.inspect(),"menubar":self.menubar.inspect(),"flyout":self.flyout.is_open(),"flyoutMotion":self.flyout.inspect(),"scroll":self.scroll.inspect(),"dialog":self.dialog.inspect(),"alert":self.alert.inspect(),"details":self.details.read(cx).inspect()})
    }
}
